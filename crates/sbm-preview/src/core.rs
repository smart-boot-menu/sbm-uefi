//! Hand-off to the native SBM Core simulator in `sbm-core`.
//!
//! The simulated computer is written to a session directory in the formats
//! `sbm-cored` reads on Linux (`lsblk --json`, `ip -j addr`, `/proc/meminfo`,
//! the EFI handoff file on the ESP, and the volatile handoff variables in
//! efivarfs layout). `sbm-sim` serves those to the real `sbm-ui` and records
//! any power request it receives.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sbm_protocol::{BootCommand, BootSuccessReport, LocalBootRequest, format_uuid_v4};
use sbm_types::{BootTargetId, ComponentKind, SCHEMA_VERSION_V1};
use serde_json::{Value, json};

use crate::machine::{
    HANDOFF_PATH, Machine, REQUEST_PATH, SUCCESS_PATH, disk_guid, part_guid, partition_device,
};
use crate::scenario::{PartKind, Partition, Scenario};
use crate::session::{CoreExit, CoreRequest};

pub const ESP_TYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

pub fn materialize(
    dir: &Path,
    machine: &Machine,
    request: Option<&CoreRequest>,
) -> Result<(), String> {
    let scenario = &machine.scenario;
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir.join("esp")).map_err(|err| format!("mkdir {}: {err}", dir.display()))?;
    let write = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::write(&path, bytes).map_err(|err| format!("write {}: {err}", path.display()))
    };
    write(
        "SIMULATED",
        b"This directory describes a simulated computer for SBM native development.\nNothing here refers to a physical disk.\n",
    )?;
    write(
        "scenario",
        format!("{}\n{}\n", scenario.id, scenario.name).as_bytes(),
    )?;
    write(
        "lsblk.json",
        &serde_json::to_vec_pretty(&lsblk(scenario)).unwrap_or_default(),
    )?;
    write(
        "ip-addr.json",
        &serde_json::to_vec_pretty(&ip_addr(scenario)).unwrap_or_default(),
    )?;
    let total_kib = scenario.firmware.ram_mib.saturating_mul(1024);
    write(
        "meminfo",
        format!(
            "MemTotal:       {} kB\nMemAvailable:   {} kB\n",
            total_kib.saturating_sub(total_kib / 40),
            total_kib.saturating_sub(total_kib / 6)
        )
        .as_bytes(),
    )?;
    let hw = &scenario.hardware;
    write(
        "hardware.json",
        &serde_json::to_vec_pretty(&json!({
            "hostname": hw.hostname,
            "vendor": hw.vendor,
            "product": hw.product,
            "cpu_model": hw.cpu,
            "cores": hw.cores,
            "gpus": hw.gpus,
            "arch": scenario.arch.name(),
            "wireless": scenario.nics.iter().filter(|nic| nic.wireless).map(|nic| &nic.name).collect::<Vec<_>>(),
        }))
        .unwrap_or_default(),
    )?;
    if let Some(request) = request {
        for (name, vendor, value) in &request.vars {
            write(&format!("efivars/{name}-{vendor}"), &efivar(value))?;
        }
        if let Some(device) = &request.backing_device {
            write("root-source", device.as_bytes())?;
        }
        write(&esp_rel(HANDOFF_PATH), &request.handoff)?;
    } else if let Some(bytes) = machine.read_boot(HANDOFF_PATH) {
        write(&esp_rel(HANDOFF_PATH), &bytes)?;
    }
    Ok(())
}

/// A volatile `BS|RT` variable as Linux efivarfs presents it: a 4-byte
/// little-endian attribute word, then the UTF-16LE value with its NUL.
pub fn efivar(value: &str) -> Vec<u8> {
    const BOOTSERVICE_RUNTIME: u32 = 0x2 | 0x4;
    let mut bytes = BOOTSERVICE_RUNTIME.to_le_bytes().to_vec();
    for unit in value.encode_utf16().chain([0]) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Copy files Core wrote to its view of the ESP back onto the boot volume.
pub fn import(dir: &Path, machine: &Machine) -> Result<(), String> {
    for path in [SUCCESS_PATH, REQUEST_PATH] {
        let source = dir.join(esp_rel(path));
        match fs::read(&source) {
            Ok(bytes) => {
                if !machine.write_boot(path, &bytes) {
                    return Err(format!("could not save {path} on the simulated boot ESP"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("read {}: {err}", source.display())),
        }
    }
    Ok(())
}

/// SIMULATED stand-in for SBM Core, for headless scripts and tests that do
/// not run `sbm-sim`: the success report Core writes once its UI is ready
/// and, when `boot` is given, the one-shot request the user made in Core.
/// Both are bound to the attempt ID the picker published.
pub fn simulated_core_reports(
    machine: &Machine,
    request: &CoreRequest,
    boot: Option<&BootTargetId>,
) -> Result<(), String> {
    let attempt_id = request
        .var("SbmBootAttemptId")
        .ok_or("the picker published no SbmBootAttemptId")?
        .to_string();
    let now =
        sbm_menu::flow::Platform::now_unix(machine).ok_or("the simulated clock is unavailable")?;
    let success = BootSuccessReport {
        schema_version: SCHEMA_VERSION_V1.into(),
        attempt_id: attempt_id.clone(),
        target_id: BootTargetId(sbm_boot::RECOVERY_TARGET_ID.into()),
        confirmed_by: ComponentKind::SbmCoreUki,
        confirmed_at_unix: now,
    };
    let bytes = sbm_boot::encode_success(&success).map_err(|err| format!("{err:?}"))?;
    if !machine.write_boot(SUCCESS_PATH, &bytes) {
        return Err("the boot ESP is not writable".into());
    }
    if let Some(target) = boot {
        let mut seed = [0u8; 16];
        for (slot, byte) in seed.iter_mut().zip(attempt_id.bytes().rev()) {
            *slot = byte ^ 0x5a;
        }
        let boot_request = LocalBootRequest {
            schema_version: SCHEMA_VERSION_V1.into(),
            request_id: format_uuid_v4(seed),
            attempt_id,
            requested_by: ComponentKind::SbmCoreUki,
            requested_at_unix: now,
            command: BootCommand::BootTarget {
                target_id: target.clone(),
                one_shot: true,
                extra_cmdline_preset: None,
            },
        };
        let bytes =
            sbm_boot::encode_boot_request(&boot_request).map_err(|err| format!("{err:?}"))?;
        if !machine.write_boot(REQUEST_PATH, &bytes) {
            return Err("the boot ESP is not writable".into());
        }
    }
    Ok(())
}

fn esp_rel(path: &str) -> String {
    format!("esp/{}", path.trim_start_matches('\\').replace('\\', "/"))
}

pub fn run(core_bin: &Path, session: &Path) -> Result<CoreExit, String> {
    let sim = core_bin.join("sbm-sim");
    if !sim.is_file() {
        return Err(format!(
            "{} was not found. Build it with `cargo run -p xtask -- core build`.",
            sim.display()
        ));
    }
    let status = Command::new(&sim)
        .arg("--session")
        .arg(session)
        .status()
        .map_err(|err| format!("start {}: {err}", sim.display()))?;
    if !status.success() {
        return Err(format!("sbm-sim exited with {status}"));
    }
    let action = fs::read_to_string(session.join("power"))
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(match action.as_str() {
        "shutdown" => CoreExit::Shutdown,
        _ => CoreExit::Reboot,
    })
}

pub fn default_core_bin() -> PathBuf {
    if let Some(dir) = std::env::var_os("SBM_CORE_BIN") {
        return PathBuf::from(dir);
    }
    let root = crate::workspace_root();
    root.parent().unwrap_or(&root).join("sbm-core/target/debug")
}

fn fstype(part: &Partition) -> Option<String> {
    if let Some(fs) = &part.fs {
        return Some(fs.clone());
    }
    if part.bitlocker {
        return Some("BitLocker".into());
    }
    match part.kind {
        PartKind::Esp => Some("vfat".into()),
        PartKind::Msr => None,
        PartKind::Windows | PartKind::WindowsRecovery => Some("ntfs".into()),
        PartKind::Linux | PartKind::LinuxHome => Some("ext4".into()),
        PartKind::Swap => Some("swap".into()),
        PartKind::Luks => Some("crypto_LUKS".into()),
        PartKind::Data => Some("exfat".into()),
        PartKind::Apfs => Some("apfs".into()),
    }
}

fn parttype(kind: PartKind) -> &'static str {
    match kind {
        PartKind::Esp => ESP_TYPE,
        PartKind::Msr => "e3c9e316-0b5c-4db8-817d-f92df00215ae",
        PartKind::Windows | PartKind::Data => "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7",
        PartKind::WindowsRecovery => "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
        PartKind::Linux | PartKind::LinuxHome | PartKind::Luks => {
            "0fc63daf-8483-4772-8e79-3d69d8477de4"
        }
        PartKind::Swap => "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f",
        PartKind::Apfs => "7c3457ef-0000-11aa-aa11-00306543ecac",
    }
}

pub fn lsblk(scenario: &Scenario) -> Value {
    let devices = scenario
        .disks
        .iter()
        .map(|disk| {
            let mut start = 2048u64;
            let children = disk
                .partitions
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    let path = partition_device(&disk.name, index + 1);
                    let name = path.trim_start_matches("/dev/").to_string();
                    let mounted = part.free.is_some();
                    let child = json!({
                        "name": name,
                        "path": path,
                        "type": "part",
                        "size": part.size.0,
                        "start": start,
                        "fstype": fstype(part),
                        "label": part.label,
                        "partlabel": part.name,
                        "parttype": parttype(part.kind),
                        "partuuid": part_guid(&scenario.id, &disk.name, index),
                        "mountpoint": if mounted { Some(format!("/run/sbm/sim/{name}")) } else { None },
                        "fsavail": part.free.map(|free| free.0),
                        "ro": false,
                        "rm": disk.removable,
                        "log-sec": 512,
                    });
                    start = start.saturating_add(part.size.0 / 512);
                    child
                })
                .collect::<Vec<_>>();
            json!({
                "name": disk.name,
                "path": format!("/dev/{}", disk.name),
                "type": "disk",
                "size": disk.size.0,
                "model": disk.model,
                "serial": format!("SIM-{}", &disk_guid(&scenario.id, &disk.name)[..8]),
                "wwn": null,
                "ptuuid": disk_guid(&scenario.id, &disk.name),
                "tran": disk.bus,
                "rm": disk.removable,
                "hotplug": disk.removable,
                "ro": false,
                "log-sec": 512,
                "children": children,
            })
        })
        .collect::<Vec<_>>();
    json!({ "blockdevices": devices })
}

fn ip_addr(scenario: &Scenario) -> Value {
    Value::Array(
        scenario
            .nics
            .iter()
            .enumerate()
            .map(|(index, nic)| {
                json!({
                    "ifname": nic.name,
                    "address": nic.mac.clone().unwrap_or_else(|| format!("02:53:42:4d:00:{index:02x}")),
                    "operstate": nic.state,
                    "addr_info": nic.addresses.iter().map(|addr| {
                        json!({ "local": addr.split('/').next().unwrap_or(addr) })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}
