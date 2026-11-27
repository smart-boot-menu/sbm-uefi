//! Menu session glue between discovery (`sbm-boot`) and the renderer
//! (`sbm-gfx`). Everything here is platform independent so the UEFI app and
//! the native host preview run the same card, input, and screen logic.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod flow;

use alloc::string::String;
use alloc::vec::Vec;

use sbm_boot::{DiscoveredTarget, compare_targets, selectable, visible};
use sbm_gfx::{
    CardView, Framebuffer, Hit, Input, MenuState, Screen, hit_test, layout_menu, paint_diagnostics,
    paint_error, paint_menu,
};
use sbm_types::{BootTargetId, BootTargetKind, CpuArch, OsFamily, SecureBootState};

pub const VERSION: &str = "0.1.0";

pub struct MenuEntry {
    pub disk: usize,
    pub item: DiscoveredTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Tab,
    Escape,
    Char(char),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointer {
    pub x: u32,
    pub y: u32,
    pub moved: bool,
    pub clicked: bool,
}

/// Why `LoadImage` or `StartImage` failed, reduced to what the menu explains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartFailure {
    SecurityViolation,
    NotFound,
    Unsupported,
    Other,
}

pub struct DiagFacts<'a> {
    pub firmware: &'a str,
    pub arch: CpuArch,
    pub secure_boot: SecureBootState,
    pub display: Option<&'a str>,
    pub ram_mib: Option<u64>,
    pub first_frame_us: u64,
    pub discovery_us: u64,
    pub menu_us: Option<u64>,
    /// What was checked about the recovery image, once known.
    pub integrity: Option<&'a str>,
}

pub fn sort_entries(entries: &mut [MenuEntry]) {
    entries.sort_by(|left, right| compare_targets(&left.item.target, &right.item.target));
}

pub fn visible_positions(entries: &[MenuEntry]) -> Vec<usize> {
    visible(
        &entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .filter_map(|target| {
        entries
            .iter()
            .position(|entry| entry.item.target.id == target.target.id)
    })
    .collect()
}

pub fn card_position(
    entries: &[MenuEntry],
    visible_index: &[usize],
    id: &BootTargetId,
) -> Option<usize> {
    visible_index.iter().position(|index| {
        entries
            .get(*index)
            .is_some_and(|entry| &entry.item.target.id == id)
    })
}

pub fn recovery_position(entries: &[MenuEntry]) -> Option<usize> {
    let preferred = entries.iter().position(|entry| {
        entry.item.target.kind == BootTargetKind::SbmCore
            && entry.item.preferred
            && selectable(&entry.item)
    });
    preferred.or_else(|| {
        entries.iter().position(|entry| {
            entry.item.target.kind == BootTargetKind::SbmCore && selectable(&entry.item)
        })
    })
}

pub fn countdown_ms(seconds: Option<u16>) -> Option<u32> {
    match seconds {
        Some(0) => Some(0),
        Some(65535) | None => None,
        Some(seconds) => Some(u32::from(seconds).saturating_mul(1000)),
    }
}

pub fn cards(entries: &[MenuEntry], visible_index: &[usize]) -> Vec<CardView> {
    visible_index
        .iter()
        .filter_map(|index| entries.get(*index))
        .map(|entry| {
            let target = &entry.item.target;
            let file = target
                .efi_path
                .as_deref()
                .and_then(|path| path.rsplit('\\').next())
                .unwrap_or("");
            CardView {
                title: target.display_name.clone(),
                subtitle: alloc::format!(
                    "{}  {file}",
                    match entry.item.integrity {
                        sbm_boot::Integrity::Eligible => "Integrity pending",
                        sbm_boot::Integrity::Failed => "Unavailable",
                        _ if target.removable_media => "Removable",
                        _ => family_name(target.os_family),
                    }
                ),
                hint: target
                    .icon_hint
                    .clone()
                    .unwrap_or_else(|| String::from("other")),
            }
        })
        .collect()
}

pub fn key_input(key: Key) -> Option<Input> {
    match key {
        Key::Up => Some(Input::Up),
        Key::Down => Some(Input::Down),
        Key::Left => Some(Input::Left),
        Key::Right => Some(Input::Right),
        Key::Activate => Some(Input::Activate),
        Key::Tab => Some(Input::Tab),
        Key::Escape => Some(Input::Escape),
        Key::Char(ch) => match ch.to_ascii_lowercase() {
            't' => Some(Input::Theme),
            'c' => Some(Input::Recovery),
            'd' => Some(Input::Diagnostics),
            'f' => Some(Input::Firmware),
            'r' => Some(Input::Reboot),
            's' => Some(Input::Shutdown),
            ' ' => Some(Input::Activate),
            _ => None,
        },
    }
}

pub fn menu_input(
    key: Option<Key>,
    pointer: &Pointer,
    width: u32,
    height: u32,
    cards: usize,
    selected: usize,
) -> Option<Input> {
    if let Some(key) = key {
        return key_input(key);
    }
    let layout = layout_menu(width, height, cards, 3, selected);
    if pointer.clicked {
        return match hit_test(&layout, pointer.x, pointer.y) {
            Hit::Card(index) => Some(Input::ClickCard(index)),
            Hit::Action(index) => Some(Input::ClickAction(index)),
            Hit::None => None,
        };
    }
    if pointer.moved
        && let Hit::Card(index) = hit_test(&layout, pointer.x, pointer.y)
    {
        return Some(Input::HoverCard(index));
    }
    None
}

pub fn paint_current(
    fb: &mut Framebuffer,
    entries: &[MenuEntry],
    visible_index: &[usize],
    state: &MenuState,
    lines: &[String],
) {
    match state.screen {
        Screen::Error => paint_error(fb, state.dark, &state.message),
        Screen::Diagnostics => {
            let borrowed = lines.iter().map(String::as_str).collect::<Vec<_>>();
            paint_diagnostics(fb, state.dark, &borrowed);
        }
        Screen::Menu => {
            let cards = cards(entries, visible_index);
            paint_menu(fb, &cards, state);
        }
    }
}

pub fn diag_lines(entries: &[MenuEntry], facts: &DiagFacts<'_>) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(alloc::format!("Smart Boot Menu {VERSION}"));
    lines.push(String::from(facts.firmware));
    lines.push(alloc::format!(
        "arch {}  secure boot {}",
        arch_name(facts.arch),
        secure_boot_label(facts.secure_boot)
    ));
    match facts.display {
        Some(display) => lines.push(String::from(display)),
        None => lines.push(String::from("graphics output unavailable")),
    }
    match facts.ram_mib {
        Some(mib) => lines.push(alloc::format!("memory {mib} MiB")),
        None => lines.push(String::from("memory unknown")),
    }
    lines.push(alloc::format!(
        "entries {}",
        entries
            .iter()
            .filter(|entry| !entry.item.target.hidden)
            .count()
    ));
    lines.push(alloc::format!("first frame {} us", facts.first_frame_us));
    lines.push(alloc::format!("discovery {} us", facts.discovery_us));
    if let Some(menu_us) = facts.menu_us {
        lines.push(alloc::format!("menu {menu_us} us"));
    }
    if let Some(integrity) = facts.integrity {
        lines.push(String::from(integrity));
    }
    lines.push(String::from("network policy is local-only in this build"));
    lines
}

pub fn boot_failure(name: &str, path: &str, failure: StartFailure, status: &str) -> String {
    let hint = match failure {
        StartFailure::SecurityViolation => "Secure Boot rejected the file.",
        StartFailure::NotFound => "The file is missing.",
        StartFailure::Unsupported => "This firmware cannot start the file.",
        StartFailure::Other => "The firmware returned an error.",
    };
    alloc::format!("{name} ({path}) did not start. {hint} Status {status}.")
}

pub fn unhealthy_message(name: &str, problem: Option<&str>) -> String {
    match problem {
        Some(problem) => alloc::format!("{name} cannot be started: {problem}."),
        None => alloc::format!("{name} did not pass the local health checks"),
    }
}

pub const NO_RECOVERY_MESSAGE: &str = "No healthy recovery system was found";

pub fn family_name(family: OsFamily) -> &'static str {
    match family {
        OsFamily::Windows => "Windows",
        OsFamily::Linux => "Linux",
        OsFamily::Bsd => "BSD",
        OsFamily::MacOs => "macOS",
        OsFamily::Recovery => "Recovery",
        OsFamily::FirmwareUtility => "Firmware",
        OsFamily::Other => "EFI",
    }
}

pub fn arch_name(arch: CpuArch) -> &'static str {
    match arch {
        CpuArch::X86_64 => "x86_64",
        CpuArch::Aarch64 => "aarch64",
        CpuArch::Ia32 => "ia32",
        CpuArch::Riscv64 => "riscv64",
    }
}

pub fn secure_boot_label(state: SecureBootState) -> &'static str {
    match state {
        SecureBootState::SetupMode => "setup",
        SecureBootState::Enabled => "enabled",
        SecureBootState::Disabled => "disabled",
        SecureBootState::Unsupported => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbm_types::BootTarget;

    fn entry(id: &str, name: &str, family: OsFamily, kind: BootTargetKind) -> MenuEntry {
        MenuEntry {
            disk: 0,
            item: DiscoveredTarget {
                target: BootTarget {
                    id: BootTargetId(id.into()),
                    display_name: name.into(),
                    os_family: family,
                    kind,
                    arch: CpuArch::X86_64,
                    partition_guid: None,
                    efi_path: Some("\\EFI\\vendor\\loader.efi".into()),
                    kernel_cmdline: None,
                    icon_hint: Some("linux".into()),
                    removable_media: false,
                    hidden: false,
                    verified_signature: None,
                },
                initrd: None,
                healthy: true,
                preferred: false,
                rank: 1,
                integrity: sbm_boot::Integrity::Discovered,
                expected_sha256: None,
                problem: None,
            },
        }
    }

    #[test]
    fn sorts_cards_and_finds_recovery() {
        let mut entries = alloc::vec![
            entry(
                "sbm-core:slot-a",
                "SBM Core Slot A",
                OsFamily::Recovery,
                BootTargetKind::SbmCore
            ),
            entry(
                "efi:fedora",
                "Fedora",
                OsFamily::Linux,
                BootTargetKind::EfiBinary
            ),
            entry(
                "efi:windows",
                "Windows",
                OsFamily::Windows,
                BootTargetKind::EfiBinary
            ),
        ];
        sort_entries(&mut entries);
        let visible = visible_positions(&entries);
        let cards = cards(&entries, &visible);
        assert_eq!(cards[0].title, "Windows");
        assert_eq!(cards[1].subtitle, "Linux  loader.efi");
        assert_eq!(recovery_position(&entries), Some(2));
        assert_eq!(
            card_position(&entries, &visible, &BootTargetId("efi:fedora".into())),
            Some(1)
        );
    }

    #[test]
    fn keys_and_pointer_map_to_menu_inputs() {
        assert_eq!(key_input(Key::Char('T')), Some(Input::Theme));
        assert_eq!(key_input(Key::Char(' ')), Some(Input::Activate));
        assert_eq!(key_input(Key::Char('x')), None);
        let layout = layout_menu(1024, 768, 3, 3, 0);
        let card = layout.cards[1].1;
        let pointer = Pointer {
            x: card.x + card.w / 2,
            y: card.y + card.h / 2,
            moved: true,
            clicked: false,
        };
        assert_eq!(
            menu_input(None, &pointer, 1024, 768, 3, 0),
            Some(Input::HoverCard(1))
        );
        let click = Pointer {
            clicked: true,
            ..pointer
        };
        assert_eq!(
            menu_input(None, &click, 1024, 768, 3, 0),
            Some(Input::ClickCard(1))
        );
    }

    #[test]
    fn countdown_and_failure_text() {
        assert_eq!(countdown_ms(Some(0)), Some(0));
        assert_eq!(countdown_ms(Some(65535)), None);
        assert_eq!(countdown_ms(Some(5)), Some(5000));
        assert!(
            boot_failure(
                "Ubuntu",
                "\\EFI\\ubuntu\\shimx64.efi",
                StartFailure::SecurityViolation,
                "SECURITY_VIOLATION"
            )
            .contains("Secure Boot rejected the file.")
        );
    }
}
