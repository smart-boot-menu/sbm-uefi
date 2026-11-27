//! A simulated computer: one `MemoryVolume` per EFI System Partition plus the
//! firmware facts the menu reads. Only ESP contents exist in memory; other
//! partitions are metadata for SBM Core.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sbm_boot::{
    MACHINE_AA64, MACHINE_X64, MemoryVolume, ParsedLoadOption, Volume, atomic_write, begin_attempt,
    canonical_recovery_path, encode_attempt, encode_manifest, encode_policy, fresh_policy,
    hex_encode, normalize, sha256,
};
use sbm_menu::flow::{self, HandoffVar, LaunchPlan, LaunchResult, Platform};
use sbm_menu::{MenuEntry, StartFailure};
use sbm_protocol::{
    BootAttemptStatus, OneShotBoot, RecoveryManifest, RecoverySlotDescriptor as RecoverySlot,
};
use sbm_types::{BootTargetId, CpuArch, FirmwarePlatform, PlatformCapabilities, SCHEMA_VERSION_V1};

use crate::scenario::{PartKind, Scenario, SecureBoot};

pub use sbm_menu::flow::{ATTEMPT_PATH, HANDOFF_PATH, POLICY_PATH, REQUEST_PATH, SUCCESS_PATH};
const MANIFEST_PATH: &str = sbm_boot::RECOVERY_MANIFEST_PATH;

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

/// The simulated computer. It implements the same [`Platform`] the UEFI
/// app does, so discovery, policy, verification and persistence run the
/// production code in `sbm_menu::flow`.
pub struct Machine {
    pub scenario: Scenario,
    pub esps: Vec<SimEsp>,
    faults: Vec<Fault>,
    /// Volatile UEFI variables; cleared on every reset.
    pub vars: BTreeMap<HandoffVar, String>,
    /// `SBM_MARK` lines from the current power-on.
    marks: RefCell<Vec<String>>,
    clock: Cell<Instant>,
    draws: u64,
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
                    // A damaged OS loader fails in the firmware PE loader. A
                    // tampered recovery UKI is caught by launch verification.
                    if part.corrupt.contains(preset) && !preset.starts_with("sbm") {
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
        // The ESP the menu was loaded from. Like firmware, there is no guess
        // when the menu is not installed anywhere.
        let boot = esps
            .iter()
            .position(|esp| {
                esp.self_path
                    .as_deref()
                    .is_some_and(|path| path.contains("\\sbm\\"))
            })
            .or_else(|| esps.iter().position(|esp| esp.self_path.is_some()));
        if let Some(index) = boot {
            esps[index].boot = true;
        }
        let mut machine = Self {
            scenario,
            esps,
            faults,
            vars: BTreeMap::new(),
            marks: RefCell::new(Vec::new()),
            clock: Cell::new(Instant::now()),
            draws: 0,
        };
        machine.write_policy();
        let read_only = machine.read_only_esps();
        for index in read_only {
            machine.esps[index].volume.set_read_only(true);
        }
        machine
    }

    /// A simulated reset: volatile variables and the mark log are gone.
    pub fn reset(&mut self) {
        self.vars.clear();
        self.marks.borrow_mut().clear();
        self.clock.set(Instant::now());
    }

    pub fn marks(&self) -> Vec<String> {
        self.marks.borrow().clone()
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
            Some(path) => vec![normalize(path)],
            None => vec![standard],
        }
    }

    /// Production discovery over this computer.
    pub fn discover(&self) -> Vec<MenuEntry> {
        flow::discover_all(self)
    }

    /// What firmware `LoadImage` would return for the plan's exact device
    /// path. `None` means the image loaded.
    pub fn load_image(&self, plan: &LaunchPlan) -> Option<LaunchResult> {
        let esp = self.esps.get(plan.volume)?;
        let guid = esp.volume.partition_guid();
        if !esp.volume.exists(&plan.path) {
            return Some(LaunchResult::LoadFailed {
                failure: StartFailure::NotFound,
                status: "NOT_FOUND".into(),
            });
        }
        let upper = normalize(&plan.path).to_ascii_uppercase();
        self.faults
            .iter()
            .find(|fault| fault.guid.eq_ignore_ascii_case(guid) && fault.path == upper)
            .map(|fault| LaunchResult::LoadFailed {
                failure: fault.failure,
                status: fault.status.into(),
            })
    }

    pub fn has_load_fault(&self, guid: &str, path: &str) -> bool {
        let upper = normalize(path).to_ascii_uppercase();
        self.faults
            .iter()
            .any(|fault| fault.guid.eq_ignore_ascii_case(guid) && fault.path == upper)
    }

    pub fn read_boot(&self, path: &str) -> Option<Vec<u8>> {
        self.boot_esp()?.volume.read(path).ok()
    }

    pub fn write_boot(&self, path: &str, bytes: &[u8]) -> bool {
        self.boot_esp()
            .is_some_and(|esp| atomic_write(&esp.volume, path, bytes).is_ok())
    }

    fn read_only_esps(&self) -> Vec<usize> {
        self.esps
            .iter()
            .enumerate()
            .filter(|(_, esp)| self.scenario.disks[esp.disk].partitions[esp.part].read_only)
            .map(|(index, _)| index)
            .collect()
    }

    fn write_policy(&mut self) {
        let wanted = self.scenario.policy.clone();
        if wanted.timeout_seconds.is_none()
            && wanted.default.is_none()
            && wanted.default_id.is_none()
            && wanted.max_failed_attempts.is_none()
            && wanted.failed_boots.is_none()
            && wanted.one_shot.is_none()
            && wanted.one_shot_id.is_none()
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
        if let Some(id) = wanted.default_id.as_deref() {
            policy.default_target = Some(BootTargetId(id.into()));
        }
        let one_shot = wanted
            .one_shot
            .as_deref()
            .and_then(|preset| self.target_for_preset(preset))
            .or_else(|| {
                wanted
                    .one_shot_id
                    .as_deref()
                    .map(|id| BootTargetId(id.into()))
            });
        policy.one_shot = one_shot.map(|target_id| OneShotBoot {
            target_id,
            command_id: Some("sim-one-shot".into()),
            expires_at_unix: None,
            consumed: false,
        });
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
                    &uuid_from(&format!("attempt:{}:{index}", self.scenario.id)),
                );
                // Each earlier boot returned to the picker without starting.
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
            .find(|entry| {
                entry
                    .item
                    .target
                    .efi_path
                    .as_deref()
                    .is_some_and(|path| normalize(path).to_ascii_uppercase() == primary)
            })
            .map(|entry| entry.item.target.id)
    }

    pub fn device_for_esp(&self, index: usize) -> Option<&str> {
        self.esps.get(index).map(|esp| esp.device.as_str())
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        let scenario = &self.scenario;
        PlatformCapabilities {
            arch: scenario.arch.cpu(),
            firmware: FirmwarePlatform::UefiNative,
            page_size_bytes: 4096,
            secure_boot_state: scenario.firmware.secure_boot.state(),
            tpm_present: scenario.firmware.tpm,
            nvram_writable: true,
            gop_framebuffer: true,
            uefi_networking: scenario.firmware.network,
            can_chainload_efi: true,
            can_direct_chainload_macos: false,
            external_usb_boot_supported: scenario.disks.iter().any(|disk| disk.removable),
            ram_total_mib: Some(scenario.firmware.ram_mib),
        }
    }
}

impl Platform for Machine {
    fn volume_count(&self) -> usize {
        self.esps.len()
    }

    fn volume(&self, index: usize) -> &dyn Volume {
        &self.esps[index].volume
    }

    fn boot_volume(&self) -> Option<usize> {
        self.esps.iter().position(|esp| esp.boot)
    }

    fn arch(&self) -> CpuArch {
        self.scenario.arch.cpu()
    }

    fn skip_paths(&self, index: usize) -> Vec<String> {
        self.esps
            .get(index)
            .map(|esp| Machine::skip_paths(self, esp))
            .unwrap_or_default()
    }

    fn load_options(&self) -> Vec<ParsedLoadOption> {
        Vec::new()
    }

    fn now_unix(&self) -> Option<i64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|elapsed| elapsed.as_secs() as i64)
    }

    fn elapsed_us(&self) -> u64 {
        self.clock.get().elapsed().as_micros() as u64
    }

    fn mark(&self, text: &str) {
        self.marks.borrow_mut().push(text.to_string());
    }

    /// Deterministic stand-in for `EFI_RNG_PROTOCOL`.
    fn random(&mut self) -> Option<[u8; 16]> {
        if !self.scenario.firmware.rng {
            return None;
        }
        self.draws += 1;
        let digest = sha256(format!("rng:{}:{}", self.scenario.id, self.draws).as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Some(bytes)
    }

    fn entropy(&self) -> Vec<u8> {
        format!("entropy:{}:{}", self.scenario.id, self.elapsed_us()).into_bytes()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        Machine::capabilities(self)
    }

    fn network_link(&self) -> bool {
        self.scenario.firmware.network
    }

    fn set_var(&mut self, var: HandoffVar, value: &str) -> bool {
        self.vars.insert(var, value.to_string());
        true
    }

    fn delete_var(&mut self, var: HandoffVar) {
        self.vars.remove(&var);
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
    let mut bytes = vec![0u8; 0x200];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3c] = 0x40;
    bytes[0x40] = b'P';
    bytes[0x41] = b'E';
    bytes[0x44] = machine as u8;
    bytes[0x45] = (machine >> 8) as u8;
    bytes[0x46] = 1;
    bytes[0x54] = 0xf0;
    bytes[0x56] = 2;
    bytes[0x58..0x5a].copy_from_slice(&[0x0b, 0x02]);
    bytes[0x94..0x98].copy_from_slice(&0x200u32.to_le_bytes());
    bytes[0x9c] = 10;
    bytes.extend_from_slice(format!("SBM SIMULATED EFI BINARY: {name}\n").as_bytes());
    bytes
}

const CORRUPT: &[u8] = b"SBM SIMULATED CORRUPT FILE\0\0\0\0";

fn preset_primary(preset: &str, arch: sbm_types::CpuArch) -> Option<String> {
    let tag = efi_tag(arch);
    let upper = tag.to_ascii_uppercase();
    Some(match preset {
        "windows" => "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(),
        "ubuntu" | "fedora" | "debian" | "opensuse" => format!("\\EFI\\{preset}\\shim{tag}.efi"),
        "arch" => "\\vmlinuz-linux".into(),
        "fedora-uki" => "\\EFI\\Linux\\fedora-45.efi".into(),
        "removable" => format!("\\EFI\\BOOT\\BOOT{upper}.EFI"),
        "sbm" | "sbm-slot-a" | "sbm-media" => canonical_recovery_path(arch)?.into(),
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
            // An installed system carries its own architecture's UKI.
            install_recovery(vol, &[arch], arch, corrupt);
        }
        "sbm-media" => {
            put(
                &format!("\\EFI\\BOOT\\BOOT{}.EFI", tag.to_ascii_uppercase()),
                &pe("Smart Boot Menu (removable media)"),
            );
            // The 0.0.1 media is dual-architecture: one slot-a descriptor
            // per architecture at the canonical paths.
            install_recovery(vol, &[CpuArch::X86_64, CpuArch::Aarch64], arch, corrupt);
        }
        _ => {}
    }
    primary
}

fn install_recovery(vol: &MemoryVolume, arches: &[CpuArch], native: CpuArch, corrupt: &[String]) {
    let mut entries = Vec::new();
    for &arch in arches {
        let Some(path) = canonical_recovery_path(arch) else {
            continue;
        };
        let good = synthetic_pe(
            arch,
            &format!("SBM Core UKI slot-a {}", sbm_menu::arch_name(arch)),
        );
        let hash = hex_encode(&sha256(&good));
        // Tampered but structurally valid: discovery accepts it and the
        // launch-time SHA-256 check refuses it.
        let damaged = arch == native && corrupt.iter().any(|item| item == "sbm-slot-a");
        let bytes = if damaged {
            synthetic_pe(arch, "SBM Core UKI slot-a TAMPERED")
        } else {
            good
        };
        let _ = vol.write(path, &bytes);
        entries.push(RecoverySlot {
            slot_name: "slot-a".into(),
            uki_efi_path: path.into(),
            sha256: hash,
            version: "0.0.1-sim".into(),
            healthy: true,
        });
    }
    let manifest = RecoveryManifest {
        schema_version: SCHEMA_VERSION_V1.into(),
        active_slot: "slot-a".into(),
        slots: entries,
    };
    if let Ok(bytes) = encode_manifest(&manifest) {
        let _ = vol.write(MANIFEST_PATH, &bytes);
    }
}
