//! Native development preview of the Smart Boot Menu UEFI menu.
//!
//! Runs the production discovery, policy, menu state machine, and renderer
//! against a simulated computer described by a TOML scenario. This crate is
//! development tooling: it is not a dependency of `sbm-app` and is never part
//! of an EFI binary.

pub mod core;
pub mod hardware;
pub mod logos;
pub mod machine;
pub mod overlay;
pub mod scenario;
pub mod session;

use std::fs;
use std::path::Path;

use sbm_gfx::Framebuffer;
pub use sbm_gfx::hardware::{ExperimentalOptions, LayoutMode, StorageDetail};
pub use sbm_gfx::{FontMode, UiScale};
use sbm_menu::Key;

pub use hardware::{DiscoveryMode, DiscoverySchedule};
pub use machine::Machine;
pub use scenario::Scenario;
pub use session::{Effect, Session};

/// The sbm-uefi checkout this binary was built from.
pub fn workspace_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .to_path_buf()
}

pub fn load_session(spec: &str, dark: bool) -> Result<Session, String> {
    let path = scenario::resolve(spec)?;
    let scenario = Scenario::load(&path)?;
    Ok(Session::power_on(Machine::new(scenario), dark))
}

/// One step of a scripted interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptStep {
    Key(Key),
    Wait(u32),
}

/// Parse `Right,Right,Enter,wait:1000,d`.
pub fn parse_script(text: &str) -> Result<Vec<ScriptStep>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| {
            let lower = token.to_ascii_lowercase();
            if let Some(ms) = lower.strip_prefix("wait:") {
                return ms
                    .parse()
                    .map(ScriptStep::Wait)
                    .map_err(|_| format!("invalid wait {token:?}"));
            }
            let key = match lower.as_str() {
                "up" => Key::Up,
                "down" => Key::Down,
                "left" => Key::Left,
                "right" => Key::Right,
                "enter" | "return" => Key::Activate,
                "tab" => Key::Tab,
                "esc" | "escape" => Key::Escape,
                "space" => Key::Char(' '),
                _ if token.chars().count() == 1 => Key::Char(token.chars().next().unwrap_or(' ')),
                _ => return Err(format!("unknown key {token:?}")),
            };
            Ok(ScriptStep::Key(key))
        })
        .collect()
}

/// Run a script headlessly. Requests to start SBM Core are answered by a
/// SIMULATED Core that reports success and asks to reboot, so scripts stay
/// deterministic without a GUI.
pub fn run_script(session: &mut Session, steps: &[ScriptStep], width: u32, height: u32) {
    let handle = |session: &mut Session, effect: Effect| {
        let mut effect = effect;
        while let Effect::LaunchCore(request) = effect {
            if let Err(message) = core::simulated_core_reports(&session.machine, &request, None) {
                session.core_unavailable(&message);
                return;
            }
            effect = session.after_core(session::CoreExit::Reboot);
        }
    };
    let pending = session.take_pending();
    handle(session, pending);
    for step in steps {
        let effect = match step {
            ScriptStep::Key(key) => session.key(*key, width, height),
            ScriptStep::Wait(ms) => {
                let mut left = *ms;
                let mut effect = Effect::None;
                while left > 0 && matches!(effect, Effect::None | Effect::Redraw) {
                    let dt = left.min(100);
                    effect = session.tick(dt, width, height);
                    left -= dt;
                }
                effect
            }
        };
        handle(session, effect);
    }
}

pub fn render(session: &Session, width: u32, height: u32) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height);
    session.render(&mut fb);
    fb
}

pub fn encode_png(fb: &Framebuffer) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity(fb.px.len() * 3);
    for &px in &fb.px {
        rgb.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8]);
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, fb.width, fb.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
        writer
            .write_image_data(&rgb)
            .map_err(|err| err.to_string())?;
    }
    Ok(out)
}

/// Write PNG, or binary PPM when the extension is `.ppm`.
pub fn write_image(path: &Path, fb: &Framebuffer) -> Result<usize, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
    }
    let bytes = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ppm"))
    {
        fb.to_ppm()
    } else {
        encode_png(fb)?
    };
    fs::write(path, &bytes).map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(bytes.len())
}

pub fn parse_resolution(text: &str) -> Result<(u32, u32), String> {
    let (w, h) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, got {text:?}"))?;
    let w: u32 = w
        .trim()
        .parse()
        .map_err(|_| format!("invalid width in {text:?}"))?;
    let h: u32 = h
        .trim()
        .parse()
        .map_err(|_| format!("invalid height in {text:?}"))?;
    if !(320..=7680).contains(&w) || !(200..=4320).contains(&h) {
        return Err(format!("resolution {w}x{h} is outside 320x200..=7680x4320"));
    }
    Ok((w, h))
}

pub const DISPLAY_PRESETS: &[(u32, u32)] = &[
    (800, 600),
    (1024, 768),
    (1280, 800),
    (1366, 768),
    (1920, 1080),
    (2560, 1600),
];
