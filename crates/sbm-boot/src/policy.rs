use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbm_protocol::{
    BootAttemptRecord, BootAttemptStatus, BootCommand, BootPolicy, BootSuccessReport,
    BootTimingMetrics, EfiHandoffPayload, LocalBootRequest, NetworkHandoffHint, OfflineBehavior,
    RecoveryLaunchMode, format_uuid_v4, is_uuid_v4,
};
use sbm_types::{
    BootTarget, BootTargetId, BootTargetKind, ComponentKind, PlatformCapabilities,
    SCHEMA_VERSION_V1,
};

use crate::discover::{DiscoveredTarget, Integrity, RECOVERY_TARGET_ID};
use crate::pathutil::hex_encode;
use crate::pathutil::sha256;

pub use crate::persist::{
    CodecError, atomic_write, decode_attempt, decode_boot_request, decode_handoff, decode_manifest,
    decode_policy, decode_success, encode_attempt, encode_boot_request, encode_handoff,
    encode_manifest, encode_policy, encode_success, read_durable,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub highlight: Option<BootTargetId>,
    /// `None` waits. `Some(0)` boots immediately. Any other value is a countdown.
    pub countdown_seconds: Option<u16>,
    pub consume_one_shot: bool,
    pub reason: &'static str,
    /// Shown instead of the menu when a request could not be honoured.
    pub notice: Option<String>,
}

pub fn fresh_policy() -> BootPolicy {
    BootPolicy {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        policy_revision: 0,
        default_target: None,
        fallback_target: None,
        timeout_seconds: 65535,
        one_shot: None,
        max_failed_attempts: 3,
        network_policy_timeout_ms: 3000,
        allow_remote_boot_select: false,
        offline_behavior: OfflineBehavior::UseLocalPolicy,
    }
}

pub fn selectable(target: &DiscoveredTarget) -> bool {
    !target.target.hidden
        && target.healthy
        && target.integrity != Integrity::Failed
        && target.target.verified_signature != Some(false)
}

/// Failures carried by `record`. Only Core reports success in 0.0.1, so a
/// `started` attempt of any other target means "handed off, outcome
/// unknown" and is not counted; only a `StartImage` return (`failed`) is.
/// Without this, every clean OS boot would count towards fallback.
pub fn effective_failures(record: Option<&BootAttemptRecord>) -> u8 {
    match record {
        Some(record) if record.status == BootAttemptStatus::Failed => record.consecutive_failures,
        Some(record)
            if record.status == BootAttemptStatus::Started
                && reports_success(&record.target_id) =>
        {
            record.consecutive_failures
        }
        _ => 0,
    }
}

/// Targets with a 0.0.1 success reporter.
fn reports_success(target: &BootTargetId) -> bool {
    target.as_str() == RECOVERY_TARGET_ID
}

/// Spec 02 §7: the count grows only across unconfirmed attempts of the same
/// target. Any other target starts at one.
pub fn begin_attempt(
    previous: Option<&BootAttemptRecord>,
    target: &BootTargetId,
    now_unix: Option<i64>,
    attempt_id: &str,
) -> BootAttemptRecord {
    let slot = target
        .as_str()
        .strip_prefix("sbm-core:")
        .map(ToString::to_string);
    let prior = previous
        .filter(|record| &record.target_id == target)
        .map(|record| effective_failures(Some(record)))
        .unwrap_or(0);
    BootAttemptRecord {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        attempt_id: attempt_id.to_string(),
        target_id: target.clone(),
        slot,
        timestamp_unix: now_unix,
        consecutive_failures: prior.saturating_add(1),
        status: BootAttemptStatus::Started,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// The current `started` recovery attempt is now `confirmed`.
    Confirmed,
    /// The report repeats an earlier confirmation of the same attempt.
    AlreadyConfirmed,
    /// The report does not belong to the current attempt; delete it.
    Stale(&'static str),
}

/// Spec 02 §7 strict reconciliation. Only Core confirms, only its own
/// current attempt, and there is no wildcard attempt ID.
pub fn confirm_attempt(record: &mut BootAttemptRecord, report: &BootSuccessReport) -> Confirmation {
    if report.schema_version != SCHEMA_VERSION_V1 {
        return Confirmation::Stale("unsupported-schema");
    }
    if report.confirmed_by != ComponentKind::SbmCoreUki {
        return Confirmation::Stale("reporter");
    }
    if report.target_id.as_str() != RECOVERY_TARGET_ID
        || record.target_id.as_str() != RECOVERY_TARGET_ID
    {
        return Confirmation::Stale("target");
    }
    if report.attempt_id != record.attempt_id || !is_uuid_v4(&record.attempt_id) {
        return Confirmation::Stale("attempt");
    }
    match record.status {
        BootAttemptStatus::Started => {
            record.status = BootAttemptStatus::Confirmed;
            record.consecutive_failures = 0;
            Confirmation::Confirmed
        }
        BootAttemptStatus::Confirmed => Confirmation::AlreadyConfirmed,
        BootAttemptStatus::Failed | BootAttemptStatus::RolledBack => {
            Confirmation::Stale("attempt-ended")
        }
    }
}

/// Why a Spec 02 §5a request or a policy one-shot was not honoured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestProblem {
    Unreadable,
    Malformed,
    Stale,
    Unsupported,
    Missing(BootTargetId),
    Ambiguous(BootTargetId),
    Unavailable(String),
    Expired,
}

impl RequestProblem {
    pub fn code(&self) -> &'static str {
        match self {
            RequestProblem::Unreadable => "unreadable",
            RequestProblem::Malformed => "malformed",
            RequestProblem::Stale => "stale",
            RequestProblem::Unsupported => "unsupported",
            RequestProblem::Missing(_) => "missing",
            RequestProblem::Ambiguous(_) => "ambiguous",
            RequestProblem::Unavailable(_) => "unavailable",
            RequestProblem::Expired => "expired",
        }
    }

    pub fn describe(&self) -> String {
        let detail = match self {
            RequestProblem::Unreadable => {
                String::from("The boot request file could not be removed, so it was not used.")
            }
            RequestProblem::Malformed => {
                String::from("The boot request is not in a format this menu accepts.")
            }
            RequestProblem::Stale => {
                String::from("The boot request does not belong to the current SBM Core session.")
            }
            RequestProblem::Unsupported => {
                String::from("The boot request asks for something this menu does not do.")
            }
            RequestProblem::Missing(id) => alloc::format!(
                "The requested boot entry ({}) is not present on this computer now.",
                id.as_str()
            ),
            RequestProblem::Ambiguous(id) => alloc::format!(
                "More than one boot entry has the identifier {}.",
                id.as_str()
            ),
            RequestProblem::Unavailable(name) => {
                alloc::format!("The requested boot entry {name} cannot be started now.")
            }
            RequestProblem::Expired => String::from("The one-time boot request has expired."),
        };
        alloc::format!("{detail} Nothing was started. Choose an entry to continue.")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedRequest {
    pub target_id: BootTargetId,
    pub preset: Option<String>,
}

/// Exactly one current target with `id`, and it is selectable.
pub fn unique_selectable(
    targets: &[DiscoveredTarget],
    id: &BootTargetId,
) -> Result<usize, RequestProblem> {
    let mut matches = targets
        .iter()
        .enumerate()
        .filter(|(_, target)| &target.target.id == id);
    let Some((index, found)) = matches.next() else {
        return Err(RequestProblem::Missing(id.clone()));
    };
    if matches.next().is_some() {
        return Err(RequestProblem::Ambiguous(id.clone()));
    }
    if !selectable(found) {
        return Err(RequestProblem::Unavailable(
            found.target.display_name.clone(),
        ));
    }
    Ok(index)
}

/// Spec 02 §5a checks 1-4. `record` is the attempt after the success report
/// was applied.
pub fn check_boot_request(
    request: &LocalBootRequest,
    record: Option<&BootAttemptRecord>,
    targets: &[DiscoveredTarget],
) -> Result<AcceptedRequest, RequestProblem> {
    if request.schema_version != SCHEMA_VERSION_V1
        || request.requested_by != ComponentKind::SbmCoreUki
        || !is_uuid_v4(&request.request_id)
    {
        return Err(RequestProblem::Malformed);
    }
    let bound = record.is_some_and(|record| {
        record.attempt_id == request.attempt_id
            && is_uuid_v4(&record.attempt_id)
            && record.target_id.as_str() == RECOVERY_TARGET_ID
            && record.status == BootAttemptStatus::Confirmed
    });
    if !bound {
        return Err(RequestProblem::Stale);
    }
    let BootCommand::BootTarget {
        target_id,
        one_shot: true,
        extra_cmdline_preset,
    } = &request.command
    else {
        return Err(RequestProblem::Unsupported);
    };
    let preset = match extra_cmdline_preset.as_deref() {
        None => None,
        Some(name) if allowlisted_preset(name).is_some() => Some(name.to_string()),
        Some(_) => return Err(RequestProblem::Unsupported),
    };
    let index = unique_selectable(targets, target_id)?;
    if targets[index].target.kind == BootTargetKind::SbmCore {
        return Err(RequestProblem::Unsupported);
    }
    Ok(AcceptedRequest {
        target_id: target_id.clone(),
        preset,
    })
}

/// A fresh UUIDv4 attempt ID (Spec 02 §7). `random` comes from
/// `EFI_RNG_PROTOCOL` when firmware has it; otherwise the bits are derived
/// from `entropy` and the previous ID. The result never repeats `previous`.
pub fn new_attempt_id(random: Option<[u8; 16]>, entropy: &[u8], previous: Option<&str>) -> String {
    let mut counter = 0u32;
    loop {
        let bytes = match random {
            Some(bytes) if counter == 0 => bytes,
            _ => {
                let mut seed = Vec::with_capacity(entropy.len() + 64);
                seed.extend_from_slice(b"sbm-attempt-id\0");
                seed.extend_from_slice(entropy);
                seed.extend_from_slice(previous.unwrap_or("").as_bytes());
                seed.extend_from_slice(&counter.to_le_bytes());
                let digest = sha256(&seed);
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(&digest[..16]);
                bytes
            }
        };
        let id = format_uuid_v4(bytes);
        if Some(id.as_str()) != previous {
            return id;
        }
        counter = counter.wrapping_add(1);
    }
}

/// systemd-stub replaces the embedded `.cmdline` with EFI load options when
/// Secure Boot is off, and ignores those options when Secure Boot is on and
/// `.cmdline` exists. Repeating the embedded line keeps the UKI's own tokens
/// in the first case and is ignored in the second. Recovery is started with
/// no load options at all (Spec 03 §9).
pub fn merge_load_options(
    embedded: Option<&str>,
    extra: Option<&str>,
) -> (Option<String>, &'static str) {
    let embedded = blank(embedded);
    let extra = blank(extra);
    match (embedded, extra) {
        (Some(embedded), Some(extra)) if tokens_cover(embedded, extra) => {
            (Some(embedded.to_string()), "embedded")
        }
        (Some(embedded), Some(extra)) => (
            Some(alloc::format!("{embedded} {extra}")),
            "embedded-repeated",
        ),
        (Some(embedded), None) => (Some(embedded.to_string()), "embedded"),
        (None, Some(extra)) => (Some(extra.to_string()), "external"),
        (None, None) => (None, "none"),
    }
}

fn blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

fn tokens_cover(line: &str, extra: &str) -> bool {
    extra
        .split_whitespace()
        .all(|token| line.split_whitespace().any(|have| have == token))
}

pub fn mark_one_shot_consumed(policy: &mut BootPolicy) {
    if let Some(one_shot) = policy.one_shot.as_mut() {
        one_shot.consumed = true;
    }
}

/// Spec 02 §5 canonical preset allowlist (`v1`).
pub fn allowlisted_preset(name: &str) -> Option<&'static str> {
    match name {
        "quiet" => Some("quiet"),
        "verbose" => Some("verbose"),
        "nomodeset" => Some("nomodeset"),
        "diagnostic" => Some("sbm.diagnostic=1"),
        _ => None,
    }
}

pub fn compose_cmdline(base: Option<&str>, preset: Option<&str>) -> Option<String> {
    let preset = preset.and_then(allowlisted_preset);
    match (base.filter(|text| !text.is_empty()), preset) {
        (Some(base), Some(preset)) => Some(alloc::format!("{base} {preset}")),
        (Some(base), None) => Some(base.to_string()),
        (None, Some(preset)) => Some(preset.to_string()),
        (None, None) => None,
    }
}

pub struct ResolveInput<'a> {
    pub policy: &'a BootPolicy,
    pub targets: &'a [DiscoveredTarget],
    pub attempt: Option<&'a BootAttemptRecord>,
    pub now_unix: Option<i64>,
    pub user_key_held: bool,
    pub remote_refresh_failed: bool,
}

pub fn resolve(input: ResolveInput<'_>) -> Resolved {
    if input.user_key_held {
        return menu(first_id(input.targets, input.policy), "local-override");
    }
    match one_shot_target(input.policy, input.targets, input.now_unix) {
        Some(Ok(one_shot)) => {
            return Resolved {
                highlight: Some(one_shot),
                countdown_seconds: Some(0),
                consume_one_shot: true,
                reason: "one-shot",
                notice: None,
            };
        }
        Some(Err(problem)) => {
            let mut resolved = menu(first_id(input.targets, input.policy), "one-shot-invalid");
            resolved.consume_one_shot = true;
            resolved.notice = Some(problem.describe());
            return resolved;
        }
        None => {}
    }
    let suppress = input.remote_refresh_failed
        && input.policy.allow_remote_boot_select
        && input.policy.offline_behavior == OfflineBehavior::WaitForUser;
    if let Some(id) = healthy_default(input.policy, input.targets, input.attempt) {
        if suppress {
            return menu(Some(id), "wait-for-user");
        }
        return timed(Some(id), input.policy.timeout_seconds, "default");
    }
    if input.policy.max_failed_attempts > 0
        && failures_for(input.attempt, input.policy.default_target.as_ref())
            >= input.policy.max_failed_attempts
        && let Some(id) = fallback_target(input.policy, input.targets)
    {
        if suppress {
            return menu(Some(id), "fallback-wait");
        }
        return timed(Some(id), input.policy.timeout_seconds, "fallback");
    }
    menu(first_id(input.targets, input.policy), "menu")
}

fn timed(id: Option<BootTargetId>, timeout: u16, reason: &'static str) -> Resolved {
    Resolved {
        highlight: id,
        countdown_seconds: Some(timeout),
        consume_one_shot: false,
        reason,
        notice: None,
    }
}

/// The interactive menu with no countdown.
pub fn menu(id: Option<BootTargetId>, reason: &'static str) -> Resolved {
    Resolved {
        highlight: id,
        countdown_seconds: None,
        consume_one_shot: false,
        reason,
        notice: None,
    }
}

/// `None` when there is no unconsumed policy one-shot. An expired one-shot,
/// or one whose expiry cannot be checked, is not honoured.
fn one_shot_target(
    policy: &BootPolicy,
    targets: &[DiscoveredTarget],
    now_unix: Option<i64>,
) -> Option<Result<BootTargetId, RequestProblem>> {
    let one_shot = policy.one_shot.as_ref()?;
    if one_shot.consumed {
        return None;
    }
    if let Some(expires) = one_shot.expires_at_unix
        && now_unix.is_none_or(|now| now > expires)
    {
        return Some(Err(RequestProblem::Expired));
    }
    Some(unique_selectable(targets, &one_shot.target_id).map(|_| one_shot.target_id.clone()))
}

/// Failures recorded against `target`; attempts of other targets do not count.
fn failures_for(record: Option<&BootAttemptRecord>, target: Option<&BootTargetId>) -> u8 {
    match (record, target) {
        (Some(record), Some(target)) if &record.target_id == target => {
            effective_failures(Some(record))
        }
        _ => 0,
    }
}

fn healthy_default(
    policy: &BootPolicy,
    targets: &[DiscoveredTarget],
    attempt: Option<&BootAttemptRecord>,
) -> Option<BootTargetId> {
    let id = policy.default_target.as_ref()?;
    if policy.max_failed_attempts > 0
        && failures_for(attempt, Some(id)) >= policy.max_failed_attempts
    {
        return None;
    }
    let index = unique_selectable(targets, id).ok()?;
    Some(targets[index].target.id.clone())
}

fn fallback_target(policy: &BootPolicy, targets: &[DiscoveredTarget]) -> Option<BootTargetId> {
    if let Some(id) = policy.fallback_target.as_ref()
        && unique_selectable(targets, id).is_ok()
    {
        return Some(id.clone());
    }
    targets
        .iter()
        .find(|target| target.target.kind == BootTargetKind::SbmCore && selectable(target))
        .map(|target| target.target.id.clone())
}

fn first_id(targets: &[DiscoveredTarget], policy: &BootPolicy) -> Option<BootTargetId> {
    if let Some(id) = policy.default_target.as_ref()
        && targets
            .iter()
            .any(|target| &target.target.id == id && selectable(target))
    {
        return Some(id.clone());
    }
    targets
        .iter()
        .find(|target| selectable(target))
        .map(|target| target.target.id.clone())
}

/// The handoff lists only what the picker could launch now (Spec 03 §8).
#[allow(clippy::too_many_arguments)]
pub fn build_handoff(
    attempt_id: &str,
    version: &str,
    platform: PlatformCapabilities,
    launch_mode: RecoveryLaunchMode,
    pending_command_id: Option<String>,
    targets: &[DiscoveredTarget],
    timing: BootTimingMetrics,
    network: Option<NetworkHandoffHint>,
) -> EfiHandoffPayload {
    EfiHandoffPayload {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        attempt_id: attempt_id.to_string(),
        sbm_uefi_version: version.to_string(),
        platform,
        launch_mode,
        pending_command_id,
        discovered_targets: targets
            .iter()
            .filter(|item| unique_selectable(targets, &item.target.id).is_ok())
            .map(|item| item.target.clone())
            .collect(),
        boot_timing_us: Some(timing),
        network_hint: network,
    }
}

/// The exact bytes written to `current.json` and their lowercase SHA-256.
pub fn encode_handoff_with_digest(
    payload: &EfiHandoffPayload,
) -> Result<(Vec<u8>, String), CodecError> {
    let bytes = encode_handoff(payload)?;
    let digest = hex_encode(&sha256(&bytes));
    Ok((bytes, digest))
}

pub fn visible(targets: &[DiscoveredTarget]) -> Vec<&DiscoveredTarget> {
    targets
        .iter()
        .filter(|target| !target.target.hidden)
        .collect()
}

pub fn find_target<'a>(
    targets: &'a [DiscoveredTarget],
    id: &BootTargetId,
) -> Option<&'a BootTarget> {
    targets
        .iter()
        .find(|target| &target.target.id == id)
        .map(|target| &target.target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::DiscoveredTarget;
    use sbm_protocol::{OfflineBehavior, OneShotBoot};
    use sbm_types::{BootTargetKind, CpuArch, OsFamily};

    const ATTEMPT: &str = "6f1c2b9e-3d4a-4e5f-8a7b-0c1d2e3f4a5b";

    fn sample(id: &str, kind: BootTargetKind, healthy: bool) -> DiscoveredTarget {
        DiscoveredTarget {
            target: BootTarget {
                id: BootTargetId(id.to_string()),
                display_name: id.to_string(),
                os_family: if kind == BootTargetKind::SbmCore {
                    OsFamily::Recovery
                } else {
                    OsFamily::Linux
                },
                kind,
                arch: CpuArch::X86_64,
                partition_guid: None,
                efi_path: Some("\\EFI\\os.efi".into()),
                kernel_cmdline: None,
                icon_hint: None,
                removable_media: false,
                hidden: false,
                verified_signature: None,
            },
            initrd: None,
            healthy,
            preferred: id.ends_with("slot-a"),
            rank: 1,
            integrity: Integrity::Discovered,
            expected_sha256: None,
            problem: None,
        }
    }

    fn policy_with(default: Option<&str>, timeout: u16) -> BootPolicy {
        let mut policy = fresh_policy();
        policy.timeout_seconds = timeout;
        policy.default_target = default.map(|id| BootTargetId(id.into()));
        policy.fallback_target = Some(BootTargetId("sbm-core:slot-a".into()));
        policy.offline_behavior = OfflineBehavior::UseLocalPolicy;
        policy
    }

    fn input<'a>(
        policy: &'a BootPolicy,
        targets: &'a [DiscoveredTarget],
        attempt: Option<&'a BootAttemptRecord>,
    ) -> ResolveInput<'a> {
        ResolveInput {
            policy,
            targets,
            attempt,
            now_unix: Some(40),
            user_key_held: false,
            remote_refresh_failed: false,
        }
    }

    fn confirmed_core() -> BootAttemptRecord {
        let mut record = begin_attempt(
            None,
            &BootTargetId(RECOVERY_TARGET_ID.into()),
            None,
            ATTEMPT,
        );
        record.status = BootAttemptStatus::Confirmed;
        record.consecutive_failures = 0;
        record
    }

    fn request(target: &str) -> LocalBootRequest {
        LocalBootRequest {
            schema_version: "1.0".into(),
            request_id: "2b3c4d5e-6f70-4182-9a3b-4c5d6e7f8091".into(),
            attempt_id: ATTEMPT.into(),
            requested_by: ComponentKind::SbmCoreUki,
            requested_at_unix: 1,
            command: BootCommand::BootTarget {
                target_id: BootTargetId(target.into()),
                one_shot: true,
                extra_cmdline_preset: None,
            },
        }
    }

    #[test]
    fn local_key_beats_one_shot_and_default() {
        let targets = vec![
            sample("efi:disk:abcdef012345", BootTargetKind::EfiBinary, true),
            sample("sbm-core:slot-a", BootTargetKind::SbmCore, true),
        ];
        let mut policy = policy_with(Some("efi:disk:abcdef012345"), 0);
        policy.one_shot = Some(OneShotBoot {
            target_id: BootTargetId("sbm-core:slot-a".into()),
            command_id: None,
            expires_at_unix: None,
            consumed: false,
        });
        let mut held = input(&policy, &targets, None);
        held.user_key_held = true;
        let resolved = resolve(held);
        assert_eq!(resolved.reason, "local-override");
        assert_eq!(resolved.countdown_seconds, None);
        assert!(!resolved.consume_one_shot);
    }

    #[test]
    fn one_shot_then_default_then_fallback() {
        let targets = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("sbm-core:slot-a", BootTargetKind::SbmCore, true),
        ];
        let mut policy = policy_with(Some("os"), 5);
        policy.one_shot = Some(OneShotBoot {
            target_id: BootTargetId("os".into()),
            command_id: Some("cmd".into()),
            expires_at_unix: Some(50),
            consumed: false,
        });
        let resolved = resolve(input(&policy, &targets, None));
        assert_eq!(resolved.reason, "one-shot");
        assert_eq!(resolved.countdown_seconds, Some(0));
        assert!(resolved.consume_one_shot);

        policy.one_shot.as_mut().unwrap().consumed = true;
        let resolved = resolve(input(&policy, &targets, None));
        assert_eq!(resolved.reason, "default");
        assert_eq!(resolved.countdown_seconds, Some(5));

        // Three launches of the default that each returned to the picker.
        let returned = |record: BootAttemptRecord| BootAttemptRecord {
            status: BootAttemptStatus::Failed,
            ..record
        };
        let os = BootTargetId("os".into());
        let failed = returned(begin_attempt(None, &os, Some(1), "a1"));
        let failed = returned(begin_attempt(Some(&failed), &os, Some(2), "a2"));
        let failed = returned(begin_attempt(Some(&failed), &os, Some(3), "a3"));
        assert_eq!(failed.consecutive_failures, 3);
        let resolved = resolve(input(&policy, &targets, Some(&failed)));
        assert_eq!(resolved.reason, "fallback");
        assert_eq!(resolved.highlight.unwrap().as_str(), "sbm-core:slot-a");
    }

    #[test]
    fn failures_are_counted_per_target() {
        let os = BootTargetId("os".into());
        let core = BootTargetId(RECOVERY_TARGET_ID.into());
        let first = begin_attempt(None, &core, None, "a1");
        let second = begin_attempt(Some(&first), &core, None, "a2");
        assert_eq!(
            second.consecutive_failures, 2,
            "unconfirmed Core attempts count"
        );
        let mut other = begin_attempt(Some(&second), &os, None, "a3");
        assert_eq!(other.consecutive_failures, 1);
        // An OS has no 0.0.1 reporter: a clean handoff is not a failure,
        // a StartImage return is.
        assert_eq!(
            begin_attempt(Some(&other), &os, None, "a4").consecutive_failures,
            1
        );
        other.status = BootAttemptStatus::Failed;
        assert_eq!(
            begin_attempt(Some(&other), &os, None, "a5").consecutive_failures,
            2
        );
        let mut os_failed = other.clone();
        os_failed.consecutive_failures = 3;
        let mut limited = policy_with(Some("os"), 5);
        limited.max_failed_attempts = 3;
        let both = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("sbm-core:slot-a", BootTargetKind::SbmCore, true),
        ];
        assert_eq!(
            resolve(input(&limited, &both, Some(&os_failed))).reason,
            "fallback"
        );
        os_failed.status = BootAttemptStatus::Started;
        assert_eq!(
            resolve(input(&limited, &both, Some(&os_failed))).reason,
            "default"
        );
        let other = second.clone();
        let targets = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("sbm-core:slot-a", BootTargetKind::SbmCore, true),
        ];
        let mut policy = policy_with(Some("os"), 5);
        policy.max_failed_attempts = 1;
        let mut core_failed = other.clone();
        core_failed.consecutive_failures = 9;
        let resolved = resolve(input(&policy, &targets, Some(&core_failed)));
        assert_eq!(
            resolved.reason, "default",
            "Core failures do not block the OS default"
        );
    }

    #[test]
    fn invalid_one_shot_waits_with_an_explanation() {
        let targets = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("broken", BootTargetKind::EfiBinary, false),
        ];
        for (target, expires, code) in [
            ("gone", None, "missing"),
            ("broken", None, "unavailable"),
            ("os", Some(10), "expired"),
        ] {
            let mut policy = policy_with(Some("os"), 0);
            policy.one_shot = Some(OneShotBoot {
                target_id: BootTargetId(target.into()),
                command_id: None,
                expires_at_unix: expires,
                consumed: false,
            });
            let resolved = resolve(input(&policy, &targets, None));
            assert_eq!(resolved.reason, "one-shot-invalid", "{code}");
            assert_eq!(resolved.countdown_seconds, None, "{code}");
            assert!(resolved.consume_one_shot, "{code}");
            assert!(resolved.notice.unwrap().contains("Nothing was started"));
        }
        let twins = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("os", BootTargetKind::EfiBinary, true),
        ];
        assert_eq!(
            unique_selectable(&twins, &BootTargetId("os".into())),
            Err(RequestProblem::Ambiguous(BootTargetId("os".into())))
        );
    }

    #[test]
    fn unknown_preset_is_ignored_and_nomodeset_is_allowed() {
        assert_eq!(
            compose_cmdline(Some("root=LABEL=root"), Some("curl|sh")),
            Some("root=LABEL=root".into())
        );
        assert_eq!(
            compose_cmdline(Some("root=LABEL=root"), Some("nomodeset")).as_deref(),
            Some("root=LABEL=root nomodeset")
        );
    }

    #[test]
    fn confirmed_attempt_resets_the_failure_count() {
        let mut record = begin_attempt(None, &BootTargetId("os".into()), None, "a");
        record.status = BootAttemptStatus::Confirmed;
        record.consecutive_failures = 9;
        assert_eq!(effective_failures(Some(&record)), 0);
        assert_eq!(
            begin_attempt(Some(&record), &BootTargetId("os".into()), None, "b")
                .consecutive_failures,
            1
        );
    }

    #[test]
    fn success_report_confirms_only_the_matching_core_attempt() {
        let core = BootTargetId(RECOVERY_TARGET_ID.into());
        let mut record = begin_attempt(None, &core, Some(10), ATTEMPT);
        let report = |attempt: &str, target: &str, by: &str| {
            decode_success(
                alloc::format!(
                    r#"{{"schema_version":"1.0","attempt_id":"{attempt}","target_id":"{target}","confirmed_by":"{by}","confirmed_at_unix":11}}"#
                )
                .as_bytes(),
            )
            .unwrap()
        };
        assert_eq!(
            confirm_attempt(
                &mut record,
                &report("local", RECOVERY_TARGET_ID, "sbm-core-uki")
            ),
            Confirmation::Stale("attempt")
        );
        assert_eq!(
            confirm_attempt(
                &mut record,
                &report(ATTEMPT, RECOVERY_TARGET_ID, "sbm-agent")
            ),
            Confirmation::Stale("reporter")
        );
        assert_eq!(
            confirm_attempt(&mut record, &report(ATTEMPT, "efi:windows", "sbm-core-uki")),
            Confirmation::Stale("target")
        );
        assert_eq!(record.status, BootAttemptStatus::Started);
        let good = report(ATTEMPT, RECOVERY_TARGET_ID, "sbm-core-uki");
        assert_eq!(confirm_attempt(&mut record, &good), Confirmation::Confirmed);
        assert_eq!(record.status, BootAttemptStatus::Confirmed);
        assert_eq!(record.consecutive_failures, 0);
        assert_eq!(
            confirm_attempt(&mut record, &good),
            Confirmation::AlreadyConfirmed
        );
        let mut later = begin_attempt(Some(&record), &core, Some(20), "second");
        assert_eq!(
            confirm_attempt(&mut later, &good),
            Confirmation::Stale("attempt")
        );
        let mut os = begin_attempt(None, &BootTargetId("os".into()), None, ATTEMPT);
        assert_eq!(
            confirm_attempt(&mut os, &good),
            Confirmation::Stale("target")
        );
    }

    #[test]
    fn boot_requests_must_be_bound_current_and_selectable() {
        let targets = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("broken", BootTargetKind::EfiBinary, false),
            sample("sbm-core:slot-a", BootTargetKind::SbmCore, true),
        ];
        let confirmed = confirmed_core();
        assert_eq!(
            check_boot_request(&request("os"), Some(&confirmed), &targets),
            Ok(AcceptedRequest {
                target_id: BootTargetId("os".into()),
                preset: None
            })
        );
        let mut started = confirmed.clone();
        started.status = BootAttemptStatus::Started;
        assert_eq!(
            check_boot_request(&request("os"), Some(&started), &targets),
            Err(RequestProblem::Stale)
        );
        assert_eq!(
            check_boot_request(&request("os"), None, &targets),
            Err(RequestProblem::Stale)
        );
        let mut other = request("os");
        other.attempt_id = "11111111-2222-4333-8444-555555555555".into();
        assert_eq!(
            check_boot_request(&other, Some(&confirmed), &targets),
            Err(RequestProblem::Stale)
        );
        assert_eq!(
            check_boot_request(&request("os:6f5fdc33"), Some(&confirmed), &targets),
            Err(RequestProblem::Missing(BootTargetId("os:6f5fdc33".into())))
        );
        assert_eq!(
            check_boot_request(&request("broken"), Some(&confirmed), &targets),
            Err(RequestProblem::Unavailable("broken".into()))
        );
        assert_eq!(
            check_boot_request(&request("sbm-core:slot-a"), Some(&confirmed), &targets),
            Err(RequestProblem::Unsupported)
        );
        let mut bad_id = request("os");
        bad_id.request_id = "local".into();
        assert_eq!(
            check_boot_request(&bad_id, Some(&confirmed), &targets),
            Err(RequestProblem::Malformed)
        );
        let mut power = request("os");
        power.command = BootCommand::Reboot;
        assert_eq!(
            check_boot_request(&power, Some(&confirmed), &targets),
            Err(RequestProblem::Unsupported)
        );
        let mut preset = request("os");
        preset.command = BootCommand::BootTarget {
            target_id: BootTargetId("os".into()),
            one_shot: true,
            extra_cmdline_preset: Some("init=/bin/sh".into()),
        };
        assert_eq!(
            check_boot_request(&preset, Some(&confirmed), &targets),
            Err(RequestProblem::Unsupported)
        );
        let mut persistent = request("os");
        persistent.command = BootCommand::BootTarget {
            target_id: BootTargetId("os".into()),
            one_shot: false,
            extra_cmdline_preset: None,
        };
        assert_eq!(
            check_boot_request(&persistent, Some(&confirmed), &targets),
            Err(RequestProblem::Unsupported)
        );
        assert!(
            decode_boot_request(br#"{"action":"boot_target","target_id":"os","one_shot":true}"#)
                .is_err(),
            "a bare BootCommand from an older Core is incompatible"
        );
    }

    #[test]
    fn attempt_ids_are_uuid_v4_and_never_repeat() {
        let random = [0xabu8; 16];
        let id = new_attempt_id(Some(random), b"", None);
        assert!(is_uuid_v4(&id), "{id}");
        let derived = new_attempt_id(None, b"rtc+counter", Some(&id));
        assert!(is_uuid_v4(&derived));
        assert_ne!(derived, id);
        assert_ne!(new_attempt_id(Some(random), b"", Some(&id)), id);
        assert_ne!(
            new_attempt_id(None, b"one", None),
            new_attempt_id(None, b"two", None)
        );
    }

    #[test]
    fn handoff_lists_only_selectable_targets_and_hashes_exact_bytes() {
        let mut failed = sample("bad-core", BootTargetKind::SbmCore, true);
        failed.integrity = Integrity::Failed;
        let targets = vec![
            sample("os", BootTargetKind::EfiBinary, true),
            sample("broken", BootTargetKind::EfiBinary, false),
            failed,
        ];
        let payload = build_handoff(
            ATTEMPT,
            "0.0.1",
            PlatformCapabilities {
                arch: CpuArch::Aarch64,
                firmware: sbm_types::FirmwarePlatform::UefiNative,
                page_size_bytes: 4096,
                secure_boot_state: sbm_types::SecureBootState::Disabled,
                tpm_present: false,
                nvram_writable: true,
                gop_framebuffer: true,
                uefi_networking: false,
                can_chainload_efi: true,
                can_direct_chainload_macos: false,
                external_usb_boot_supported: true,
                ram_total_mib: None,
            },
            RecoveryLaunchMode::Interactive,
            None,
            &targets,
            BootTimingMetrics {
                firmware_init_us: None,
                uefi_entry_us: 0,
                first_frame_us: None,
                network_us: None,
                handoff_us: 1,
            },
            None,
        );
        assert_eq!(payload.discovered_targets.len(), 1);
        let (bytes, digest) = encode_handoff_with_digest(&payload).unwrap();
        assert_eq!(digest, hex_encode(&sha256(&bytes)));
        assert_eq!(decode_handoff(&bytes).unwrap(), payload);
    }

    #[test]
    fn embedded_cmdline_is_repeated_before_external_tokens() {
        let (line, mode) = merge_load_options(Some("console=tty0 quiet"), Some("root=/dev/sda2"));
        assert_eq!(mode, "embedded-repeated");
        assert_eq!(line.as_deref(), Some("console=tty0 quiet root=/dev/sda2"));
        let (line, mode) = merge_load_options(None, Some("quiet"));
        assert_eq!(mode, "external");
        assert_eq!(line.as_deref(), Some("quiet"));
    }
}
