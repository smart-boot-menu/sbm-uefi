//! Scenario behaviour through the production discovery, policy and menu code.

use sbm_menu::Key;
use sbm_preview::scenario::{self, Scenario};
use sbm_preview::session::{CoreExit, SimScreen};
use sbm_preview::{Effect, Session, core, load_session, parse_script, render, run_script};

const W: u32 = 1280;
const H: u32 = 800;

fn cards(session: &Session) -> Vec<String> {
    session
        .visible
        .iter()
        .map(|&index| {
            let item = &session.entries[index].item;
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
        ("empty-uefi", &["SBM Core Removable"]),
        ("windows-only", &["Windows", "SBM Core Removable"]),
        (
            "linux-only",
            &["Fedora Linux 45", "SBM Core Slot A", "SBM Core Slot B"],
        ),
        (
            "dual-boot",
            &["Windows", "Ubuntu", "SBM Core Slot A", "SBM Core Slot B"],
        ),
        (
            "multi-disk",
            &[
                "Windows",
                "Arch Linux",
                "systemd-boot",
                "Debian",
                "External drive",
                "SBM Core Removable",
            ],
        ),
        ("low-disk-space", &["Windows", "SBM Core Removable"]),
        ("bitlocker", &["Windows", "SBM Core Removable"]),
        (
            "corrupt-boot",
            &[
                "Windows",
                "Ubuntu",
                "SBM Core Slot A (unverified)!",
                "SBM Core Slot B",
            ],
        ),
        (
            "secure-boot-reject",
            &[
                "Windows",
                "Arch Linux",
                "systemd-boot",
                "SBM Core Slot A",
                "SBM Core Slot B",
            ],
        ),
        (
            "existing-sbm",
            &["Debian", "Fedora", "SBM Core Slot A", "SBM Core Slot B"],
        ),
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
    let dir = std::env::temp_dir().join(format!("sbm-preview-core-{}", std::process::id()));
    core::materialize(&dir, &session.machine, Some(&request)).unwrap();
    assert!(dir.join("SIMULATED").is_file());
    assert!(dir.join("esp/EFI/sbm/handoff/current.json").is_file());

    // What sbm-sim writes when the user picks Ubuntu in Core and reboots.
    let ubuntu = session
        .entries
        .iter()
        .find(|entry| entry.item.target.display_name == "Ubuntu")
        .unwrap()
        .item
        .target
        .id
        .clone();
    let command = sbm_protocol::BootCommand::BootTarget {
        target_id: ubuntu,
        one_shot: true,
        extra_cmdline_preset: None,
    };
    let path = dir.join("esp/EFI/sbm/handoff/requested-command.json");
    std::fs::write(&path, serde_json::to_vec(&command).unwrap()).unwrap();
    core::import(&dir, &session.machine);

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
    let dir = std::env::temp_dir().join(format!("sbm-preview-matrix-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("sbm-preview-presets-{}", std::process::id()));
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

#[test]
fn headless_typography_modes_contrast_and_scale_render_distinct_outputs() {
    let bin = env!("CARGO_BIN_EXE_sbm-preview");
    let dir = std::env::temp_dir().join(format!("sbm-preview-typo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let output = std::process::Command::new(bin)
        .args([
            "--headless",
            "--scenario",
            "multi-disk",
            "--screen",
            "menu",
            "--theme",
            "dark",
            "--font",
            "all",
            "--contrast",
            "both",
            "--scale",
            "125",
            "--size",
            "1280x800",
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

    let bitmap = std::fs::read(dir.join("uefi-multi-disk-menu-dark-bitmap-s125.png")).unwrap();
    let next = std::fs::read(dir.join("uefi-multi-disk-menu-dark-next-s125.png")).unwrap();
    let next_mono =
        std::fs::read(dir.join("uefi-multi-disk-menu-dark-next-mono-s125.png")).unwrap();
    let hc = std::fs::read(dir.join("uefi-multi-disk-menu-dark-hc-next-mono-s125.png")).unwrap();

    assert_ne!(bitmap, next);
    assert_ne!(next, next_mono);
    assert_ne!(next_mono, hc);

    let _ = std::fs::remove_dir_all(&dir);
}
