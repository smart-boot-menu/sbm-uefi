use alloc::string::{String, ToString};
use sbm_types::CpuArch;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlsEntry {
    pub title: String,
    pub linux: Option<String>,
    pub efi: Option<String>,
    pub initrd: Option<String>,
    pub options: Option<String>,
    pub architecture: Option<String>,
}

pub fn parse_bls(text: &str) -> BlsEntry {
    let mut entry = BlsEntry::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(|c: char| c.is_whitespace())
            .unwrap_or((line, ""));
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "title" => entry.title = value.to_string(),
            "linux" => entry.linux = Some(value.to_string()),
            "efi" => entry.efi = Some(value.to_string()),
            "initrd" => entry.initrd = Some(value.to_string()),
            "options" => entry.options = Some(value.to_string()),
            "architecture" | "arch" => entry.architecture = Some(value.to_string()),
            _ => {}
        }
    }
    entry
}

pub fn bls_arch_ok(field: Option<&str>, arch: CpuArch) -> bool {
    let Some(field) = field else {
        return true;
    };
    let field = field.to_ascii_lowercase();
    match arch {
        CpuArch::X86_64 => matches!(field.as_str(), "x64" | "x86_64" | "amd64" | "x86-64"),
        CpuArch::Aarch64 => matches!(field.as_str(), "aa64" | "aarch64" | "arm64"),
        CpuArch::Ia32 => matches!(field.as_str(), "ia32" | "x86" | "i386"),
        CpuArch::Riscv64 => matches!(field.as_str(), "riscv64" | "riscv"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type1_entry() {
        let entry = parse_bls(
            "# comment\ntitle Fedora Linux\nlinux /EFI/Linux/fedora.efi\noptions quiet\narchitecture x64\n",
        );
        assert_eq!(entry.title, "Fedora Linux");
        assert_eq!(entry.linux.as_deref(), Some("/EFI/Linux/fedora.efi"));
        assert_eq!(entry.options.as_deref(), Some("quiet"));
        assert!(bls_arch_ok(entry.architecture.as_deref(), CpuArch::X86_64));
        assert!(!bls_arch_ok(Some("arm64"), CpuArch::X86_64));
        assert!(bls_arch_ok(None, CpuArch::Aarch64));
    }
}
