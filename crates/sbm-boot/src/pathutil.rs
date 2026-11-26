use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbm_types::BootTargetId;
use sha2::{Digest, Sha256};

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// ESP-relative path with backslashes, `.` and `..` removed, and a leading slash.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['\\', '/']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
            continue;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return String::from("\\");
    }
    let mut out = String::from("\\");
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push('\\');
        }
        out.push_str(part);
    }
    out
}

pub fn normalize_upper(path: &str) -> String {
    normalize(path).to_ascii_uppercase()
}

pub fn sanitize_id_part(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push((byte as char).to_ascii_lowercase());
        } else if byte == b' ' {
            out.push('-');
        } else {
            out.push('_');
        }
        if out.len() >= 80 {
            break;
        }
    }
    if out.is_empty() {
        out.push_str("entry");
    }
    out
}

pub fn efi_target_id(partition_guid: &str, path: &str) -> BootTargetId {
    let digest = sha256(normalize_upper(path).as_bytes());
    let prefix = hex_encode(&digest[..6]);
    BootTargetId(alloc::format!(
        "efi:{}:{prefix}",
        partition_guid.to_ascii_lowercase()
    ))
}

pub fn bls_target_id(partition_guid: &str, entry_id: &str) -> BootTargetId {
    BootTargetId(alloc::format!(
        "bls:{}:{}",
        partition_guid.to_ascii_lowercase(),
        sanitize_id_part(entry_id)
    ))
}

pub fn recovery_target_id(slot_name: &str) -> BootTargetId {
    let key = recovery_slot_key(slot_name);
    if key == "other" {
        BootTargetId(alloc::format!("sbm-core:{}", sanitize_id_part(slot_name)))
    } else {
        BootTargetId(alloc::format!("sbm-core:{key}"))
    }
}

pub fn recovery_slot_key(slot_name: &str) -> &'static str {
    let folded = slot_name.to_ascii_lowercase().replace('_', "-");
    if folded == "a" || folded == "slota" || folded.contains("slot-a") {
        "slot-a"
    } else if folded == "b" || folded == "slotb" || folded.contains("slot-b") {
        "slot-b"
    } else if folded.contains("removable") {
        "removable"
    } else {
        "other"
    }
}

pub fn file_name(path: &str) -> String {
    normalize(path)
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_is_stable_and_resolves_dots() {
        assert_eq!(normalize("//EFI//Boot//"), "\\EFI\\Boot");
        assert_eq!(
            normalize("EFI/Boot/../Linux/./a.efi"),
            "\\EFI\\Linux\\a.efi"
        );
        assert_eq!(normalize(&normalize("\\EFI\\Boot")), "\\EFI\\Boot");
        assert_eq!(normalize(""), "\\");
    }

    #[test]
    fn efi_ids_ignore_case_and_slash_style() {
        let a = efi_target_id("ABCD", "/efi/microsoft/boot/bootmgfw.efi");
        let b = efi_target_id("abcd", "\\EFI\\Microsoft\\Boot\\bootmgfw.efi");
        assert_eq!(a, b);
        assert!(a.is_valid());
        assert!(a.as_str().starts_with("efi:abcd:"));
        assert_eq!(a.as_str().split(':').nth(2).unwrap().len(), 12);
    }
}
