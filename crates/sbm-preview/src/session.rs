//! One power-on of the simulated computer, driven by the production menu
//! state machine (`sbm_gfx::step`) and policy (`sbm_boot::resolve`).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sbm_boot::{
    LocalRequest, ResolveInput, begin_attempt, build_handoff, confirm_attempt, decode_attempt,
    decode_command, decode_policy, decode_success, encode_attempt, encode_handoff, encode_policy,
    fresh_policy, handoff_sha256_hex, linux_load_options, local_request, mark_one_shot_consumed,
    recovery_for_slot, resolve, selectable,
};
use sbm_gfx::{Framebuffer, Input, MenuState, Outcome, Screen, layout_menu, paint_splash, step};
use sbm_menu::{
    DiagFacts, Key, MenuEntry, NO_RECOVERY_MESSAGE, Pointer, VERSION, boot_failure, card_position,
    countdown_ms, diag_lines, menu_input, paint_current, recovery_position, sort_entries,
    unhealthy_message, visible_positions,
};
use sbm_protocol::{
    BootAttemptRecord, BootAttemptStatus, BootPolicy, BootSuccessReport, BootTimingMetrics,
    NetworkHandoffHint, RecoveryLaunchMode,
};
use sbm_types::{
    BootTargetKind, ComponentKind, FirmwarePlatform, OsFamily, PlatformCapabilities,
    SCHEMA_VERSION_V1,
};

use crate::machine::{
    ATTEMPT_PATH, COMMAND_PATH, HANDOFF_PATH, Machine, POLICY_PATH, SUCCESS_PATH,
};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreRequest {
    pub handoff: Vec<u8>,
    pub cmdline: String,
    pub selected_target: String,
    pub backing_device: Option<String>,
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
    pub entries: Vec<MenuEntry>,
    pub visible: Vec<usize>,
    pub state: MenuState,
    pub sim: Option<SimScreen>,
    pub help: bool,
    policy: BootPolicy,
    attempt: Option<BootAttemptRecord>,
    discovery_us: u64,
    menu_us: u64,
    boots: u32,
    pub last_reason: &'static str,
    pending: Option<Effect>,
}

impl Session {
    pub fn power_on(machine: Machine, dark: bool) -> Self {
        let mut session = Self {
            machine,
            entries: Vec::new(),
            visible: Vec::new(),
            state: MenuState::new(0, dark, None),
            sim: None,
            help: false,
            policy: fresh_policy(),
            attempt: None,
            discovery_us: 0,
            menu_us: 0,
            boots: 0,
            last_reason: "menu",
            pending: None,
        };
        session.boot();
        session
    }

    /// Simulated reset: rediscover, reload persisted policy and attempt state.
    pub fn boot(&mut self) {
        let started = Instant::now();
        self.boots += 1;
        self.sim = None;
        self.entries = self
            .machine
            .discover()
            .into_iter()
            .map(|(disk, item)| MenuEntry { disk, item })
            .collect();
        sort_entries(&mut self.entries);
        self.discovery_us = started.elapsed().as_micros() as u64;
        self.policy = self
            .machine
            .read_boot(POLICY_PATH)
            .and_then(|bytes| decode_policy(&bytes).ok())
            .unwrap_or_else(fresh_policy);
        self.attempt = self
            .machine
            .read_boot(ATTEMPT_PATH)
            .and_then(|bytes| decode_attempt(&bytes).ok());
        self.apply_success();
        let targets = self
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let mut resolved = resolve(ResolveInput {
            policy: &self.policy,
            targets: &targets,
            attempt: self.attempt.as_ref(),
            now_unix: now_unix(),
            user_key_held: false,
            remote_refresh_failed: false,
        });
        let mut power_off = false;
        if let Some(bytes) = self.machine.remove_any(COMMAND_PATH)
            && let Ok(command) = decode_command(&bytes)
        {
            match local_request(&command) {
                LocalRequest::Boot(id) => {
                    resolved.highlight = Some(id);
                    resolved.countdown_seconds = Some(0);
                }
                LocalRequest::Recovery(slot) => {
                    if let Some(target) = recovery_for_slot(&targets, slot) {
                        resolved.highlight = Some(target.target.id.clone());
                        resolved.countdown_seconds = Some(0);
                    }
                }
                LocalRequest::Firmware => self.sim = Some(SimScreen::FirmwareSetup),
                LocalRequest::Reboot => {}
                LocalRequest::Shutdown => power_off = true,
            }
        }
        self.last_reason = resolved.reason;
        self.visible = visible_positions(&self.entries);
        let selected = resolved
            .highlight
            .as_ref()
            .and_then(|id| card_position(&self.entries, &self.visible, id))
            .unwrap_or(0);
        let dark = self.state.dark;
        self.state = MenuState::new(selected, dark, countdown_ms(resolved.countdown_seconds));
        self.menu_us = started.elapsed().as_micros() as u64;
        if power_off {
            self.sim = Some(SimScreen::PoweredOff);
            return;
        }
        if self.state.countdown_ms == Some(0)
            && let Some(index) = self.visible.get(self.state.selected).copied()
        {
            self.state.countdown_ms = None;
            let effect = self.boot_at(index);
            self.pending = Some(effect);
        }
    }

    /// An effect produced during `boot()` (an immediate one-shot or command).
    pub fn take_pending(&mut self) -> Effect {
        self.pending.take().unwrap_or(Effect::None)
    }

    fn apply_success(&mut self) {
        let Some(bytes) = self.machine.remove_any(SUCCESS_PATH) else {
            return;
        };
        let Ok(report) = decode_success(&bytes) else {
            return;
        };
        if let Some(record) = self.attempt.as_mut()
            && confirm_attempt(record, &report)
            && let Ok(encoded) = encode_attempt(record)
        {
            self.machine.write_boot(ATTEMPT_PATH, &encoded);
        }
    }

    pub fn cards(&self) -> usize {
        self.visible.len()
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
            Outcome::Boot => match self.visible.get(self.state.selected).copied() {
                Some(index) => self.boot_at(index),
                None => Effect::Redraw,
            },
            Outcome::Recovery => match recovery_position(&self.entries) {
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
            (SimScreen::Booted { .. }, Key::Activate) => {
                self.confirm_running_os();
                self.boot();
                self.pending.take().unwrap_or(Effect::Redraw)
            }
            (SimScreen::Booted { .. }, Key::Char('x' | 'X')) => {
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

    /// What an OS agent (`sbm-agent`) would write after a healthy boot.
    fn confirm_running_os(&mut self) {
        let Some(record) = self.attempt.as_ref() else {
            return;
        };
        let report = BootSuccessReport {
            schema_version: SCHEMA_VERSION_V1.into(),
            attempt_id: record.attempt_id.clone(),
            target_id: record.target_id.clone(),
            confirmed_by: ComponentKind::SbmAgent,
            confirmed_at_unix: now_unix().unwrap_or(0),
        };
        if let Ok(bytes) = serde_json::to_vec(&report) {
            self.machine.write_boot(SUCCESS_PATH, &bytes);
        }
    }

    fn boot_at(&mut self, index: usize) -> Effect {
        match self.launch(index) {
            Ok(effect) => {
                self.state.screen = Screen::Menu;
                self.state.countdown_ms = None;
                self.state.message.clear();
                effect
            }
            Err(message) => {
                self.state.screen = Screen::Error;
                self.state.message = message;
                self.state.countdown_ms = None;
                Effect::Redraw
            }
        }
    }

    fn launch(&mut self, index: usize) -> Result<Effect, String> {
        let Some(found) = self.entries.get(index) else {
            return Err("That boot entry is no longer available".into());
        };
        if !selectable(&found.item) {
            return Err(unhealthy_message(&found.item.target.display_name));
        }
        let target = found.item.target.clone();
        let name = target.display_name.clone();
        let path = target
            .efi_path
            .clone()
            .ok_or_else(|| format!("{name} has no EFI path"))?;
        if self
            .policy
            .one_shot
            .as_ref()
            .is_some_and(|shot| shot.target_id == target.id && !shot.consumed)
        {
            mark_one_shot_consumed(&mut self.policy);
            if let Ok(bytes) = encode_policy(&self.policy) {
                self.machine.write_boot(POLICY_PATH, &bytes);
            }
        }
        let mut record = begin_attempt(
            self.attempt.as_ref(),
            &target.id,
            now_unix(),
            &format!("uefi-sim-{}", self.boots),
        );
        self.persist_attempt(&record);
        let guid = target.partition_guid.clone().unwrap_or_default();
        if let Some((failure, status)) = self.machine.start_result(&guid, &path) {
            record.status = BootAttemptStatus::Failed;
            self.persist_attempt(&record);
            self.attempt = Some(record);
            return Err(boot_failure(&name, &path, failure, status));
        }
        self.attempt = Some(record.clone());
        if target.kind == BootTargetKind::SbmCore {
            return Ok(Effect::LaunchCore(Box::new(
                self.core_request(&record, &target)?,
            )));
        }
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
        if let Some(cmdline) = target.kernel_cmdline.as_deref() {
            notes.push(format!("Kernel command line: {cmdline}"));
        }
        self.sim = Some(SimScreen::Booted {
            name,
            path,
            family: target.os_family,
            notes,
        });
        Ok(Effect::Redraw)
    }

    fn persist_attempt(&self, record: &BootAttemptRecord) {
        if let Ok(bytes) = encode_attempt(record) {
            self.machine.write_boot(ATTEMPT_PATH, &bytes);
        }
    }

    fn core_request(
        &self,
        record: &BootAttemptRecord,
        target: &sbm_types::BootTarget,
    ) -> Result<CoreRequest, String> {
        let snapshot = self
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let payload = build_handoff(
            &record.attempt_id,
            VERSION,
            self.capabilities(),
            RecoveryLaunchMode::Interactive,
            None,
            &snapshot,
            BootTimingMetrics {
                firmware_init_us: None,
                uefi_entry_us: 0,
                first_frame_us: Some(0),
                network_us: Some(0),
                handoff_us: self.menu_us,
            },
            Some(NetworkHandoffHint {
                link_up: self.machine.scenario.firmware.network,
                dhcp_acquired: false,
                server_reachable: None,
            }),
        );
        let bytes = encode_handoff(&payload).map_err(|_| "handoff encoding failed".to_string())?;
        self.machine.write_boot(HANDOFF_PATH, &bytes);
        let hash = handoff_sha256_hex(&payload).ok();
        let boot_esp = self.machine.boot_esp();
        let esp_guid =
            boot_esp.map(|esp| sbm_boot::Volume::partition_guid(&esp.volume).to_string());
        let cmdline = linux_load_options(
            target.kernel_cmdline.as_deref(),
            hash.as_deref(),
            esp_guid.as_deref(),
        )
        .unwrap_or_default();
        Ok(CoreRequest {
            handoff: bytes,
            cmdline,
            selected_target: target.id.as_str().to_string(),
            backing_device: boot_esp.map(|esp| esp.device.clone()),
        })
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        let scenario = &self.machine.scenario;
        PlatformCapabilities {
            arch: scenario.arch.cpu(),
            firmware: FirmwarePlatform::UefiNative,
            page_size_bytes: 4096,
            secure_boot_state: scenario.firmware.secure_boot.state(),
            tpm_present: scenario.firmware.tpm,
            nvram_writable: true,
            gop_framebuffer: true,
            uefi_networking: scenario.firmware.network,
            can_chainload_efi: true,
            can_direct_chainload_macos: false,
            external_usb_boot_supported: scenario.disks.iter().any(|disk| disk.removable),
            ram_total_mib: Some(scenario.firmware.ram_mib),
        }
    }

    /// Return from SBM Core. Files Core wrote to the shared ESP are already
    /// on the boot volume; this resets or powers off the computer.
    pub fn after_core(&mut self, exit: CoreExit) -> Effect {
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

    pub fn core_unavailable(&mut self, message: &str) {
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
            &self.entries,
            &DiagFacts {
                firmware: &firmware,
                arch: scenario.arch.cpu(),
                secure_boot: scenario.firmware.secure_boot.state(),
                display: Some(&display),
                ram_mib: Some(scenario.firmware.ram_mib),
                first_frame_us: 0,
                discovery_us: self.discovery_us,
                menu_us: Some(self.menu_us),
            },
        );
        lines.push(format!("scenario {} ({})", scenario.id, scenario.name));
        lines.push(format!(
            "policy decision {}  attempts {}",
            self.last_reason,
            self.attempt
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
                paint_current(fb, &self.entries, &self.visible, &self.state, &lines);
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
}

fn now_unix() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as i64)
}
