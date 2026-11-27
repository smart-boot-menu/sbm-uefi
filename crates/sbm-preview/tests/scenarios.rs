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
