use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use sbm_gfx::{FontMode, UiScale};
use sbm_preview::scenario::{self, Scenario};
use sbm_preview::{
    DISPLAY_PRESETS, Session, core, load_session, parse_resolution, parse_script, render,
    run_script, write_image,
};

#[cfg(feature = "window")]
mod window;

const USAGE: &str = "\
sbm-preview: native Smart Boot Menu UEFI preview against a simulated computer

  sbm-preview [--scenario NAME|PATH] [--theme dark|light] [--display window|WxH]
              [--font bitmap|next|next-mono] [--scale PERCENT] [--contrast normal|high]
  sbm-preview --headless [--scenario NAME] [--screen menu|splash|diagnostics|error|all]
              [--theme dark|light|both] [--font bitmap|next|next-mono|all]
              [--contrast normal|high|both] [--scale PERCENT] [--keys Right,Enter,wait:1000]
              [--size 1280x800 | --sizes 800x600,1280x800,1920x1080,3840x2160|all]
              [--out FILE.png|DIR]
  sbm-preview --list
  sbm-preview --materialize DIR [--scenario NAME]

Options
  --scenario      scenario name from sim/scenarios or a TOML path (default dual-boot)
  --all-scenarios headless: render every scenario
  --theme         dark, light, or both (headless comparison)
  --font          bitmap, next, next-mono (default), or all (headless comparison)
  --contrast      normal (default), high, or both (headless comparison)
  --scale         interface scale percentage 75..=200 (default 100)
  --display       window follows the window size; WxH renders a fixed virtual display
  --size          window or headless size (default 1280x800)
  --sizes         headless: comma-separated WxH list or `all` (display presets)
  --core-bin DIR  directory holding sbm-sim and sbm-ui (default ../sbm-core/target/debug)
  --exit-after-ms quit the window after N ms (measurements)

Everything shown is SIMULATED. No disk, NVRAM variable, or boot setting on
this computer is read or changed.";

pub struct Options {
    pub scenario: String,
    pub all_scenarios: bool,
    pub dark: bool,
    pub both_themes: bool,
    pub font_mode: FontMode,
    pub all_fonts: bool,
    pub high_contrast: bool,
    pub both_contrasts: bool,
    pub scale: UiScale,
    pub fixed: Option<(u32, u32)>,
    pub size: (u32, u32),
    pub sizes: Vec<(u32, u32)>,
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
        both_themes: false,
        font_mode: FontMode::default(),
        all_fonts: false,
        high_contrast: false,
        both_contrasts: false,
        scale: UiScale::default(),
        fixed: None,
        size: (1280, 800),
        sizes: Vec::new(),
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
            "--theme" => match value()?.as_str() {
                "dark" => {
                    opts.dark = true;
                    opts.both_themes = false;
                }
                "light" => {
                    opts.dark = false;
                    opts.both_themes = false;
                }
                "both" => opts.both_themes = true,
                other => return Err(format!("unknown theme {other:?} (dark|light|both)")),
            },
            "--font" => {
                let v = value()?;
                if v.eq_ignore_ascii_case("all") {
                    opts.all_fonts = true;
                } else if let Some(mode) = FontMode::parse(&v) {
                    opts.font_mode = mode;
                    opts.all_fonts = false;
                } else {
                    return Err(format!("unknown font {v:?} (bitmap|next|next-mono|all)"));
                }
            }
            "--contrast" => match value()?.to_ascii_lowercase().as_str() {
                "normal" | "standard" => {
                    opts.high_contrast = false;
                    opts.both_contrasts = false;
                }
                "high" | "hc" => {
                    opts.high_contrast = true;
                    opts.both_contrasts = false;
                }
                "both" => opts.both_contrasts = true,
                other => return Err(format!("unknown contrast {other:?} (normal|high|both)")),
            },
            "--scale" => {
                let pct: u16 = value()?
                    .trim_end_matches('%')
                    .parse()
                    .map_err(|_| "invalid --scale (expected 75..=200)")?;
                opts.scale = UiScale::new(pct);
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
            "--sizes" => {
                let v = value()?;
                if v == "all" {
                    opts.sizes = DISPLAY_PRESETS.to_vec();
                } else {
                    opts.sizes = v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(parse_resolution)
                        .collect::<Result<Vec<_>, _>>()?;
                    if opts.sizes.is_empty() {
                        return Err("--sizes requires at least one WxH resolution".into());
                    }
                }
            }
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
    let themes: Vec<bool> = if opts.both_themes || opts.screen == "all" {
        vec![true, false]
    } else {
        vec![opts.dark]
    };
    let fonts: Vec<FontMode> = if opts.all_fonts {
        FontMode::ALL.to_vec()
    } else {
        vec![opts.font_mode]
    };
    let contrasts: Vec<bool> = if opts.both_contrasts {
        vec![false, true]
    } else {
        vec![opts.high_contrast]
    };
    let sizes: Vec<(u32, u32)> = if opts.sizes.is_empty() {
        vec![opts.size]
    } else {
        opts.sizes.clone()
    };
    let multi_size = sizes.len() > 1;
    let tag_font = fonts.len() > 1 || opts.font_mode != FontMode::default();
    let tag_contrast = contrasts.len() > 1 || opts.high_contrast;
    let tag_scale = opts.scale.percent() != 100;
    let single = names.len() == 1
        && screens.len() == 1
        && themes.len() == 1
        && fonts.len() == 1
        && contrasts.len() == 1
        && !multi_size;

    for &(width, height) in &sizes {
        for name in &names {
            for screen in &screens {
                for &dark in &themes {
                    for &font_mode in &fonts {
                        for &high_contrast in &contrasts {
                            let started = Instant::now();
                            let Some(mut session) =
                                scripted(name, dark, screen, &opts.keys, width, height)?
                            else {
                                println!("skip {name} {screen}: this scenario has no such state");
                                continue;
                            };
                            session.set_font_mode(font_mode);
                            session.set_scale(opts.scale);
                            session.high_contrast = high_contrast;

                            let mut fb = sbm_gfx::Framebuffer::new(width, height);
                            if *screen == "splash" {
                                session.render_splash(&mut fb);
                            } else {
                                fb = render(&session, width, height);
                            }
                            let theme = if dark { "dark" } else { "light" };
                            let mut stem = format!("uefi-{name}-{screen}-{theme}");
                            if tag_contrast && high_contrast {
                                stem.push_str("-hc");
                            }
                            if tag_font {
                                stem.push('-');
                                stem.push_str(font_mode.id());
                            }
                            if tag_scale {
                                stem.push_str(&format!("-s{}", opts.scale.percent()));
                            }
                            if multi_size {
                                stem.push_str(&format!("-{width}x{height}"));
                            }
                            let filename = format!("{stem}.png");
                            let path = match (&opts.out, single) {
                                (Some(path), true) if path.extension().is_some() => path.clone(),
                                (Some(dir), _) => dir.join(&filename),
                                (None, _) => out_dir().join(&filename),
                            };
                            let bytes = write_image(&path, &fb)?;
                            println!(
                                "SIMULATED preview {} ({}x{}, {} bytes, {}, {} screen, {:.1} ms)",
                                path.display(),
                                width,
                                height,
                                bytes,
                                font_mode.label(),
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
