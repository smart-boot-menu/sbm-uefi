use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbm_protocol::RecoverySlotPreference;
use sbm_types::{BootTarget, BootTargetId, BootTargetKind, CpuArch, OsFamily, SCHEMA_VERSION_V1};

use crate::bls::{bls_arch_ok, parse_bls};
use crate::nvram::ParsedLoadOption;
use crate::pathutil::{
    bls_target_id, efi_target_id, file_name, hex_encode, normalize, normalize_upper,
    recovery_slot_key, recovery_target_id,
};
use crate::pe::{machine_matches, pe_machine, pe_section_span, section_text};
use crate::persist::decode_manifest;
use crate::volume::Volume;

const LOAD_OPTION_ACTIVE: u32 = 0x1;
const LOAD_OPTION_HIDDEN: u32 = 0x8;

#[derive(Clone, Debug)]
pub struct DiscoveredTarget {
    pub target: BootTarget,
    pub initrd: Option<String>,
    pub healthy: bool,
    pub preferred: bool,
    pub rank: u8,
}

pub fn discover_volume(
    vol: &dyn Volume,
    arch: CpuArch,
    skip_paths: &[String],
) -> Vec<DiscoveredTarget> {
    let mut out = Vec::new();
    scan_recovery(vol, arch, skip_paths, &mut out);
    scan_bls(vol, arch, skip_paths, &mut out);
    scan_linux_dir(vol, arch, skip_paths, &mut out);
    scan_efi_tree(vol, arch, skip_paths, &mut out);
    sort_discovered(&mut out);
    out
}

pub fn add_load_options(
    vol: &dyn Volume,
    options: &[ParsedLoadOption],
    arch: CpuArch,
    skip_paths: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    for option in options {
        if option.attributes & LOAD_OPTION_ACTIVE == 0
            || option.attributes & LOAD_OPTION_HIDDEN != 0
        {
            continue;
        }
        if let Some(guid) = &option.partition_guid
            && !guid.eq_ignore_ascii_case(vol.partition_guid())
        {
            continue;
        }
        let Some(path) = option.file_path.as_deref() else {
            continue;
        };
        let title = if option.description.is_empty() {
            file_name(path)
        } else {
            option.description.clone()
        };
        push_file(
            vol,
            arch,
            efi_target_id(vol.partition_guid(), path),
            path,
            &title,
            OsFamily::Other,
            BootTargetKind::EfiBinary,
            "other",
            30,
            None,
            None,
            true,
            false,
            None,
            false,
            skip_paths,
            out,
        );
    }
}

pub fn sort_discovered(targets: &mut [DiscoveredTarget]) {
    targets.sort_by(|a, b| compare_targets(&a.target, &b.target));
}

pub fn compare_targets(a: &BootTarget, b: &BootTarget) -> core::cmp::Ordering {
    family_ord(a.os_family)
        .cmp(&family_ord(b.os_family))
        .then_with(|| a.removable_media.cmp(&b.removable_media))
        .then_with(|| {
            a.display_name
                .to_ascii_lowercase()
                .cmp(&b.display_name.to_ascii_lowercase())
        })
        .then_with(|| a.id.as_str().cmp(b.id.as_str()))
}

pub fn embedded_cmdline(vol: &dyn Volume, path: &str) -> Option<String> {
    let header = vol.read_prefix(path, 16 * 1024).ok()?;
    let (offset, size) = pe_section_span(&header, ".cmdline")?;
    let size = usize::try_from(size).ok()?.min(4096);
    let end = offset.saturating_add(size as u64);
    let raw = if end <= header.len() as u64 {
        header[offset as usize..end as usize].to_vec()
    } else {
        vol.read_at(path, offset, size).ok()?
    };
    section_text(&raw)
}

pub fn best_recovery(targets: &[DiscoveredTarget]) -> Option<&DiscoveredTarget> {
    targets
        .iter()
        .filter(|target| target.target.kind == BootTargetKind::SbmCore && selectable_target(target))
        .find(|target| target.preferred)
        .or_else(|| {
            targets.iter().find(|target| {
                target.target.kind == BootTargetKind::SbmCore && selectable_target(target)
            })
        })
}

pub fn recovery_for_slot(
    targets: &[DiscoveredTarget],
    slot: RecoverySlotPreference,
) -> Option<&DiscoveredTarget> {
    let want = match slot {
        RecoverySlotPreference::Auto => return best_recovery(targets),
        RecoverySlotPreference::SlotA => "sbm-core:slot-a",
        RecoverySlotPreference::SlotB => "sbm-core:slot-b",
        RecoverySlotPreference::Removable => "sbm-core:removable",
    };
    targets
        .iter()
        .find(|target| target.target.id.as_str() == want)
}

fn selectable_target(target: &DiscoveredTarget) -> bool {
    !target.target.hidden && target.healthy && target.target.verified_signature != Some(false)
}

fn family_ord(family: OsFamily) -> u8 {
    match family {
        OsFamily::Windows => 0,
        OsFamily::Linux => 1,
        OsFamily::Bsd => 2,
        OsFamily::MacOs => 3,
        OsFamily::Other => 4,
        OsFamily::Recovery => 5,
        OsFamily::FirmwareUtility => 6,
    }
}

fn scan_recovery(
    vol: &dyn Volume,
    arch: CpuArch,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    let Ok(bytes) = vol.read(r"\EFI\sbm\recovery\manifest.json") else {
        return;
    };
    let Ok(manifest) = decode_manifest(&bytes) else {
        return;
    };
    if manifest.schema_version != SCHEMA_VERSION_V1 {
        return;
    }
    for slot in &manifest.slots {
        let path = normalize(&slot.uki_efi_path);
        let verified = check_hash(vol, &path, &slot.sha256);
        let hash_ok = verified != Some(false);
        let id = recovery_target_id(&slot.slot_name);
        let label = match recovery_slot_key(&slot.slot_name) {
            "slot-a" => "Slot A",
            "slot-b" => "Slot B",
            "removable" => "Removable",
            _ => "Slot",
        };
        let mut title = alloc::format!("SBM Core {label}");
        if !hash_ok {
            title.push_str(" (unverified)");
        }
        let preferred =
            recovery_slot_key(&manifest.active_slot) == recovery_slot_key(&slot.slot_name);
        push_file(
            vol,
            arch,
            id,
            &path,
            &title,
            OsFamily::Recovery,
            BootTargetKind::SbmCore,
            "recovery",
            100,
            None,
            None,
            slot.healthy && hash_ok,
            preferred,
            verified,
            false,
            skip,
            out,
        );
    }
}

fn check_hash(vol: &dyn Volume, path: &str, expected: &str) -> Option<bool> {
    if expected.is_empty() {
        return None;
    }
    let meta = vol.metadata(path).ok()?;
    if meta.is_dir || meta.size > 2 * 1024 * 1024 * 1024 {
        return None;
    }
    let digest = hash_file(vol, path, meta.size)?;
    Some(hex_encode(&digest).eq_ignore_ascii_case(expected))
}

fn hash_file(vol: &dyn Volume, path: &str, size: u64) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut offset = 0u64;
    while offset < size {
        let chunk = vol.read_at(path, offset, 64 * 1024).ok()?;
        if chunk.is_empty() {
            return None;
        }
        hasher.update(&chunk);
        offset = offset.saturating_add(chunk.len() as u64);
    }
    Some(hasher.finalize().into())
}

fn scan_bls(vol: &dyn Volume, arch: CpuArch, skip: &[String], out: &mut Vec<DiscoveredTarget>) {
    scan_bls_dir(vol, r"\loader\entries", arch, skip, out);
    for entry in list_dir(vol, r"\loader\entries") {
        if entry.is_dir {
            scan_bls_dir(
                vol,
                &alloc::format!("\\loader\\entries\\{}", entry.name),
                arch,
                skip,
                out,
            );
        }
    }
}

fn scan_bls_dir(
    vol: &dyn Volume,
    dir: &str,
    arch: CpuArch,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    for entry in list_dir(vol, dir) {
        if entry.is_dir || !entry.name.to_ascii_lowercase().ends_with(".conf") {
            continue;
        }
        let path = alloc::format!("{dir}\\{}", entry.name);
        let Ok(bytes) = vol.read(&path) else {
            continue;
        };
        let Ok(text) = core::str::from_utf8(&bytes) else {
            continue;
        };
        let parsed = parse_bls(text);
        if !bls_arch_ok(parsed.architecture.as_deref(), arch) {
            continue;
        }
        let Some(efi_path) = parsed.efi.as_deref().or(parsed.linux.as_deref()) else {
            continue;
        };
        let efi_path = normalize(efi_path);
        let stem = entry
            .name
            .trim_end_matches(".conf")
            .trim_end_matches(".CONF");
        let title = if parsed.title.is_empty() {
            stem.to_string()
        } else {
            parsed.title.clone()
        };
        let kind = if normalize_upper(&efi_path).contains("\\EFI\\LINUX\\") {
            BootTargetKind::BlsUki
        } else {
            BootTargetKind::BlsType1
        };
        push_file(
            vol,
            arch,
            bls_target_id(vol.partition_guid(), stem),
            &efi_path,
            &title,
            OsFamily::Linux,
            kind,
            "linux",
            70,
            parsed.options,
            parsed.initrd.map(|path| normalize(&path)),
            true,
            false,
            None,
            false,
            skip,
            out,
        );
    }
}

fn scan_linux_dir(
    vol: &dyn Volume,
    arch: CpuArch,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    for entry in list_dir(vol, r"\EFI\Linux") {
        if entry.is_dir || !entry.name.to_ascii_lowercase().ends_with(".efi") {
            continue;
        }
        let path = alloc::format!("\\EFI\\Linux\\{}", entry.name);
        let stem = entry.name.trim_end_matches(".efi").trim_end_matches(".EFI");
        push_file(
            vol,
            arch,
            bls_target_id(vol.partition_guid(), stem),
            &path,
            stem,
            OsFamily::Linux,
            BootTargetKind::BlsUki,
            "linux",
            60,
            None,
            None,
            true,
            false,
            None,
            false,
            skip,
            out,
        );
    }
}

fn scan_efi_tree(
    vol: &dyn Volume,
    arch: CpuArch,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    for entry in list_dir(vol, r"\EFI") {
        if !entry.is_dir {
            continue;
        }
        let dir = alloc::format!("\\EFI\\{}", entry.name);
        consider_dir(vol, &dir, &entry.name, arch, skip, out);
        for nested in list_dir(vol, &dir) {
            if nested.is_dir {
                consider_dir(
                    vol,
                    &alloc::format!("{dir}\\{}", nested.name),
                    &entry.name,
                    arch,
                    skip,
                    out,
                );
            }
        }
    }
}

fn consider_dir(
    vol: &dyn Volume,
    dir: &str,
    vendor: &str,
    arch: CpuArch,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    let mut best: Option<(u8, String)> = None;
    for entry in list_dir(vol, dir) {
        if entry.is_dir {
            continue;
        }
        let Some(rank) = loader_rank(&entry.name, arch) else {
            continue;
        };
        let path = alloc::format!("{dir}\\{}", entry.name);
        if skipped(&path, skip) {
            continue;
        }
        if best.as_ref().is_none_or(|current| rank > current.0) {
            best = Some((rank, path));
        }
    }
    let Some((rank, path)) = best else {
        return;
    };
    let name = file_name(&path);
    let (title, family, hint) = classify(vol, vendor, &name);
    push_file(
        vol,
        arch,
        efi_target_id(vol.partition_guid(), &path),
        &path,
        &title,
        family,
        BootTargetKind::EfiBinary,
        hint,
        rank,
        None,
        None,
        true,
        false,
        None,
        false,
        skip,
        out,
    );
}

fn loader_rank(name: &str, arch: CpuArch) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    if is_helper_file(&name) {
        return None;
    }
    let tag = arch_tag(arch);
    if name == "bootmgfw.efi" {
        return Some(90);
    }
    if name == alloc::format!("shim{tag}.efi") {
        return Some(80);
    }
    if name == alloc::format!("systemd-boot{tag}.efi") {
        return Some(75);
    }
    if name == alloc::format!("grub{tag}.efi") || name == "grub.efi" {
        return Some(70);
    }
    if name == alloc::format!("limine_{tag}.efi") || name == "limine.efi" {
        return Some(65);
    }
    if name == alloc::format!("refind_{tag}.efi") {
        return Some(60);
    }
    if name == "loader.efi" {
        return Some(50);
    }
    if name == alloc::format!("boot{tag}.efi") {
        return Some(20);
    }
    None
}

fn arch_tag(arch: CpuArch) -> &'static str {
    match arch {
        CpuArch::X86_64 => "x64",
        CpuArch::Aarch64 => "aa64",
        CpuArch::Ia32 => "ia32",
        CpuArch::Riscv64 => "riscv64",
    }
}

fn is_helper_file(name: &str) -> bool {
    matches!(
        name,
        "mmx64.efi"
            | "mmaa64.efi"
            | "mmia32.efi"
            | "fbx64.efi"
            | "fbaa64.efi"
            | "fbia32.efi"
            | "sbm.efi"
            | "sbm-x86_64.efi"
            | "sbm-aarch64.efi"
    )
}

fn classify(vol: &dyn Volume, vendor: &str, file: &str) -> (String, OsFamily, &'static str) {
    let vendor_l = vendor.to_ascii_lowercase();
    let file_l = file.to_ascii_lowercase();
    let hint = if file_l.contains("systemd") {
        "systemd"
    } else if file_l.contains("grub") || file_l.contains("shim") {
        "grub"
    } else if vendor_l.contains("bsd") {
        "bsd"
    } else if vendor_l == "microsoft" || file_l.contains("bootmgfw") {
        "windows"
    } else if vendor_l == "boot" {
        "usb"
    } else {
        "linux"
    };
    let (name, family) = match vendor_l.as_str() {
        "microsoft" => ("Windows".to_string(), OsFamily::Windows),
        "ubuntu" => ("Ubuntu".to_string(), OsFamily::Linux),
        "fedora" => ("Fedora".to_string(), OsFamily::Linux),
        "debian" => ("Debian".to_string(), OsFamily::Linux),
        "arch" => ("Arch Linux".to_string(), OsFamily::Linux),
        "endeavouros" => ("EndeavourOS".to_string(), OsFamily::Linux),
        "pop" => ("Pop!_OS".to_string(), OsFamily::Linux),
        "opensuse" | "suse" | "sles" => ("openSUSE".to_string(), OsFamily::Linux),
        "centos" => ("CentOS".to_string(), OsFamily::Linux),
        "rocky" => ("Rocky Linux".to_string(), OsFamily::Linux),
        "almalinux" => ("AlmaLinux".to_string(), OsFamily::Linux),
        "gentoo" => ("Gentoo".to_string(), OsFamily::Linux),
        "nixos" => ("NixOS".to_string(), OsFamily::Linux),
        "void" => ("Void Linux".to_string(), OsFamily::Linux),
        "linuxmint" | "mint" => ("Linux Mint".to_string(), OsFamily::Linux),
        "zorin" => ("Zorin OS".to_string(), OsFamily::Linux),
        "manjaro" => ("Manjaro".to_string(), OsFamily::Linux),
        "kali" => ("Kali Linux".to_string(), OsFamily::Linux),
        "elementary" => ("elementary OS".to_string(), OsFamily::Linux),
        "freebsd" => ("FreeBSD".to_string(), OsFamily::Bsd),
        "netbsd" => ("NetBSD".to_string(), OsFamily::Bsd),
        "openbsd" => ("OpenBSD".to_string(), OsFamily::Bsd),
        "systemd" => ("systemd-boot".to_string(), OsFamily::Linux),
        "refind" => ("rEFInd".to_string(), OsFamily::Other),
        "limine" => ("Limine".to_string(), OsFamily::Other),
        "boot" => {
            if vol.removable() {
                ("External drive".to_string(), OsFamily::Other)
            } else {
                ("EFI application".to_string(), OsFamily::Other)
            }
        }
        _ => (vendor.to_string(), OsFamily::Other),
    };
    (name, family, hint)
}

#[allow(clippy::too_many_arguments)]
fn push_file(
    vol: &dyn Volume,
    arch: CpuArch,
    id: BootTargetId,
    path: &str,
    title: &str,
    family: OsFamily,
    kind: BootTargetKind,
    hint: &str,
    rank: u8,
    cmdline: Option<String>,
    initrd: Option<String>,
    healthy: bool,
    preferred: bool,
    verified: Option<bool>,
    hidden: bool,
    skip: &[String],
    out: &mut Vec<DiscoveredTarget>,
) {
    let path = normalize(path);
    if skipped(&path, skip) || !id.is_valid() {
        return;
    }
    match vol.metadata(&path) {
        Ok(meta) if !meta.is_dir => {}
        _ => return,
    }
    if let Ok(prefix) = vol.read_prefix(&path, 4096)
        && let Some(machine) = pe_machine(&prefix)
        && !machine_matches(machine, arch)
    {
        return;
    }
    let guid = vol.partition_guid().to_ascii_lowercase();
    let found = DiscoveredTarget {
        target: BootTarget {
            id,
            display_name: title.to_string(),
            os_family: family,
            kind,
            arch,
            partition_guid: Some(guid),
            efi_path: Some(path),
            kernel_cmdline: cmdline,
            icon_hint: Some(hint.to_string()),
            removable_media: vol.removable(),
            hidden,
            verified_signature: verified,
        },
        initrd,
        healthy,
        preferred,
        rank,
    };
    merge(out, found);
}

fn skipped(path: &str, extra: &[String]) -> bool {
    let upper = normalize_upper(path);
    if extra.iter().any(|item| normalize_upper(item) == upper) {
        return true;
    }
    let name = upper.rsplit('\\').next().unwrap_or("");
    is_helper_file(&name.to_ascii_lowercase())
}

fn merge(out: &mut Vec<DiscoveredTarget>, new: DiscoveredTarget) {
    let new_path = new.target.efi_path.as_deref().map(normalize_upper);
    if let Some(path) = new_path.as_deref() {
        if let Some(index) = out.iter().position(|existing| {
            existing
                .target
                .efi_path
                .as_deref()
                .map(normalize_upper)
                .as_deref()
                == Some(path)
        }) {
            merge_same_path(&mut out[index], new);
            return;
        }
        if new.target.kind == BootTargetKind::EfiBinary {
            let dir = parent_dir(path);
            if let Some(index) = out.iter().position(|existing| {
                existing.target.kind == BootTargetKind::EfiBinary
                    && existing.target.partition_guid == new.target.partition_guid
                    && existing
                        .target
                        .efi_path
                        .as_deref()
                        .map(normalize_upper)
                        .as_deref()
                        .is_some_and(|existing_path| parent_dir(existing_path) == dir)
            }) {
                if new.rank > out[index].rank {
                    let title =
                        preferred_title(&out[index].target.display_name, &new.target.display_name);
                    out[index] = new;
                    out[index].target.display_name = title;
                }
                return;
            }
        }
    }
    if out
        .iter()
        .any(|existing| existing.target.id == new.target.id)
    {
        return;
    }
    out.push(new);
}

fn merge_same_path(existing: &mut DiscoveredTarget, new: DiscoveredTarget) {
    let new_kind = kind_rank(new.target.kind);
    let old_kind = kind_rank(existing.target.kind);
    if new_kind > old_kind || (new_kind == old_kind && new.rank > existing.rank) {
        let title = preferred_title(&existing.target.display_name, &new.target.display_name);
        let initrd = new.initrd.clone().or_else(|| existing.initrd.clone());
        let cmdline = new
            .target
            .kernel_cmdline
            .clone()
            .or_else(|| existing.target.kernel_cmdline.clone());
        *existing = new;
        existing.target.display_name = title;
        if existing.initrd.is_none() {
            existing.initrd = initrd;
        }
        if existing.target.kernel_cmdline.is_none() {
            existing.target.kernel_cmdline = cmdline;
        }
    } else if generic_title(&existing.target.display_name)
        && !generic_title(&new.target.display_name)
    {
        existing.target.display_name = new.target.display_name;
        if existing.initrd.is_none() {
            existing.initrd = new.initrd;
        }
        if existing.target.kernel_cmdline.is_none() {
            existing.target.kernel_cmdline = new.target.kernel_cmdline;
        }
    }
}

fn preferred_title(old: &str, new: &str) -> String {
    if generic_title(new) && !generic_title(old) {
        old.to_string()
    } else if !generic_title(new) {
        new.to_string()
    } else {
        old.to_string()
    }
}

fn generic_title(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.is_empty()
        || name.ends_with(".efi")
        || name.ends_with(".conf")
        || name == "efi application"
}

fn kind_rank(kind: BootTargetKind) -> u8 {
    match kind {
        BootTargetKind::SbmCore => 4,
        BootTargetKind::BlsUki => 3,
        BootTargetKind::BlsType1 => 2,
        BootTargetKind::EfiBinary
        | BootTargetKind::FirmwareSetup
        | BootTargetKind::AppleMacOsReboot => 1,
    }
}

fn parent_dir(path: &str) -> String {
    match path.rsplit_once('\\') {
        Some((dir, _)) if !dir.is_empty() => dir.to_string(),
        _ => String::from("\\"),
    }
}

fn list_dir(vol: &dyn Volume, path: &str) -> Vec<crate::volume::DirEntry> {
    vol.list(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Volume;
    use crate::pathutil::{hex_encode, sha256};
    use crate::pe::{MACHINE_AA64, MACHINE_X64, fake_pe};
    use crate::volume::MemoryVolume;

    fn guid() -> &'static str {
        "01234567-89ab-cdef-0123-456789abcdef"
    }

    #[test]
    fn discovers_loaders_without_duplicates_or_self() {
        let vol = MemoryVolume::new(guid(), false, "ESP");
        let x64 = fake_pe(MACHINE_X64);
        let aa64 = fake_pe(MACHINE_AA64);
        let hash = hex_encode(&sha256(&x64));
        vol.write(r"\EFI\BOOT\BOOTX64.EFI", &x64).unwrap();
        vol.write(r"\EFI\Microsoft\Boot\bootmgfw.efi", &x64)
            .unwrap();
        vol.write(r"\EFI\ubuntu\shimx64.efi", &x64).unwrap();
        vol.write(r"\EFI\ubuntu\grubx64.efi", &x64).unwrap();
        vol.write(r"\EFI\ubuntu\mmx64.efi", &x64).unwrap();
        vol.write(r"\EFI\systemd\systemd-bootx64.efi", &x64)
            .unwrap();
        vol.write(r"\EFI\Linux\fedora.efi", &x64).unwrap();
        vol.write(
            r"\loader\entries\fedora.conf",
            b"title Fedora Linux\nlinux \\EFI\\Linux\\fedora.efi\noptions quiet\narchitecture x64\n",
        )
        .unwrap();
        vol.write(r"\EFI\sbm\sbm.efi", &x64).unwrap();
        vol.write(r"\EFI\sbm\recovery\slot-a\sbm-core.efi", &x64)
            .unwrap();
        vol.write(r"\EFI\sbm\recovery\slot-b\sbm-core.efi", &aa64)
            .unwrap();
        let manifest = alloc::format!(
            r#"{{"schema_version":"1.0","active_slot":"slot-a","slots":[{{"slot_name":"slot-a","uki_efi_path":"\\EFI\\sbm\\recovery\\slot-a\\sbm-core.efi","sha256":"{hash}","version":"0.1.0-fixture","healthy":true}},{{"slot_name":"slot-b","uki_efi_path":"\\EFI\\sbm\\recovery\\slot-b\\sbm-core.efi","sha256":"","version":"0.1.0-fixture","healthy":true}}]}}"#
        );
        vol.write(r"\EFI\sbm\recovery\manifest.json", manifest.as_bytes())
            .unwrap();

        let found = discover_volume(&vol, CpuArch::X86_64, &[r"\EFI\BOOT\BOOTX64.EFI".into()]);
        let names: Vec<_> = found
            .iter()
            .map(|item| item.target.display_name.as_str())
            .collect();
        assert!(names.contains(&"Windows"), "{names:?}");
        assert!(names.contains(&"Ubuntu"), "{names:?}");
        assert!(names.contains(&"systemd-boot"), "{names:?}");
        assert!(names.contains(&"Fedora Linux"), "{names:?}");
        assert!(names.contains(&"SBM Core Slot A"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("Slot B")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.contains("EFI application")),
            "{names:?}"
        );
        assert_eq!(
            found
                .iter()
                .filter(|item| item.target.display_name == "Ubuntu")
                .count(),
            1
        );
        assert_eq!(
            found
                .iter()
                .filter(|item| item
                    .target
                    .efi_path
                    .as_deref()
                    .is_some_and(|path| { normalize_upper(path).contains("FEDORA.EFI") }))
                .count(),
            1
        );
        let fedora = found
            .iter()
            .find(|item| item.target.display_name == "Fedora Linux")
            .unwrap();
        assert_eq!(fedora.target.kind, BootTargetKind::BlsUki);
        assert_eq!(fedora.target.kernel_cmdline.as_deref(), Some("quiet"));
        let ubuntu = found
            .iter()
            .find(|item| item.target.display_name == "Ubuntu")
            .unwrap();
        assert!(
            normalize_upper(ubuntu.target.efi_path.as_deref().unwrap()).ends_with("SHIMX64.EFI")
        );
        let recovery = found
            .iter()
            .find(|item| item.target.display_name == "SBM Core Slot A")
            .unwrap();
        assert_eq!(recovery.target.id.as_str(), "sbm-core:slot-a");
        assert_eq!(recovery.target.verified_signature, Some(true));
        assert!(recovery.preferred);
        let again = discover_volume(&vol, CpuArch::X86_64, &[r"\EFI\BOOT\BOOTX64.EFI".into()]);
        assert_eq!(
            found
                .iter()
                .map(|item| item.target.id.as_str())
                .collect::<Vec<_>>(),
            again
                .iter()
                .map(|item| item.target.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn external_default_path_stays_visible() {
        let vol = MemoryVolume::new(guid(), true, "USB");
        vol.write(r"\EFI\BOOT\BOOTX64.EFI", &fake_pe(MACHINE_X64))
            .unwrap();
        let found = discover_volume(&vol, CpuArch::X86_64, &[]);
        assert_eq!(found.len(), 1);
        assert!(found[0].target.removable_media);
        assert_eq!(found[0].target.display_name, "External drive");
    }

    #[test]
    fn core_uki_name_follows_the_manifest() {
        let vol = MemoryVolume::new(guid(), false, "ESP");
        let uki = fake_pe(MACHINE_AA64);
        let hash = hex_encode(&sha256(&uki));
        vol.write(r"\EFI\sbm\recovery\slot-a\sbm-core-aarch64.efi", &uki)
            .unwrap();
        let manifest = format!(
            r#"{{"schema_version":"1.0","active_slot":"slot-a","slots":[{{"slot_name":"slot-a","uki_efi_path":"\\EFI\\sbm\\recovery\\slot-a\\sbm-core-aarch64.efi","sha256":"{hash}","version":"0.1.0","healthy":true}}]}}"#
        );
        vol.write(r"\EFI\sbm\recovery\manifest.json", manifest.as_bytes())
            .unwrap();
        let found = discover_volume(&vol, CpuArch::Aarch64, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].target.id.as_str(), "sbm-core:slot-a");
        assert_eq!(found[0].target.verified_signature, Some(true));
        assert!(discover_volume(&vol, CpuArch::X86_64, &[]).is_empty());
    }
}
