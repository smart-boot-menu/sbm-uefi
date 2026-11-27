//! Declarative description of a simulated computer.
//!
//! A scenario lists disks, partitions, and which boot loaders sit on each EFI
//! System Partition. Sizes are metadata only; no disk image is allocated.

use std::fs;
use std::path::{Path, PathBuf};

use sbm_types::{CpuArch, SecureBootState};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub summary: String,
    #[serde(default = "default_arch")]
    pub arch: Arch,
    #[serde(default)]
    pub firmware: Firmware,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub hardware: Hardware,
    #[serde(default, rename = "nic")]
    pub nics: Vec<Nic>,
    #[serde(rename = "disk")]
    pub disks: Vec<Disk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn cpu(self) -> CpuArch {
        match self {
            Arch::X86_64 => CpuArch::X86_64,
            Arch::Aarch64 => CpuArch::Aarch64,
        }
    }

    pub fn efi_tag(self) -> &'static str {
        match self {
            Arch::X86_64 => "x64",
            Arch::Aarch64 => "aa64",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

fn default_arch() -> Arch {
    Arch::X86_64
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Firmware {
    pub vendor: String,
    pub secure_boot: SecureBoot,
    pub ram_mib: u64,
    pub tpm: bool,
    pub network: bool,
}

impl Default for Firmware {
    fn default() -> Self {
        Self {
            vendor: "EDK II".into(),
            secure_boot: SecureBoot::Disabled,
            ram_mib: 16_384,
            tpm: true,
            network: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecureBoot {
    Enabled,
    Disabled,
    SetupMode,
    Unsupported,
}

impl SecureBoot {
    pub fn state(self) -> SecureBootState {
        match self {
            SecureBoot::Enabled => SecureBootState::Enabled,
            SecureBoot::Disabled => SecureBootState::Disabled,
            SecureBoot::SetupMode => SecureBootState::SetupMode,
            SecureBoot::Unsupported => SecureBootState::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Policy {
    /// Seconds before the default boots. Omit to wait for the user.
    pub timeout_seconds: Option<u16>,
    /// Loader preset name (for example `windows`) whose target is the default.
    pub default: Option<String>,
    pub max_failed_attempts: Option<u8>,
    /// Consecutive unconfirmed boots of the default already recorded on the ESP.
    pub failed_boots: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Hardware {
    pub hostname: String,
    pub vendor: String,
    pub product: String,
    pub cpu: String,
    pub cores: usize,
    pub gpus: Vec<String>,
}

impl Default for Hardware {
    fn default() -> Self {
        Self {
            hostname: "sim-computer".into(),
            vendor: "Generic".into(),
            product: "UEFI PC".into(),
            cpu: "Generic CPU".into(),
            cores: 4,
            gpus: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nic {
    pub name: String,
    #[serde(default)]
    pub wireless: bool,
    #[serde(default = "default_up")]
    pub state: String,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub addresses: Vec<String>,
}

fn default_up() -> String {
    "UP".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    pub name: String,
    pub model: String,
    pub size: Size,
    #[serde(default = "default_bus")]
    pub bus: String,
    #[serde(default)]
    pub removable: bool,
    #[serde(default, rename = "partition")]
    pub partitions: Vec<Partition>,
}

fn default_bus() -> String {
    "nvme".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    Esp,
    Msr,
    Windows,
    WindowsRecovery,
    Linux,
    LinuxHome,
    Swap,
    Luks,
    Data,
    Apfs,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Partition {
    pub name: String,
    pub kind: PartKind,
    pub size: Size,
    #[serde(default)]
    pub free: Option<Size>,
    #[serde(default)]
    pub fs: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub bitlocker: bool,
    /// Loader presets installed on this EFI System Partition.
    #[serde(default)]
    pub loaders: Vec<String>,
    /// Presets whose binaries Secure Boot refuses when it is enabled.
    #[serde(default)]
    pub rejected: Vec<String>,
    /// Presets whose binaries exist but are damaged.
    #[serde(default)]
    pub corrupt: Vec<String>,
}

/// Byte count written as `512MiB`, `2TB`, or a plain integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size(pub u64);

impl<'de> Deserialize<'de> for Size {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(u64),
            Text(String),
        }
        match Raw::deserialize(de)? {
            Raw::Int(value) => Ok(Size(value)),
            Raw::Text(text) => parse_size(&text)
                .map(Size)
                .map_err(serde::de::Error::custom),
        }
    }
}

pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size {text:?}"))?;
    let scale: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" | "k" => 1024.0,
        "mib" | "m" => 1024.0 * 1024.0,
        "gib" | "g" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown size unit {other:?} in {text:?}")),
    };
    Ok((value * scale) as u64)
}

impl Scenario {
    pub fn parse(id: &str, text: &str) -> Result<Self, String> {
        let mut scenario: Scenario =
            toml::from_str(text).map_err(|err| format!("scenario {id}: {err}"))?;
        scenario.id = id.to_string();
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("scenario");
        Self::parse(id, &text)
    }

    fn validate(&self) -> Result<(), String> {
        for disk in &self.disks {
            let used: u64 = disk.partitions.iter().map(|part| part.size.0).sum();
            if used > disk.size.0 {
                return Err(format!(
                    "scenario {}: partitions on {} exceed the disk size",
                    self.id, disk.name
                ));
            }
            for part in &disk.partitions {
                if part.kind != PartKind::Esp && !part.loaders.is_empty() {
                    return Err(format!(
                        "scenario {}: loaders on {} require kind = \"esp\"",
                        self.id, part.name
                    ));
                }
                for name in part
                    .loaders
                    .iter()
                    .chain(&part.rejected)
                    .chain(&part.corrupt)
                {
                    if !crate::machine::known_preset(name) {
                        return Err(format!(
                            "scenario {}: unknown loader preset {name:?}",
                            self.id
                        ));
                    }
                }
                if let Some(free) = part.free
                    && free.0 > part.size.0
                {
                    return Err(format!(
                        "scenario {}: free space on {} exceeds its size",
                        self.id, part.name
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn scenario_dir() -> PathBuf {
    crate::workspace_root().join("sim/scenarios")
}

pub fn list(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir(dir)
        .map_err(|err| format!("read {}: {err}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

/// Resolve a scenario name (`dual-boot`) or path to a file.
pub fn resolve(spec: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(spec);
    if direct.is_file() {
        return Ok(direct);
    }
    let named = scenario_dir().join(format!("{spec}.toml"));
    if named.is_file() {
        return Ok(named);
    }
    let names = list(&scenario_dir())?
        .iter()
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("unknown scenario {spec:?} (available: {names})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_decimal_and_binary_units() {
        assert_eq!(parse_size("512MiB").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("2TB").unwrap(), 2_000_000_000_000);
        assert_eq!(parse_size("1.5GiB").unwrap(), 1_610_612_736);
        assert!(parse_size("12 parsecs").is_err());
    }

    #[test]
    fn rejects_overfull_disks_and_unknown_presets() {
        let base = r#"
            name = "x"
            summary = "x"
            [[disk]]
            name = "sda"
            model = "m"
            size = "1GiB"
            [[disk.partition]]
            name = "EFI"
            kind = "esp"
            size = "2GiB"
        "#;
        assert!(Scenario::parse("x", base).unwrap_err().contains("exceed"));
        let unknown = base
            .replace("2GiB", "100MiB")
            .replace("kind = \"esp\"", "kind = \"esp\"\nloaders = [\"beos\"]");
        assert!(
            Scenario::parse("x", &unknown)
                .unwrap_err()
                .contains("unknown loader preset")
        );
    }
}
