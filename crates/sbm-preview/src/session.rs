//! One power-on of the simulated computer. Startup, launch checks,
//! verification, persistence and the handoff run through the same
//! `sbm_menu::flow::Picker` as the UEFI app; this file only adds the screens
//! real firmware would hand to another program.

use sbm_gfx::{Framebuffer, Input, MenuState, Outcome, Screen, layout_menu, paint_splash, step};
use sbm_menu::flow::{HandoffVar, LaunchPlan, LaunchResult, Picker};
use sbm_menu::{
    DiagFacts, Key, MenuEntry, NO_RECOVERY_MESSAGE, Pointer, StartFailure, diag_lines, menu_input,
    paint_current, recovery_position,
};
use sbm_types::{BootTargetKind, OsFamily, PlatformCapabilities};

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
    menu_us: u64,
    boots: u32,
    pub last_reason: &'static str,
    pending: Option<Effect>,
    /// The Core launch in progress, until Core exits or cannot start.
    core_plan: Option<LaunchPlan>,
}

impl Session {
    pub fn power_on(mut machine: Machine, dark: bool) -> Self {
        machine.reset();
        let picker = Picker::start(&mut machine, &mut || false);
        let mut session = Self {
            machine,
            picker,
            state: MenuState::new(0, dark, None),
            sim: None,
            help: false,
            menu_us: 0,
            boots: 1,
            last_reason: "menu",
            pending: None,
            core_plan: None,
        };
        session.show_menu();
        session
    }

    /// Simulated reset: volatile variables are gone; discovery, policy and
    /// the ESP state files run again.
    pub fn boot(&mut self) {
        self.boots += 1;
        self.sim = None;
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
            menu_input(
                None,
                &pointer,
                width,
                height,
                self.cards(),
                self.state.selected,
            )
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
        let shown = self.state.countdown_ms.map(|ms| ms.div_ceil(1000));
        let animating = self.state.anim_ms > 0;
        let effect = self.drive(None, width, height, dt_ms);
        if effect != Effect::None {
            return effect;
        }
        if animating || shown != self.state.countdown_ms.map(|ms| ms.div_ceil(1000)) {
            Effect::Redraw
        } else {
            Effect::None
        }
    }

    /// True while something on screen changes without input.
    pub fn animating(&self) -> bool {
        self.sim.is_none()
            && self.state.screen == Screen::Menu
            && (self.state.anim_ms > 0 || self.state.countdown_ms.is_some())
    }

    fn drive(&mut self, input: Option<Input>, width: u32, height: u32, dt_ms: u32) -> Effect {
        let cards = self.cards();
        let cols = layout_menu(width, height, cards, 3, self.state.selected).cols;
        let had_input = input.is_some();
        let outcome = step(&mut self.state, input, cards, cols, dt_ms);
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
        lines.push(format!("scenario {} ({})", scenario.id, scenario.name));
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
        match &self.sim {
            Some(screen) => overlay::paint_sim_screen(fb, self.state.dark, screen),
            None => {
                let lines = self.diagnostics(fb.width, fb.height);
                paint_current(
                    fb,
                    &self.picker.entries,
                    &self.picker.visible,
                    &self.state,
                    &lines,
                );
            }
        }
        if self.help {
            overlay::paint_help(fb, self.state.dark);
        }
    }

    pub fn render_splash(&self, fb: &mut Framebuffer) {
        paint_splash(fb, self.state.dark);
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
