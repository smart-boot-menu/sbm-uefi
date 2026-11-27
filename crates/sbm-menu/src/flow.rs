//! The picker's startup and launch logic, shared by the UEFI app and the
//! native simulator. Platform code supplies volumes, clocks, variables and
//! `LoadImage`/`StartImage`; everything that decides what to boot, what to
//! persist, and what Core receives lives here.

use alloc::string::String;
use alloc::vec::Vec;

use sbm_boot::{
    AcceptedRequest, Confirmation, DiscoveredTarget, Integrity, ParsedLoadOption,
    RECOVERY_TARGET_ID, RemoteDecision, RequestProblem, ResolveInput, Resolved, VERIFY_CHUNK_BYTES,
    Volume, add_load_options, atomic_write, begin_attempt, build_handoff, check_boot_request,
    compose_cmdline, confirm_attempt, decode_attempt, decode_boot_request, decode_policy,
    decode_success, discover_recovery, discover_volume, embedded_cmdline, encode_attempt,
    encode_handoff_with_digest, encode_policy, evaluate_remote, fresh_policy,
    mark_one_shot_consumed, menu, merge_load_options, new_attempt_id, read_durable,
    recovery_for_slot, resolve, selectable, unique_selectable, verify_image,
};
use sbm_gfx::{MenuState, Screen};
use sbm_protocol::{
    BootAttemptRecord, BootAttemptStatus, BootPolicy, BootTimingMetrics, NetworkHandoffHint,
    RecoveryLaunchMode,
};
use sbm_types::{BootTargetId, BootTargetKind, CpuArch, PlatformCapabilities, SecureBootState};

use crate::{
    MenuEntry, StartFailure, VERSION, boot_failure, card_position, countdown_ms, sort_entries,
    visible_positions,
};

pub const POLICY_PATH: &str = "\\EFI\\sbm\\policy.json";
pub const ATTEMPT_PATH: &str = "\\EFI\\sbm\\state\\last_attempt.json";
pub const HANDOFF_PATH: &str = "\\EFI\\sbm\\handoff\\current.json";
pub const SUCCESS_PATH: &str = "\\EFI\\sbm\\state\\boot-success.json";
pub const REQUEST_PATH: &str = "\\EFI\\sbm\\handoff\\requested-command.json";
pub const HANDOFF_VERSION: &str = "1.0";

/// Spec 03 §8 volatile variables. The first two use the systemd Boot Loader
/// Interface vendor GUID, the rest the SBM vendor GUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandoffVar {
    LoaderDevicePartUuid,
    LoaderEntrySelected,
    SbmHandoffVersion,
    SbmBootAttemptId,
    SbmHandoffSha256,
}

impl HandoffVar {
    pub const SBM: [HandoffVar; 3] = [
        HandoffVar::SbmHandoffVersion,
        HandoffVar::SbmBootAttemptId,
        HandoffVar::SbmHandoffSha256,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HandoffVar::LoaderDevicePartUuid => "LoaderDevicePartUUID",
            HandoffVar::LoaderEntrySelected => "LoaderEntrySelected",
            HandoffVar::SbmHandoffVersion => "SbmHandoffVersion",
            HandoffVar::SbmBootAttemptId => "SbmBootAttemptId",
            HandoffVar::SbmHandoffSha256 => "SbmHandoffSha256",
        }
    }

    /// Vendor GUID the variable is published under.
    pub fn vendor_guid(self) -> &'static str {
        if self.systemd_vendor() {
            "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"
        } else {
            "d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40"
        }
    }

    pub const ALL: [HandoffVar; 5] = [
        HandoffVar::LoaderDevicePartUuid,
        HandoffVar::LoaderEntrySelected,
        HandoffVar::SbmHandoffVersion,
        HandoffVar::SbmBootAttemptId,
        HandoffVar::SbmHandoffSha256,
    ];

    pub fn systemd_vendor(self) -> bool {
        matches!(
            self,
            HandoffVar::LoaderDevicePartUuid | HandoffVar::LoaderEntrySelected
        )
    }
}

/// What the picker needs from firmware or from the simulated computer.
pub trait Platform {
    fn volume_count(&self) -> usize;
    fn volume(&self, index: usize) -> &dyn Volume;
    /// The ESP the picker itself was loaded from, if it could be identified.
    fn boot_volume(&self) -> Option<usize>;
    fn arch(&self) -> CpuArch;
    /// Paths on `index` that must not be listed (the running picker).
    fn skip_paths(&self, index: usize) -> Vec<String>;
    /// Parsed firmware `Boot####` entries.
    fn load_options(&self) -> Vec<ParsedLoadOption>;
    fn now_unix(&self) -> Option<i64>;
    fn elapsed_us(&self) -> u64;
    fn mark(&self, text: &str);
    /// 16 bytes from a hardware or firmware RNG, when one exists.
    fn random(&mut self) -> Option<[u8; 16]>;
    /// Monotonic count, RTC and cycle counter for the derived-ID fallback.
    fn entropy(&self) -> Vec<u8>;
    fn capabilities(&self) -> PlatformCapabilities;
    fn network_link(&self) -> bool;
    fn set_var(&mut self, var: HandoffVar, value: &str) -> bool;
    fn delete_var(&mut self, var: HandoffVar);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub start_us: u64,
    pub discovery_us: u64,
    pub decided_us: u64,
    pub first_frame_us: u64,
}

/// Everything platform code needs to call `LoadImage` and `StartImage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub entry: usize,
    pub volume: usize,
    pub path: String,
    pub name: String,
    pub target_id: BootTargetId,
    pub kind: BootTargetKind,
    /// Always `None` for recovery; the UKI's signed `.cmdline` applies.
    pub load_options: Option<String>,
    /// The exact `current.json` bytes for a recovery launch.
    pub handoff: Option<Vec<u8>>,
    pub attempt_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchResult {
    /// `LoadImage` failed; nothing ran.
    LoadFailed {
        failure: StartFailure,
        status: String,
    },
    /// `StartImage` returned control to the picker.
    Returned {
        success: bool,
        failure: StartFailure,
        status: String,
    },
}

pub struct Picker {
    pub entries: Vec<MenuEntry>,
    pub visible: Vec<usize>,
    pub policy: BootPolicy,
    pub attempt: Option<BootAttemptRecord>,
    pub resolved: Resolved,
    pub timings: Timings,
    /// A validated Core request waiting to be launched.
    request: Option<AcceptedRequest>,
    verify_buf: Vec<u8>,
    /// Latest integrity facts for the diagnostics screen.
    pub integrity_note: Option<String>,
}

impl Picker {
    /// Discover, load state from the boot ESP, apply Core's success report
    /// and boot request, and resolve policy. `user_key` is polled once,
    /// after discovery, for a key held by the person at the machine.
    pub fn start(platform: &mut dyn Platform, user_key: &mut dyn FnMut() -> bool) -> Picker {
        let start_us = platform.elapsed_us();
        let entries = discover_all(platform);
        let discovery_us = platform.elapsed_us();
        platform.mark(&alloc::format!(
            "discovery_done us={discovery_us} count={}",
            entries.len()
        ));
        let user_key_held = user_key();
        for entry in &entries {
            if let Some(problem) = &entry.item.problem {
                platform.mark(&alloc::format!(
                    "target {} ineligible: {problem}",
                    entry.item.target.id.as_str()
                ));
            }
        }
        let boot = platform.boot_volume();
        let policy = load_policy(platform, boot);
        let mut attempt = load_attempt(platform, boot);
        apply_success(platform, boot, &mut attempt);
        let targets = entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let request = take_request(platform, boot, attempt.as_ref(), &targets);
        let decision = remote_decision(platform, &policy, &targets, user_key_held);
        // There is no signed remote channel in 0.0.1, so an enrolled policy
        // always sees a failed refresh and honours `offline_behavior`.
        let remote_refresh_failed = policy.allow_remote_boot_select
            && !matches!(
                decision,
                RemoteDecision::Skip {
                    reason: "local-key",
                    ..
                } | RemoteDecision::Accept(_)
            );
        let mut resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: attempt.as_ref(),
            now_unix: platform.now_unix(),
            user_key_held,
            remote_refresh_failed,
        });
        apply_remote(platform, decision, &targets, &mut resolved);
        let mut picker = Picker {
            visible: visible_positions(&entries),
            entries,
            policy,
            attempt,
            resolved,
            timings: Timings {
                start_us,
                discovery_us,
                decided_us: 0,
                first_frame_us: 0,
            },
            request: None,
            verify_buf: Vec::new(),
            integrity_note: None,
        };
        match request {
            Some(Ok(accepted)) if !user_key_held => {
                platform.mark(&alloc::format!(
                    "request accepted {}",
                    accepted.target_id.as_str()
                ));
                picker.resolved = Resolved {
                    highlight: Some(accepted.target_id.clone()),
                    countdown_seconds: Some(0),
                    consume_one_shot: false,
                    reason: "core-request",
                    notice: None,
                };
                picker.request = Some(accepted);
            }
            Some(Ok(_)) => platform.mark("request ignored local-key"),
            Some(Err(problem)) => {
                platform.mark(&alloc::format!("request rejected {}", problem.code()));
                let highlight = picker.resolved.highlight.clone();
                picker.resolved = menu(highlight, "request-invalid");
                picker.resolved.notice = Some(problem.describe());
            }
            None => {}
        }
        if picker.resolved.reason == "one-shot-invalid" {
            platform.mark("one-shot rejected");
            mark_one_shot_consumed(&mut picker.policy);
            if !picker.persist_policy(platform) {
                platform.mark("persist policy failed");
            }
        }
        picker.timings.decided_us = platform.elapsed_us();
        platform.mark(&alloc::format!(
            "policy reason={} us={}",
            picker.resolved.reason,
            picker.timings.decided_us
        ));
        picker
    }

    /// The menu state to show first. A countdown runs only when its target is
    /// a visible card, so an unknown target can never boot card 0.
    pub fn initial_state(&self, dark: bool) -> MenuState {
        let card = self
            .resolved
            .highlight
            .as_ref()
            .and_then(|id| card_position(&self.entries, &self.visible, id));
        let countdown = if card.is_some() {
            countdown_ms(self.resolved.countdown_seconds)
        } else {
            None
        };
        let mut state = MenuState::new(card.unwrap_or(0), dark, countdown);
        if let Some(notice) = &self.resolved.notice {
            state.screen = Screen::Error;
            state.message = notice.clone();
            state.countdown_ms = None;
        }
        state
    }

    /// The entry index for the visible card `card`.
    pub fn entry_for_card(&self, card: usize) -> Option<usize> {
        self.visible.get(card).copied()
    }

    /// Checks, verification and persistence before `LoadImage`. An `Err`
    /// is the message to show; nothing was started.
    pub fn prepare(
        &mut self,
        platform: &mut dyn Platform,
        index: usize,
        first_frame_us: u64,
    ) -> Result<LaunchPlan, String> {
        let Some(found) = self.entries.get(index) else {
            return Err(String::from("That boot entry is no longer available"));
        };
        let item = found.item.clone();
        let disk = found.disk;
        if !selectable(&item) {
            return Err(crate::unhealthy_message(
                &item.target.display_name,
                item.problem.as_deref(),
            ));
        }
        let name = item.target.display_name.clone();
        let id = item.target.id.clone();
        let kind = item.target.kind;
        let path = item
            .target
            .efi_path
            .clone()
            .ok_or_else(|| alloc::format!("{name} has no EFI path"))?;
        let boot = platform.boot_volume();
        let recovery = kind == BootTargetKind::SbmCore;
        if recovery {
            if boot != Some(disk) {
                return Err(alloc::format!(
                    "{name} is not on the boot medium and was not started."
                ));
            }
            self.verify(platform, index)?;
        }
        let preset = self
            .request
            .take()
            .filter(|request| request.target_id == id)
            .and_then(|request| request.preset);
        if self
            .policy
            .one_shot
            .as_ref()
            .is_some_and(|shot| shot.target_id == id && !shot.consumed)
        {
            mark_one_shot_consumed(&mut self.policy);
            if !self.persist_policy(platform) {
                platform.mark("persist policy failed");
                return Err(alloc::format!(
                    "{name} was not started because the one-time boot could not be recorded as used."
                ));
            }
        }
        let previous = self
            .attempt
            .as_ref()
            .map(|record| record.attempt_id.clone());
        let random = platform.random();
        if random.is_none() {
            platform.mark("attempt-id source=derived");
        }
        let attempt_id = new_attempt_id(random, &platform.entropy(), previous.as_deref());
        let record = begin_attempt(self.attempt.as_ref(), &id, platform.now_unix(), &attempt_id);
        let persisted = persist(platform, boot, ATTEMPT_PATH, encode_attempt(&record).ok());
        if !persisted {
            platform.mark("persist attempt failed");
            if recovery {
                return Err(alloc::format!(
                    "{name} was not started because the boot attempt could not be saved to the boot medium."
                ));
            }
        }
        self.attempt = Some(record);
        let esp_guid = boot
            .map(|index| platform.volume(index).partition_guid().to_ascii_lowercase())
            .filter(|guid| is_guid(guid));
        let mut handoff = None;
        let load_options = if recovery {
            let esp_guid = esp_guid.ok_or_else(|| {
                alloc::format!(
                    "{name} was not started because the boot medium has no GPT partition UUID."
                )
            })?;
            let (bytes, digest) =
                self.write_handoff(platform, boot, &attempt_id, first_frame_us)?;
            let published = [
                (HandoffVar::LoaderDevicePartUuid, esp_guid.as_str()),
                (HandoffVar::LoaderEntrySelected, RECOVERY_TARGET_ID),
                (HandoffVar::SbmHandoffVersion, HANDOFF_VERSION),
                (HandoffVar::SbmBootAttemptId, attempt_id.as_str()),
                (HandoffVar::SbmHandoffSha256, digest.as_str()),
            ]
            .iter()
            .all(|(var, value)| platform.set_var(*var, value));
            if !published {
                clear_sbm_vars(platform);
                platform.mark("handoff vars failed");
                return Err(alloc::format!(
                    "{name} was not started because the firmware refused the handoff variables."
                ));
            }
            platform.mark(&alloc::format!(
                "handoff published attempt={attempt_id} sha256={digest}"
            ));
            handoff = Some(bytes);
            None
        } else {
            clear_sbm_vars(platform);
            if let Some(guid) = esp_guid.as_deref() {
                let _ = platform.set_var(HandoffVar::LoaderDevicePartUuid, guid);
            }
            let _ = platform.set_var(HandoffVar::LoaderEntrySelected, id.as_str());
            let extra = compose_cmdline(item.target.kernel_cmdline.as_deref(), preset.as_deref());
            let embedded = embedded_cmdline(platform.volume(disk), &path);
            let (options, mode) = merge_load_options(embedded.as_deref(), extra.as_deref());
            platform.mark(&alloc::format!("load-options {mode}"));
            options
        };
        platform.mark(&alloc::format!("chainload {name}"));
        Ok(LaunchPlan {
            entry: index,
            volume: disk,
            path,
            name,
            target_id: id,
            kind,
            load_options,
            handoff,
            attempt_id,
        })
    }

    /// `LoadImage` succeeded. Record what firmware verified.
    pub fn loaded(&mut self, platform: &mut dyn Platform, plan: &LaunchPlan) {
        let secure = platform.capabilities().secure_boot_state == SecureBootState::Enabled;
        let verdict = if secure {
            "firmware-verified"
        } else {
            "firmware-not-verified"
        };
        platform.mark(&alloc::format!(
            "integrity {verdict} target={}",
            plan.target_id.as_str()
        ));
        if plan.kind == BootTargetKind::SbmCore {
            let note = if secure {
                "Secure Boot verified the recovery image."
            } else {
                "Secure Boot is off, so firmware did not verify the recovery image."
            };
            self.integrity_note = Some(String::from(note));
        }
    }

    /// Control came back from `LoadImage` or `StartImage`. Returns the
    /// message to show, if any.
    pub fn finish(
        &mut self,
        platform: &mut dyn Platform,
        plan: &LaunchPlan,
        result: &LaunchResult,
    ) -> Option<String> {
        if plan.kind == BootTargetKind::SbmCore {
            clear_sbm_vars(platform);
        }
        if let Some(record) = self.attempt.as_mut()
            && record.attempt_id == plan.attempt_id
        {
            record.status = BootAttemptStatus::Failed;
            let bytes = encode_attempt(record).ok();
            if !persist(platform, platform.boot_volume(), ATTEMPT_PATH, bytes) {
                platform.mark("persist attempt failed");
            }
        }
        match result {
            LaunchResult::LoadFailed { failure, status } => {
                platform.mark(&alloc::format!("load_failed status={status}"));
                if *failure == StartFailure::SecurityViolation
                    && let Some(entry) = self.entries.get_mut(plan.entry)
                {
                    entry.item.target.verified_signature = Some(false);
                    entry.item.problem = Some(String::from("Secure Boot rejected it"));
                }
                Some(boot_failure(&plan.name, &plan.path, *failure, status))
            }
            LaunchResult::Returned {
                success,
                failure,
                status,
            } => {
                platform.mark(&alloc::format!("chainload_returned status={status}"));
                if *success {
                    None
                } else {
                    Some(boot_failure(&plan.name, &plan.path, *failure, status))
                }
            }
        }
    }

    fn verify(&mut self, platform: &mut dyn Platform, index: usize) -> Result<(), String> {
        let item = &self.entries[index].item;
        let name = item.target.display_name.clone();
        let path = item.target.efi_path.clone().unwrap_or_default();
        let expected = item.expected_sha256.clone();
        let disk = self.entries[index].disk;
        if self.verify_buf.len() != VERIFY_CHUNK_BYTES {
            self.verify_buf = alloc::vec![0u8; VERIFY_CHUNK_BYTES];
        }
        let started = platform.elapsed_us();
        let result = verify_image(
            platform.volume(disk),
            &path,
            expected.as_deref(),
            &mut self.verify_buf,
        );
        let elapsed = platform.elapsed_us().saturating_sub(started);
        let entry = &mut self.entries[index].item;
        match result {
            Ok(done) => {
                platform.mark(&alloc::format!(
                    "verify_done us={elapsed} bytes={} reads={} chunk={} result=ok",
                    done.bytes_read,
                    done.reads,
                    VERIFY_CHUNK_BYTES
                ));
                platform.mark(&alloc::format!("integrity verified sha256={}", done.digest));
                entry.integrity = Integrity::Verified;
                self.integrity_note = Some(alloc::format!(
                    "recovery SHA-256 matched the manifest ({} bytes, {elapsed} us)",
                    done.bytes_read
                ));
                Ok(())
            }
            Err(err) => {
                let why = err.describe();
                platform.mark(&alloc::format!(
                    "verify_done us={elapsed} result=failed reason={why}"
                ));
                entry.integrity = Integrity::Failed;
                entry.problem = Some(why.clone());
                self.integrity_note = Some(alloc::format!("recovery verification failed: {why}"));
                Err(alloc::format!(
                    "{name} was not started: {why}. It stays disabled until the computer restarts."
                ))
            }
        }
    }

    fn write_handoff(
        &self,
        platform: &mut dyn Platform,
        boot: Option<usize>,
        attempt_id: &str,
        first_frame_us: u64,
    ) -> Result<(Vec<u8>, String), String> {
        let targets = self
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let payload = build_handoff(
            attempt_id,
            VERSION,
            platform.capabilities(),
            RecoveryLaunchMode::Interactive,
            None,
            &targets,
            BootTimingMetrics {
                firmware_init_us: None,
                uefi_entry_us: 0,
                first_frame_us: Some(first_frame_us),
                network_us: Some(0),
                handoff_us: platform.elapsed_us(),
            },
            Some(NetworkHandoffHint {
                link_up: platform.network_link(),
                dhcp_acquired: false,
                server_reachable: None,
            }),
        );
        let failed = || {
            String::from(
                "SBM Core was not started because the handoff file could not be saved to the boot medium.",
            )
        };
        let (bytes, digest) = encode_handoff_with_digest(&payload).map_err(|_| failed())?;
        if !persist(platform, boot, HANDOFF_PATH, Some(bytes.clone())) {
            platform.mark("persist handoff failed");
            return Err(failed());
        }
        Ok((bytes, digest))
    }

    fn persist_policy(&self, platform: &mut dyn Platform) -> bool {
        persist(
            platform,
            platform.boot_volume(),
            POLICY_PATH,
            encode_policy(&self.policy).ok(),
        )
    }
}

/// Discovery over every volume; recovery only on the boot ESP. Sorted.
pub fn discover_all(platform: &dyn Platform) -> Vec<MenuEntry> {
    let arch = platform.arch();
    let options = platform.load_options();
    let boot = platform.boot_volume();
    let mut entries = Vec::new();
    for index in 0..platform.volume_count() {
        let vol = platform.volume(index);
        let skip = platform.skip_paths(index);
        let mut found = discover_volume(vol, arch, &skip);
        add_load_options(vol, &options, arch, &skip, &mut found);
        if boot == Some(index)
            && let Some(recovery) = discover_recovery(vol, arch)
        {
            found.push(recovery);
        }
        for item in found {
            entries.push(MenuEntry { disk: index, item });
        }
    }
    sort_entries(&mut entries);
    entries
}

fn load_policy(platform: &dyn Platform, boot: Option<usize>) -> BootPolicy {
    let Some(boot) = boot else {
        return fresh_policy();
    };
    match read_durable(platform.volume(boot), POLICY_PATH, decode_policy) {
        Some(Ok(policy)) => policy,
        Some(Err(_)) => {
            platform.mark("policy rejected");
            fresh_policy()
        }
        None => fresh_policy(),
    }
}

/// A malformed or interrupted record is treated as an unconfirmed attempt of
/// unknown target: it confirms nothing and binds no request.
fn load_attempt(platform: &dyn Platform, boot: Option<usize>) -> Option<BootAttemptRecord> {
    match read_durable(platform.volume(boot?), ATTEMPT_PATH, decode_attempt)? {
        Ok(record) => Some(record),
        Err(_) => {
            platform.mark("attempt record rejected");
            None
        }
    }
}

/// Spec 02 §7: only the boot ESP's report, only for the current Core
/// attempt. Stale reports are deleted; a valid one is deleted once the
/// confirmed record is saved.
fn apply_success(
    platform: &mut dyn Platform,
    boot: Option<usize>,
    attempt: &mut Option<BootAttemptRecord>,
) {
    let Some(boot) = boot else {
        return;
    };
    let Ok(bytes) = platform.volume(boot).read(SUCCESS_PATH) else {
        return;
    };
    let mut reconciled = attempt.clone();
    let verdict = match (decode_success(&bytes), reconciled.as_mut()) {
        (Err(_), _) => Confirmation::Stale("malformed"),
        (Ok(_), None) => Confirmation::Stale("no-attempt"),
        (Ok(report), Some(record)) => confirm_attempt(record, &report),
    };
    match verdict {
        Confirmation::Confirmed => {
            let record = reconciled.as_ref().map(|record| record.attempt_id.clone());
            let bytes = reconciled
                .as_ref()
                .and_then(|record| encode_attempt(record).ok());
            if persist(platform, Some(boot), ATTEMPT_PATH, bytes) {
                *attempt = reconciled;
                platform.mark(&alloc::format!(
                    "attempt confirmed {}",
                    record.unwrap_or_default()
                ));
                let _ = platform.volume(boot).remove(SUCCESS_PATH);
            } else {
                platform.mark("persist attempt failed");
            }
        }
        Confirmation::AlreadyConfirmed => {
            platform.mark("attempt confirm repeated");
            let _ = platform.volume(boot).remove(SUCCESS_PATH);
        }
        Confirmation::Stale(why) => {
            platform.mark(&alloc::format!("attempt confirm rejected {why}"));
            let _ = platform.volume(boot).remove(SUCCESS_PATH);
        }
    }
}

/// Spec 02 §5a: read from the boot ESP only, delete before acting, then
/// validate. `None` when there is no request.
fn take_request(
    platform: &mut dyn Platform,
    boot: Option<usize>,
    attempt: Option<&BootAttemptRecord>,
    targets: &[DiscoveredTarget],
) -> Option<Result<AcceptedRequest, RequestProblem>> {
    let vol = platform.volume(boot?);
    let bytes = match vol.read(REQUEST_PATH) {
        Ok(bytes) => bytes,
        Err(sbm_boot::VolError::NotFound) => return None,
        Err(sbm_boot::VolError::Io) => return Some(Err(RequestProblem::Unreadable)),
    };
    if vol.remove(REQUEST_PATH).is_err()
        || !matches!(vol.read(REQUEST_PATH), Err(sbm_boot::VolError::NotFound))
    {
        return Some(Err(RequestProblem::Unreadable));
    }
    let Ok(request) = decode_boot_request(&bytes) else {
        return Some(Err(RequestProblem::Malformed));
    };
    Some(check_boot_request(&request, attempt, targets))
}

/// No signed remote channel exists in 0.0.1, so this only records why the
/// network was skipped. An accepted remote target is validated like a local
/// request and never boots an unrelated entry.
fn remote_decision(
    platform: &dyn Platform,
    policy: &BootPolicy,
    targets: &[DiscoveredTarget],
    user_key_held: bool,
) -> RemoteDecision {
    evaluate_remote(
        policy.allow_remote_boot_select,
        policy.network_policy_timeout_ms,
        platform.network_link(),
        user_key_held,
        None,
        |id| unique_selectable(targets, id).is_ok(),
        "sbm-uefi",
        0,
        platform.now_unix(),
    )
}

fn apply_remote(
    platform: &dyn Platform,
    decision: RemoteDecision,
    targets: &[DiscoveredTarget],
    resolved: &mut Resolved,
) {
    match decision {
        RemoteDecision::Skip { reason, .. } => platform.mark(&alloc::format!("remote {reason}")),
        RemoteDecision::Reject(_) => platform.mark("remote rejected"),
        RemoteDecision::Accept(applied) => {
            platform.mark("remote accepted");
            let id = match applied {
                sbm_boot::Applied::Boot { id, .. } => Some(id),
                sbm_boot::Applied::Recovery(slot) => {
                    recovery_for_slot(targets, slot).map(|found| found.target.id.clone())
                }
                _ => None,
            };
            match id.filter(|id| unique_selectable(targets, id).is_ok()) {
                Some(id) => {
                    resolved.highlight = Some(id);
                    resolved.countdown_seconds = Some(0);
                }
                None => resolved.countdown_seconds = None,
            }
        }
    }
}

fn clear_sbm_vars(platform: &mut dyn Platform) {
    for var in HandoffVar::SBM {
        platform.delete_var(var);
    }
}

fn persist(
    platform: &dyn Platform,
    boot: Option<usize>,
    path: &str,
    bytes: Option<Vec<u8>>,
) -> bool {
    match (boot, bytes) {
        (Some(boot), Some(bytes)) => atomic_write(platform.volume(boot), path, &bytes).is_ok(),
        _ => false,
    }
}

fn is_guid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}
