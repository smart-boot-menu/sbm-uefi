//! One power-on of the simulated computer. Startup, launch checks,
//! verification, persistence and the handoff run through the same
//! `sbm_menu::flow::Picker` as the UEFI app; this file only adds the screens
//! real firmware would hand to another program.

use std::cell::RefCell;

use sbm_gfx::hardware::{
    ExperimentalHit, ExperimentalOptions, LayoutMode, StorageDetail, layout_experimental_with,
    paint_experimental_with,
};
use sbm_gfx::{
    CacheStats, FontMode, Framebuffer, Input, MenuState, Outcome, Screen, TextRenderer, UiScale,
    layout_menu, paint_splash_with, step,
};
use sbm_menu::flow::{HandoffVar, LaunchPlan, LaunchResult, Picker};
use sbm_menu::{
    DiagFacts, Key, MenuEntry, NO_RECOVERY_MESSAGE, Pointer, StartFailure, diag_lines, menu_input,
    paint_current_with, recovery_position,
};
use sbm_types::{BootTargetKind, OsFamily, PlatformCapabilities};

use crate::hardware::{DiscoverySchedule, build_system_spec};
use crate::machine::Machine;
use crate::overlay;

/// Screens that exist only in the simulator. Real firmware hands control to
/// another program at these points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimScreen {
    Booted {
        name: String,
        path: String,
        family: OsFamily,
        notes: Vec<String>,
    },
    FirmwareSetup,
    PoweredOff,
}

/// What firmware hands SBM Core: the handoff file and the Spec 03 §8
/// variables exactly as the picker published them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreRequest {
    pub handoff: Vec<u8>,
    /// `(name, vendor GUID, value)` for each published variable.
    pub vars: Vec<(String, String, String)>,
    pub backing_device: Option<String>,
}

impl CoreRequest {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(var, _, _)| var == name)
            .map(|(_, _, value)| value.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Redraw,
    LaunchCore(Box<CoreRequest>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreExit {
    Reboot,
    Shutdown,
}

pub struct Session {
    pub machine: Machine,
    pub picker: Picker,
    pub state: MenuState,
    pub sim: Option<SimScreen>,
    pub help: bool,
    pub layout: LayoutMode,
    pub detail: StorageDetail,
    pub overflow_drive: Option<usize>,
    pub discovery: DiscoverySchedule,
    pub font_mode: FontMode,
    pub high_contrast: bool,
    pub scale: UiScale,
    renderer: RefCell<TextRenderer<'static>>,
    pub elapsed_ms: u32,
    pub scroll_y: u32,
    menu_us: u64,
    boots: u32,
    pub last_reason: &'static str,
    pending: Option<Effect>,
    /// The Core launch in progress, until Core exits or cannot start.
    core_plan: Option<LaunchPlan>,
}

impl Session {
    pub fn power_on(machine: Machine, dark: bool) -> Self {
        Self::power_on_with(
            machine,
            dark,
            LayoutMode::Classic,
            DiscoverySchedule::default(),
        )
    }

    pub fn power_on_with(
        mut machine: Machine,
        dark: bool,
        layout: LayoutMode,
        discovery: DiscoverySchedule,
    ) -> Self {
        machine.reset();
        let picker = Picker::start(&mut machine, &mut || false);
        let font_mode = FontMode::AtkinsonNextMono;
        let scale = UiScale::default();
        let mut session = Self {
            machine,
            picker,
            state: MenuState::new(0, dark, None),
            sim: None,
            help: false,
            layout,
            detail: StorageDetail::default(),
            overflow_drive: None,
            discovery,
            font_mode,
            high_contrast: false,
            scale,
            renderer: RefCell::new(TextRenderer::new(font_mode, 800, scale)),
            elapsed_ms: 0,
            scroll_y: 0,
            menu_us: 0,
            boots: 1,
            last_reason: "menu",
            pending: None,
            core_plan: None,
        };
        session.show_menu();
        session
    }

    pub fn set_layout(&mut self, layout: LayoutMode) {
        self.layout = layout;
        self.scroll_y = 0;
        self.overflow_drive = None;
    }

    pub fn set_detail(&mut self, detail: StorageDetail) {
        self.detail = detail;
        self.scroll_y = 0;
        self.overflow_drive = None;
    }

    pub fn toggle_detail(&mut self) {
        self.set_detail(self.detail.toggle());
    }

    pub fn set_overflow_drive(&mut self, drive: Option<usize>) {
        self.overflow_drive = if self.detail == StorageDetail::Compact {
            drive
        } else {
            None
        };
    }

    pub fn experimental_options(&self) -> ExperimentalOptions {
        ExperimentalOptions {
            scroll_y: self.scroll_y,
            high_contrast: self.high_contrast,
            detail: self.detail,
            overflow_drive: if self.detail == StorageDetail::Compact {
                self.overflow_drive
            } else {
                None
            },
        }
    }

    pub fn set_discovery(&mut self, discovery: DiscoverySchedule) {
        self.discovery = discovery;
        self.elapsed_ms = 0;
    }

    pub fn set_font_mode(&mut self, mode: FontMode) {
        self.font_mode = mode;
        self.renderer.borrow_mut().set_mode(mode);
    }

    pub fn set_high_contrast(&mut self, high_contrast: bool) {
        self.high_contrast = high_contrast;
    }

    pub fn set_scale(&mut self, scale: UiScale) {
        self.scale = scale;
        self.renderer.borrow_mut().set_scale(scale);
        self.scroll_y = 0;
        self.overflow_drive = None;
    }

    pub fn renderer_stats(&self) -> CacheStats {
        self.renderer.borrow().cache_stats()
    }

    /// Abort any active policy autoboot countdown without moving focus or launching a target.
    pub fn abort_autoboot(&mut self) -> Effect {
        if self.state.countdown_ms.take().is_some() {
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// Select a bootable partition on first click and cancel any active policy
    /// countdown. Never starts a new automatic boot timer.
    fn select_card(&mut self, index: usize, width: u32, height: u32) -> Effect {
        if index >= self.cards() {
            return Effect::None;
        }
        self.state.selected = index;
        self.state.focus = sbm_gfx::Focus::Cards;
        self.state.countdown_ms = None;
        self.state.anim_ms = 160;
        self.ensure_selected_visible(width, height);
        Effect::Redraw
    }

    fn experimental_hit_map(&self, width: u32, height: u32) -> sbm_gfx::hardware::LayoutHitMap {
        let spec = self.system_spec(width, height);
        let opts = self.experimental_options();
        let mut renderer = self.renderer.borrow_mut();
        layout_experimental_with(
            self.layout,
            width,
            height,
            &spec,
            &self.state,
            opts,
            &mut renderer,
        )
    }

    /// Scroll the multi-drive storage workspace vertically by `delta` pixels.
    pub fn scroll_by(&mut self, delta: i32, width: u32, height: u32) -> Effect {
        if self.sim.is_some()
            || self.state.screen != Screen::Menu
            || self.layout == LayoutMode::Classic
            || delta == 0
        {
            return Effect::None;
        }
        let map = self.experimental_hit_map(width, height);
        let next = (self.scroll_y as i64 + delta as i64).clamp(0, map.max_scroll_y as i64) as u32;
        if next != self.scroll_y {
            self.scroll_y = next;
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// True when `(x, y)` lies on the experimental storage scrollbar track (starts drag).
    pub fn on_scroll_track(&self, x: u32, y: u32, width: u32, height: u32) -> bool {
        if self.sim.is_some()
            || self.state.screen != Screen::Menu
            || self.layout == LayoutMode::Classic
        {
            return false;
        }
        let map = self.experimental_hit_map(width, height);
        map.scroll_track.is_some_and(|r| r.contains(x, y))
    }

    /// Scrub `scroll_y` continuously while dragging the scrollbar thumb.
    pub fn scroll_drag(&mut self, y: u32, width: u32, height: u32) -> Effect {
        if self.sim.is_some()
            || self.state.screen != Screen::Menu
            || self.layout == LayoutMode::Classic
        {
            return Effect::None;
        }
        let map = self.experimental_hit_map(width, height);
        if let Some(track) = map.scroll_track
            && map.max_scroll_y > 0
        {
            let thumb_h = map.scroll_thumb.map(|t| t.h).unwrap_or(24);
            let travel = track.h.saturating_sub(thumb_h).max(1);
            let rel = y.saturating_sub(track.y + thumb_h / 2).min(travel);
            let target = ((rel as u64 * map.max_scroll_y as u64) / travel as u64) as u32;
            let clamped = target.min(map.max_scroll_y);
            if clamped != self.scroll_y {
                self.scroll_y = clamped;
                return Effect::Redraw;
            }
        }
        Effect::None
    }

    /// Auto-scroll the storage viewport so the drive holding `state.selected` is visible.
    fn ensure_selected_visible(&mut self, width: u32, height: u32) {
        if self.layout == LayoutMode::Classic || self.state.screen != Screen::Menu {
            return;
        }
        let map = self.experimental_hit_map(width, height);
        if map.max_scroll_y == 0 {
            self.scroll_y = 0;
            return;
        }
        if let Some((top, bottom)) = map.selected_drive_band {
            let mut next = self.scroll_y.min(map.max_scroll_y);
            if top < next {
                next = top;
            } else if bottom > next.saturating_add(map.viewport_h) {
                next = bottom.saturating_sub(map.viewport_h);
            }
            self.scroll_y = next.min(map.max_scroll_y);
        }
    }

    /// Simulated reset: volatile variables are gone; discovery, policy and
    /// the ESP state files run again.
    pub fn boot(&mut self) {
        self.boots += 1;
        self.sim = None;
        self.elapsed_ms = 0;
        self.scroll_y = 0;
        self.overflow_drive = None;
        self.core_plan = None;
        self.machine.reset();
        self.picker = Picker::start(&mut self.machine, &mut || false);
        self.show_menu();
    }

    fn show_menu(&mut self) {
        self.last_reason = self.picker.resolved.reason;
        self.state = self.picker.initial_state(self.state.dark);
        self.menu_us = sbm_menu::flow::Platform::elapsed_us(&self.machine);
        if self.state.countdown_ms == Some(0)
            && let Some(index) = self.picker.entry_for_card(self.state.selected)
        {
            let effect = self.boot_at(index);
            self.pending = Some(effect);
        }
    }

    pub fn entries(&self) -> &[MenuEntry] {
        &self.picker.entries
    }

    pub fn visible(&self) -> &[usize] {
        &self.picker.visible
    }

    /// An effect produced during `boot()` (an immediate one-shot or request).
    pub fn take_pending(&mut self) -> Effect {
        self.pending.take().unwrap_or(Effect::None)
    }

    pub fn cards(&self) -> usize {
        self.picker.visible.len()
    }

    pub fn key(&mut self, key: Key, width: u32, height: u32) -> Effect {
        if let Some(screen) = self.sim.clone() {
            return self.sim_key(screen, key);
        }
        if self.state.screen == Screen::Menu && self.layout != LayoutMode::Classic {
            if self.overflow_drive.is_some() && key == Key::Escape {
                self.overflow_drive = None;
                return Effect::Redraw;
            }
            if matches!(key, Key::Char('v' | 'V')) {
                self.state.countdown_ms = None;
                self.toggle_detail();
                return Effect::Redraw;
            }
        }
        let input = menu_input(
            Some(key),
            &Pointer::default(),
            width,
            height,
            self.cards(),
            self.state.selected,
        );
        self.drive(input, width, height, 0)
    }

    pub fn pointer(&mut self, pointer: Pointer, width: u32, height: u32) -> Effect {
        if self.sim.is_some() {
            return Effect::None;
        }
        let before = self.state.cursor;
        self.state.cursor = Some((pointer.x, pointer.y));
        let input = if self.state.screen == Screen::Menu {
            if self.layout == LayoutMode::Classic {
                menu_input(
                    None,
                    &pointer,
                    width,
                    height,
                    self.cards(),
                    self.state.selected,
                )
            } else {
                let map = self.experimental_hit_map(width, height);
                match map.hit_test_experimental(pointer.x, pointer.y) {
                    ExperimentalHit::AbortAutoboot if pointer.clicked => {
                        return self.abort_autoboot();
                    }
                    ExperimentalHit::SetDetail(detail) if pointer.clicked => {
                        self.state.countdown_ms = None;
                        self.set_detail(detail);
                        return Effect::Redraw;
                    }
                    ExperimentalHit::ToggleOverflow(drive) if pointer.clicked => {
                        self.state.countdown_ms = None;
                        self.overflow_drive = if self.overflow_drive == Some(drive) {
                            None
                        } else {
                            Some(drive)
                        };
                        return Effect::Redraw;
                    }
                    ExperimentalHit::CloseOverflow if pointer.clicked => {
                        self.overflow_drive = None;
                        return Effect::Redraw;
                    }
                    ExperimentalHit::ScrollDelta(delta) if pointer.clicked => {
                        return self.scroll_by(delta, width, height);
                    }
                    ExperimentalHit::ScrollTo(target) if pointer.clicked => {
                        let clamped = target.min(map.max_scroll_y);
                        if clamped != self.scroll_y {
                            self.scroll_y = clamped;
                            return Effect::Redraw;
                        }
                        None
                    }
                    ExperimentalHit::Card(index) if pointer.clicked => {
                        // Safe two-step click on dense partition maps & Compact cards:
                        // - Clicking an unselected target selects it & cancels any policy
                        //   countdown (never boots on first click and never arms a new timer).
                        // - Clicking the already-selected target boots it immediately.
                        let already_selected = self.state.focus == sbm_gfx::Focus::Cards
                            && self.state.selected == index;
                        if already_selected {
                            self.overflow_drive = None;
                            Some(Input::ClickCard(index))
                        } else {
                            return self.select_card(index, width, height);
                        }
                    }
                    ExperimentalHit::Card(_) if pointer.moved => {
                        // Hover updates visual highlight & LBA inspect via `state.cursor`
                        // without changing `state.selected` or touching policy countdown.
                        None
                    }
                    ExperimentalHit::Action(index) if pointer.clicked => {
                        self.overflow_drive = None;
                        Some(Input::ClickAction(index))
                    }
                    _ => None,
                }
            }
        } else if pointer.clicked {
            Some(Input::Escape)
        } else {
            None
        };
        let effect = self.drive(input, width, height, 0);
        if effect == Effect::None && before != self.state.cursor {
            Effect::Redraw
        } else {
            effect
        }
    }

    pub fn tick(&mut self, dt_ms: u32, width: u32, height: u32) -> Effect {
        if self.sim.is_some() {
            return Effect::None;
        }
        let before_settled = self.discovery.settled_count(self.elapsed_ms);
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
        let probe_changed = self.layout != LayoutMode::Classic
            && self.discovery.settled_count(self.elapsed_ms) != before_settled;
        let shown = self.state.countdown_ms.map(|ms| ms.div_ceil(1000));
        let flashing = self.layout != LayoutMode::Classic
            && self.state.screen == Screen::Menu
            && self.state.countdown_ms.is_some();
        let animating = self.state.anim_ms > 0;
        let effect = self.drive(None, width, height, dt_ms);
        if effect != Effect::None {
            return effect;
        }
        if probe_changed
            || animating
            || flashing
            || shown != self.state.countdown_ms.map(|ms| ms.div_ceil(1000))
        {
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// True while something on screen changes without input.
    pub fn animating(&self) -> bool {
        self.sim.is_none()
            && self.state.screen == Screen::Menu
            && (self.state.anim_ms > 0
                || self.state.countdown_ms.is_some()
                || (self.layout != LayoutMode::Classic
                    && self.discovery.is_probing(self.elapsed_ms)))
    }

    pub fn system_spec(&self, width: u32, height: u32) -> sbm_gfx::hardware::SystemSpec {
        build_system_spec(
            &self.machine,
            &self.picker.entries,
            &self.picker.visible,
            self.last_reason,
            self.discovery,
            self.elapsed_ms,
            width,
            height,
        )
    }

    fn drive(&mut self, input: Option<Input>, width: u32, height: u32, dt_ms: u32) -> Effect {
        let cards = self.cards();
        let cols = if self.layout == LayoutMode::Classic {
            layout_menu(width, height, cards, 3, self.state.selected).cols
        } else {
            self.experimental_hit_map(width, height).cols.max(1)
        };
        let prev_selected = self.state.selected;
        let had_input = input.is_some();
        let outcome = step(&mut self.state, input, cards, cols, dt_ms);
        if self.state.selected != prev_selected {
            self.ensure_selected_visible(width, height);
        }
        let effect = match outcome {
            Outcome::Boot => match self.picker.entry_for_card(self.state.selected) {
                Some(index) => self.boot_at(index),
                None => Effect::Redraw,
            },
            Outcome::Recovery => match recovery_position(&self.picker.entries) {
                Some(index) => self.boot_at(index),
                None => {
                    self.state.screen = Screen::Error;
                    self.state.message = NO_RECOVERY_MESSAGE.into();
                    Effect::Redraw
                }
            },
            Outcome::Action(0) => {
                self.sim = Some(SimScreen::FirmwareSetup);
                Effect::Redraw
            }
            Outcome::Action(1) => {
                self.boot();
                self.pending.take().unwrap_or(Effect::Redraw)
            }
            Outcome::Action(2) => {
                self.sim = Some(SimScreen::PoweredOff);
                Effect::Redraw
            }
            Outcome::Action(_) | Outcome::Exit | Outcome::None => Effect::None,
        };
        if effect == Effect::None && had_input {
            Effect::Redraw
        } else {
            effect
        }
    }

    fn sim_key(&mut self, screen: SimScreen, key: Key) -> Effect {
        match (screen, key) {
            // Restart the computer. No OS reports success in 0.0.1.
            (SimScreen::Booted { .. }, Key::Activate | Key::Char('x' | 'X')) => {
                self.boot();
                self.pending.take().unwrap_or(Effect::Redraw)
            }
            (SimScreen::Booted { .. }, Key::Char('p' | 'P')) => {
                self.sim = Some(SimScreen::PoweredOff);
                Effect::Redraw
            }
            (SimScreen::FirmwareSetup | SimScreen::PoweredOff, Key::Activate) => {
                self.boot();
                self.pending.take().unwrap_or(Effect::Redraw)
            }
            (_, Key::Char('t' | 'T')) => {
                self.state.dark = !self.state.dark;
                Effect::Redraw
            }
            _ => Effect::None,
        }
    }

    fn boot_at(&mut self, index: usize) -> Effect {
        self.state.countdown_ms = None;
        let plan = match self.picker.prepare(&mut self.machine, index, 0) {
            Ok(plan) => plan,
            Err(message) => return self.show_error(message),
        };
        if let Some(failed) = self.machine.load_image(&plan) {
            let message = self.picker.finish(&mut self.machine, &plan, &failed);
            return self.show_error(message.unwrap_or_default());
        }
        self.picker.loaded(&mut self.machine, &plan);
        self.state.screen = Screen::Menu;
        self.state.message.clear();
        if plan.kind == BootTargetKind::SbmCore {
            let request = self.core_request(&plan);
            self.core_plan = Some(plan);
            return Effect::LaunchCore(Box::new(request));
        }
        let target = &self.picker.entries[plan.entry].item.target;
        let mut notes = Vec::new();
        if target.os_family == OsFamily::Windows
            && self
                .machine
                .scenario
                .disks
                .iter()
                .any(|disk| disk.partitions.iter().any(|part| part.bitlocker))
        {
            notes.push(
                "BitLocker: Windows Boot Manager would unseal the volume key from the TPM or ask for the recovery key."
                    .into(),
            );
        }
        if let Some(options) = plan.load_options.as_deref() {
            notes.push(format!("Load options: {options}"));
        }
        self.sim = Some(SimScreen::Booted {
            name: plan.name.clone(),
            path: plan.path.clone(),
            family: target.os_family,
            notes,
        });
        Effect::Redraw
    }

    fn show_error(&mut self, message: String) -> Effect {
        self.state.screen = Screen::Error;
        self.state.message = message;
        self.state.countdown_ms = None;
        Effect::Redraw
    }

    fn core_request(&self, plan: &LaunchPlan) -> CoreRequest {
        let vars = HandoffVar::ALL
            .iter()
            .filter_map(|var| {
                self.machine.vars.get(var).map(|value| {
                    (
                        var.name().to_string(),
                        var.vendor_guid().to_string(),
                        value.clone(),
                    )
                })
            })
            .collect();
        CoreRequest {
            handoff: plan.handoff.clone().unwrap_or_default(),
            vars,
            backing_device: self.machine.device_for_esp(plan.volume).map(str::to_string),
        }
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        self.machine.capabilities()
    }

    /// Return from SBM Core. Files Core wrote to the shared ESP are already
    /// on the boot volume; this resets or powers off the computer.
    pub fn after_core(&mut self, exit: CoreExit) -> Effect {
        self.core_plan = None;
        match exit {
            CoreExit::Reboot => {
                self.boot();
                self.pending.take().unwrap_or(Effect::Redraw)
            }
            CoreExit::Shutdown => {
                self.sim = Some(SimScreen::PoweredOff);
                Effect::Redraw
            }
        }
    }

    /// The native Core simulator could not run. Treat it like `StartImage`
    /// returning an error: the attempt fails and the variables are cleared.
    pub fn core_unavailable(&mut self, message: &str) {
        if let Some(plan) = self.core_plan.take() {
            let result = LaunchResult::Returned {
                success: false,
                failure: StartFailure::Other,
                status: "SIMULATED_UNAVAILABLE".into(),
            };
            let _ = self.picker.finish(&mut self.machine, &plan, &result);
        }
        self.state.screen = Screen::Error;
        self.state.message = format!("SIMULATED: SBM Core could not start natively. {message}");
    }

    pub fn diagnostics(&self, width: u32, height: u32) -> Vec<String> {
        let scenario = &self.machine.scenario;
        let firmware = format!(
            "{} (SIMULATED, native host preview)",
            scenario.firmware.vendor
        );
        let display = format!("{width}x{height} simulated GOP framebuffer");
        let mut lines = diag_lines(
            &self.picker.entries,
            &DiagFacts {
                firmware: &firmware,
                arch: scenario.arch.cpu(),
                secure_boot: scenario.firmware.secure_boot.state(),
                display: Some(&display),
                ram_mib: Some(scenario.firmware.ram_mib),
                first_frame_us: 0,
                discovery_us: self.picker.timings.discovery_us,
                menu_us: Some(self.menu_us),
                integrity: self.picker.integrity_note.as_deref(),
            },
        );
        let cache = self.renderer.borrow().cache_stats();
        lines.push(format!("scenario {} ({})", scenario.id, scenario.name));
        lines.push(format!(
            "typography {}  scale {}%  contrast {}  glyphs {} ({} B)",
            self.font_mode.label(),
            self.scale.percent(),
            if self.high_contrast { "high" } else { "normal" },
            cache.entries,
            cache.bytes
        ));
        lines.push(format!(
            "policy decision {}  attempts {}",
            self.last_reason,
            self.picker
                .attempt
                .as_ref()
                .map(|record| record.consecutive_failures)
                .unwrap_or(0)
        ));
        lines
    }

    pub fn render(&self, fb: &mut Framebuffer) {
        let mut renderer = self.renderer.borrow_mut();
        match &self.sim {
            Some(screen) => overlay::paint_sim_screen(
                fb,
                self.state.dark,
                self.high_contrast,
                screen,
                &mut renderer,
            ),
            None if self.state.screen == Screen::Menu && self.layout != LayoutMode::Classic => {
                let spec = self.system_spec(fb.width, fb.height);
                let opts = self.experimental_options();
                let _ = paint_experimental_with(
                    fb,
                    self.layout,
                    &spec,
                    &self.state,
                    opts,
                    &mut renderer,
                );
            }
            None => {
                drop(renderer);
                let lines = self.diagnostics(fb.width, fb.height);
                let mut renderer = self.renderer.borrow_mut();
                paint_current_with(
                    fb,
                    &self.picker.entries,
                    &self.picker.visible,
                    &self.state,
                    &lines,
                    self.high_contrast,
                    &mut renderer,
                );
                if self.help {
                    overlay::paint_help(fb, self.state.dark, self.high_contrast, &mut renderer);
                }
                return;
            }
        }
        if self.help {
            overlay::paint_help(fb, self.state.dark, self.high_contrast, &mut renderer);
        }
    }

    pub fn render_splash(&self, fb: &mut Framebuffer) {
        let mut renderer = self.renderer.borrow_mut();
        paint_splash_with(fb, self.state.dark, self.high_contrast, &mut renderer);
    }

    pub fn screen_name(&self) -> &'static str {
        match (&self.sim, self.state.screen) {
            (Some(SimScreen::Booted { .. }), _) => "booted",
            (Some(SimScreen::FirmwareSetup), _) => "firmware",
            (Some(SimScreen::PoweredOff), _) => "off",
            (None, Screen::Menu) => "menu",
            (None, Screen::Diagnostics) => "diagnostics",
            (None, Screen::Error) => "error",
        }
    }

    pub fn boots(&self) -> u32 {
        self.boots
    }
}
