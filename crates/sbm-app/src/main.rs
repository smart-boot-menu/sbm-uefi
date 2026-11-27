#![no_main]
#![no_std]

extern crate alloc;

mod chainload;
mod firmware;
mod present;
mod volume;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;

use sbm_boot::{
    DiscoveredTarget, LocalRequest, ResolveInput, Resolved, Volume, add_load_options, atomic_write,
    begin_attempt, build_handoff, confirm_attempt, decode_attempt, decode_command, decode_policy,
    decode_success, discover_volume, embedded_cmdline, encode_attempt, encode_handoff,
    encode_policy, evaluate_remote, fresh_policy, handoff_sha256_hex, linux_load_options,
    local_request, mark_one_shot_consumed, merge_load_options, normalize, parse_load_option,
    recovery_for_slot, resolve, selectable,
};
use sbm_gfx::{
    FontMode, Framebuffer, Input, MenuState, Outcome, Screen, TextRenderer, UiScale, layout_menu,
    paint_splash_with, step,
};
use sbm_menu::{
    DiagFacts, MenuEntry as Found, NO_RECOVERY_MESSAGE, Pointer, StartFailure, VERSION, arch_name,
    card_position, countdown_ms, menu_input, paint_current_with, recovery_position, sort_entries,
    unhealthy_message, visible_positions,
};
use sbm_protocol::{
    BootAttemptRecord, BootAttemptStatus, BootTimingMetrics, NetworkHandoffHint, RecoveryLaunchMode,
};
use sbm_types::{BootTargetKind, CpuArch};
use uefi::boot;
use uefi::proto::network::snp::SimpleNetwork;
use uefi::proto::tcg::v1::Tcg as Tcg1;
use uefi::proto::tcg::v2::Tcg as Tcg2;
use uefi::{Status, entry, helpers};

use crate::chainload::{ChainOutcome, chainload};
use crate::firmware::{cpu_arch, firmware_unix, mark, platform, poll_key};
use crate::present::{Mouse, Screen as Gop};
use crate::volume::{Disk, HostImage, host_image, scan_disks};

const POLICY_PATH: &str = "\\EFI\\sbm\\policy.json";
const ATTEMPT_PATH: &str = "\\EFI\\sbm\\state\\last_attempt.json";
const HANDOFF_PATH: &str = "\\EFI\\sbm\\handoff\\current.json";
const SUCCESS_PATH: &str = "\\EFI\\sbm\\state\\boot-success.json";
const COMMAND_PATH: &str = "\\EFI\\sbm\\handoff\\requested-command.json";

#[used]
#[unsafe(link_section = ".sbat")]
static SBAT: [u8; 148] = *b"\
sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md\n\
sbm,1,Smart Boot Menu,sbm,1,https://github.com/smart-boot-menu/sbm-uefi\n";

#[derive(Clone, PartialEq, Eq)]
struct FrameKey {
    selected: usize,
    screen: Screen,
    dark: bool,
    countdown_sec: Option<u32>,
    cursor: Option<(u32, u32)>,
    message: String,
}

impl FrameKey {
    fn from_state(state: &MenuState) -> Self {
        Self {
            selected: state.selected,
            screen: state.screen,
            dark: state.dark,
            countdown_sec: state.countdown_ms.map(|ms| ms.div_ceil(1000)),
            cursor: state.cursor,
            message: state.message.clone(),
        }
    }
}

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
    let mut text_renderer =
        TextRenderer::new(FontMode::AtkinsonNextMono, height, UiScale::default());
    if let Some(screen) = gop.as_mut() {
        paint_splash_with(&mut fb, true, false, &mut text_renderer);
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
    let mut disks = scan_disks(&host);
    let skip = self_skip(&host, arch);
    let parsed = firmware::boot_entries()
        .iter()
        .filter_map(|bytes| parse_load_option(bytes))
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    for (index, disk) in disks.iter().enumerate() {
        let skips: &[String] = if disk.boot { &skip } else { &[] };
        let mut found = discover_volume(&disk.volume, arch, skips);
        add_load_options(&disk.volume, &parsed, arch, skips, &mut found);
        for item in found {
            entries.push(Found { disk: index, item });
        }
    }
    sort_entries(&mut entries);
    let discovery_us = clock.elapsed_us();
    mark(&alloc::format!(
        "discovery_done us={discovery_us} count={}",
        entries.len()
    ));
    if let (Some(before), Some(after)) = (before_pages, conventional_pages()) {
        let delta = before.saturating_sub(after);
        mark(&alloc::format!(
            "mem conventional_pages={after} delta={delta}"
        ));
    }
    for entry in entries.iter().filter(|entry| !entry.item.target.hidden) {
        mark(&alloc::format!("target {}", entry.item.target.display_name));
    }

    let mut policy = load_policy(&disks);
    let mut attempt = load_attempt(&disks);
    apply_success(&mut disks, &mut attempt);
    let early = poll_key();
    let decision = evaluate_remote(
        policy.allow_remote_boot_select,
        policy.network_policy_timeout_ms,
        gop_network(),
        early.is_some(),
        None,
        |id| entries.iter().any(|entry| &entry.item.target.id == id),
        "sbm-uefi",
        0,
        firmware_unix(),
    );
    let remote_reason = match &decision {
        sbm_boot::RemoteDecision::Skip { reason, .. } => *reason,
        sbm_boot::RemoteDecision::Reject(_) => "rejected",
        sbm_boot::RemoteDecision::Accept(_) => "accepted",
    };
    mark(&alloc::format!("remote {remote_reason}"));
    let remote_refresh_failed = policy.allow_remote_boot_select
        && remote_reason != "local-key"
        && remote_reason != "accepted";
    let targets = entries
        .iter()
        .map(|entry| entry.item.clone())
        .collect::<Vec<_>>();
    let mut resolved = resolve(ResolveInput {
        policy: &policy,
        targets: &targets,
        attempt: attempt.as_ref(),
        now_unix: firmware_unix(),
        user_key_held: early.is_some(),
        remote_refresh_failed,
    });
    if let sbm_boot::RemoteDecision::Accept(applied) = &decision {
        apply_remote(&mut resolved, applied, &targets);
    }
    apply_command(&mut disks, &targets, &mut resolved);
    let visible_index = visible_positions(&entries);
    let selected = resolved
        .highlight
        .as_ref()
        .and_then(|id| card_position(&entries, &visible_index, id))
        .unwrap_or(0);
    let mut state = MenuState::new(selected, true, countdown_ms(resolved.countdown_seconds));
    let mut mouse = Mouse::open(width, height);
    paint_current_with(
        &mut fb,
        &entries,
        &visible_index,
        &state,
        &diag_lines(&entries, &gop, first_frame_us, discovery_us, None),
        false,
        &mut text_renderer,
    );
    if let Some(screen) = gop.as_mut() {
        screen.present(&fb);
    }
    let menu_ready_us = clock.elapsed_us();
    let cache_stats = text_renderer.cache_stats();
    mark(&alloc::format!(
        "glyph_cache entries={} bytes={} hits={} misses={}",
        cache_stats.entries,
        cache_stats.bytes,
        cache_stats.hits,
        cache_stats.misses
    ));
    mark(&alloc::format!("menu_ready us={menu_ready_us}"));
    firmware::lab_marker(
        "menu-ready",
        &alloc::format!(
            "us={menu_ready_us} arch={} targets={}",
            arch_name(arch),
            visible_index.len()
        ),
    );
    let caps = platform(
        gop.is_some(),
        disks.iter().any(|disk| disk.volume.removable()),
        firmware::nvram_accepts_volatile_write(),
        firmware::protocol_present::<Tcg2>() || firmware::protocol_present::<Tcg1>(),
        gop_network(),
        firmware::ram_mib(),
    );
    let mut pending = early;
    if state.countdown_ms == Some(0)
        && let Some(index) = visible_index.get(state.selected).copied()
    {
        state.countdown_ms = None;
        boot_at(
            &mut disks,
            &entries,
            index,
            &mut policy,
            &mut attempt,
            &mut state,
            &caps,
            &clock,
            first_frame_us,
        );
    }

    let mut last_frame = FrameKey::from_state(&state);
    let mut last_us = clock.elapsed_us();
    loop {
        let now = clock.elapsed_us();
        let dt_ms = u32::try_from(now.saturating_sub(last_us) / 1_000)
            .unwrap_or(1000)
            .min(1000);
        last_us = now;
        let key = pending.take().or_else(poll_key);
        let sample = mouse.poll(width, height);
        if sample.active {
            state.cursor = Some((sample.x, sample.y));
        }
        let cards = visible_index.len();
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
            Outcome::Exit => {}
            Outcome::Boot => {
                if let Some(index) = visible_index.get(state.selected).copied() {
                    boot_at(
                        &mut disks,
                        &entries,
                        index,
                        &mut policy,
                        &mut attempt,
                        &mut state,
                        &caps,
                        &clock,
                        first_frame_us,
                    );
                }
            }
            Outcome::Recovery => match recovery_position(&entries) {
                Some(index) => boot_at(
                    &mut disks,
                    &entries,
                    index,
                    &mut policy,
                    &mut attempt,
                    &mut state,
                    &caps,
                    &clock,
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
            Outcome::Action(_) => {}
            Outcome::None => {}
        }
        let next_frame = FrameKey::from_state(&state);
        if next_frame != last_frame {
            last_frame = next_frame;
            let ready = Some(menu_ready_us);
            paint_current_with(
                &mut fb,
                &entries,
                &visible_index,
                &state,
                &diag_lines(&entries, &gop, first_frame_us, discovery_us, ready),
                false,
                &mut text_renderer,
            );
            if let Some(screen) = gop.as_mut() {
                screen.present(&fb);
            }
        }
        boot::stall(Duration::from_millis(16));
    }
}

#[allow(clippy::too_many_arguments)]
fn boot_at(
    disks: &mut [Disk],
    entries: &[Found],
    index: usize,
    policy: &mut sbm_protocol::BootPolicy,
    attempt: &mut Option<BootAttemptRecord>,
    state: &mut MenuState,
    caps: &sbm_types::PlatformCapabilities,
    clock: &firmware::Clock,
    first_frame_us: u64,
) {
    match launch(
        disks,
        entries,
        index,
        policy,
        attempt,
        caps,
        clock,
        first_frame_us,
    ) {
        Ok(()) => {
            state.screen = Screen::Menu;
            state.countdown_ms = None;
            state.message.clear();
        }
        Err(message) => {
            state.screen = Screen::Error;
            state.message = message;
            state.countdown_ms = None;
        }
    }
    let _ = boot::set_watchdog_timer(0, 0, None);
}

#[allow(clippy::too_many_arguments)]
fn launch(
    disks: &mut [Disk],
    entries: &[Found],
    index: usize,
    policy: &mut sbm_protocol::BootPolicy,
    attempt: &mut Option<BootAttemptRecord>,
    caps: &sbm_types::PlatformCapabilities,
    clock: &firmware::Clock,
    first_frame_us: u64,
) -> Result<(), String> {
    let Some(found) = entries.get(index) else {
        return Err(String::from("That boot entry is no longer available"));
    };
    if !selectable(&found.item) {
        return Err(unhealthy_message(&found.item.target.display_name));
    }
    let name = found.item.target.display_name.clone();
    let id = found.item.target.id.clone();
    let path = found
        .item
        .target
        .efi_path
        .clone()
        .ok_or_else(|| alloc::format!("{name} has no EFI path"))?;
    let disk = found.disk;
    let kind = found.item.target.kind;
    let cmdline = found.item.target.kernel_cmdline.clone();
    if policy
        .one_shot
        .as_ref()
        .is_some_and(|shot| shot.target_id == id && !shot.consumed)
    {
        mark_one_shot_consumed(policy);
        match encode_policy(policy) {
            Ok(bytes) => persist(disks, POLICY_PATH, &bytes, "policy"),
            Err(_) => mark("persist policy failed"),
        }
    }
    let mut record = begin_attempt(
        attempt.as_ref(),
        &id,
        firmware_unix(),
        &alloc::format!("uefi-{}", clock.elapsed_us()),
    );
    match encode_attempt(&record) {
        Ok(bytes) => persist(disks, ATTEMPT_PATH, &bytes, "attempt"),
        Err(_) => mark("persist attempt failed"),
    }
    let mut options = cmdline.clone();
    if kind == BootTargetKind::SbmCore {
        let snapshot = entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let payload = build_handoff(
            &record.attempt_id,
            VERSION,
            caps.clone(),
            RecoveryLaunchMode::Interactive,
            None,
            &snapshot,
            BootTimingMetrics {
                firmware_init_us: None,
                uefi_entry_us: 0,
                first_frame_us: Some(first_frame_us),
                network_us: Some(0),
                handoff_us: clock.elapsed_us(),
            },
            Some(NetworkHandoffHint {
                link_up: gop_network(),
                dhcp_acquired: false,
                server_reachable: None,
            }),
        );
        let hash = encode_handoff(&payload).ok().and_then(|bytes| {
            persist(disks, HANDOFF_PATH, &bytes, "handoff");
            handoff_sha256_hex(&payload).ok()
        });
        let esp = disks
            .iter()
            .find(|disk| disk.boot)
            .map(|disk| disk.volume.partition_guid().to_string());
        options = linux_load_options(cmdline.as_deref(), hash.as_deref(), esp.as_deref());
    }
    let embedded = disks
        .get(disk)
        .and_then(|item| embedded_cmdline(&item.volume, &path));
    mark(if embedded.is_some() {
        "uki cmdline=embedded"
    } else {
        "uki cmdline=absent"
    });
    let (options, option_mode) = merge_load_options(embedded.as_deref(), options.as_deref());
    mark(&alloc::format!("load-options {option_mode}"));
    firmware::publish_selection(id.as_str());
    mark(&alloc::format!("chainload {name}"));
    let device_path = disks
        .get(disk)
        .map(|item| item.volume.device_path().to_vec())
        .unwrap_or_default();
    let outcome = chainload(&device_path, &path, options.as_deref(), || {
        disks
            .get(disk)
            .and_then(|item| item.volume.read(&path).ok())
    });
    record.status = BootAttemptStatus::Failed;
    match encode_attempt(&record) {
        Ok(bytes) => persist(disks, ATTEMPT_PATH, &bytes, "attempt"),
        Err(_) => mark("persist attempt failed"),
    }
    *attempt = Some(record);
    let _ = boot::set_watchdog_timer(0, 0, None);
    match outcome {
        ChainOutcome::LoadFailed(status) => Err(boot_failure(&name, &path, status)),
        ChainOutcome::Returned(status) => {
            mark(&alloc::format!("chainload_returned status={status:?}"));
            firmware::lab_marker("chainload-returned", &alloc::format!("status={status:?}"));
            if status == Status::SUCCESS {
                Ok(())
            } else {
                Err(boot_failure(&name, &path, status))
            }
        }
    }
}

fn boot_failure(name: &str, path: &str, status: Status) -> String {
    let failure = if status == Status::SECURITY_VIOLATION {
        StartFailure::SecurityViolation
    } else if status == Status::NOT_FOUND {
        StartFailure::NotFound
    } else if status == Status::UNSUPPORTED {
        StartFailure::Unsupported
    } else {
        StartFailure::Other
    };
    sbm_menu::boot_failure(name, path, failure, &alloc::format!("{status:?}"))
}

fn diag_lines(
    entries: &[Found],
    gop: &Option<Gop>,
    first_us: u64,
    discovery_us: u64,
    menu_us: Option<u64>,
) -> Vec<String> {
    let firmware = firmware::firmware_summary();
    let display = gop.as_ref().map(Gop::describe);
    sbm_menu::diag_lines(
        entries,
        &DiagFacts {
            firmware: &firmware,
            arch: cpu_arch(),
            secure_boot: firmware::secure_boot_state(),
            display: display.as_deref(),
            ram_mib: firmware::ram_mib(),
            first_frame_us: first_us,
            discovery_us,
            menu_us,
        },
    )
}

fn load_policy(disks: &[Disk]) -> sbm_protocol::BootPolicy {
    let Some(bytes) = read_boot(disks, POLICY_PATH) else {
        return fresh_policy();
    };
    match decode_policy(&bytes) {
        Ok(policy) => policy,
        Err(_) => {
            mark("persist policy failed");
            fresh_policy()
        }
    }
}

fn load_attempt(disks: &[Disk]) -> Option<BootAttemptRecord> {
    let bytes = read_boot(disks, ATTEMPT_PATH)?;
    decode_attempt(&bytes).ok()
}

fn read_boot(disks: &[Disk], path: &str) -> Option<Vec<u8>> {
    let disk = disks.iter().find(|disk| disk.boot)?;
    disk.volume.read(path).ok()
}

fn persist(disks: &mut [Disk], path: &str, bytes: &[u8], what: &str) {
    let Some(disk) = disks.iter_mut().find(|disk| disk.boot) else {
        mark(&alloc::format!("persist {what} failed"));
        return;
    };
    if atomic_write(&disk.volume, path, bytes).is_err() {
        mark(&alloc::format!("persist {what} failed"));
    }
}

fn self_skip(host: &HostImage, arch: CpuArch) -> Vec<String> {
    let mut skip = Vec::new();
    if let Some(path) = &host.path {
        skip.push(normalize(path));
        let name = path.rsplit('\\').next().unwrap_or("").to_ascii_uppercase();
        let standard = match (arch, name.as_str()) {
            (CpuArch::X86_64, "BOOTX64.EFI") => Some("\\EFI\\BOOT\\BOOTX64.EFI"),
            (CpuArch::Aarch64, "BOOTAA64.EFI") => Some("\\EFI\\BOOT\\BOOTAA64.EFI"),
            (CpuArch::Ia32, "BOOTIA32.EFI") => Some("\\EFI\\BOOT\\BOOTIA32.EFI"),
            _ => None,
        };
        if let Some(path) = standard
            && !skip.iter().any(|item| item.eq_ignore_ascii_case(path))
        {
            skip.push(String::from(path));
        }
    } else {
        let standard = match arch {
            CpuArch::X86_64 => Some("\\EFI\\BOOT\\BOOTX64.EFI"),
            CpuArch::Aarch64 => Some("\\EFI\\BOOT\\BOOTAA64.EFI"),
            CpuArch::Ia32 => Some("\\EFI\\BOOT\\BOOTIA32.EFI"),
            _ => None,
        };
        if let Some(path) = standard {
            skip.push(String::from(path));
        }
    }
    skip
}

fn apply_remote(
    resolved: &mut sbm_boot::Resolved,
    applied: &sbm_boot::Applied,
    targets: &[sbm_boot::DiscoveredTarget],
) {
    match applied {
        sbm_boot::Applied::Boot { id, .. } => {
            resolved.highlight = Some(id.clone());
            resolved.countdown_seconds = Some(0);
        }
        sbm_boot::Applied::Recovery(slot) => {
            if let Some(found) = sbm_boot::recovery_for_slot(targets, *slot) {
                resolved.highlight = Some(found.target.id.clone());
                resolved.countdown_seconds = Some(0);
            }
        }
        sbm_boot::Applied::Firmware | sbm_boot::Applied::Reboot | sbm_boot::Applied::Shutdown => {
            resolved.countdown_seconds = None;
        }
    }
}

fn apply_success(disks: &mut [Disk], attempt: &mut Option<BootAttemptRecord>) {
    let Some((index, bytes)) = read_any(disks, SUCCESS_PATH) else {
        return;
    };
    let Ok(report) = decode_success(&bytes) else {
        mark("attempt confirm rejected");
        let _ = disks[index].volume.remove(SUCCESS_PATH);
        return;
    };
    let Some(record) = attempt.as_mut() else {
        mark("attempt confirm skipped");
        return;
    };
    if !confirm_attempt(record, &report) {
        mark("attempt confirm skipped");
        return;
    }
    mark(&alloc::format!("attempt confirmed {}", record.attempt_id));
    if let Ok(encoded) = encode_attempt(record) {
        persist(disks, ATTEMPT_PATH, &encoded, "attempt");
    }
    let _ = disks[index].volume.remove(SUCCESS_PATH);
}

fn apply_command(disks: &mut [Disk], targets: &[DiscoveredTarget], resolved: &mut Resolved) {
    let Some((index, bytes)) = read_any(disks, COMMAND_PATH) else {
        return;
    };
    let Ok(command) = decode_command(&bytes) else {
        mark("command rejected");
        let _ = disks[index].volume.remove(COMMAND_PATH);
        return;
    };
    let _ = disks[index].volume.remove(COMMAND_PATH);
    match local_request(&command) {
        LocalRequest::Reboot => {
            mark("command reboot");
            firmware::reboot();
        }
        LocalRequest::Shutdown => {
            mark("command shutdown");
            firmware::shutdown();
        }
        LocalRequest::Firmware => {
            mark("command firmware");
            if firmware::reboot_to_firmware_ui().is_err() {
                mark("command firmware unavailable");
            }
        }
        LocalRequest::Boot(id) => {
            mark(&alloc::format!("command boot {}", id.as_str()));
            resolved.highlight = Some(id);
            resolved.countdown_seconds = Some(0);
        }
        LocalRequest::Recovery(slot) => {
            if let Some(target) = recovery_for_slot(targets, slot) {
                mark(&alloc::format!(
                    "command recovery {}",
                    target.target.id.as_str()
                ));
                resolved.highlight = Some(target.target.id.clone());
                resolved.countdown_seconds = Some(0);
            } else {
                mark("command recovery missing");
            }
        }
    }
}

fn read_any(disks: &[Disk], path: &str) -> Option<(usize, Vec<u8>)> {
    if let Some(index) = disks.iter().position(|disk| disk.boot)
        && let Ok(bytes) = disks[index].volume.read(path)
    {
        return Some((index, bytes));
    }
    disks.iter().enumerate().find_map(|(index, disk)| {
        if disk.boot {
            None
        } else {
            disk.volume.read(path).ok().map(|bytes| (index, bytes))
        }
    })
}

fn gop_network() -> bool {
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
