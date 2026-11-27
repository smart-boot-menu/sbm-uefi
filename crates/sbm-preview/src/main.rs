use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use sbm_preview::scenario::{self, Scenario};
use sbm_preview::{
    Session, core, load_session, parse_resolution, parse_script, render, run_script, write_image,
};

#[cfg(feature = "window")]
mod window;

const USAGE: &str = "\
sbm-preview: native Smart Boot Menu UEFI preview against a simulated computer

  sbm-preview [--scenario NAME|PATH] [--theme dark|light] [--display window|WxH]
  sbm-preview --headless [--scenario NAME] [--screen menu|splash|diagnostics|error|all]
              [--keys Right,Enter,wait:1000] [--size 1280x800] [--out FILE.png|DIR]
  sbm-preview --list
  sbm-preview --materialize DIR [--scenario NAME]

Options
  --scenario      scenario name from sim/scenarios or a TOML path (default dual-boot)
  --all-scenarios headless: render every scenario
  --display       window follows the window size; WxH renders a fixed virtual display
  --size          window or headless size (default 1280x800)
  --core-bin DIR  directory holding sbm-sim and sbm-ui (default ../sbm-core/target/debug)
  --exit-after-ms quit the window after N ms (measurements)

Everything shown is SIMULATED. No disk, NVRAM variable, or boot setting on
this computer is read or changed.";

pub struct Options {
    pub scenario: String,
    pub all_scenarios: bool,
    pub dark: bool,
    pub fixed: Option<(u32, u32)>,
    pub size: (u32, u32),
    pub headless: bool,
    pub screen: String,
    pub keys: String,
    pub out: Option<PathBuf>,
    pub core_bin: PathBuf,
    pub exit_after_ms: Option<u64>,
    pub materialize: Option<PathBuf>,
    pub started: Instant,
}

fn parse(args: Vec<String>) -> Result<Option<Options>, String> {
    let mut opts = Options {
        scenario: "dual-boot".into(),
        all_scenarios: false,
        dark: true,
        fixed: None,
        size: (1280, 800),
        headless: false,
        screen: "menu".into(),
        keys: String::new(),
        out: None,
        core_bin: core::default_core_bin(),
        exit_after_ms: None,
        materialize: None,
        started: Instant::now(),
    };
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{arg} requires a value"));
        match arg.as_str() {
            "--scenario" => opts.scenario = value()?,
            "--all-scenarios" => opts.all_scenarios = true,
            "--theme" => {
                opts.dark = match value()?.as_str() {
                    "dark" => true,
                    "light" => false,
                    other => return Err(format!("unknown theme {other:?} (dark|light)")),
                }
            }
            "--display" => {
                let v = value()?;
                opts.fixed = if v == "window" {
                    None
                } else {
                    Some(parse_resolution(&v)?)
                };
            }
            "--size" | "--resolution" => opts.size = parse_resolution(&value()?)?,
            "--width" => opts.size.0 = value()?.parse().map_err(|_| "invalid --width")?,
            "--height" => opts.size.1 = value()?.parse().map_err(|_| "invalid --height")?,
            "--headless" => opts.headless = true,
            "--screen" => opts.screen = value()?,
            "--keys" => opts.keys = value()?,
            "--out" => opts.out = Some(PathBuf::from(value()?)),
            "--core-bin" => opts.core_bin = PathBuf::from(value()?),
            "--exit-after-ms" => {
                opts.exit_after_ms = Some(value()?.parse().map_err(|_| "invalid --exit-after-ms")?)
            }
            "--list" => {
                for path in scenario::list(&scenario::scenario_dir())? {
                    let parsed = Scenario::load(&path)?;
                    println!("{:<22} {}", parsed.id, parsed.summary);
                }
                return Ok(None);
            }
            "--materialize" => opts.materialize = Some(PathBuf::from(value()?)),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => return Err(format!("unknown option {other}\n\n{USAGE}")),
        }
    }
    if let Some(dir) = &opts.materialize {
        materialize(dir, &opts.scenario)?;
        return Ok(None);
    }
    Ok(Some(opts))
}

fn materialize(dir: &Path, spec: &str) -> Result<(), String> {
    let mut session = load_session(spec, true)?;
    let _ = session.take_pending();
    // Press the production recovery key so Core gets a real handoff.
    let effect = session.key(sbm_menu::Key::Char('c'), 1280, 800);
    let request = match effect {
        sbm_preview::Effect::LaunchCore(request) => Some(request),
        _ => None,
    };
    core::materialize(dir, &session.machine, request.as_deref())?;
    println!(
        "SIMULATED computer {spec} written to {} ({})",
        dir.display(),
        if request.is_some() {
            "with an SBM Core handoff"
        } else {
            "no SBM Core target in this scenario; handoff absent"
        }
    );
    Ok(())
}

fn out_dir() -> PathBuf {
    sbm_preview::workspace_root().join("build/preview")
}

fn headless(opts: &Options) -> Result<(), String> {
    let names: Vec<String> = if opts.all_scenarios {
        scenario::list(&scenario::scenario_dir())?
            .iter()
            .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect()
    } else {
        vec![opts.scenario.clone()]
    };
    let screens: Vec<&str> = if opts.screen == "all" {
        vec!["menu", "diagnostics", "error", "splash"]
    } else {
        vec![opts.screen.as_str()]
    };
    let themes: Vec<bool> = if opts.screen == "all" {
        vec![true, false]
    } else {
        vec![opts.dark]
    };
    let single = names.len() == 1 && screens.len() == 1 && themes.len() == 1;
    let (width, height) = opts.size;
    for name in &names {
        for screen in &screens {
            for dark in &themes {
                let started = Instant::now();
                let Some(session) = scripted(name, *dark, screen, &opts.keys, width, height)?
                else {
                    println!("skip {name} {screen}: this scenario has no such state");
                    continue;
                };
                let mut fb = sbm_gfx::Framebuffer::new(width, height);
                if *screen == "splash" {
                    session.render_splash(&mut fb);
                } else {
                    fb = render(&session, width, height);
                }
                let theme = if *dark { "dark" } else { "light" };
                let path = match (&opts.out, single) {
                    (Some(path), true) if path.extension().is_some() => path.clone(),
                    (Some(dir), _) => dir.join(format!("uefi-{name}-{screen}-{theme}.png")),
                    (None, _) => out_dir().join(format!("uefi-{name}-{screen}-{theme}.png")),
                };
                let bytes = write_image(&path, &fb)?;
                println!(
                    "SIMULATED preview {} ({}x{}, {} bytes, {} screen, {:.1} ms)",
                    path.display(),
                    width,
                    height,
                    bytes,
                    if *screen == "splash" {
                        "splash"
                    } else {
                        session.screen_name()
                    },
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
    }
    Ok(())
}

/// Build the session for a named screen. `error` searches for a card whose
/// start fails in this scenario.
fn scripted(
    name: &str,
    dark: bool,
    screen: &str,
    keys: &str,
    width: u32,
    height: u32,
) -> Result<Option<Session>, String> {
    let steps = parse_script(keys)?;
    let mut session = load_session(name, dark)?;
    match screen {
        "menu" | "splash" | "custom" => {}
        "diagnostics" => {
            run_script(&mut session, &parse_script("d")?, width, height);
        }
        "error" => {
            let cards = session.cards();
            let mut found = None;
            for index in 0..cards.max(1) {
                let mut probe = load_session(name, dark)?;
                let mut script = vec!["right"; index].join(",");
                script.push_str(if cards == 0 { "c" } else { ",enter" });
                run_script(&mut probe, &parse_script(&script)?, width, height);
                if probe.screen_name() == "error" {
                    found = Some(probe);
                    break;
                }
            }
            match found {
                Some(probe) => session = probe,
                None => return Ok(None),
            }
        }
        other => return Err(format!("unknown screen {other:?}")),
    }
    run_script(&mut session, &steps, width, height);
    Ok(Some(session))
}

fn main() -> ExitCode {
    let opts = match parse(std::env::args().skip(1).collect()) {
        Ok(Some(opts)) => opts,
        Ok(None) => return ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let result = if opts.headless || !cfg!(feature = "window") {
        if !opts.headless {
            eprintln!("built without the `window` feature; rendering headless");
        }
        headless(&opts)
    } else {
        run_window(&opts)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "window")]
fn run_window(opts: &Options) -> Result<(), String> {
    window::run(opts)
}

#[cfg(not(feature = "window"))]
fn run_window(_opts: &Options) -> Result<(), String> {
    Ok(())
}
