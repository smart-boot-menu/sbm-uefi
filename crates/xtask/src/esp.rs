use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use sbm_boot::{encode_manifest, encode_policy, fresh_policy, hex_encode, sha256};
use sbm_protocol::{RecoveryManifest, RecoverySlotDescriptor};
use sbm_types::SCHEMA_VERSION_V1;

pub struct EspPaths {
    pub dir: PathBuf,
}

pub fn assemble(
    root: &Path,
    arch: &str,
    tag: &str,
    bls_arch: &str,
    menu: &[u8],
    fixture: &[u8],
) -> Result<EspPaths, String> {
    let dir = root.join(format!("esp-{arch}"));
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("remove {}: {err}", dir.display()))?;
    }
    let boot = if arch == "aarch64" {
        "BOOTAA64.EFI"
    } else {
        "BOOTX64.EFI"
    };
    write(
        &dir.join("startup.nsh"),
        format!("\\EFI\\BOOT\\{boot}\n").as_bytes(),
    )?;
    write(&dir.join("EFI/BOOT").join(boot), menu)?;
    write(&dir.join("EFI/Microsoft/Boot/bootmgfw.efi"), fixture)?;
    write(&dir.join(format!("EFI/ubuntu/shim{tag}.efi")), fixture)?;
    write(&dir.join(format!("EFI/ubuntu/grub{tag}.efi")), fixture)?;
    write(&dir.join(format!("EFI/ubuntu/mm{tag}.efi")), fixture)?;
    write(
        &dir.join(format!("EFI/systemd/systemd-boot{tag}.efi")),
        fixture,
    )?;
    write(&dir.join("EFI/Linux/fedora.efi"), fixture)?;
    write(&dir.join("EFI/sbm/sbm.efi"), fixture)?;
    let conf = format!(
        "title Fedora Linux\nlinux \\EFI\\Linux\\fedora.efi\noptions quiet\narchitecture {bls_arch}\n"
    );
    write(&dir.join("loader/entries/fedora.conf"), conf.as_bytes())?;
    let hash = hex_encode(&sha256(fixture));
    for slot in ["slot-a", "slot-b"] {
        write(
            &dir.join(format!("EFI/sbm/recovery/{slot}/sbm-core.efi")),
            fixture,
        )?;
    }
    let manifest = RecoveryManifest {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        active_slot: String::from("slot-a"),
        slots: vec![slot("slot-a", &hash), slot("slot-b", &hash)],
    };
    let bytes = encode_manifest(&manifest)
        .map_err(|_| String::from("could not encode the recovery manifest"))?;
    write(&dir.join("EFI/sbm/recovery/manifest.json"), &bytes)?;
    let policy =
        encode_policy(&fresh_policy()).map_err(|_| String::from("could not encode policy.json"))?;
    write(&dir.join("EFI/sbm/policy.json"), &policy)?;
    Ok(EspPaths { dir })
}

fn slot(name: &str, hash: &str) -> RecoverySlotDescriptor {
    RecoverySlotDescriptor {
        slot_name: name.to_string(),
        uki_efi_path: format!("\\EFI\\sbm\\recovery\\{name}\\sbm-core.efi"),
        sha256: hash.to_string(),
        version: String::from("0.1.0-fixture"),
        healthy: true,
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|err| format!("write {}: {err}", path.display()))
}

struct Attached {
    dev: String,
    mount: Option<PathBuf>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        if let Some(mount) = self.mount.take() {
            let _ = Command::new("diskutil")
                .args(["unmount", "force"])
                .arg(&mount)
                .status();
        }
        let _ = Command::new("hdiutil").args(["detach", &self.dev]).status();
    }
}

/// QEMU's `fat:` directory backend corrupts files larger than a few hundred
/// bytes. A real FAT16 image keeps the handoff JSON intact.
pub fn create_fat_image(src: &Path, image: &Path) -> Result<(), String> {
    if let Some(parent) = image.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let file = File::create(image).map_err(|err| format!("create image: {err}"))?;
    file.set_len(32 * 1024 * 1024)
        .map_err(|err| format!("size image: {err}"))?;
    drop(file);
    let mut disk = attach(image)?;
    assert_disk_image(&disk.dev)?;
    let formatted = Command::new("newfs_msdos")
        .args(["-F", "16", "-v", "SBM", &disk.dev])
        .status()
        .map_err(|err| format!("newfs_msdos: {err}"))?;
    if !formatted.success() {
        return Err(String::from("newfs_msdos failed"));
    }
    let mount = image.with_extension("mnt");
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(&mount).map_err(|err| err.to_string())?;
    disk.mount_at(&mount)?;
    let copied = Command::new("cp")
        .arg("-R")
        .arg(src.join("."))
        .arg(&mount)
        .status()
        .map_err(|err| format!("cp: {err}"))?;
    if !copied.success() {
        return Err(String::from("copy into the FAT image failed"));
    }
    Ok(())
}

pub fn read_fat_file(image: &Path, relative: &str) -> Result<Vec<u8>, String> {
    let mut disk = attach(image)?;
    assert_disk_image(&disk.dev)?;
    let mount = image.with_extension("mnt");
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(&mount).map_err(|err| err.to_string())?;
    disk.mount_at(&mount)?;
    let path = mount.join(relative);
    fs::read(&path).map_err(|err| format!("read {}: {err}", path.display()))
}

fn attach(image: &Path) -> Result<Attached, String> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nomount"])
        .arg(image)
        .output()
        .map_err(|err| format!("hdiutil: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let dev = text
        .split_whitespace()
        .find(|word| word.starts_with("/dev/disk"))
        .ok_or_else(|| format!("hdiutil returned no disk: {text}"))?
        .to_string();
    Ok(Attached { dev, mount: None })
}

fn assert_disk_image(dev: &str) -> Result<(), String> {
    let output = Command::new("diskutil")
        .args(["info", dev])
        .output()
        .map_err(|err| format!("diskutil info: {err}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains("Disk Image") {
        return Err(format!("{dev} is not a disk image; refusing to format it"));
    }
    Ok(())
}

impl Attached {
    fn mount_at(&mut self, mount: &Path) -> Result<(), String> {
        let status = Command::new("diskutil")
            .args(["mount", "-mountPoint"])
            .arg(mount)
            .arg(&self.dev)
            .status()
            .map_err(|err| format!("diskutil mount: {err}"))?;
        if !status.success() {
            return Err(format!("diskutil mount {} failed", self.dev));
        }
        self.mount = Some(mount.to_path_buf());
        Ok(())
    }
}
