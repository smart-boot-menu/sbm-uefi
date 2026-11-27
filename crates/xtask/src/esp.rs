use sbm_media::esp::{self, EspContents};
use sbm_media::gpt::{self, GptPartition, Guid, SECTOR, types};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

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
    // Same filename mkosi.postoutput writes: sbm-core-<arch>.efi.
    let uki_name = format!("sbm-core-{arch}.efi");
    write(
        &dir.join(format!("EFI/sbm/recovery/slot-a/{uki_name}")),
        fixture,
    )?;
    let foreign = if arch == "aarch64" {
        "x86_64"
    } else {
        "aarch64"
    };
    let manifest = RecoveryManifest {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        active_slot: String::from("slot-a"),
        slots: vec![
            slot("slot-a", &uki_name, &hash),
            slot(
                "slot-a",
                &format!("sbm-core-{foreign}.efi"),
                &"0".repeat(64),
            ),
        ],
    };
    let bytes = encode_manifest(&manifest)
        .map_err(|_| String::from("could not encode the recovery manifest"))?;
    write(&dir.join("EFI/sbm/recovery/manifest.json"), &bytes)?;
    let policy =
        encode_policy(&fresh_policy()).map_err(|_| String::from("could not encode policy.json"))?;
    write(&dir.join("EFI/sbm/policy.json"), &policy)?;
    Ok(EspPaths { dir })
}

fn slot(name: &str, uki_name: &str, hash: &str) -> RecoverySlotDescriptor {
    RecoverySlotDescriptor {
        slot_name: name.to_string(),
        uki_efi_path: format!("\\EFI\\sbm\\recovery\\{name}\\{uki_name}"),
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

const ESP_START: u64 = 2048;
const ESP_SECTORS: u64 = 32 * 1024 * 1024 / SECTOR;
const DISK_SECTORS: u64 = ESP_START + ESP_SECTORS + 2048;

fn fixture_partition() -> GptPartition {
    GptPartition {
        number: 1,
        type_guid: types::esp(),
        unique_guid: Guid::derive("sbm-uefi-smoke-esp", &[]),
        first_lba: ESP_START,
        last_lba: ESP_START + ESP_SECTORS - 1,
        attributes: 0,
        name: "SBM EFI smoke".into(),
    }
}

pub fn fixture_partuuid() -> String {
    fixture_partition()
        .unique_guid
        .to_string()
        .to_ascii_lowercase()
}

pub fn create_fat_image(src: &Path, image: &Path) -> Result<(), String> {
    fn collect(root: &Path, current: &Path, files: &mut EspContents) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let path = entry.path();
            if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
                collect(root, &path, files)?;
            } else {
                let name = path.strip_prefix(root).map_err(|err| err.to_string())?;
                files.add_file(
                    &name.to_string_lossy(),
                    fs::read(&path).map_err(|err| err.to_string())?,
                );
            }
        }
        Ok(())
    }

    let mut files = EspContents::default();
    collect(src, src, &mut files)?;
    let disk = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(image)
        .map_err(|err| err.to_string())?;
    disk.set_len(DISK_SECTORS * SECTOR)
        .map_err(|err| err.to_string())?;
    let part = fixture_partition();
    gpt::write_gpt(
        &disk,
        DISK_SECTORS,
        Guid::derive("sbm-uefi-smoke-disk", &[]),
        core::slice::from_ref(&part),
    )
    .map_err(|err| err.to_string())?;
    esp::format_with(
        &disk,
        part.offset(),
        part.size(),
        fatfs::FatType::Fat16,
        2048,
        42,
        "SBM",
        0,
        &files,
    )
    .map_err(|err| err.to_string())?;
    disk.sync_all().map_err(|err| err.to_string())
}

pub fn read_fat_file(image: &Path, relative: &str) -> Result<Vec<u8>, String> {
    let disk = File::open(image).map_err(|err| err.to_string())?;
    let part = fixture_partition();
    esp::read_file(
        &disk,
        part.offset(),
        part.size(),
        relative,
        1024 * 1024,
        "smoke ESP",
    )
    .map_err(|err| err.to_string())?
    .ok_or_else(|| format!("{relative} missing from smoke ESP"))
}
