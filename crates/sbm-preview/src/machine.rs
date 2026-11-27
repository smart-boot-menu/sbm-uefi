//! A simulated computer: one `MemoryVolume` per EFI System Partition plus the
//! firmware facts the menu reads. Only ESP contents exist in memory; other
//! partitions are metadata for SBM Core.

use sbm_boot::{
    DiscoveredTarget, MACHINE_AA64, MACHINE_X64, MemoryVolume, Volume, atomic_write, begin_attempt,
    discover_volume, encode_attempt, encode_manifest, encode_policy, fresh_policy, hex_encode,
    normalize, sha256,
};
use sbm_menu::StartFailure;
use sbm_protocol::{BootAttemptStatus, RecoveryManifest, RecoverySlotDescriptor as RecoverySlot};
use sbm_types::{BootTargetId, SCHEMA_VERSION_V1};

use crate::scenario::{PartKind, Scenario, SecureBoot};

pub const POLICY_PATH: &str = "\\EFI\\sbm\\policy.json";
pub const ATTEMPT_PATH: &str = "\\EFI\\sbm\\state\\last_attempt.json";
pub const HANDOFF_PATH: &str = "\\EFI\\sbm\\handoff\\current.json";
pub const SUCCESS_PATH: &str = "\\EFI\\sbm\\state\\boot-success.json";
pub const COMMAND_PATH: &str = "\\EFI\\sbm\\handoff\\requested-command.json";
const MANIFEST_PATH: &str = "\\EFI\\sbm\\recovery\\manifest.json";

const PRESETS: &[&str] = &[
    "windows",
    "ubuntu",
    "fedora",
    "debian",
    "opensuse",
    "arch",
    "fedora-uki",
    "removable",
    "sbm",
    "sbm-media",
    "sbm-slot-a",
    "sbm-slot-b",
];

pub fn known_preset(name: &str) -> bool {
    PRESETS.contains(&name)
}

pub struct SimEsp {
    pub disk: usize,
    pub part: usize,
    pub device: String,
    pub volume: MemoryVolume,
    pub boot: bool,
    /// Path the running menu was loaded from, skipped during discovery.
    pub self_path: Option<String>,
}

struct Fault {
    guid: String,
    path: String,
    failure: StartFailure,
    status: &'static str,
}

pub struct Machine {
    pub scenario: Scenario,
    pub esps: Vec<SimEsp>,
    faults: Vec<Fault>,
}

impl Machine {
    pub fn new(scenario: Scenario) -> Self {
        let arch = scenario.arch;
        let mut esps = Vec::new();
        let mut faults = Vec::new();
        for (disk_index, disk) in scenario.disks.iter().enumerate() {
            for (part_index, part) in disk.partitions.iter().enumerate() {
                if part.kind != PartKind::Esp {
                    continue;
                }
                let guid = part_guid(&scenario.id, &disk.name, part_index);
                let volume = MemoryVolume::new(&guid, disk.removable, &part.name);
                let mut self_path = None;
                for preset in &part.loaders {
                    let primary = install_preset(&volume, preset, arch.cpu(), &part.corrupt);
                    if preset == "sbm" {
                        self_path = Some(format!("\\EFI\\sbm\\sbm-{}.efi", arch.name()));
                    } else if preset == "sbm-media" && self_path.is_none() {
                        self_path = Some(format!(
                            "\\EFI\\BOOT\\BOOT{}.EFI",
                            arch.efi_tag().to_ascii_uppercase()
                        ));
                    }
                    if part.rejected.contains(preset)
                        && scenario.firmware.secure_boot == SecureBoot::Enabled
                    {
                        faults.push(Fault {
                            guid: guid.clone(),
                            path: primary.to_ascii_uppercase(),
                            failure: StartFailure::SecurityViolation,
                            status: "SECURITY_VIOLATION",
                        });
                    }
                    if part.corrupt.contains(preset) {
                        faults.push(Fault {
                            guid: guid.clone(),
                            path: primary.to_ascii_uppercase(),
                            failure: StartFailure::Other,
                            status: "LOAD_ERROR",
                        });
                    }
                }
                esps.push(SimEsp {
                    disk: disk_index,
                    part: part_index,
                    device: partition_device(&disk.name, part_index + 1),
                    volume,
                    boot: false,
                    self_path,
                });
            }
        }
        let boot = esps
            .iter()
            .position(|esp| {
                esp.self_path
                    .as_deref()
                    .is_some_and(|path| path.contains("\\sbm\\"))
            })
            .or_else(|| esps.iter().position(|esp| esp.self_path.is_some()))
            .or(if esps.is_empty() { None } else { Some(0) });
        if let Some(index) = boot {
            esps[index].boot = true;
        }
        let mut machine = Self {
            scenario,
            esps,
            faults,
        };
        machine.write_policy();
        machine
    }

    pub fn boot_esp(&self) -> Option<&SimEsp> {
        self.esps.iter().find(|esp| esp.boot)
    }

    pub fn skip_paths(&self, esp: &SimEsp) -> Vec<String> {
        if !esp.boot {
            return Vec::new();
        }
        let standard = format!(
            "\\EFI\\BOOT\\BOOT{}.EFI",
            self.scenario.arch.efi_tag().to_ascii_uppercase()
        );
        match &esp.self_path {
            Some(path) => {
                let mut skip = vec![normalize(path)];
                if path.eq_ignore_ascii_case(&standard) {
                    skip.dedup();
                }
                skip
            }
            None => vec![standard],
        }
    }

    pub fn discover(&self) -> Vec<(usize, DiscoveredTarget)> {
        let arch = self.scenario.arch.cpu();
        let mut out = Vec::new();
        for (index, esp) in self.esps.iter().enumerate() {
            let skip = self.skip_paths(esp);
            for item in discover_volume(&esp.volume, arch, &skip) {
                out.push((index, item));
            }
        }
        out
    }

    /// What firmware would return from `LoadImage`/`StartImage` for this file.
    pub fn start_result(&self, guid: &str, path: &str) -> Option<(StartFailure, &'static str)> {
        let upper = normalize(path).to_ascii_uppercase();
        self.faults
            .iter()
            .find(|fault| fault.guid.eq_ignore_ascii_case(guid) && fault.path == upper)
            .map(|fault| (fault.failure, fault.status))
    }

    pub fn read_boot(&self, path: &str) -> Option<Vec<u8>> {
        self.boot_esp()?.volume.read(path).ok()
    }

    pub fn write_boot(&self, path: &str, bytes: &[u8]) -> bool {
        self.boot_esp()
            .is_some_and(|esp| atomic_write(&esp.volume, path, bytes).is_ok())
    }

    pub fn remove_any(&self, path: &str) -> Option<Vec<u8>> {
        let mut order: Vec<&SimEsp> = self.esps.iter().filter(|esp| esp.boot).collect();
        order.extend(self.esps.iter().filter(|esp| !esp.boot));
        for esp in order {
            if let Ok(bytes) = esp.volume.read(path) {
                let _ = esp.volume.remove(path);
                return Some(bytes);
            }
        }
        None
    }

    fn write_policy(&mut self) {
        let wanted = &self.scenario.policy;
        if wanted.timeout_seconds.is_none()
            && wanted.default.is_none()
            && wanted.max_failed_attempts.is_none()
            && wanted.failed_boots.is_none()
        {
            return;
        }
        let mut policy = fresh_policy();
        if let Some(timeout) = wanted.timeout_seconds {
            policy.timeout_seconds = timeout;
        }
        if let Some(max) = wanted.max_failed_attempts {
            policy.max_failed_attempts = max;
        }
        if let Some(preset) = wanted.default.as_deref() {
            policy.default_target = self.target_for_preset(preset);
        }
        if let Ok(bytes) = encode_policy(&policy) {
            self.write_boot(POLICY_PATH, &bytes);
        }
        if let (Some(count), Some(target)) = (wanted.failed_boots, policy.default_target.as_ref()) {
            let mut record = None;
            for index in 0..count {
                let mut next = begin_attempt(
                    record.as_ref(),
                    target,
                    None,
                    &format!("uefi-sim-history-{index}"),
                );
                next.status = BootAttemptStatus::Failed;
                record = Some(next);
            }
            if let Some(record) = record
                && let Ok(bytes) = encode_attempt(&record)
            {
                self.write_boot(ATTEMPT_PATH, &bytes);
            }
        }
    }

    pub fn target_for_preset(&self, preset: &str) -> Option<BootTargetId> {
        let arch = self.scenario.arch.cpu();
        let primary = normalize(&preset_primary(preset, arch)?).to_ascii_uppercase();
        self.discover()
            .into_iter()
            .find(|(_, item)| {
                item.target
                    .efi_path
                    .as_deref()
                    .is_some_and(|path| normalize(path).to_ascii_uppercase() == primary)
            })
            .map(|(_, item)| item.target.id)
    }

    pub fn device_for_esp(&self, index: usize) -> Option<&str> {
        self.esps.get(index).map(|esp| esp.device.as_str())
    }
}

pub fn part_guid(scenario: &str, disk: &str, index: usize) -> String {
    uuid_from(&format!("part:{scenario}:{disk}:{index}"))
}

pub fn disk_guid(scenario: &str, disk: &str) -> String {
    uuid_from(&format!("disk:{scenario}:{disk}"))
}

fn uuid_from(seed: &str) -> String {
    let hex = hex_encode(&sha256(seed.as_bytes()));
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

pub fn partition_device(disk: &str, number: usize) -> String {
    if disk.ends_with(|ch: char| ch.is_ascii_digit()) {
        format!("/dev/{disk}p{number}")
    } else {
        format!("/dev/{disk}{number}")
    }
}

fn machine_code(arch: sbm_types::CpuArch) -> u16 {
    match arch {
        sbm_types::CpuArch::Aarch64 => MACHINE_AA64,
        _ => MACHINE_X64,
    }
}

/// A PE header with the right machine type and a visible simulation marker.
pub fn synthetic_pe(arch: sbm_types::CpuArch, name: &str) -> Vec<u8> {
    let machine = machine_code(arch);
    let mut bytes = vec![0u8; 0x80];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3c] = 0x40;
    bytes[0x40] = b'P';
    bytes[0x41] = b'E';
    bytes[0x44] = machine as u8;
    bytes[0x45] = (machine >> 8) as u8;
    bytes.extend_from_slice(format!("SBM SIMULATED EFI BINARY: {name}\n").as_bytes());
    bytes
}

const CORRUPT: &[u8] = b"SBM SIMULATED CORRUPT FILE\0\0\0\0";

fn preset_primary(preset: &str, arch: sbm_types::CpuArch) -> Option<String> {
    let tag = efi_tag(arch);
    let upper = tag.to_ascii_uppercase();
    let arch_name = sbm_menu::arch_name(arch);
    Some(match preset {
        "windows" => "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(),
        "ubuntu" | "fedora" | "debian" | "opensuse" => format!("\\EFI\\{preset}\\shim{tag}.efi"),
        "arch" => "\\vmlinuz-linux".into(),
        "fedora-uki" => "\\EFI\\Linux\\fedora-45.efi".into(),
        "removable" => format!("\\EFI\\BOOT\\BOOT{upper}.EFI"),
        "sbm" | "sbm-slot-a" => format!("\\EFI\\sbm\\recovery\\slot-a\\sbm-core-{arch_name}.efi"),
        "sbm-slot-b" => format!("\\EFI\\sbm\\recovery\\slot-b\\sbm-core-{arch_name}.efi"),
        "sbm-media" => format!("\\EFI\\sbm\\recovery\\removable\\sbm-core-{arch_name}.efi"),
        _ => return None,
    })
}

fn efi_tag(arch: sbm_types::CpuArch) -> &'static str {
    match arch {
        sbm_types::CpuArch::Aarch64 => "aa64",
        _ => "x64",
    }
}

fn install_preset(
    vol: &MemoryVolume,
    preset: &str,
    arch: sbm_types::CpuArch,
    corrupt: &[String],
) -> String {
    let tag = efi_tag(arch);
    let primary = preset_primary(preset, arch).unwrap_or_default();
    let pe = |name: &str| synthetic_pe(arch, name);
    let damaged = corrupt.iter().any(|item| item == preset);
    let main = |name: &str| if damaged { CORRUPT.to_vec() } else { pe(name) };
    let put = |path: &str, bytes: &[u8]| {
        let _ = vol.write(path, bytes);
    };
    match preset {
        "windows" => {
            put(&primary, &main("Windows Boot Manager"));
            put("\\EFI\\Microsoft\\Boot\\BCD", b"SBM SIMULATED BCD STORE");
        }
        "ubuntu" | "fedora" | "debian" | "opensuse" => {
            put(&primary, &main(&format!("{preset} shim")));
            put(
                &format!("\\EFI\\{preset}\\grub{tag}.efi"),
                &pe(&format!("{preset} grub")),
            );
            put(&format!("\\EFI\\{preset}\\mm{tag}.efi"), &pe("MokManager"));
            put(
                &format!("\\EFI\\{preset}\\grub.cfg"),
                b"# SBM SIMULATED grub.cfg\n",
            );
        }
        "arch" => {
            put(
                &format!("\\EFI\\systemd\\systemd-boot{tag}.efi"),
                &pe("systemd-boot"),
            );
            put(&primary, &main("Linux EFI stub kernel"));
            put("\\initramfs-linux.img", b"SBM SIMULATED INITRAMFS");
            put(
                "\\loader\\entries\\arch.conf",
                format!(
                    "title Arch Linux\nlinux /vmlinuz-linux\ninitrd /initramfs-linux.img\noptions root=LABEL=arch rw quiet\narchitecture {tag}\n"
                )
                .as_bytes(),
            );
        }
        "fedora-uki" => {
            put(&primary, &main("Fedora UKI"));
            put(
                "\\loader\\entries\\fedora-45.conf",
                format!(
                    "title Fedora Linux 45\nefi /EFI/Linux/fedora-45.efi\narchitecture {tag}\n"
                )
                .as_bytes(),
            );
        }
        "removable" => put(&primary, &main("removable media loader")),
        "sbm" => {
            put(
                &format!("\\EFI\\sbm\\sbm-{}.efi", sbm_menu::arch_name(arch)),
                &pe("Smart Boot Menu"),
            );
            install_recovery(
                vol,
                arch,
                "slot-a",
                &[("slot-a", true), ("slot-b", true)],
                corrupt,
            );
        }
        "sbm-media" => {
            put(
                &format!("\\EFI\\BOOT\\BOOT{}.EFI", tag.to_ascii_uppercase()),
                &pe("Smart Boot Menu (removable media)"),
            );
            install_recovery(vol, arch, "removable", &[("removable", true)], corrupt);
        }
        _ => {}
    }
    primary
}

fn install_recovery(
    vol: &MemoryVolume,
    arch: sbm_types::CpuArch,
    active: &str,
    slots: &[(&str, bool)],
    corrupt: &[String],
) {
    let arch_name = sbm_menu::arch_name(arch);
    let mut entries = Vec::new();
    for (slot, healthy) in slots {
        let path = format!("\\EFI\\sbm\\recovery\\{slot}\\sbm-core-{arch_name}.efi");
        let good = synthetic_pe(arch, &format!("SBM Core UKI {slot}"));
        let hash = hex_encode(&sha256(&good));
        let damaged = corrupt.iter().any(|item| item == &format!("sbm-{slot}"));
        let _ = vol.write(&path, if damaged { CORRUPT } else { &good });
        entries.push(RecoverySlot {
            slot_name: (*slot).into(),
            uki_efi_path: path,
            sha256: hash,
            version: "0.1.0-sim".into(),
            healthy: *healthy,
        });
    }
    let manifest = RecoveryManifest {
        schema_version: SCHEMA_VERSION_V1.into(),
        active_slot: active.into(),
        slots: entries,
    };
    if let Ok(bytes) = encode_manifest(&manifest) {
        let _ = vol.write(MANIFEST_PATH, &bytes);
    }
}
