pub fn set_nx_compat(bytes: &mut [u8]) -> Result<(), String> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return Err(String::from("not a PE image"));
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    let pe = e_lfanew
        .checked_add(4)
        .ok_or_else(|| String::from("PE header is out of range"))?;
    if bytes.len() < pe || &bytes[e_lfanew..pe] != b"PE\0\0" {
        return Err(String::from("PE signature is missing"));
    }
    let optional = pe
        .checked_add(20)
        .ok_or_else(|| String::from("optional header is out of range"))?;
    if bytes.len() < optional + 2 {
        return Err(String::from("optional header is truncated"));
    }
    let magic = u16::from_le_bytes(bytes[optional..optional + 2].try_into().unwrap());
    if magic != 0x20b {
        return Err(alloc_msg("expected a PE32+ image"));
    }
    let flags_at = optional
        .checked_add(70)
        .ok_or_else(|| String::from("DLL characteristics are out of range"))?;
    if bytes.len() < flags_at + 2 {
        return Err(String::from("DLL characteristics are truncated"));
    }
    let flags = u16::from_le_bytes(bytes[flags_at..flags_at + 2].try_into().unwrap());
    bytes[flags_at..flags_at + 2].copy_from_slice(&(flags | 0x0100).to_le_bytes());
    Ok(())
}

fn alloc_msg(text: &str) -> String {
    String::from(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_nx_compat_on_pe32_plus() {
        let mut bytes = vec![0u8; 0x200];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let optional = 0x80 + 24;
        bytes[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        let flags_at = optional + 70;
        bytes[flags_at..flags_at + 2].copy_from_slice(&0x0020u16.to_le_bytes());
        set_nx_compat(&mut bytes).unwrap();
        let flags = u16::from_le_bytes(bytes[flags_at..flags_at + 2].try_into().unwrap());
        assert_eq!(flags, 0x0120);
        set_nx_compat(&mut bytes).unwrap();
        let flags = u16::from_le_bytes(bytes[flags_at..flags_at + 2].try_into().unwrap());
        assert_eq!(flags, 0x0120);
    }
}
