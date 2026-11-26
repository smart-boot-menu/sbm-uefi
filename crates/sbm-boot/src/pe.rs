use alloc::string::ToString;
use sbm_types::CpuArch;

pub const MACHINE_X64: u16 = 0x8664;
pub const MACHINE_AA64: u16 = 0xAA64;
pub const MACHINE_IA32: u16 = 0x014C;
pub const MACHINE_RISCV64: u16 = 0x5064;

pub fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 || bytes[0] != b'M' || bytes[1] != b'Z' {
        return None;
    }
    let lfanew = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    let pe = lfanew.checked_add(6)?;
    if bytes.len() < pe || &bytes[lfanew..lfanew + 4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes(
        bytes[lfanew + 4..lfanew + 6].try_into().ok()?,
    ))
}

/// File offset and raw size of a PE section, from the headers in `bytes`.
pub fn pe_section_span(bytes: &[u8], name: &str) -> Option<(u64, u64)> {
    if bytes.len() < 0x40 || bytes[0] != b'M' || bytes[1] != b'Z' || name.len() > 8 {
        return None;
    }
    let lfanew = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    let coff = lfanew.checked_add(4)?;
    if bytes.len() < coff.saturating_add(20) || &bytes[lfanew..lfanew + 4] != b"PE\0\0" {
        return None;
    }
    let sections = u16::from_le_bytes(bytes[coff + 2..coff + 4].try_into().ok()?) as usize;
    let optional = u16::from_le_bytes(bytes[coff + 16..coff + 18].try_into().ok()?) as usize;
    if sections > 96 {
        return None;
    }
    let table = coff.checked_add(20)?.checked_add(optional)?;
    let need = table.checked_add(sections.checked_mul(40)?)?;
    if bytes.len() < need {
        return None;
    }
    for index in 0..sections {
        let entry = table + index * 40;
        if !section_name_eq(&bytes[entry..entry + 8], name) {
            continue;
        }
        let size = u32::from_le_bytes(bytes[entry + 16..entry + 20].try_into().ok()?) as u64;
        let offset = u32::from_le_bytes(bytes[entry + 20..entry + 24].try_into().ok()?) as u64;
        return Some((offset, size));
    }
    None
}

pub fn section_text(bytes: &[u8]) -> Option<alloc::string::String> {
    let text = core::str::from_utf8(bytes).ok()?;
    let text = text.trim_matches('\0').trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn section_name_eq(raw: &[u8], name: &str) -> bool {
    let mut padded = [0u8; 8];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    raw == padded
}

pub fn machine_matches(machine: u16, arch: CpuArch) -> bool {
    match arch {
        CpuArch::X86_64 => machine == MACHINE_X64,
        CpuArch::Aarch64 => machine == MACHINE_AA64,
        CpuArch::Ia32 => machine == MACHINE_IA32,
        CpuArch::Riscv64 => machine == MACHINE_RISCV64,
    }
}

#[cfg(test)]
pub fn fake_pe(machine: u16) -> alloc::vec::Vec<u8> {
    let mut bytes = alloc::vec![0u8; 0x80];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3c] = 0x40;
    bytes[0x40] = b'P';
    bytes[0x41] = b'E';
    bytes[0x44] = machine as u8;
    bytes[0x45] = (machine >> 8) as u8;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pe_machine() {
        assert_eq!(pe_machine(&fake_pe(MACHINE_X64)), Some(MACHINE_X64));
        assert!(machine_matches(MACHINE_AA64, CpuArch::Aarch64));
        assert!(!machine_matches(MACHINE_X64, CpuArch::Aarch64));
        assert_eq!(pe_machine(b"not a pe"), None);
    }

    #[test]
    fn reads_a_cmdline_section() {
        let mut bytes = alloc::vec![0u8; 0xA0];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[0x3c] = 0x40;
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        bytes[0x46] = 1;
        bytes[0x54] = 0x10;
        let entry = 0x40 + 4 + 20 + 0x10;
        bytes[entry..entry + 8].copy_from_slice(b".cmdline");
        bytes[entry + 16] = 12;
        bytes[entry + 20] = 0x90;
        bytes[0x90..0x9C].copy_from_slice(b"console=tty0");
        assert_eq!(pe_section_span(&bytes, ".cmdline"), Some((0x90, 12)));
        assert_eq!(
            section_text(&bytes[0x90..0x9C]).as_deref(),
            Some("console=tty0")
        );
        assert_eq!(pe_section_span(&bytes, ".linux"), None);
    }
}
