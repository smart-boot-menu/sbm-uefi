use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbm_protocol::RecoverySlotPreference;
use sbm_types::{BootTarget, BootTargetId, BootTargetKind, CpuArch, OsFamily};

use crate::bls::{bls_arch_ok, parse_bls};
use crate::nvram::ParsedLoadOption;
use crate::pathutil::{
    bls_target_id, efi_target_id, file_name, normalize, normalize_upper, recovery_target_id,
};
use crate::pe::{
    efi_application_machine, machine_matches, pe_machine, pe_section_span, section_text,
};
use crate::persist::decode_manifest;
use crate::volume::Volume;

const LOAD_OPTION_ACTIVE: u32 = 0x1;
const LOAD_OPTION_HIDDEN: u32 = 0x8;

/// Spec 03 §9 integrity states. A digest match is never a signature result;
/// `BootTarget::verified_signature` stays `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrity {
    /// A loader exists with the native PE machine. No digest is published.
    Discovered,
    /// Recovery passed the startup checks. The digest is not computed yet.
    Eligible,
    /// The streamed SHA-256 matched the manifest just before launch.
    Verified,
    /// Verification failed. The entry is ineligible for this picker session.
    Failed,
}

#[derive(Clone, Debug)]
pub struct DiscoveredTarget {
    pub target: BootTarget,
    pub initrd: Option<String>,
    pub healthy: bool,
    pub preferred: bool,
    pub rank: u8,
    pub integrity: Integrity,
    /// Lowercase hex digest that launch-time verification must match.
    pub expected_sha256: Option<String>,
    /// Why the entry cannot be started, shown when the user selects it.
    pub problem: Option<String>,
}

/// Ordinary loaders on one ESP. Recovery comes from [`discover_recovery`] on
/// the boot ESP only.
pub fn discover_volume(
    vol: &dyn Volume,
    arch: CpuArch,
    skip_paths: &[String],
) -> Vec<DiscoveredTarget> {
    let mut out = Vec::new();
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

/// Slot B and removable recovery are future profiles; 0.0.1 has Slot A only.
pub fn recovery_for_slot(
    targets: &[DiscoveredTarget],
    slot: RecoverySlotPreference,
) -> Option<&DiscoveredTarget> {
    match slot {
        RecoverySlotPreference::Auto | RecoverySlotPreference::SlotA => best_recovery(targets),
        RecoverySlotPreference::SlotB | RecoverySlotPreference::Removable => None,
    }
}

fn selectable_target(target: &DiscoveredTarget) -> bool {
    crate::policy::selectable(target)
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

/// Spec 03 §9 startup checks for the boot ESP's recovery manifest. Reads the
/// manifest and the UKI's PE header only; the digest is verified at launch.
/// Returns `None` when the ESP has no manifest, and an ineligible entry that
/// names the problem when the manifest exists but fails a check.
pub fn discover_recovery(vol: &dyn Volume, arch: CpuArch) -> Option<DiscoveredTarget> {
    let native = canonical_recovery_path(arch)?;
    let bytes = match vol.read(RECOVERY_MANIFEST_PATH) {
        Ok(bytes) => bytes,
        Err(crate::volume::VolError::NotFound) => return None,
        Err(_) => {
            return Some(ineligible_recovery(
                vol,
                arch,
                native,
                "its manifest could not be read",
            ));
        }
    };
    let Ok(manifest) = decode_manifest(&bytes) else {
        return Some(ineligible_recovery(
            vol,
            arch,
            native,
            "its manifest is malformed or has an unsupported schema",
        ));
    };
    if manifest.active_slot != RECOVERY_SLOT {
        return Some(ineligible_recovery(
            vol,
            arch,
            native,
            "its manifest does not select slot-a",
        ));
    }
    let mut chosen = None;
    for slot in &manifest.slots {
        if slot.slot_name != RECOVERY_SLOT {
            continue;
        }
        let path = normalize(&slot.uki_efi_path);
        if path.eq_ignore_ascii_case(native) {
            if chosen.is_some() {
                return Some(ineligible_recovery(
                    vol,
                    arch,
                    native,
                    "its manifest lists this architecture twice",
                ));
            }
            chosen = Some(slot);
        } else if !RECOVERY_ARCHES
            .iter()
            .filter_map(|other| canonical_recovery_path(*other))
            .any(|other| path.eq_ignore_ascii_case(other))
        {
            return Some(ineligible_recovery(
                vol,
                arch,
                native,
                "its manifest names a file outside the recovery layout",
            ));
        }
    }
    let Some(slot) = chosen else {
        return Some(ineligible_recovery(
            vol,
            arch,
            native,
            "its manifest has no entry for this architecture",
        ));
    };
    let problem = if !slot.healthy {
        Some("its publisher marked it unhealthy")
    } else if !is_sha256_hex(&slot.sha256) {
        Some("its manifest digest is not a SHA-256 value")
    } else {
        match vol.metadata(native) {
            Ok(meta) if !meta.is_dir && meta.size > 0 => {
                match vol
                    .read_prefix(native, 4096)
                    .ok()
                    .and_then(|p| efi_application_machine(&p, meta.size))
                {
                    Some(machine) if machine_matches(machine, arch) => None,
                    Some(_) => Some("the image is for another architecture"),
                    None => Some("the image is not a PE executable"),
                }
            }
            Ok(_) => Some("the image is empty"),
            Err(crate::volume::VolError::NotFound) => Some("the image is missing"),
            Err(_) => Some("the image could not be read"),
        }
    };
    if let Some(problem) = problem {
        return Some(ineligible_recovery(vol, arch, native, problem));
    }
    let mut found = recovery_entry(vol, arch, native);
    found.preferred = true;
    found.integrity = Integrity::Eligible;
    found.expected_sha256 = Some(slot.sha256.to_ascii_lowercase());
    Some(found)
}

/// The only recovery slot in the 0.0.1 profile.
pub const RECOVERY_SLOT: &str = "slot-a";
pub const RECOVERY_TARGET_ID: &str = "sbm-core:slot-a";
pub const RECOVERY_MANIFEST_PATH: &str = "\\EFI\\sbm\\recovery\\manifest.json";
const RECOVERY_ARCHES: [CpuArch; 2] = [CpuArch::X86_64, CpuArch::Aarch64];

/// Spec 03 §9 canonical Slot A path for `arch`.
pub fn canonical_recovery_path(arch: CpuArch) -> Option<&'static str> {
    match arch {
        CpuArch::X86_64 => Some("\\EFI\\sbm\\recovery\\slot-a\\sbm-core-x86_64.efi"),
        CpuArch::Aarch64 => Some("\\EFI\\sbm\\recovery\\slot-a\\sbm-core-aarch64.efi"),
        CpuArch::Ia32 | CpuArch::Riscv64 => None,
    }
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn recovery_entry(vol: &dyn Volume, arch: CpuArch, path: &str) -> DiscoveredTarget {
    DiscoveredTarget {
        target: BootTarget {
            id: recovery_target_id(RECOVERY_SLOT),
            display_name: String::from("SBM Core Slot A"),
            os_family: OsFamily::Recovery,
            kind: BootTargetKind::SbmCore,
            arch,
            partition_guid: Some(vol.partition_guid().to_ascii_lowercase()),
            efi_path: Some(normalize(path)),
            kernel_cmdline: None,
            icon_hint: Some(String::from("recovery")),
            removable_media: vol.removable(),
            hidden: false,
            verified_signature: None,
        },
        initrd: None,
        healthy: true,
        preferred: false,
        rank: 100,
        integrity: Integrity::Discovered,
        expected_sha256: None,
        problem: None,
    }
}

fn ineligible_recovery(
    vol: &dyn Volume,
    arch: CpuArch,
    path: &str,
    problem: &str,
) -> DiscoveredTarget {
    let mut found = recovery_entry(vol, arch, path);
    found.healthy = false;
    found.target.display_name = String::from("SBM Core Slot A (unavailable)");
    found.problem = Some(problem.to_string());
    found
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
        integrity: Integrity::Discovered,
        expected_sha256: None,
        problem: None,
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

    const X64_UKI: &str = r"\EFI\sbm\recovery\slot-a\sbm-core-x86_64.efi";
    const AA64_UKI: &str = r"\EFI\sbm\recovery\slot-a\sbm-core-aarch64.efi";

    fn guid() -> &'static str {
        "01234567-89ab-cdef-0123-456789abcdef"
    }

    fn slot(name: &str, path: &str, hash: &str, healthy: bool) -> String {
        alloc::format!(
            r#"{{"slot_name":"{name}","uki_efi_path":"{}","sha256":"{hash}","version":"0.1.0","healthy":{healthy}}}"#,
            path.replace('\\', "\\\\")
        )
    }

    fn manifest(vol: &MemoryVolume, active: &str, slots: &[String]) {
        let text = alloc::format!(
            r#"{{"schema_version":"1.0","active_slot":"{active}","slots":[{}]}}"#,
            slots.join(",")
        );
        vol.write(RECOVERY_MANIFEST_PATH, text.as_bytes()).unwrap();
    }

    /// Dual-architecture media: both UKIs on one ESP, one descriptor each.
    fn dual_arch() -> MemoryVolume {
        let vol = MemoryVolume::new(guid(), true, "SBM");
        let x64 = fake_pe(MACHINE_X64);
        let aa64 = fake_pe(MACHINE_AA64);
        vol.write(X64_UKI, &x64).unwrap();
        vol.write(AA64_UKI, &aa64).unwrap();
        manifest(
            &vol,
            "slot-a",
            &[
                slot("slot-a", X64_UKI, &hex_encode(&sha256(&x64)), true),
                slot("slot-a", AA64_UKI, &hex_encode(&sha256(&aa64)), true),
            ],
        );
        vol
    }

    #[test]
    fn discovers_loaders_without_duplicates_or_self() {
        let vol = MemoryVolume::new(guid(), false, "ESP");
        let x64 = fake_pe(MACHINE_X64);
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
        vol.write(r"\EFI\sbm\sbm-x86_64.efi", &x64).unwrap();
        vol.write(X64_UKI, &x64).unwrap();

        let found = discover_volume(&vol, CpuArch::X86_64, &[r"\EFI\BOOT\BOOTX64.EFI".into()]);
        let names: Vec<_> = found
            .iter()
            .map(|item| item.target.display_name.as_str())
            .collect();
        assert!(names.contains(&"Windows"), "{names:?}");
        assert!(names.contains(&"Ubuntu"), "{names:?}");
        assert!(names.contains(&"systemd-boot"), "{names:?}");
        assert!(names.contains(&"Fedora Linux"), "{names:?}");
        assert!(
            !names
                .iter()
                .any(|name| name.contains("SBM") || name.contains("sbm")),
            "recovery comes only from discover_recovery: {names:?}"
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
        let fedora = found
            .iter()
            .find(|item| item.target.display_name == "Fedora Linux")
            .unwrap();
        assert_eq!(fedora.target.kind, BootTargetKind::BlsUki);
        assert_eq!(fedora.target.kernel_cmdline.as_deref(), Some("quiet"));
        assert!(
            found
                .iter()
                .all(|item| item.integrity == Integrity::Discovered)
        );
        let ubuntu = found
            .iter()
            .find(|item| item.target.display_name == "Ubuntu")
            .unwrap();
        assert!(
            normalize_upper(ubuntu.target.efi_path.as_deref().unwrap()).ends_with("SHIMX64.EFI")
        );
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
    fn native_slot_a_is_eligible_without_hashing() {
        let vol = dual_arch();
        // A wrong byte in the foreign UKI is never read at startup.
        vol.write(X64_UKI, &fake_pe(MACHINE_X64)[..64]).unwrap();
        vol.truncate_reads(AA64_UKI, 0);
        let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
        assert_eq!(found.target.id.as_str(), RECOVERY_TARGET_ID);
        assert_eq!(found.target.efi_path.as_deref(), Some(AA64_UKI));
        assert_eq!(found.integrity, Integrity::Eligible);
        assert_eq!(found.target.verified_signature, None);
        assert!(found.healthy && found.preferred && found.problem.is_none());
        let expected = hex_encode(&sha256(&fake_pe(MACHINE_AA64)));
        assert_eq!(found.expected_sha256.as_deref(), Some(expected.as_str()));
        let x64 = discover_recovery(&dual_arch(), CpuArch::X86_64).unwrap();
        assert_eq!(x64.target.efi_path.as_deref(), Some(X64_UKI));
        assert_eq!(x64.integrity, Integrity::Eligible);
    }

    #[test]
    fn recovery_checks_make_the_entry_ineligible() {
        let aa64 = fake_pe(MACHINE_AA64);
        let hash = hex_encode(&sha256(&aa64));
        type DescriptorExample<'a> = (&'a str, &'a str, &'a str, bool);
        let cases: &[(&str, &[DescriptorExample<'_>], &str)] = &[
            ("slot-b", &[("slot-a", AA64_UKI, &hash, true)], "slot-a"),
            (
                "slot-a",
                &[
                    ("slot-a", AA64_UKI, &hash, true),
                    ("slot-a", AA64_UKI, &hash, true),
                ],
                "twice",
            ),
            (
                "slot-a",
                &[("slot-a", r"\EFI\sbm\recovery\slot-a\other.efi", &hash, true)],
                "outside",
            ),
            ("slot-a", &[("slot-a", X64_UKI, &hash, true)], "no entry"),
            ("slot-a", &[("slot-a", AA64_UKI, &hash, false)], "unhealthy"),
            ("slot-a", &[("slot-a", AA64_UKI, "", true)], "SHA-256"),
            ("slot-a", &[("slot-a", AA64_UKI, "zz", true)], "SHA-256"),
        ];
        for (active, slots, problem) in cases {
            let vol = MemoryVolume::new(guid(), true, "SBM");
            vol.write(AA64_UKI, &aa64).unwrap();
            let slots = slots
                .iter()
                .map(|(name, path, hash, healthy)| slot(name, path, hash, *healthy))
                .collect::<Vec<_>>();
            manifest(&vol, active, &slots);
            let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
            assert!(!crate::policy::selectable(&found), "{problem}");
            assert!(
                found.problem.as_deref().unwrap().contains(problem),
                "{problem}: {:?}",
                found.problem
            );
        }
        let vol = dual_arch();
        vol.write(AA64_UKI, &fake_pe(MACHINE_X64)).unwrap();
        let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
        assert!(found.problem.unwrap().contains("another architecture"));
        vol.write(AA64_UKI, b"").unwrap();
        let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
        assert!(found.problem.unwrap().contains("empty"));
        vol.remove(AA64_UKI).unwrap();
        let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
        assert!(found.problem.unwrap().contains("missing"));
        vol.write(RECOVERY_MANIFEST_PATH, b"{").unwrap();
        let found = discover_recovery(&vol, CpuArch::Aarch64).unwrap();
        assert!(found.problem.unwrap().contains("malformed"));
        vol.remove(RECOVERY_MANIFEST_PATH).unwrap();
        assert!(discover_recovery(&vol, CpuArch::Aarch64).is_none());
    }
}
