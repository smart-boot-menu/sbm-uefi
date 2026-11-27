//! Interactive macOS/Linux/Windows window via `minifb`.
//!
//! Redraws only when the menu state changes, and polls input at a lower rate
//! when nothing animates, so an idle menu costs close to nothing.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use minifb::{Key as MKey, KeyRepeat, MouseButton, MouseMode, ScaleMode, Window, WindowOptions};
use sbm_gfx::Framebuffer;
use sbm_menu::{Key, Pointer};
use sbm_preview::scenario::{self, Scenario};
use sbm_preview::session::CoreExit;
use sbm_preview::{
    DISPLAY_PRESETS, Effect, FontMode, LayoutMode, Machine, Session, StorageDetail, UiScale, core,
    write_image,
};

use crate::Options;

const ACTIVE_FRAME: Duration = Duration::from_millis(16);
const IDLE_FRAME: Duration = Duration::from_millis(33);

struct View {
    window: Window,
    fixed: Option<(u32, u32)>,
    fb: Framebuffer,
}

impl View {
    fn open(size: (u32, u32), fixed: Option<(u32, u32)>) -> Result<Self, String> {
        let mut window = Window::new(
            "Smart Boot Menu",
            size.0 as usize,
            size.1 as usize,
            WindowOptions {
                resize: true,
                scale_mode: ScaleMode::AspectRatioStretch,
                ..WindowOptions::default()
            },
        )
        .map_err(|err| format!("open window: {err}"))?;
        window.set_target_fps(0);
        window.set_cursor_visibility(false);
        let dims = fixed.unwrap_or(size);
        Ok(Self {
            window,
            fixed,
            fb: Framebuffer::new(dims.0, dims.1),
        })
    }

    fn window_size(&self) -> (u32, u32) {
        let (w, h) = self.window.get_size();
        ((w as u32).max(320), (h as u32).max(200))
    }

    /// Resize the virtual framebuffer when it follows the window.
    fn sync_size(&mut self) -> bool {
        let want = self.fixed.unwrap_or_else(|| self.window_size());
        if (self.fb.width, self.fb.height) != want {
            self.fb = Framebuffer::new(want.0, want.1);
            return true;
        }
        false
    }

    /// Map a window coordinate to the letterboxed framebuffer.
    fn to_fb(&self, x: f32, y: f32) -> Option<(u32, u32)> {
        let (ww, wh) = self.window.get_size();
        let (fw, fh) = (self.fb.width as f32, self.fb.height as f32);
        let scale = (ww as f32 / fw).min(wh as f32 / fh);
        let ox = (ww as f32 - fw * scale) / 2.0;
        let oy = (wh as f32 - fh * scale) / 2.0;
        let fx = (x - ox) / scale;
        let fy = (y - oy) / scale;
        if fx < 0.0 || fy < 0.0 || fx >= fw || fy >= fh {
            return None;
        }
        Some((fx as u32, fy as u32))
    }

    fn present(&mut self) -> Result<(), String> {
        self.window
            .update_with_buffer(&self.fb.px, self.fb.width as usize, self.fb.height as usize)
            .map_err(|err| format!("present: {err}"))
    }
}

pub fn run(opts: &Options) -> Result<(), String> {
    let scenarios = scenario::list(&scenario::scenario_dir())?;
    let mut current = scenario::resolve(&opts.scenario)?;
    let initial_layout = opts.layouts.first().copied().unwrap_or(LayoutMode::Classic);
    let initial_detail = opts
        .details
        .first()
        .copied()
        .unwrap_or(StorageDetail::Compact);
    let initial_font = opts
        .fonts
        .first()
        .copied()
        .unwrap_or(FontMode::AtkinsonNextMono);
    let mut session = Session::power_on_with(
        Machine::new(Scenario::load(&current)?),
        opts.dark,
        initial_layout,
        opts.discovery,
    );
    session.set_detail(initial_detail);
    session.set_font_mode(initial_font);
    session.set_high_contrast(opts.high_contrast);
    session.set_scale(opts.scale);
    let mut view = View::open(opts.size, opts.fixed)?;
    let mut preset = None::<usize>;
    let mut shots = 0u32;
    let mut dirty = true;
    let mut mouse_last = None::<(u32, u32)>;
    let mut mouse_down = false;
    let mut scroll_dragging = false;
    let mut last = Instant::now();
    let mut first_frame = true;
    let mut pending = session.take_pending();
    let session_dir = sbm_preview::workspace_root().join("build/sim/session");
    set_title(&mut view, &session);

    while view.window.is_open() {
        let now = Instant::now();
        let dt = now.duration_since(last).as_millis().min(1000) as u32;
        last = now;
        let (w, h) = (view.fb.width, view.fb.height);
        let mut effects = vec![std::mem::replace(&mut pending, Effect::None)];

        let shift =
            view.window.is_key_down(MKey::LeftShift) || view.window.is_key_down(MKey::RightShift);
        let command =
            view.window.is_key_down(MKey::LeftSuper) || view.window.is_key_down(MKey::RightSuper);
        for key in view.window.get_keys_pressed(KeyRepeat::Yes) {
            match key {
                MKey::Q if command => return Ok(()),
                MKey::F1 => {
                    session.help = !session.help;
                    dirty = true;
                }
                MKey::F2 => {
                    let index = scenarios
                        .iter()
                        .position(|path| path == &current)
                        .unwrap_or(0);
                    let next = if shift {
                        (index + scenarios.len() - 1) % scenarios.len()
                    } else {
                        (index + 1) % scenarios.len()
                    };
                    current = scenarios[next].clone();
                    let dark = session.state.dark;
                    let layout = session.layout;
                    let detail = session.detail;
                    let discovery = session.discovery;
                    let font = session.font_mode;
                    let hc = session.high_contrast;
                    let scale = session.scale;
                    session = Session::power_on_with(
                        Machine::new(Scenario::load(&current)?),
                        dark,
                        layout,
                        discovery,
                    );
                    session.set_detail(detail);
                    session.set_font_mode(font);
                    session.set_high_contrast(hc);
                    session.set_scale(scale);
                    effects.push(session.take_pending());
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F3 => {
                    preset = match preset {
                        None => Some(0),
                        Some(index) if index + 1 < DISPLAY_PRESETS.len() => Some(index + 1),
                        Some(_) => None,
                    };
                    view.fixed = preset.map(|index| DISPLAY_PRESETS[index]);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F4 => {
                    let next = if shift {
                        session.layout.prev()
                    } else {
                        session.layout.next()
                    };
                    session.set_layout(next);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::Key1 | MKey::Key2 | MKey::Key3 | MKey::Key4 if session.sim.is_none() => {
                    let next = match key {
                        MKey::Key1 => LayoutMode::Classic,
                        MKey::Key2 => LayoutMode::Bento,
                        MKey::Key3 => LayoutMode::Schematic,
                        _ => LayoutMode::Storage,
                    };
                    session.set_layout(next);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F5 => {
                    let dark = session.state.dark;
                    let layout = session.layout;
                    let detail = session.detail;
                    let discovery = session.discovery;
                    let font = session.font_mode;
                    let hc = session.high_contrast;
                    let scale = session.scale;
                    session = Session::power_on_with(
                        Machine::new(Scenario::load(&current)?),
                        dark,
                        layout,
                        discovery,
                    );
                    session.set_detail(detail);
                    session.set_font_mode(font);
                    session.set_high_contrast(hc);
                    session.set_scale(scale);
                    effects.push(session.take_pending());
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F6 => {
                    let mut next = session.discovery;
                    next.mode = next.mode.next();
                    session.set_discovery(next);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F7 => {
                    let next = if shift {
                        session.font_mode.prev()
                    } else {
                        session.font_mode.next()
                    };
                    session.set_font_mode(next);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F8 => {
                    session.set_high_contrast(!session.high_contrast);
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::F9 => {
                    if session.layout != LayoutMode::Classic {
                        session.state.countdown_ms = None;
                        session.toggle_detail();
                        set_title(&mut view, &session);
                        dirty = true;
                    }
                }
                MKey::Equal | MKey::NumPadPlus => {
                    session.set_scale(session.scale.step_up());
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::Minus | MKey::NumPadMinus => {
                    session.set_scale(session.scale.step_down());
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::Key0 | MKey::NumPad0 => {
                    session.set_scale(UiScale::default());
                    set_title(&mut view, &session);
                    dirty = true;
                }
                MKey::PageUp if session.sim.is_none() => {
                    effects.push(session.scroll_by(-180, w, h));
                }
                MKey::PageDown if session.sim.is_none() => {
                    effects.push(session.scroll_by(180, w, h));
                }
                MKey::F12 => {
                    shots += 1;
                    let path = screenshot_path(&session, &view.fb, shots);
                    match write_image(&path, &view.fb) {
                        Ok(bytes) => println!("screenshot {} ({bytes} bytes)", path.display()),
                        Err(err) => eprintln!("screenshot failed: {err}"),
                    }
                }
                other => {
                    if let Some(key) = map_key(other, shift) {
                        if session.help {
                            session.help = false;
                            dirty = true;
                        }
                        let prev_detail = session.detail;
                        effects.push(session.key(key, w, h));
                        if session.detail != prev_detail {
                            set_title(&mut view, &session);
                        }
                    }
                }
            }
        }

        if let Some((_sx, sy)) = view.window.get_scroll_wheel()
            && sy.abs() > 0.001
        {
            let mut delta = (-sy * 28.0).round() as i32;
            if delta == 0 {
                delta = if sy < 0.0 { 1 } else { -1 };
            }
            effects.push(session.scroll_by(delta, w, h));
        }

        let pressed = view.window.get_mouse_down(MouseButton::Left);
        let clicked = pressed && !mouse_down;
        if !pressed {
            scroll_dragging = false;
        }
        mouse_down = pressed;
        if let Some(pos) = view
            .window
            .get_mouse_pos(MouseMode::Discard)
            .and_then(|(x, y)| view.to_fb(x, y))
        {
            if clicked && session.on_scroll_track(pos.0, pos.1, w, h) {
                scroll_dragging = true;
            }
            let moved = mouse_last != Some(pos);
            if scroll_dragging && pressed {
                mouse_last = Some(pos);
                effects.push(session.scroll_drag(pos.1, w, h));
            } else if moved || clicked {
                mouse_last = Some(pos);
                let prev_detail = session.detail;
                effects.push(session.pointer(
                    Pointer {
                        x: pos.0,
                        y: pos.1,
                        moved,
                        clicked,
                    },
                    w,
                    h,
                ));
                if session.detail != prev_detail {
                    set_title(&mut view, &session);
                }
            }
        }

        effects.push(session.tick(dt, w, h));

        for effect in effects {
            match effect {
                Effect::None => {}
                Effect::Redraw => dirty = true,
                Effect::LaunchCore(request) => {
                    let size = view.window_size();
                    let fixed = view.fixed;
                    drop(view);
                    let next = launch_core(&mut session, &session_dir, &opts.core_bin, &request);
                    view = View::open(size, fixed)?;
                    set_title(&mut view, &session);
                    pending = next;
                    mouse_last = None;
                    last = Instant::now();
                    dirty = true;
                }
            }
        }

        if view.sync_size() {
            dirty = true;
        }
        if dirty {
            session.render(&mut view.fb);
            view.present()?;
            dirty = false;
            if first_frame {
                first_frame = false;
                println!(
                    "first frame {:.1} ms after process start ({}x{})",
                    opts.started.elapsed().as_secs_f64() * 1000.0,
                    view.fb.width,
                    view.fb.height
                );
            }
        } else {
            view.window.update();
        }
        if let Some(limit) = opts.exit_after_ms
            && opts.started.elapsed().as_millis() as u64 >= limit
        {
            return Ok(());
        }
        // Poll at 60 fps while cards animate or an experimental countdown partition flashes.
        let frame = if session.sim.is_none()
            && (session.state.anim_ms > 0
                || (session.layout != LayoutMode::Classic && session.state.countdown_ms.is_some()))
        {
            ACTIVE_FRAME
        } else {
            IDLE_FRAME
        };
        let spent = now.elapsed();
        if spent < frame {
            thread::sleep(frame - spent);
        }
    }
    Ok(())
}

fn launch_core(
    session: &mut Session,
    dir: &Path,
    core_bin: &Path,
    request: &sbm_preview::session::CoreRequest,
) -> Effect {
    println!(
        "SIMULATED: starting native SBM Core ({})",
        session.machine.scenario.id
    );
    if let Err(err) = core::materialize(dir, &session.machine, Some(request)) {
        session.core_unavailable(&err);
        return Effect::Redraw;
    }
    match core::run(core_bin, dir) {
        Ok(exit) => {
            if let Err(err) = core::import(dir, &session.machine) {
                session.core_unavailable(&err);
                return Effect::Redraw;
            }
            println!(
                "SIMULATED: SBM Core requested {}",
                if exit == CoreExit::Shutdown {
                    "shutdown"
                } else {
                    "reboot"
                }
            );
            session.after_core(exit)
        }
        Err(err) => {
            eprintln!("{err}");
            session.core_unavailable(&err);
            Effect::Redraw
        }
    }
}

fn set_title(view: &mut View, session: &Session) {
    let size = match view.fixed {
        Some((w, h)) => format!("{w}x{h} fixed"),
        None => "follows window".into(),
    };
    let hc = if session.high_contrast { " HC" } else { "" };
    let detail = if session.layout == LayoutMode::Classic {
        String::new()
    } else {
        format!(" [{}]", session.detail.label())
    };
    view.window.set_title(&format!(
        "Smart Boot Menu - SIMULATED - {} - {}{detail} - {} - {} {}%{hc} - {size} - F1 help",
        session.machine.scenario.id,
        session.layout.label(),
        session.discovery.mode.id(),
        session.font_mode.label(),
        session.scale.percent(),
    ));
}

fn screenshot_path(session: &Session, fb: &Framebuffer, n: u32) -> PathBuf {
    let hc = if session.high_contrast { "-hc" } else { "" };
    let detail = if session.layout == LayoutMode::Classic {
        String::new()
    } else {
        format!("-{}", session.detail.id())
    };
    sbm_preview::workspace_root()
        .join("build/sim/screenshots")
        .join(format!(
            "{}-{}{detail}-{}-{}{hc}-{}pct-{}x{}-{n:03}.png",
            session.machine.scenario.id,
            session.layout.id(),
            session.font_mode.id(),
            session.screen_name(),
            session.scale.percent(),
            fb.width,
            fb.height
        ))
}

fn map_key(key: MKey, shift: bool) -> Option<Key> {
    let letter = |ch: char| Some(Key::Char(if shift { ch.to_ascii_uppercase() } else { ch }));
    match key {
        MKey::Up => Some(Key::Up),
        MKey::Down => Some(Key::Down),
        MKey::Left => Some(Key::Left),
        MKey::Right => Some(Key::Right),
        MKey::Enter | MKey::NumPadEnter => Some(Key::Activate),
        MKey::Tab => Some(Key::Tab),
        MKey::Escape => Some(Key::Escape),
        MKey::Space => Some(Key::Char(' ')),
        MKey::A => letter('a'),
        MKey::B => letter('b'),
        MKey::C => letter('c'),
        MKey::D => letter('d'),
        MKey::E => letter('e'),
        MKey::F => letter('f'),
        MKey::G => letter('g'),
        MKey::H => letter('h'),
        MKey::I => letter('i'),
        MKey::J => letter('j'),
        MKey::K => letter('k'),
        MKey::L => letter('l'),
        MKey::M => letter('m'),
        MKey::N => letter('n'),
        MKey::O => letter('o'),
        MKey::P => letter('p'),
        MKey::Q => letter('q'),
        MKey::R => letter('r'),
        MKey::S => letter('s'),
        MKey::T => letter('t'),
        MKey::U => letter('u'),
        MKey::V => letter('v'),
        MKey::W => letter('w'),
        MKey::X => letter('x'),
        MKey::Y => letter('y'),
        MKey::Z => letter('z'),
        _ => None,
    }
}
