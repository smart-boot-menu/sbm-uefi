//! Maps a simulated `Machine` + discovered `MenuEntry` list into the
//! `sbm_gfx::hardware::SystemSpec` view model, with exact LBA-ordered regions
//! (partitions + unallocated gaps), simulated progressive hardware discovery,
//! and missing-information states.

use sbm_boot::selectable;
use sbm_gfx::hardware::{
    BoardSpec, BootTargetView, CpuSpec, DiskRegion, DisplaySpec, FirmwareSpec, MemorySpec, NicSpec,
    PartitionKind, PartitionView, ProbeState, RegionKind, SecureBootSummary, SecuritySpec,
    StorageDevice, SystemSpec,
};
use sbm_menu::{MenuEntry, VERSION};
use sbm_types::{BootTargetKind, OsFamily};

use crate::logos::ResolvedHardwareLogos;
use crate::machine::Machine;
use crate::scenario::{Disk, PartKind, Partition, SecureBoot};

const SECTOR_SIZE: u64 = 512;
const GPT_FIRST_USABLE_LBA: u64 = 2048; // 1 MiB alignment
const MIN_GAP_BYTES: u64 = 1024 * 1024; // 1 MiB threshold for showing unallocated gaps

/// Simulated hardware discovery behavior for prototyping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// All available hardware facts present on frame 0.
    #[default]
    Instant,
    /// Bootable storage + display + firmware ready at 0 ms; optional hardware
    /// probes arrive progressively over ~950 ms without blocking boot choice.
    Progressive,
    /// Same staged arrival at 3x delay (~2.85 s) to inspect intermediate states.
    Slow,
    /// Simulates firmware/SMBIOS omitting optional specs (CPU model, board,
    /// GPU, TPM, network, some disk models) to verify honest fallback states.
    Sparse,
}

impl DiscoveryMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Progressive => "progressive",
            Self::Slow => "slow",
            Self::Sparse => "sparse",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Progressive => "progressive",
            Self::Slow => "slow (3x)",
            Self::Sparse => "sparse (missing info)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Progressive => Self::Slow,
            Self::Slow => Self::Sparse,
            Self::Sparse => Self::Instant,
            Self::Instant => Self::Progressive,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "instant" => Some(Self::Instant),
            "progressive" | "prog" => Some(Self::Progressive),
            "slow" => Some(Self::Slow),
            "sparse" | "missing" => Some(Self::Sparse),
            _ => None,
        }
    }
}

/// Staged probe arrival schedule (milliseconds).
#[derive(Clone, Copy, Debug)]
pub struct DiscoverySchedule {
    pub mode: DiscoveryMode,
    /// Optional total duration override in ms (scales all progressive stages).
    pub delay_ms: Option<u32>,
}

impl Default for DiscoverySchedule {
    fn default() -> Self {
        Self {
            mode: DiscoveryMode::Instant,
            delay_ms: None,
        }
    }
}

impl DiscoverySchedule {
    pub fn new(mode: DiscoveryMode, delay_ms: Option<u32>) -> Self {
        Self { mode, delay_ms }
    }

    /// Thresholds in milliseconds for: [cpu_ram, security, board_gpu, network].
    /// Storage + display + firmware are ALWAYS ready at 0 ms so boot selection
    /// is never blocked by optional hardware probes.
    pub fn thresholds(self) -> [u32; 4] {
        match self.mode {
            DiscoveryMode::Instant | DiscoveryMode::Sparse => [0, 0, 0, 0],
            DiscoveryMode::Progressive => self.scale([180, 380, 620, 950]),
            DiscoveryMode::Slow => self.scale([540, 1140, 1860, 2850]),
        }
    }

    fn scale(self, base: [u32; 4]) -> [u32; 4] {
        let Some(total) = self.delay_ms else {
            return base;
        };
        if total == 0 {
            return [0, 0, 0, 0];
        }
        let max_base = base[3].max(1);
        [
            (base[0] as u64 * total as u64 / max_base as u64) as u32,
            (base[1] as u64 * total as u64 / max_base as u64) as u32,
            (base[2] as u64 * total as u64 / max_base as u64) as u32,
            total,
        ]
    }

    pub fn settled_count(self, elapsed_ms: u32) -> usize {
        self.thresholds()
            .into_iter()
            .filter(|&t| elapsed_ms >= t)
            .count()
    }

    pub fn is_probing(self, elapsed_ms: u32) -> bool {
        self.settled_count(elapsed_ms) < 4
    }
}

/// Build the hardware + LBA-ordered storage view model for the current frame.
#[allow(clippy::too_many_arguments)]
pub fn build_system_spec(
    machine: &Machine,
    entries: &[MenuEntry],
    visible: &[usize],
    last_reason: &'static str,
    schedule: DiscoverySchedule,
    elapsed_ms: u32,
    width: u32,
    height: u32,
    logos: &ResolvedHardwareLogos,
) -> SystemSpec {
    let scenario = &machine.scenario;
    let [t_cpu, t_sec, t_gpu, t_net] = schedule.thresholds();
    let sparse = schedule.mode == DiscoveryMode::Sparse;

    let cpu = if elapsed_ms < t_cpu {
        ProbeState::Pending
    } else if sparse {
        ProbeState::Ready(CpuSpec {
            model: None,
            arch: scenario.arch.name().into(),
            cores: None,
        })
    } else {
        let model = (!scenario.hardware.cpu.is_empty()).then(|| scenario.hardware.cpu.clone());
        let cores = (scenario.hardware.cores > 0).then_some(scenario.hardware.cores as u32);
        ProbeState::Ready(CpuSpec {
            model,
            arch: scenario.arch.name().into(),
            cores,
        })
    };

    let memory = if elapsed_ms < t_cpu {
        ProbeState::Pending
    } else if scenario.firmware.ram_mib == 0 {
        ProbeState::Unavailable
    } else {
        ProbeState::Ready(MemorySpec {
            total_mib: scenario.firmware.ram_mib,
        })
    };

    let board = if elapsed_ms < t_gpu {
        ProbeState::Pending
    } else if sparse {
        ProbeState::Unavailable
    } else {
        ProbeState::Ready(BoardSpec {
            vendor: (!scenario.hardware.vendor.is_empty())
                .then(|| scenario.hardware.vendor.clone()),
            product: (!scenario.hardware.product.is_empty())
                .then(|| scenario.hardware.product.clone()),
        })
    };

    let gpu = if elapsed_ms < t_gpu {
        ProbeState::Pending
    } else if sparse {
        ProbeState::Unavailable
    } else {
        ProbeState::Ready(scenario.hardware.gpus.clone())
    };

    let firmware = ProbeState::Ready(FirmwareSpec {
        vendor: (!sparse && !scenario.firmware.vendor.is_empty())
            .then(|| scenario.firmware.vendor.clone()),
        version: VERSION.into(),
        arch: scenario.arch.name().into(),
    });

    let security = if elapsed_ms < t_sec {
        ProbeState::Pending
    } else {
        ProbeState::Ready(SecuritySpec {
            secure_boot: match scenario.firmware.secure_boot {
                SecureBoot::Enabled => SecureBootSummary::Enabled,
                SecureBoot::Disabled => SecureBootSummary::Disabled,
                SecureBoot::SetupMode => SecureBootSummary::SetupMode,
                SecureBoot::Unsupported => SecureBootSummary::Unsupported,
            },
            tpm_present: if sparse {
                None
            } else {
                Some(scenario.firmware.tpm)
            },
        })
    };

    let network = if elapsed_ms < t_net {
        ProbeState::Pending
    } else if sparse {
        ProbeState::Unavailable
    } else {
        let nics = scenario
            .nics
            .iter()
            .map(|nic| NicSpec {
                name: nic.name.clone(),
                wireless: nic.wireless,
                up: nic.state.eq_ignore_ascii_case("up"),
                address: nic.addresses.first().cloned(),
            })
            .collect();
        ProbeState::Ready(nics)
    };

    let storage = ProbeState::Ready(build_storage_devices(machine, entries, visible, sparse));

    let settled = schedule.settled_count(elapsed_ms);
    let discovery_status = if settled < 4 {
        Some(format!("PROBING {settled}/4"))
    } else if sparse {
        Some("SPARSE SMBIOS".into())
    } else {
        None
    };

    let policy_note = match last_reason {
        "menu" => None,
        other => Some(format!("POLICY: {}", other.to_ascii_uppercase())),
    };

    let host_label = if sparse {
        format!(
            "{} · {} (missing optional SMBIOS)",
            scenario.id,
            scenario.arch.name()
        )
    } else {
        format!("{} · {}", scenario.hardware.product, scenario.id)
    };

    let cpu_logo = match &cpu {
        ProbeState::Ready(spec) if spec.model.is_some() => logos.cpu_logo.clone(),
        _ => None,
    };
    let gpu_logo = match &gpu {
        ProbeState::Ready(gpus) if !gpus.is_empty() => logos.gpu_logo.clone(),
        _ => None,
    };

    SystemSpec {
        host_label,
        sbm_version: VERSION.into(),
        cpu,
        cpu_logo,
        memory,
        board,
        gpu,
        gpu_logo,
        firmware,
        display: DisplaySpec { width, height },
        security,
        network,
        storage,
        policy_note,
        discovery_status,
    }
}

fn build_storage_devices(
    machine: &Machine,
    entries: &[MenuEntry],
    visible: &[usize],
    sparse: bool,
) -> Vec<StorageDevice> {
    let disks = &machine.scenario.disks;
    let mut nvme_n = 0usize;
    let mut sata_n = 0usize;
    let mut usb_n = 0usize;
    let mut other_n = 0usize;

    // Collect boot targets grouped by `(physical_disk_index, host_partition_1based)`.
    // When an OS partition is unambiguously inferred on the same disk, attach
    // the boot target to that OS partition while preserving explicit `esp_part`
    // provenance; otherwise attach it to the ESP itself (e.g. SBM Core / USB).
    let mut targets_by_part: Vec<Vec<(usize, BootTargetView)>> = vec![Vec::new(); disks.len()];
    let mut loader_counts: Vec<Vec<usize>> = disks
        .iter()
        .map(|d| vec![0usize; d.partitions.len()])
        .collect();

    for (card_index, &entry_index) in visible.iter().enumerate() {
        let Some(entry) = entries.get(entry_index) else {
            continue;
        };
        let Some(esp) = machine.esps.get(entry.disk) else {
            continue;
        };
        let Some(disk_cfg) = disks.get(esp.disk) else {
            continue;
        };
        if let Some(cnt) = loader_counts
            .get_mut(esp.disk)
            .and_then(|v| v.get_mut(esp.part))
        {
            *cnt += 1;
        }
        let target = &entry.item.target;
        let esp_part = esp.part + 1;
        let os_part = infer_os_partition(disk_cfg, target);
        let host_part = os_part.unwrap_or(esp_part);
        let healthy = selectable(&entry.item);
        let is_recovery = target.kind == BootTargetKind::SbmCore;
        let loader_file = target
            .efi_path
            .as_deref()
            .and_then(|p| p.rsplit(['\\', '/']).next())
            .unwrap_or("efi")
            .to_string();

        let note = if !healthy {
            Some("CORRUPT".into())
        } else if target.os_family == OsFamily::Windows
            && disk_cfg.partitions.iter().any(|p| p.bitlocker)
        {
            Some("BITLOCKER".into())
        } else if target.os_family == OsFamily::Linux
            && disk_cfg.partitions.iter().any(|p| p.kind == PartKind::Luks)
        {
            Some("LUKS".into())
        } else if target
            .efi_path
            .as_deref()
            .zip(target.partition_guid.as_deref())
            .is_some_and(|(path, guid)| machine.has_load_fault(guid, path))
        {
            Some("SB REJECT".into())
        } else {
            None
        };

        let hint = icon_hint(target, disk_cfg.removable);
        let family = if target.kind == BootTargetKind::SbmCore {
            "Recovery"
        } else if disk_cfg.removable {
            "Removable"
        } else {
            match target.os_family {
                OsFamily::Windows => "Windows",
                OsFamily::Linux => "Linux",
                OsFamily::MacOs => "macOS",
                OsFamily::Bsd => "BSD",
                OsFamily::Recovery => "Recovery",
                OsFamily::FirmwareUtility => "Firmware",
                OsFamily::Other => "Other",
            }
        };

        targets_by_part[esp.disk].push((
            host_part,
            BootTargetView {
                card_index,
                title: target.display_name.clone(),
                family: family.into(),
                hint: hint.into(),
                loader_file,
                esp_part: Some(esp_part),
                os_part,
                healthy,
                is_recovery,
                note,
            },
        ));
    }

    disks
        .iter()
        .enumerate()
        .map(|(disk_idx, disk)| {
            let designator = if disk.removable || disk.bus.eq_ignore_ascii_case("usb") {
                let d = format!("USB{usb_n}");
                usb_n += 1;
                d
            } else if disk.bus.eq_ignore_ascii_case("nvme") {
                let d = format!("NVME{nvme_n}");
                nvme_n += 1;
                d
            } else if disk.bus.eq_ignore_ascii_case("sata") {
                let d = format!("SATA{sata_n}");
                sata_n += 1;
                d
            } else {
                let d = format!("BLK{other_n}");
                other_n += 1;
                d
            };

            let total_sectors = (disk.size.0 / SECTOR_SIZE).max(GPT_FIRST_USABLE_LBA + 34);
            let last_usable_lba = total_sectors.saturating_sub(34);
            let mut placed: Vec<(u64, u64, PartitionView)> =
                Vec::with_capacity(disk.partitions.len());
            let mut cursor_lba = GPT_FIRST_USABLE_LBA;

            for (part_idx, part) in disk.partitions.iter().enumerate() {
                let part_num = part_idx + 1;
                let sectors = (part.size.0 / SECTOR_SIZE).max(1);
                let gap_sectors = part.gap_before.map(|g| g.0 / SECTOR_SIZE).unwrap_or(0);
                let start_lba = match part.start_lba {
                    Some(explicit) => explicit.max(GPT_FIRST_USABLE_LBA),
                    None => cursor_lba.saturating_add(gap_sectors),
                };
                let end_lba = start_lba.saturating_add(sectors).min(last_usable_lba);
                cursor_lba = end_lba;

                let boot_targets: Vec<BootTargetView> = targets_by_part[disk_idx]
                    .iter()
                    .filter(|(hp, _)| *hp == part_num)
                    .map(|(_, t)| t.clone())
                    .collect();
                let count = loader_counts[disk_idx][part_idx];
                placed.push((
                    start_lba,
                    end_lba,
                    map_partition(part_num, part, count, boot_targets),
                ));
            }

            // Law #1: Strictly sort by ascending `start_lba` and synthesize explicit
            // `Unallocated` regions for every gap >= 1 MiB.
            placed.sort_by_key(|(start, _, _)| *start);

            let mut regions = Vec::new();
            let mut prev_end = GPT_FIRST_USABLE_LBA;
            for (start, end, view) in placed {
                if start > prev_end {
                    let gap_bytes = (start - prev_end).saturating_mul(SECTOR_SIZE);
                    if gap_bytes >= MIN_GAP_BYTES {
                        regions.push(DiskRegion {
                            start_lba: prev_end,
                            end_lba: start,
                            size_bytes: gap_bytes,
                            kind: RegionKind::Unallocated,
                        });
                    }
                }
                let size_bytes = end
                    .saturating_sub(start)
                    .saturating_mul(SECTOR_SIZE)
                    .max(SECTOR_SIZE);
                regions.push(DiskRegion {
                    start_lba: start,
                    end_lba: end,
                    size_bytes,
                    kind: RegionKind::Partition(view),
                });
                prev_end = prev_end.max(end);
            }
            if last_usable_lba > prev_end {
                let tail_bytes = (last_usable_lba - prev_end).saturating_mul(SECTOR_SIZE);
                if tail_bytes >= MIN_GAP_BYTES {
                    regions.push(DiskRegion {
                        start_lba: prev_end,
                        end_lba: last_usable_lba,
                        size_bytes: tail_bytes,
                        kind: RegionKind::Unallocated,
                    });
                }
            }

            let hide_model = sparse && (disk_idx + 1 == disks.len());
            let model = (!hide_model && !disk.model.is_empty()).then(|| disk.model.clone());

            StorageDevice {
                designator,
                device_name: disk.name.clone(),
                bus: disk.bus.clone(),
                removable: disk.removable,
                model,
                size_bytes: disk.size.0,
                sector_size: SECTOR_SIZE as u32,
                total_sectors,
                regions,
            }
        })
        .collect()
}

fn map_partition(
    index: usize,
    part: &Partition,
    loader_count: usize,
    boot_targets: Vec<BootTargetView>,
) -> PartitionView {
    let kind = match part.kind {
        PartKind::Esp => PartitionKind::Esp,
        PartKind::Msr => PartitionKind::Msr,
        PartKind::Windows => PartitionKind::Windows,
        PartKind::WindowsRecovery => PartitionKind::Recovery,
        PartKind::Linux | PartKind::LinuxHome => PartitionKind::Linux,
        PartKind::Swap => PartitionKind::Swap,
        PartKind::Luks => PartitionKind::Luks,
        PartKind::Data => PartitionKind::Data,
        PartKind::Apfs => PartitionKind::Apfs,
    };
    let encrypted = if part.bitlocker {
        Some("BitLocker".into())
    } else if part.kind == PartKind::Luks {
        Some("LUKS".into())
    } else {
        None
    };
    let detected_os = match part.kind {
        PartKind::Windows => Some(part.label.clone().unwrap_or_else(|| "Windows".into())),
        PartKind::Linux => Some(part.label.clone().unwrap_or_else(|| part.name.clone())),
        PartKind::Luks => Some(
            part.label
                .clone()
                .unwrap_or_else(|| "Encrypted Linux".into()),
        ),
        PartKind::WindowsRecovery => Some("WinRE Recovery".into()),
        _ => None,
    };
    PartitionView {
        index,
        label: part.label.clone().unwrap_or_else(|| part.name.clone()),
        kind,
        fs: part.fs.clone(),
        free_bytes: part.free.map(|s| s.0),
        encrypted,
        is_recovery: part.kind == PartKind::WindowsRecovery,
        detected_os,
        loader_count,
        boot_targets,
    }
}

/// Best-effort association between a boot target and an OS partition on the
/// same physical disk. Returns `None` rather than guessing when ambiguous,
/// and never assumes the OS partition is the same partition as the ESP.
fn infer_os_partition(disk: &Disk, target: &sbm_types::BootTarget) -> Option<usize> {
    if target.kind == BootTargetKind::SbmCore || disk.removable {
        return None;
    }
    match target.os_family {
        OsFamily::Windows => {
            let matches: Vec<usize> = disk
                .partitions
                .iter()
                .enumerate()
                .filter(|(_, p)| p.kind == PartKind::Windows)
                .map(|(i, _)| i + 1)
                .collect();
            (matches.len() == 1).then(|| matches[0])
        }
        OsFamily::Linux => {
            let name_lower = target.display_name.to_ascii_lowercase();
            let candidates: Vec<(usize, &Partition)> = disk
                .partitions
                .iter()
                .enumerate()
                .filter(|(_, p)| matches!(p.kind, PartKind::Linux | PartKind::Luks))
                .collect();
            if candidates.len() == 1 {
                return Some(candidates[0].0 + 1);
            }
            // Match by distro keyword in partition name or label if unambiguous,
            // ignoring generic words like "linux" or "boot" that appear on many partitions.
            let named: Vec<usize> = candidates
                .iter()
                .filter(|(_, p)| {
                    let hay = format!(
                        "{} {}",
                        p.name.to_ascii_lowercase(),
                        p.label.as_deref().unwrap_or("").to_ascii_lowercase()
                    );
                    name_lower.split_whitespace().any(|tok| {
                        tok.len() >= 4
                            && !matches!(
                                tok,
                                "linux" | "boot" | "efi" | "system" | "drive" | "disk"
                            )
                            && hay.contains(tok)
                    })
                })
                .map(|(i, _)| i + 1)
                .collect();
            if named.len() == 1 {
                return Some(named[0]);
            }
            // `systemd-boot` on a disk containing an Arch partition belongs to Arch when unique.
            if name_lower.contains("systemd") {
                let arch: Vec<usize> = candidates
                    .iter()
                    .filter(|(_, p)| {
                        let hay = format!(
                            "{} {}",
                            p.name.to_ascii_lowercase(),
                            p.label.as_deref().unwrap_or("").to_ascii_lowercase()
                        );
                        hay.contains("arch")
                    })
                    .map(|(i, _)| i + 1)
                    .collect();
                if arch.len() == 1 {
                    return Some(arch[0]);
                }
            }
            None
        }
        _ => None,
    }
}

fn icon_hint(target: &sbm_types::BootTarget, removable: bool) -> &'static str {
    if target.kind == BootTargetKind::SbmCore {
        return "recovery";
    }
    if removable {
        return "usb";
    }
    if target.os_family == OsFamily::Windows {
        return "windows";
    }
    let path = target
        .efi_path
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    if path.contains("systemd") {
        "systemd"
    } else if path.contains("grub") || path.contains("shim") {
        "grub"
    } else if target.os_family == OsFamily::Linux {
        "linux"
    } else {
        "other"
    }
}
