#![no_main]
#![no_std]

extern crate alloc;

mod chainload;
mod firmware;
mod present;
mod volume;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::OnceCell;
use core::time::Duration;

use sbm_boot::{ParsedLoadOption, Volume, normalize, parse_load_option};
use sbm_gfx::{Framebuffer, Input, MenuState, Outcome, Screen, layout_menu, paint_splash, step};
use sbm_menu::flow::{HandoffVar, LaunchResult, Picker, Platform};
use sbm_menu::{
    DiagFacts, NO_RECOVERY_MESSAGE, Pointer, arch_name, menu_input, paint_current,
    recovery_position,
};
use sbm_types::{CpuArch, PlatformCapabilities, SecureBootState};
use uefi::boot;
use uefi::proto::network::snp::SimpleNetwork;
use uefi::proto::tcg::v1::Tcg as Tcg1;
use uefi::proto::tcg::v2::Tcg as Tcg2;
use uefi::{Status, entry, helpers};

use crate::chainload::chainload;
use crate::firmware::{cpu_arch, firmware_unix, mark, poll_key};
use crate::present::{Mouse, Screen as Gop};
use crate::volume::{Disk, HostImage, host_image, scan_disks};

#[used]
#[unsafe(link_section = ".sbat")]
static SBAT: [u8; 148] = *b"\
sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md\n\
sbm,1,Smart Boot Menu,sbm,1,https://github.com/smart-boot-menu/sbm-uefi\n";

#[entry]
fn main() -> Status {
    let _ = helpers::init();
    let _ = boot::set_watchdog_timer(0, 0, None);
    let _keep = SBAT[0];
    if core::str::from_utf8(&SBAT).is_ok_and(|text| text.contains("sbat,1,")) {
        mark("sbat ok");
    }
    run();
}

/// Firmware side of [`Platform`]: the decisions live in `sbm_menu::flow`.
struct UefiPlatform {
    disks: Vec<Disk>,
    clock: firmware::Clock,
    skip: Vec<String>,
    options: Vec<ParsedLoadOption>,
    gop: bool,
    /// Probed on first use (at launch), not on the menu's critical path.
    caps: OnceCell<PlatformCapabilities>,
}

impl Platform for UefiPlatform {
    fn volume_count(&self) -> usize {
        self.disks.len()
    }

    fn volume(&self, index: usize) -> &dyn Volume {
        &self.disks[index].volume
    }

    fn boot_volume(&self) -> Option<usize> {
        self.disks.iter().position(|disk| disk.boot)
    }

    fn arch(&self) -> CpuArch {
        cpu_arch()
    }

    fn skip_paths(&self, index: usize) -> Vec<String> {
        if self.disks.get(index).is_some_and(|disk| disk.boot) {
            self.skip.clone()
        } else {
            Vec::new()
        }
    }

    fn load_options(&self) -> Vec<ParsedLoadOption> {
        self.options.clone()
    }

    fn now_unix(&self) -> Option<i64> {
        firmware_unix()
    }

    fn elapsed_us(&self) -> u64 {
        self.clock.elapsed_us()
    }

    fn mark(&self, text: &str) {
        mark(text);
    }

    fn random(&mut self) -> Option<[u8; 16]> {
        firmware::rng_bytes()
    }

    fn entropy(&self) -> Vec<u8> {
        firmware::entropy()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        self.caps
            .get_or_init(|| {
                firmware::platform(
                    self.gop,
                    self.disks.iter().any(|disk| disk.volume.removable()),
                    firmware::nvram_accepts_volatile_write(),
                    firmware::protocol_present::<Tcg2>() || firmware::protocol_present::<Tcg1>(),
                    network_present(),
                    firmware::ram_mib(),
                )
            })
            .clone()
    }

    fn network_link(&self) -> bool {
        network_present()
    }

    fn set_var(&mut self, var: HandoffVar, value: &str) -> bool {
        firmware::set_handoff_var(var, value)
    }

    fn delete_var(&mut self, var: HandoffVar) {
        firmware::delete_handoff_var(var);
    }
}

/// Facts for the diagnostics screen, read once instead of every frame.
struct Facts {
    firmware: String,
    secure_boot: SecureBootState,
    display: Option<String>,
    ram_mib: Option<u64>,
}

fn run() -> ! {
    let clock = firmware::Clock::start();
    mark("uefi_entry");
    mark(&alloc::format!("clock hz={}", clock.hz()));
    let arch = cpu_arch();
    let host = host_image();
    let mut gop = Gop::open();
    let (width, height) = gop
        .as_ref()
        .map(|screen| (screen.width(), screen.height()))
        .unwrap_or((1024, 768));
    let mut fb = Framebuffer::new(width, height);
    if let Some(screen) = gop.as_mut() {
        paint_splash(&mut fb, true);
        screen.present(&fb);
        mark(&alloc::format!("gop {}", screen.describe()));
        mark(&alloc::format!(
            "backbuffer bytes={}",
            fb.px.len().saturating_mul(4)
        ));
    } else {
        mark("gop missing");
    }
    let first_frame_us = clock.elapsed_us();
    mark(&alloc::format!("first_frame us={first_frame_us}"));

    let before_pages = conventional_pages();
    let disks = scan_disks(&host);
    let skip = self_skip(&host, arch);
    let options = firmware::boot_entries()
        .iter()
        .filter_map(|bytes| parse_load_option(bytes))
        .collect::<Vec<_>>();
    let mut platform = UefiPlatform {
        disks,
        clock,
        skip,
        options,
        gop: gop.is_some(),
        caps: OnceCell::new(),
    };
    let mut early = None;
    let mut picker = Picker::start(&mut platform, &mut || {
        early = poll_key();
        early.is_some()
    });
    if let (Some(before), Some(after)) = (before_pages, conventional_pages()) {
        let delta = before.saturating_sub(after);
        mark(&alloc::format!(
            "mem conventional_pages={after} delta={delta}"
        ));
    }
    for &index in &picker.visible {
        mark(&alloc::format!(
            "target {}",
            picker.entries[index].item.target.display_name
        ));
    }
    let facts = Facts {
        firmware: firmware::firmware_summary(),
        secure_boot: firmware::secure_boot_state(),
        display: gop.as_ref().map(Gop::describe),
        ram_mib: firmware::ram_mib(),
    };
    let discovery_us = picker.timings.discovery_us;
    let mut state = picker.initial_state(true);
    let mut mouse = Mouse::open(width, height);
    paint_current(
        &mut fb,
        &picker.entries,
        &picker.visible,
        &state,
        &diag_lines(&picker, &facts, first_frame_us, discovery_us, None),
    );
    if let Some(screen) = gop.as_mut() {
        screen.present(&fb);
    }
    let menu_ready_us = platform.clock.elapsed_us();
    mark(&alloc::format!("menu_ready us={menu_ready_us}"));
    firmware::lab_marker(
        "menu-ready",
        &alloc::format!(
            "us={menu_ready_us} arch={} targets={}",
            arch_name(arch),
            picker.visible.len()
        ),
    );
    let mut pending = early;
    if state.countdown_ms == Some(0)
        && let Some(index) = picker.entry_for_card(state.selected)
    {
        boot_entry(
            &mut platform,
            &mut picker,
            index,
            &mut state,
            first_frame_us,
        );
    }

    let mut last_us = platform.clock.elapsed_us();
    loop {
        let now = platform.clock.elapsed_us();
        let dt_ms = u32::try_from(now.saturating_sub(last_us) / 1_000)
            .unwrap_or(1000)
            .min(1000);
        last_us = now;
        let key = pending.take().or_else(poll_key);
        let sample = mouse.poll(width, height);
        if sample.active {
            state.cursor = Some((sample.x, sample.y));
        }
        let cards = picker.visible.len();
        let cols = layout_menu(width, height, cards, 3, state.selected).cols;
        let pointer = Pointer {
            x: sample.x,
            y: sample.y,
            moved: sample.moved,
            clicked: sample.clicked,
        };
        let pressed = key.is_some();
        let input = menu_input(key, &pointer, width, height, cards, state.selected);
        if pressed {
            match input {
                Some(Input::Activate) => mark("key ret"),
                Some(Input::Theme) => mark("key t"),
                Some(Input::Recovery) => mark("key c"),
                _ => {}
            }
        }
        let outcome = step(&mut state, input, cards, cols, dt_ms);
        match outcome {
            Outcome::Boot => {
                if let Some(index) = picker.entry_for_card(state.selected) {
                    boot_entry(
                        &mut platform,
                        &mut picker,
                        index,
                        &mut state,
                        first_frame_us,
                    );
                }
            }
            Outcome::Recovery => match recovery_position(&picker.entries) {
                Some(index) => boot_entry(
                    &mut platform,
                    &mut picker,
                    index,
                    &mut state,
                    first_frame_us,
                ),
                None => {
                    state.screen = Screen::Error;
                    state.message = String::from(NO_RECOVERY_MESSAGE);
                }
            },
            Outcome::Action(0) => match firmware::reboot_to_firmware_ui() {
                Ok(()) => {}
                Err(message) => {
                    state.screen = Screen::Error;
                    state.message = String::from(message);
                }
            },
            Outcome::Action(1) => firmware::reboot(),
            Outcome::Action(2) => firmware::shutdown(),
            Outcome::Action(_) | Outcome::Exit | Outcome::None => {}
        }
        paint_current(
            &mut fb,
            &picker.entries,
            &picker.visible,
            &state,
            &diag_lines(
                &picker,
                &facts,
                first_frame_us,
                discovery_us,
                Some(menu_ready_us),
            ),
        );
        if let Some(screen) = gop.as_mut() {
            screen.present(&fb);
        }
        boot::stall(Duration::from_millis(16));
    }
}

/// Prepare (checks, verification, persistence, handoff), then `LoadImage`
/// and `StartImage`. Any problem ends on the error screen; nothing else is
/// started in its place.
fn boot_entry(
    platform: &mut UefiPlatform,
    picker: &mut Picker,
    index: usize,
    state: &mut MenuState,
    first_frame_us: u64,
) {
    state.countdown_ms = None;
    let message = match picker.prepare(platform, index, first_frame_us) {
        Err(message) => Some(message),
        Ok(plan) => {
            let device_path = platform.disks[plan.volume].volume.device_path().to_vec();
            let result = chainload(
                &device_path,
                &plan.path,
                plan.load_options.as_deref(),
                &mut || picker.loaded(platform, &plan),
            );
            for disk in &platform.disks {
                disk.volume.forget_root();
            }
            if let LaunchResult::Returned { status, .. } = &result {
                firmware::lab_marker("chainload-returned", &alloc::format!("status={status}"));
            }
            picker.finish(platform, &plan, &result)
        }
    };
    match message {
        Some(message) => {
            state.screen = Screen::Error;
            state.message = message;
        }
        None => {
            state.screen = Screen::Menu;
            state.message.clear();
        }
    }
    let _ = boot::set_watchdog_timer(0, 0, None);
}

fn diag_lines(
    picker: &Picker,
    facts: &Facts,
    first_us: u64,
    discovery_us: u64,
    menu_us: Option<u64>,
) -> Vec<String> {
    sbm_menu::diag_lines(
        &picker.entries,
        &DiagFacts {
            firmware: &facts.firmware,
            arch: cpu_arch(),
            secure_boot: facts.secure_boot,
            display: facts.display.as_deref(),
            ram_mib: facts.ram_mib,
            first_frame_us: first_us,
            discovery_us,
            menu_us,
            integrity: picker.integrity_note.as_deref(),
        },
    )
}

fn self_skip(host: &HostImage, arch: CpuArch) -> Vec<String> {
    let standard = match arch {
        CpuArch::X86_64 => Some("\\EFI\\BOOT\\BOOTX64.EFI"),
        CpuArch::Aarch64 => Some("\\EFI\\BOOT\\BOOTAA64.EFI"),
        CpuArch::Ia32 => Some("\\EFI\\BOOT\\BOOTIA32.EFI"),
        CpuArch::Riscv64 => None,
    };
    let mut skip = Vec::new();
    match &host.path {
        Some(path) => {
            skip.push(normalize(path));
            let name = path.rsplit('\\').next().unwrap_or("");
            if let Some(standard) = standard
                && standard
                    .rsplit('\\')
                    .next()
                    .is_some_and(|file| file.eq_ignore_ascii_case(name))
                && !skip.iter().any(|item| item.eq_ignore_ascii_case(standard))
            {
                skip.push(String::from(standard));
            }
        }
        None => skip.extend(standard.map(String::from)),
    }
    skip
}

fn network_present() -> bool {
    firmware::protocol_present::<SimpleNetwork>()
}

fn conventional_pages() -> Option<u64> {
    use uefi::mem::memory_map::{MemoryMap, MemoryType};
    let map = boot::memory_map(MemoryType::LOADER_DATA).ok()?;
    let mut pages = 0u64;
    for descriptor in map.entries() {
        if descriptor.ty == MemoryType::CONVENTIONAL {
            pages = pages.saturating_add(descriptor.page_count);
        }
    }
    Some(pages)
}
