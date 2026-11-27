//! Scenario behaviour through the production discovery, policy and menu code.

use sbm_menu::Key;
use sbm_preview::scenario::{self, Scenario};
use sbm_preview::session::{CoreExit, SimScreen};
use sbm_preview::{Effect, Session, core, load_session, parse_script, render, run_script};

const W: u32 = 1280;
const H: u32 = 800;

fn cards(session: &Session) -> Vec<String> {
    session
        .visible()
        .iter()
        .map(|&index| {
            let item = &session.entries()[index].item;
            let mark = if sbm_boot::selectable(item) { "" } else { "!" };
            format!("{}{mark}", item.target.display_name)
        })
        .collect()
}

fn session(name: &str) -> Session {
    let mut session = load_session(name, true).unwrap();
    assert_eq!(session.take_pending(), Effect::None);
    session
}

#[test]
fn every_scenario_parses_and_shows_a_menu() {
    let paths = scenario::list(&scenario::scenario_dir()).unwrap();
    assert!(paths.len() >= 10, "{paths:?}");
    for path in paths {
        let scenario = Scenario::load(&path).unwrap();
        let name = scenario.id.clone();
        let session = session(&name);
        assert!(session.cards() > 0, "{name} has no cards");
        assert_eq!(session.screen_name(), "menu", "{name}");
    }
}

#[test]
fn scenarios_discover_the_expected_targets() {
    let expect: &[(&str, &[&str])] = &[
        ("empty-uefi", &["SBM Core Slot A"]),
        ("windows-only", &["Windows", "SBM Core Slot A"]),
        ("linux-only", &["Fedora Linux 45", "SBM Core Slot A"]),
        ("dual-boot", &["Windows", "Ubuntu", "SBM Core Slot A"]),
        (
            "multi-disk",
            &[
                "Windows",
                "Arch Linux",
                "systemd-boot",
                "Debian",
                "External drive",
                "SBM Core Slot A",
            ],
        ),
        ("low-disk-space", &["Windows", "SBM Core Slot A"]),
        ("bitlocker", &["Windows", "SBM Core Slot A"]),
        ("corrupt-boot", &["Windows", "Ubuntu", "SBM Core Slot A"]),
        (
            "secure-boot-reject",
            &["Windows", "Arch Linux", "systemd-boot", "SBM Core Slot A"],
        ),
        ("existing-sbm", &["Debian", "Fedora", "SBM Core Slot A"]),
    ];
    for (name, want) in expect {
        assert_eq!(cards(&session(name)), *want, "{name}");
    }
}

#[test]
fn countdown_boots_the_policy_default() {
    let mut session = session("dual-boot");
    assert_eq!(session.last_reason, "default");
    assert_eq!(session.state.countdown_ms, Some(8000));
    run_script(&mut session, &parse_script("wait:8100").unwrap(), W, H);
    match &session.sim {
        Some(SimScreen::Booted { name, .. }) => assert_eq!(name, "Windows"),
        other => panic!("expected Windows to boot, got {other:?}"),
    }
}

#[test]
fn failed_default_falls_back_to_recovery() {
    let session = session("existing-sbm");
    assert_eq!(session.last_reason, "fallback");
    assert_eq!(cards(&session)[session.state.selected], "SBM Core Slot A");
}

#[test]
fn secure_boot_rejection_shows_the_production_error() {
    let mut session = session("secure-boot-reject");
    run_script(&mut session, &parse_script("Right,Enter").unwrap(), W, H);
    assert_eq!(session.screen_name(), "error");
    assert!(
        session.state.message.contains("Secure Boot"),
        "{}",
        session.state.message
    );
    run_script(&mut session, &parse_script("Escape").unwrap(), W, H);
    assert_eq!(session.screen_name(), "menu");
}

#[test]
fn corrupt_loader_and_slot_are_reported() {
    let mut session = session("corrupt-boot");
    run_script(&mut session, &parse_script("Right,Enter").unwrap(), W, H);
    assert!(
        session.state.message.contains("LOAD_ERROR"),
        "{}",
        session.state.message
    );
    let mut session = load_session("corrupt-boot", true).unwrap();
    run_script(
        &mut session,
        &parse_script("Right,Right,Enter").unwrap(),
        W,
        H,
    );
    assert_eq!(session.screen_name(), "error");
    assert!(session.state.message.contains("SHA-256"));
    assert!(session.machine.vars.is_empty());
    assert!(
        session
            .machine
            .read_boot(sbm_preview::machine::HANDOFF_PATH)
            .is_none()
    );
}

#[test]
fn bitlocker_boot_explains_the_unlock() {
    let mut session = session("bitlocker");
    run_script(&mut session, &parse_script("Enter").unwrap(), W, H);
    match &session.sim {
        Some(SimScreen::Booted { notes, .. }) => {
            assert!(notes.iter().any(|note| note.contains("BitLocker")))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn power_actions_reach_simulated_screens() {
    let mut session = session("windows-only");
    run_script(&mut session, &parse_script("Tab,Enter").unwrap(), W, H);
    assert_eq!(session.sim, Some(SimScreen::FirmwareSetup));
    session.key(Key::Activate, W, H);
    assert_eq!(session.screen_name(), "menu");
}

#[test]
fn rendering_is_deterministic() {
    for name in ["dual-boot", "multi-disk", "corrupt-boot"] {
        let a = render(&session(name), W, H);
        let b = render(&session(name), W, H);
        assert!(a.px == b.px, "{name} renders differently");
    }
}

#[test]
fn core_round_trip_boots_the_requested_target_once() {
    let mut session = session("dual-boot");
    let Effect::LaunchCore(request) = session.key(Key::Char('c'), W, H) else {
        panic!("recovery key did not launch Core");
    };
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sbm-preview-core-{}", std::process::id()));
    core::materialize(&dir, &session.machine, Some(&request)).unwrap();
    assert!(dir.join("SIMULATED").is_file());
    assert!(dir.join("esp/EFI/sbm/handoff/current.json").is_file());

    // What sbm-sim writes when the user picks Ubuntu in Core and reboots.
    let ubuntu = session
        .entries()
        .iter()
        .find(|entry| entry.item.target.display_name == "Ubuntu")
        .unwrap()
        .item
        .target
        .id
        .clone();
    let payload: sbm_protocol::EfiHandoffPayload =
        serde_json::from_slice(&request.handoff).unwrap();
    let attempt_id = payload.attempt_id;
    let success = sbm_protocol::BootSuccessReport {
        schema_version: "1.0".into(),
        attempt_id: attempt_id.clone(),
        target_id: sbm_types::BootTargetId(sbm_boot::RECOVERY_TARGET_ID.into()),
        confirmed_by: sbm_types::ComponentKind::SbmCoreUki,
        confirmed_at_unix: 0,
    };
    let command = sbm_protocol::LocalBootRequest {
        schema_version: "1.0".into(),
        request_id: sbm_protocol::format_uuid_v4([27; 16]),
        attempt_id,
        requested_by: sbm_types::ComponentKind::SbmCoreUki,
        requested_at_unix: 0,
        command: sbm_protocol::BootCommand::BootTarget {
            target_id: ubuntu,
            one_shot: true,
            extra_cmdline_preset: None,
        },
    };
    let path = dir.join("esp/EFI/sbm/state/boot-success.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&success).unwrap()).unwrap();
    let path = dir.join("esp/EFI/sbm/handoff/requested-command.json");
    std::fs::write(path, serde_json::to_vec(&command).unwrap()).unwrap();
    core::import(&dir, &session.machine).unwrap();

    let effect = session.after_core(CoreExit::Reboot);
    assert_eq!(effect, Effect::Redraw);
    match &session.sim {
        Some(SimScreen::Booted { name, .. }) => assert_eq!(name, "Ubuntu"),
        other => panic!("expected the requested Ubuntu boot, got {other:?}"),
    }
    // The command is consumed: the next reset returns to the normal countdown.
    session.key(Key::Char('x'), W, H);
    assert_eq!(session.screen_name(), "menu");
    assert_eq!(session.last_reason, "default");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn materialized_lsblk_matches_scenario_layout() {
    let scenario = Scenario::load(&scenario::resolve("multi-disk").unwrap()).unwrap();
    let lsblk = core::lsblk(&scenario);
    let disks = lsblk["blockdevices"].as_array().unwrap();
    assert_eq!(disks.len(), scenario.disks.len());
    for (disk, want) in disks.iter().zip(&scenario.disks) {
        assert_eq!(
            disk["children"].as_array().unwrap().len(),
            want.partitions.len()
        );
        assert_eq!(disk["rm"].as_bool().unwrap(), want.removable);
    }
}

fn png_dimensions(path: &std::path::Path) -> (u32, u32) {
    let file = std::fs::File::open(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    (info.width, info.height)
}

#[test]
fn headless_captures_theme_pairs_and_multiple_resolutions() {
    let bin = env!("CARGO_BIN_EXE_sbm-preview");
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sbm-preview-matrix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let output = std::process::Command::new(bin)
        .args([
            "--headless",
            "--scenario",
            "dual-boot",
            "--screen",
            "menu",
            "--theme",
            "both",
            "--sizes",
            "800x600,1280x800",
            "--out",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (w, h) in [(800, 600), (1280, 800)] {
        let dark = dir.join(format!("uefi-dual-boot-menu-dark-{w}x{h}.png"));
        let light = dir.join(format!("uefi-dual-boot-menu-light-{w}x{h}.png"));
        assert_eq!(png_dimensions(&dark), (w, h));
        assert_eq!(png_dimensions(&light), (w, h));
        assert_ne!(
            std::fs::read(&dark).unwrap(),
            std::fs::read(&light).unwrap()
        );
    }

    // Single-size theme-pair keeps the unsuffixed filename format.
    let pair_dir = dir.join("single-size");
    let output = std::process::Command::new(bin)
        .args([
            "--headless",
            "--scenario",
            "windows-only",
            "--theme",
            "both",
            "--size",
            "1024x768",
            "--out",
        ])
        .arg(&pair_dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        png_dimensions(&pair_dir.join("uefi-windows-only-menu-dark.png")),
        (1024, 768)
    );
    assert_eq!(
        png_dimensions(&pair_dir.join("uefi-windows-only-menu-light.png")),
        (1024, 768)
    );

    // Invalid --sizes and --theme are rejected.
    for bad in [
        &["--headless", "--sizes", "not-a-size"][..],
        &["--headless", "--theme", "sepia"][..],
    ] {
        let status = std::process::Command::new(bin).args(bad).status().unwrap();
        assert!(!status.success(), "expected failure for {bad:?}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headless_sizes_all_renders_every_display_preset() {
    let bin = env!("CARGO_BIN_EXE_sbm-preview");
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sbm-preview-presets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let output = std::process::Command::new(bin)
        .args([
            "--headless",
            "--scenario",
            "empty-uefi",
            "--theme",
            "dark",
            "--sizes",
            "all",
            "--out",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for &(w, h) in sbm_preview::DISPLAY_PRESETS {
        let path = dir.join(format!("uefi-empty-uefi-menu-dark-{w}x{h}.png"));
        assert_eq!(png_dimensions(&path), (w, h));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

fn prepare_core(session: &mut Session) -> Box<sbm_preview::session::CoreRequest> {
    let Effect::LaunchCore(request) = session.key(Key::Char('c'), W, H) else {
        panic!("Manage Computer did not start the recovery UKI");
    };
    request
}

fn assert_interactive_error(session: &mut Session, message: &str) {
    assert_eq!(session.screen_name(), "error");
    assert!(
        session.state.message.contains(message),
        "{}",
        session.state.message
    );
    assert_eq!(session.state.countdown_ms, None);
    assert!(session.sim.is_none());
    session.key(Key::Escape, W, H);
    session.tick(30_000, W, H);
    assert_eq!(session.screen_name(), "menu");
    assert!(session.sim.is_none());
}

#[test]
fn unknown_and_legacy_requests_suppress_an_immediate_default() {
    use sbm_preview::machine::{POLICY_PATH, REQUEST_PATH};
    use sbm_types::BootTargetId;

    for legacy in [false, true] {
        let mut session = session("dual-boot");
        let request = prepare_core(&mut session);
        let policy =
            sbm_boot::decode_policy(&session.machine.read_boot(POLICY_PATH).unwrap()).unwrap();
        let immediate = sbm_protocol::BootPolicy {
            timeout_seconds: 0,
            ..policy
        };
        assert!(
            session
                .machine
                .write_boot(POLICY_PATH, &sbm_boot::encode_policy(&immediate).unwrap())
        );
        let target = BootTargetId("os:11111111-1111-1111-1111-111111111111".into());
        core::simulated_core_reports(&session.machine, &request, Some(&target)).unwrap();
        if legacy {
            let bare = sbm_protocol::BootCommand::BootTarget {
                target_id: target,
                one_shot: true,
                extra_cmdline_preset: None,
            };
            assert!(
                session
                    .machine
                    .write_boot(REQUEST_PATH, &serde_json::to_vec(&bare).unwrap())
            );
        }
        session.after_core(CoreExit::Reboot);
        assert_interactive_error(&mut session, if legacy { "format" } else { "present" });
        assert!(session.machine.read_boot(REQUEST_PATH).is_none());
    }
}

#[test]
fn stale_success_and_request_cannot_use_a_later_attempt() {
    use sbm_preview::machine::{ATTEMPT_PATH, REQUEST_PATH, SUCCESS_PATH};

    let mut session = session("dual-boot");
    let first = prepare_core(&mut session);
    let target = session.machine.target_for_preset("ubuntu").unwrap();
    core::simulated_core_reports(&session.machine, &first, Some(&target)).unwrap();
    let saved = [REQUEST_PATH, SUCCESS_PATH].map(|path| session.machine.read_boot(path).unwrap());
    session.after_core(CoreExit::Reboot);
    session.key(Key::Char('x'), W, H);

    let second = prepare_core(&mut session);
    assert_ne!(
        first.var("SbmBootAttemptId"),
        second.var("SbmBootAttemptId")
    );
    for (path, bytes) in [REQUEST_PATH, SUCCESS_PATH].into_iter().zip(saved) {
        assert!(session.machine.write_boot(path, &bytes));
    }
    session.after_core(CoreExit::Reboot);
    assert_interactive_error(&mut session, "current SBM Core session");
    let attempt =
        sbm_boot::decode_attempt(&session.machine.read_boot(ATTEMPT_PATH).unwrap()).unwrap();
    assert_eq!(attempt.status, sbm_protocol::BootAttemptStatus::Started);
    assert_eq!(
        Some(attempt.attempt_id.as_str()),
        second.var("SbmBootAttemptId")
    );
    assert!(session.machine.read_boot(SUCCESS_PATH).is_none());
}

#[test]
fn multiple_loaders_stay_distinct_and_missing_disk_is_reported() {
    let mut session = session("multi-disk");
    let request = prepare_core(&mut session);
    let payload = sbm_boot::decode_handoff(&request.handoff).unwrap();
    let arch = payload
        .discovered_targets
        .iter()
        .find(|target| target.display_name == "Arch Linux")
        .unwrap();
    let systemd = payload
        .discovered_targets
        .iter()
        .find(|target| target.display_name == "systemd-boot")
        .unwrap();
    assert_ne!(arch.id, systemd.id);
    assert_eq!(arch.partition_guid, systemd.partition_guid);
    core::simulated_core_reports(&session.machine, &request, Some(&arch.id)).unwrap();
    // Unplug the disk containing both loaders between selecting in Core and rebooting.
    session.machine.esps.retain(|esp| {
        sbm_boot::Volume::partition_guid(&esp.volume) != arch.partition_guid.as_deref().unwrap()
    });
    session.after_core(CoreExit::Reboot);
    assert_interactive_error(&mut session, "present");
}

#[test]
fn read_only_esp_blocks_the_recovery_handoff_and_unconsumed_requests() {
    use sbm_preview::machine::{HANDOFF_PATH, REQUEST_PATH};

    let mut session = session("dual-boot");
    session
        .machine
        .boot_esp()
        .unwrap()
        .volume
        .set_read_only(true);
    assert_eq!(session.key(Key::Char('c'), W, H), Effect::Redraw);
    assert_interactive_error(&mut session, "could not be saved");
    assert!(session.machine.vars.is_empty());
    assert!(session.machine.read_boot(HANDOFF_PATH).is_none());

    session
        .machine
        .boot_esp()
        .unwrap()
        .volume
        .set_read_only(false);
    let request = prepare_core(&mut session);
    let target = session.machine.target_for_preset("ubuntu").unwrap();
    core::simulated_core_reports(&session.machine, &request, Some(&target)).unwrap();
    session
        .machine
        .boot_esp()
        .unwrap()
        .volume
        .set_read_only(true);
    session.after_core(CoreExit::Reboot);
    assert_interactive_error(&mut session, "removed");
    assert!(session.machine.read_boot(REQUEST_PATH).is_some());
}

#[test]
fn unknown_policy_one_shot_suppresses_the_default() {
    let mut scenario = Scenario::load(&scenario::resolve("dual-boot").unwrap()).unwrap();
    scenario.policy.timeout_seconds = Some(0);
    scenario.policy.one_shot_id = Some("efi:missing:000000000000".into());
    let mut session = Session::power_on(sbm_preview::machine::Machine::new(scenario), true);
    assert_interactive_error(&mut session, "present");
}

#[test]
fn local_key_overrides_a_valid_core_request() {
    let mut session = session("dual-boot");
    let request = prepare_core(&mut session);
    let target = session.machine.target_for_preset("ubuntu").unwrap();
    core::simulated_core_reports(&session.machine, &request, Some(&target)).unwrap();
    session.machine.reset();
    let picker = sbm_menu::flow::Picker::start(&mut session.machine, &mut || true);
    assert_eq!(picker.resolved.reason, "local-override");
    assert!(picker.initial_state(true).countdown_ms.is_none());
    assert!(
        session
            .machine
            .read_boot(sbm_preview::machine::REQUEST_PATH)
            .is_none()
    );
}

#[test]
fn experimental_layouts_and_progressive_discovery_work() {
    use sbm_preview::{DiscoveryMode, DiscoverySchedule, LayoutMode};

    let mut classic = session("dual-boot");
    classic.set_layout(LayoutMode::Classic);
    let fb_classic = render(&classic, W, H);

    let mut rendered = Vec::new();
    for mode in LayoutMode::EXPERIMENTS {
        let mut s = session("dual-boot");
        s.set_layout(mode);
        let fb = render(&s, W, H);
        assert_ne!(fb.px, fb_classic.px, "{mode:?} matched classic");
        rendered.push(fb.px);
    }
    assert_ne!(rendered[0], rendered[1], "Bento and Schematic must differ");
    assert_ne!(
        rendered[1], rendered[2],
        "Schematic and Storage must differ"
    );
    assert_ne!(rendered[0], rendered[2], "Bento and Storage must differ");

    // Progressive discovery: t=0 keeps all boot targets selectable immediately,
    // and crossing probe thresholds triggers discrete redraws.
    let mut prog = session("multi-disk");
    prog.set_layout(LayoutMode::Bento);
    prog.set_discovery(DiscoverySchedule::new(DiscoveryMode::Progressive, None));
    assert!(prog.cards() > 0);
    assert!(prog.animating());
    let spec_0 = prog.system_spec(W, H);
    assert!(matches!(spec_0.cpu, sbm_gfx::hardware::ProbeState::Pending));
    assert!(matches!(
        spec_0.storage,
        sbm_gfx::hardware::ProbeState::Ready(_)
    ));

    let fb_t0 = render(&prog, W, H);
    let _ = prog.tick(1000, W, H);
    let spec_done = prog.system_spec(W, H);
    assert!(matches!(
        spec_done.cpu,
        sbm_gfx::hardware::ProbeState::Ready(_)
    ));
    let fb_done = render(&prog, W, H);
    assert_ne!(fb_t0.px, fb_done.px);

    // Sparse mode marks optional SMBIOS unavailable while keeping boot targets.
    let mut sparse = session("dual-boot");
    sparse.set_layout(LayoutMode::Schematic);
    sparse.set_discovery(DiscoverySchedule::new(DiscoveryMode::Sparse, None));
    let spec_sparse = sparse.system_spec(W, H);
    assert!(matches!(
        spec_sparse.board,
        sbm_gfx::hardware::ProbeState::Unavailable
    ));
    assert!(matches!(
        spec_sparse.gpu,
        sbm_gfx::hardware::ProbeState::Unavailable
    ));
    assert!(matches!(
        spec_sparse.network,
        sbm_gfx::hardware::ProbeState::Unavailable
    ));
    assert!(sparse.cards() >= 3);
}

#[test]
fn lba_stress_preserves_exact_lba_order_gaps_and_multi_target_polygons() {
    use sbm_gfx::hardware::{ProbeState, RegionKind};
    use sbm_preview::LayoutMode;

    let mut s = session("lba-stress");
    s.set_layout(LayoutMode::Storage);
    let spec = s.system_spec(W, H);
    let ProbeState::Ready(disks) = &spec.storage else {
        panic!("expected storage ready");
    };
    assert_eq!(disks.len(), 2, "lba-stress should have 2 disks");

    // Verify strict ascending LBA ordering and explicit unallocated gaps on both disks.
    for disk in disks {
        assert!(!disk.regions.is_empty());
        let mut prev_end = 0u64;
        let mut gap_count = 0usize;
        for region in &disk.regions {
            assert!(
                region.start_lba >= prev_end,
                "disk {} LBA out of order: {} < {}",
                disk.device_name,
                region.start_lba,
                prev_end
            );
            assert!(region.end_lba > region.start_lba);
            if matches!(region.kind, RegionKind::Unallocated) {
                gap_count += 1;
            }
            prev_end = region.end_lba;
        }
        assert!(
            gap_count >= 3,
            "disk {} expected >=3 unallocated gaps, got {}",
            disk.device_name,
            gap_count
        );
    }

    // Verify Arch Linux partition owns both Arch Linux + systemd-boot targets
    // while ESP owns SBM Core Slot A, never conflating ESP and OS partitions.
    let mut found_multi_target_os = false;
    let mut found_esp_with_loaders = false;
    for region in &disks[0].regions {
        if let RegionKind::Partition(p) = &region.kind {
            if p.label == "arch" {
                assert_eq!(p.boot_targets.len(), 2, "Arch should own 2 boot targets");
                for t in &p.boot_targets {
                    assert_eq!(t.esp_part, Some(1));
                    assert_eq!(t.os_part, Some(p.index));
                }
                found_multi_target_os = true;
            }
            if p.index == 1 {
                assert!(p.loader_count >= 4);
                assert_eq!(p.boot_targets.len(), 1, "ESP should own SBM Core Slot A");
                found_esp_with_loaders = true;
            }
        }
    }
    assert!(found_multi_target_os, "missing multi-target Arch partition");
    assert!(found_esp_with_loaders, "missing ESP loader metadata");

    // Verify all 3 layouts render cleanly across 800x600, 1280x800, 1920x1080.
    for mode in LayoutMode::EXPERIMENTS {
        s.set_layout(mode);
        for (w, h) in [(800, 600), (1280, 800), (1920, 1080)] {
            let fb = render(&s, w, h);
            assert!(fb.px.iter().any(|&p| p != fb.px[0]));
        }
    }
}

#[test]
fn autoboot_abort_x_buttons_flashing_and_multi_drive_scroll_work() {
    use sbm_gfx::hardware::{ExperimentalHit, layout_experimental_scrolled};
    use sbm_menu::Pointer;
    use sbm_preview::LayoutMode;

    // 1. Both [X] abort buttons (header + flashing partition) appear during countdown,
    //    the partition flashes visibly between countdown ticks, and clicking EITHER [X]
    //    aborts autoboot without booting.
    for mode in LayoutMode::EXPERIMENTS {
        for click_which in [0usize, 1usize] {
            let mut s = session("lba-stress");
            s.set_layout(mode);
            assert!(
                s.state.countdown_ms.is_some(),
                "expected active autoboot countdown"
            );

            // Check flashing: advance by 300 ms and verify pixels change smoothly.
            let fb_a = render(&s, W, H);
            let _ = s.tick(300, W, H);
            let fb_b = render(&s, W, H);
            assert_ne!(
                fb_a.px, fb_b.px,
                "{mode:?} countdown partition should flash"
            );

            let spec = s.system_spec(W, H);
            let map = layout_experimental_scrolled(mode, W, H, &spec, &s.state, s.scroll_y);
            assert_eq!(
                map.abort_buttons.len(),
                2,
                "{mode:?} should expose both top-right and partition [X] abort buttons"
            );
            let header_x = map
                .abort_buttons
                .iter()
                .find(|r| r.y < 50)
                .copied()
                .unwrap();
            let part_x = map
                .abort_buttons
                .iter()
                .find(|r| r.y >= 50)
                .copied()
                .unwrap();
            let target_btn = if click_which == 0 { header_x } else { part_x };
            let cx = target_btn.x + target_btn.w / 2;
            let cy = target_btn.y + target_btn.h / 2;
            assert_eq!(
                map.hit_test_experimental(cx, cy),
                ExperimentalHit::AbortAutoboot
            );

            let eff = s.pointer(
                Pointer {
                    x: cx,
                    y: cy,
                    moved: true,
                    clicked: true,
                },
                W,
                H,
            );
            assert_eq!(eff, Effect::Redraw);
            assert_eq!(
                s.state.countdown_ms, None,
                "clicking [X] must abort autoboot"
            );
            assert_eq!(s.screen_name(), "menu", "clicking [X] must not launch OS");

            // Once aborted, both [X] buttons disappear.
            let spec2 = s.system_spec(W, H);
            let map2 = layout_experimental_scrolled(mode, W, H, &spec2, &s.state, s.scroll_y);
            assert!(map2.abort_buttons.is_empty());
        }
    }

    // 2. Multi-drive scrolling: 6 drives in `multi-disk` exceed viewport height in Bento/Storage,
    //    expose a scrollbar, respond to scroll_by & scrollbar clicks, and auto-scroll on keyboard nav.
    for mode in [
        LayoutMode::Bento,
        LayoutMode::Schematic,
        LayoutMode::Storage,
    ] {
        let mut s = session("multi-disk");
        s.set_layout(mode);
        let spec = s.system_spec(W, H);
        let map0 = layout_experimental_scrolled(mode, W, H, &spec, &s.state, s.scroll_y);
        assert!(
            map0.max_scroll_y > 0,
            "{mode:?} multi-disk (6 drives) should be scrollable"
        );
        assert!(map0.scroll_up.is_some());
        assert!(map0.scroll_down.is_some());
        assert!(map0.scroll_track.is_some());
        assert!(map0.scroll_thumb.is_some());

        let fb_top = render(&s, W, H);
        assert_eq!(s.scroll_by(220, W, H), Effect::Redraw);
        assert!(s.scroll_y > 0);
        let fb_mid = render(&s, W, H);
        assert_ne!(fb_top.px, fb_mid.px, "{mode:?} scrolling must move drives");

        // Clicking scrollbar down button scrolls further (or stays clamped at bottom).
        let dn = map0.scroll_down.unwrap();
        let _ = s.pointer(
            Pointer {
                x: dn.x + dn.w / 2,
                y: dn.y + dn.h / 2,
                moved: true,
                clicked: true,
            },
            W,
            H,
        );
        assert!(s.scroll_y >= 220.min(map0.max_scroll_y));

        // Reset scroll to top and step through cards to bottom drive: auto-scroll brings it in view.
        let _ = s.scroll_by(-10_000, W, H);
        assert_eq!(s.scroll_y, 0);
        for _ in 0..5 {
            let _ = s.key(Key::Right, W, H);
        }
        assert!(
            s.scroll_y > 0,
            "{mode:?} navigating to bottom drive should auto-scroll into view"
        );
    }
}

#[test]
fn clicking_partition_selects_safely_without_arming_unintended_autoboot() {
    use sbm_gfx::hardware::layout_experimental_scrolled;
    use sbm_menu::Pointer;
    use sbm_preview::LayoutMode;

    let mut s = session("lba-stress");
    s.set_layout(LayoutMode::Bento);
    assert!(
        s.state.countdown_ms.is_some(),
        "policy startup countdown should be active on boot"
    );

    // Locate Fedora's polygon zone (a different boot target than Windows).
    let fedora_card = s
        .visible()
        .iter()
        .position(|&idx| s.entries()[idx].item.target.display_name.contains("Fedora"))
        .expect("lba-stress should have Fedora");
    let spec = s.system_spec(W, H);
    let map = layout_experimental_scrolled(LayoutMode::Bento, W, H, &spec, &s.state, s.scroll_y);
    let fedora_seg = map
        .zones
        .iter()
        .find(|z| z.card_index == fedora_card)
        .expect("Fedora hit zone")
        .segments[0];
    let fx = fedora_seg.x + fedora_seg.w / 2;
    let fy = fedora_seg.y + fedora_seg.h / 2;

    // 1. Hovering Fedora must NOT hijack selection or touch the active policy countdown.
    let _ = s.pointer(
        Pointer {
            x: fx,
            y: fy,
            moved: true,
            clicked: false,
        },
        W,
        H,
    );
    assert_ne!(s.state.selected, fedora_card);
    assert!(s.state.countdown_ms.is_some());

    // 2. First click on unselected Fedora selects it, cancels policy countdown,
    //    NEVER boots on first click, and NEVER arms a new automatic boot timer.
    let _ = s.pointer(
        Pointer {
            x: fx,
            y: fy,
            moved: false,
            clicked: true,
        },
        W,
        H,
    );
    assert_eq!(s.screen_name(), "menu");
    assert_eq!(s.state.selected, fedora_card);
    assert_eq!(
        s.state.countdown_ms, None,
        "selecting a partition must cancel policy countdown, never arm a new timer"
    );
    // Tick 30 full seconds to prove zero unintended automatic boot behavior.
    run_script(&mut s, &parse_script("wait:30000").unwrap(), W, H);
    assert_eq!(s.screen_name(), "menu");
    assert!(s.sim.is_none());

    // 3. Clicking a DIFFERENT partition (Windows) switches selection without booting
    //    or starting any automatic boot timer.
    let win_card = 0usize;
    let win_seg = map
        .zones
        .iter()
        .find(|z| z.card_index == win_card)
        .expect("Windows hit zone")
        .segments[0];
    let wx = win_seg.x + win_seg.w / 2;
    let wy = win_seg.y + win_seg.h / 2;
    let _ = s.pointer(
        Pointer {
            x: wx,
            y: wy,
            moved: true,
            clicked: true,
        },
        W,
        H,
    );
    assert_eq!(s.screen_name(), "menu");
    assert_eq!(s.state.selected, win_card);
    assert_eq!(s.state.countdown_ms, None);
    run_script(&mut s, &parse_script("wait:30000").unwrap(), W, H);
    assert_eq!(s.screen_name(), "menu");

    // 4. Second click on the already-selected partition boots it immediately.
    let _ = s.pointer(
        Pointer {
            x: wx,
            y: wy,
            moved: false,
            clicked: true,
        },
        W,
        H,
    );
    match &s.sim {
        Some(SimScreen::Booted { name, .. }) => assert_eq!(name, "Windows"),
        other => panic!("second click on selected partition should boot Windows, got {other:?}"),
    }

    // 5. Keyboard matches production UEFI semantics: Escape cancels policy countdown
    //    without booting; Enter boots the selected target immediately (no surprise timer).
    let mut k = session("lba-stress");
    k.set_layout(LayoutMode::Storage);
    assert!(k.state.countdown_ms.is_some());
    let _ = k.key(Key::Escape, W, H);
    assert_eq!(k.screen_name(), "menu");
    assert_eq!(k.state.countdown_ms, None);
    run_script(&mut k, &parse_script("wait:30000").unwrap(), W, H);
    assert_eq!(k.screen_name(), "menu");

    let _ = k.key(Key::Right, W, H);
    assert_eq!(k.state.countdown_ms, None);
    let _ = k.key(Key::Activate, W, H);
    assert_ne!(
        k.screen_name(),
        "menu",
        "Enter on selected target must boot immediately"
    );
}

#[test]
fn unified_typography_high_contrast_and_scaling_work_across_layouts() {
    use sbm_preview::{FontMode, LayoutMode, UiScale};

    for layout in LayoutMode::ALL {
        let mut s = session("dual-boot");
        s.set_layout(layout);

        // Compare Atkinson Next+Mono vs Bitmap8x8.
        s.set_font_mode(FontMode::AtkinsonNextMono);
        let fb_next_mono = render(&s, W, H);
        assert!(
            s.renderer_stats().entries > 0,
            "{layout:?} should populate glyph cache"
        );
        s.set_font_mode(FontMode::Bitmap8x8);
        let fb_bitmap = render(&s, W, H);
        assert_ne!(
            fb_next_mono.px, fb_bitmap.px,
            "{layout:?} typography modes must differ"
        );

        // Compare normal vs high-contrast themes.
        s.set_font_mode(FontMode::AtkinsonNextMono);
        s.set_high_contrast(true);
        let fb_hc_dark = render(&s, W, H);
        assert_ne!(
            fb_next_mono.px, fb_hc_dark.px,
            "{layout:?} high-contrast dark must differ from normal dark"
        );
        s.state.dark = false;
        let fb_hc_light = render(&s, W, H);
        assert_ne!(
            fb_hc_dark.px, fb_hc_light.px,
            "{layout:?} high-contrast light must differ from dark"
        );

        // Adjustable scaling (75%, 100%, 150%, 200%) renders cleanly without panicking.
        s.set_high_contrast(false);
        s.state.dark = true;
        s.set_scale(UiScale::new(150));
        let fb_scaled = render(&s, W, H);
        assert_ne!(
            fb_next_mono.px, fb_scaled.px,
            "{layout:?} 150% scale must differ from 100%"
        );
        for pct in [75u16, 200] {
            s.set_scale(UiScale::new(pct));
            let fb = render(&s, W, H);
            assert!(fb.px.iter().any(|&p| p != fb.px[0]));
        }
    }
}
