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
}
