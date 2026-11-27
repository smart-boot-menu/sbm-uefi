use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use sbm_preview::scenario::{self, Scenario};
use sbm_preview::{
    DISPLAY_PRESETS, DiscoveryMode, DiscoverySchedule, FontMode, LayoutMode, Session,
    StorageDetail, UiScale, core, load_session, parse_resolution, parse_script, render, run_script,
    write_image,
};

#[cfg(feature = "window")]
mod window;

const USAGE: &str = "\
sbm-preview: native Smart Boot Menu UEFI preview against a simulated computer

  sbm-preview [--scenario NAME|PATH] [--theme dark|light] [--display window|WxH]
              [--layout classic|bento|schematic|storage] [--detail compact|detailed]
              [--font next-mono|next|bitmap] [--contrast normal|high] [--scale 75..200]
              [--discovery instant|progressive|slow|sparse] [--delay-ms N]
  sbm-preview --headless [--scenario NAME] [--screen menu|splash|diagnostics|error|all]
              [--layout classic|bento|schematic|storage|experiments|all]
              [--detail compact|detailed|both]
              [--font next-mono|next|bitmap|all] [--contrast normal|high|both] [--scale N]
              [--discovery instant|progressive|slow|sparse] [--delay-ms N] [--at-ms N]
              [--theme dark|light|both] [--keys Right,Enter,wait:1000]
              [--size 1280x800 | --sizes 800x600,1280x800,1920x1080|all] [--out FILE.png|DIR]
  sbm-preview --list
  sbm-preview --materialize DIR [--scenario NAME]

Options
  --scenario      scenario name from sim/scenarios or a TOML path (default dual-boot)
  --all-scenarios headless: render every scenario
  --layout        classic, bento (A), schematic (B), storage (C), experiments, or all
  --detail        compact (default: OS cards, max 4/drive), detailed (LBA polygons), or both
  --font          next-mono (default), next, bitmap, or all (headless comparison)
  --contrast      normal (default), high, or both (headless comparison)
  --scale         UI typography scale percentage 75..=200 (default 100)
  --discovery     instant (default), progressive, slow, or sparse (missing SMBIOS)
  --delay-ms      total progressive discovery duration in ms
  --at-ms         headless: capture progressive discovery at N ms after boot
  --theme         dark, light, or both (headless comparison)
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
    pub layouts: Vec<LayoutMode>,
    pub details: Vec<StorageDetail>,
    pub overflow_drive: Option<usize>,
    pub fonts: Vec<FontMode>,
    pub high_contrast: bool,
    pub both_contrasts: bool,
    pub scale: UiScale,
    pub discovery: DiscoverySchedule,
    pub at_ms: Option<u32>,
    pub scroll: u32,
    pub dark: bool,
    pub both_themes: bool,
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

fn parse_fonts(text: &str) -> Result<Vec<FontMode>, String> {
    let lower = text.trim().to_ascii_lowercase();
    if lower == "all" {
        return Ok(FontMode::ALL.to_vec());
    }
    let mut out = Vec::new();
    for tok in lower.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let mode = FontMode::parse(tok)
            .ok_or_else(|| format!("unknown font {tok:?} (next-mono|next|bitmap|all)"))?;
        out.push(mode);
    }
    if out.is_empty() {
        return Err("--font requires at least one font mode".into());
    }
    Ok(out)
}

fn parse_layouts(text: &str) -> Result<Vec<LayoutMode>, String> {
    let lower = text.trim().to_ascii_lowercase();
    if lower == "all" {
        return Ok(LayoutMode::ALL.to_vec());
    }
    if lower == "experiments" || lower == "exp" {
        return Ok(LayoutMode::EXPERIMENTS.to_vec());
    }
    let mut out = Vec::new();
    for tok in lower.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let mode = LayoutMode::parse(tok).ok_or_else(|| {
            format!("unknown layout {tok:?} (classic|bento|schematic|storage|experiments|all)")
        })?;
        out.push(mode);
    }
    if out.is_empty() {
        return Err("--layout requires at least one layout name".into());
    }
    Ok(out)
}

fn parse_details(text: &str) -> Result<Vec<StorageDetail>, String> {
    let lower = text.trim().to_ascii_lowercase();
    if lower == "both" || lower == "all" {
        return Ok(StorageDetail::ALL.to_vec());
    }
    let mut out = Vec::new();
    for tok in lower.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let detail = StorageDetail::parse(tok)
            .ok_or_else(|| format!("unknown detail {tok:?} (compact|detailed|both)"))?;
        out.push(detail);
    }
    if out.is_empty() {
        return Err("--detail requires at least one mode (compact|detailed|both)".into());
    }
    Ok(out)
}

fn parse(args: Vec<String>) -> Result<Option<Options>, String> {
    let mut opts = Options {
        scenario: "dual-boot".into(),
        all_scenarios: false,
        layouts: vec![LayoutMode::Classic],
        details: vec![StorageDetail::Compact],
        overflow_drive: None,
        fonts: vec![FontMode::AtkinsonNextMono],
        high_contrast: false,
        both_contrasts: false,
        scale: UiScale::default(),
        discovery: DiscoverySchedule::default(),
        at_ms: None,
        scroll: 0,
        dark: true,
        both_themes: false,
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
            "--layout" => opts.layouts = parse_layouts(&value()?)?,
            "--detail" => opts.details = parse_details(&value()?)?,
            "--overflow-drive" => {
                opts.overflow_drive =
                    Some(value()?.parse().map_err(|_| "invalid --overflow-drive")?);
            }
            "--font" => opts.fonts = parse_fonts(&value()?)?,
            "--contrast" => match value()?.to_ascii_lowercase().as_str() {
                "normal" => {
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
                let pct: u16 = value()?.parse().map_err(|_| "invalid --scale (75..=200)")?;
                opts.scale = UiScale::new(pct);
            }
            "--discovery" => {
                let raw = value()?;
                opts.discovery.mode = DiscoveryMode::parse(&raw).ok_or_else(|| {
                    format!("unknown discovery mode {raw:?} (instant|progressive|slow|sparse)")
                })?;
            }
            "--delay-ms" => {
                opts.discovery.delay_ms = Some(value()?.parse().map_err(|_| "invalid --delay-ms")?);
            }
            "--at-ms" => {
                opts.at_ms = Some(value()?.parse().map_err(|_| "invalid --at-ms")?);
            }
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
            "--scroll" => opts.scroll = value()?.parse().map_err(|_| "invalid --scroll")?,
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
    let multi_layout = opts.layouts.len() > 1 || opts.layouts[0] != LayoutMode::Classic;
    let multi_detail = opts.details.len() > 1 || opts.details[0] != StorageDetail::Compact;
    let multi_font = opts.fonts.len() > 1 || opts.fonts[0] != FontMode::AtkinsonNextMono;
    let multi_contrast = contrasts.len() > 1 || contrasts[0];
    let single = names.len() == 1
        && screens.len() == 1
        && themes.len() == 1
        && contrasts.len() == 1
        && opts.layouts.len() == 1
        && opts.details.len() == 1
        && opts.fonts.len() == 1
        && !multi_size;
    for &(width, height) in &sizes {
        for &layout in &opts.layouts {
            let layout_details: &[StorageDetail] = if layout == LayoutMode::Classic {
                &[StorageDetail::Compact]
            } else {
                &opts.details
            };
            for &detail in layout_details {
                for &font in &opts.fonts {
                    for name in &names {
                        for screen in &screens {
                            for &dark in &themes {
                                for &high_contrast in &contrasts {
                                    let started = Instant::now();
                                    let Some(mut session) = scripted(
                                        name,
                                        dark,
                                        high_contrast,
                                        layout,
                                        detail,
                                        opts.overflow_drive,
                                        font,
                                        opts.scale,
                                        opts.discovery,
                                        opts.at_ms,
                                        screen,
                                        &opts.keys,
                                        width,
                                        height,
                                    )?
                                    else {
                                        println!(
                                            "skip {name} {screen}: this scenario has no such state"
                                        );
                                        continue;
                                    };
                                    if opts.scroll > 0 {
                                        let _ =
                                            session.scroll_by(opts.scroll as i32, width, height);
                                    }
                                    let mut fb = sbm_gfx::Framebuffer::new(width, height);
                                    if *screen == "splash" {
                                        session.render_splash(&mut fb);
                                    } else {
                                        fb = render(&session, width, height);
                                    }
                                    let theme = if dark { "dark" } else { "light" };
                                    let mut parts = vec![format!("uefi-{name}")];
                                    if multi_layout {
                                        parts.push(layout.id().into());
                                    }
                                    if layout != LayoutMode::Classic && multi_detail {
                                        parts.push(detail.id().into());
                                    }
                                    if multi_font {
                                        parts.push(font.id().into());
                                    }
                                    parts.push((*screen).into());
                                    parts.push(theme.into());
                                    if multi_contrast {
                                        parts.push(if high_contrast { "hc" } else { "std" }.into());
                                    }
                                    if opts.scale != UiScale::default() {
                                        parts.push(format!("{}pct", opts.scale.percent()));
                                    }
                                    if multi_size {
                                        parts.push(format!("{width}x{height}"));
                                    }
                                    let filename = format!("{}.png", parts.join("-"));
                                    let path = match (&opts.out, single) {
                                        (Some(path), true) if path.extension().is_some() => {
                                            path.clone()
                                        }
                                        (Some(dir), _) => dir.join(&filename),
                                        (None, _) => out_dir().join(&filename),
                                    };
                                    let bytes = write_image(&path, &fb)?;
                                    let stats = session.renderer_stats();
                                    println!(
                                        "SIMULATED preview {} ({}x{}, layout {}, detail {}, font {}, {}%, {}, discovery {}, {} glyphs/{} B, {} bytes, {} screen, {:.1} ms)",
                                        path.display(),
                                        width,
                                        height,
                                        layout.id(),
                                        detail.id(),
                                        font.id(),
                                        opts.scale.percent(),
                                        if high_contrast { "hc" } else { "std" },
                                        opts.discovery.mode.id(),
                                        stats.entries,
                                        stats.bytes,
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
                    }
                }
            }
        }
    }
    Ok(())
}

/// Build the session for a named screen. `error` searches for a card whose
/// start fails in this scenario.
#[allow(clippy::too_many_arguments)]
fn scripted(
    name: &str,
    dark: bool,
    high_contrast: bool,
    layout: LayoutMode,
    detail: StorageDetail,
    overflow_drive: Option<usize>,
    font: FontMode,
    scale: UiScale,
    discovery: DiscoverySchedule,
    at_ms: Option<u32>,
    screen: &str,
    keys: &str,
    width: u32,
    height: u32,
) -> Result<Option<Session>, String> {
    let steps = parse_script(keys)?;
    let mut session = load_session(name, dark)?;
    session.set_layout(layout);
    session.set_detail(detail);
    session.set_font_mode(font);
    session.set_high_contrast(high_contrast);
    session.set_scale(scale);
    session.set_discovery(discovery);
    session.set_overflow_drive(overflow_drive);
    if let Some(ms) = at_ms {
        session.elapsed_ms = ms;
    }
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
                probe.set_layout(layout);
                probe.set_detail(detail);
                probe.set_font_mode(font);
                probe.set_high_contrast(high_contrast);
                probe.set_scale(scale);
                probe.set_discovery(discovery);
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
