use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sbm_protocol::{
    BootAttemptRecord, BootAttemptStatus, BootCommand, BootPolicy, BootSuccessReport,
    BootTimingMetrics, EfiHandoffPayload, NetworkHandoffHint, OfflineBehavior, RecoveryLaunchMode,
    RecoverySlotPreference,
};
use sbm_types::{BootTarget, BootTargetId, PlatformCapabilities, SCHEMA_VERSION_V1};

use crate::discover::DiscoveredTarget;
use crate::pathutil::hex_encode;
use crate::pathutil::sha256;

pub use crate::persist::{
    CodecError, atomic_write, decode_attempt, decode_command, decode_handoff, decode_manifest,
    decode_policy, decode_success, encode_attempt, encode_handoff, encode_manifest, encode_policy,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub highlight: Option<BootTargetId>,
    /// `None` waits. `Some(0)` boots immediately. Any other value is a countdown.
    pub countdown_seconds: Option<u16>,
    pub consume_one_shot: bool,
    pub reason: &'static str,
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
    !target.target.hidden && target.healthy && target.target.verified_signature != Some(false)
}

pub fn effective_failures(record: Option<&BootAttemptRecord>) -> u8 {
    match record {
        Some(record)
            if matches!(
                record.status,
                BootAttemptStatus::Started | BootAttemptStatus::Failed
            ) =>
        {
            record.consecutive_failures
        }
        _ => 0,
    }
}

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
    BootAttemptRecord {
        schema_version: SCHEMA_VERSION_V1.to_string(),
        attempt_id: attempt_id.to_string(),
        target_id: target.clone(),
        slot,
        timestamp_unix: now_unix,
        consecutive_failures: effective_failures(previous).saturating_add(1),
        status: BootAttemptStatus::Started,
    }
}

pub fn confirm_attempt(record: &mut BootAttemptRecord, report: &BootSuccessReport) -> bool {
    if report.schema_version != SCHEMA_VERSION_V1 {
        return false;
    }
    let same_target = report.target_id == record.target_id;
    let same_attempt = report.attempt_id == record.attempt_id || report.attempt_id == "local";
    if !same_target || !same_attempt {
        return false;
    }
    record.status = BootAttemptStatus::Confirmed;
    record.consecutive_failures = 0;
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalRequest {
    Boot(BootTargetId),
    Recovery(RecoverySlotPreference),
    Firmware,
    Reboot,
    Shutdown,
}

pub fn local_request(command: &BootCommand) -> LocalRequest {
    match command {
        BootCommand::BootTarget { target_id, .. } => LocalRequest::Boot(target_id.clone()),
        BootCommand::BootRecovery { slot, .. } => LocalRequest::Recovery(*slot),
        BootCommand::RebootToFirmwareSetup => LocalRequest::Firmware,
        BootCommand::Reboot => LocalRequest::Reboot,
        BootCommand::Shutdown => LocalRequest::Shutdown,
    }
}

/// systemd-stub replaces the embedded `.cmdline` with EFI load options when
/// Secure Boot is off, and ignores those options when Secure Boot is on and
/// `.cmdline` exists. Repeating the embedded line keeps the UKI's own tokens
/// in the first case and is ignored in the second.
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

pub fn allowlisted_preset(name: &str) -> Option<&'static str> {
    match name {
        "quiet" => Some("quiet"),
        "verbose" => Some("verbose"),
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

pub fn linux_load_options(
    cmdline: Option<&str>,
    handoff_sha256: Option<&str>,
    esp_partuuid: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(cmdline) = cmdline.filter(|text| !text.is_empty()) {
        parts.push(cmdline.to_string());
    }
    if let Some(hash) = handoff_sha256 {
        parts.push(alloc::format!("sbm.handoff_sha256={hash}"));
    }
    if let Some(guid) = esp_partuuid {
        parts.push(alloc::format!("sbm.esp_partuuid={guid}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
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
    let failures = effective_failures(input.attempt);
    if input.user_key_held {
        return menu(first_id(input.targets, input.policy), "local-override");
    }
    if let Some(one_shot) = one_shot_target(input.policy, input.targets, input.now_unix) {
        return Resolved {
            highlight: Some(one_shot),
            countdown_seconds: Some(0),
            consume_one_shot: true,
            reason: "one-shot",
        };
    }
    let suppress = input.remote_refresh_failed
        && input.policy.allow_remote_boot_select
        && input.policy.offline_behavior == OfflineBehavior::WaitForUser;
    if let Some(id) = healthy_default(input.policy, input.targets, failures) {
        if suppress {
            return menu(Some(id), "wait-for-user");
        }
        return timed(Some(id), input.policy.timeout_seconds, "default");
    }
    if input.policy.max_failed_attempts > 0
        && failures >= input.policy.max_failed_attempts
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
    }
}

fn menu(id: Option<BootTargetId>, reason: &'static str) -> Resolved {
    Resolved {
        highlight: id,
        countdown_seconds: None,
        consume_one_shot: false,
        reason,
    }
}

fn one_shot_target(
    policy: &BootPolicy,
    targets: &[DiscoveredTarget],
    now_unix: Option<i64>,
) -> Option<BootTargetId> {
    let one_shot = policy.one_shot.as_ref()?;
    if one_shot.consumed {
        return None;
    }
    let target = targets
        .iter()
        .find(|item| item.target.id == one_shot.target_id)?;
    if target.target.hidden {
        return None;
    }
    if let Some(expires) = one_shot.expires_at_unix {
        let now = now_unix?;
        if now > expires {
            return None;
        }
    }
    Some(one_shot.target_id.clone())
}

fn healthy_default(
    policy: &BootPolicy,
    targets: &[DiscoveredTarget],
    failures: u8,
) -> Option<BootTargetId> {
    if policy.max_failed_attempts > 0 && failures >= policy.max_failed_attempts {
        return None;
    }
    let id = policy.default_target.as_ref()?;
    let target = targets.iter().find(|item| &item.target.id == id)?;
    if selectable(target) {
        Some(id.clone())
    } else {
        None
    }
}

fn fallback_target(policy: &BootPolicy, targets: &[DiscoveredTarget]) -> Option<BootTargetId> {
    if let Some(id) = policy.fallback_target.as_ref()
        && let Some(target) = targets.iter().find(|item| &item.target.id == id)
        && selectable(target)
    {
        return Some(id.clone());
    }
    targets
        .iter()
        .find(|target| {
            target.target.kind == sbm_types::BootTargetKind::SbmCore && selectable(target)
        })
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
        discovered_targets: targets.iter().map(|item| item.target.clone()).collect(),
        boot_timing_us: Some(timing),
        network_hint: network,
    }
}

pub fn handoff_sha256_hex(
    payload: &EfiHandoffPayload,
) -> Result<String, crate::persist::CodecError> {
    let bytes = encode_handoff(payload)?;
    Ok(hex_encode(&sha256(&bytes)))
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
        let resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: None,
            now_unix: Some(10),
            user_key_held: true,
            remote_refresh_failed: false,
        });
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
        let resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: None,
            now_unix: Some(40),
            user_key_held: false,
            remote_refresh_failed: false,
        });
        assert_eq!(resolved.reason, "one-shot");
        assert_eq!(resolved.countdown_seconds, Some(0));
        assert!(resolved.consume_one_shot);

        policy.one_shot.as_mut().unwrap().consumed = true;
        let resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: None,
            now_unix: Some(40),
            user_key_held: false,
            remote_refresh_failed: false,
        });
        assert_eq!(resolved.reason, "default");
        assert_eq!(resolved.countdown_seconds, Some(5));

        let failed = begin_attempt(None, &BootTargetId("os".into()), Some(1), "a1");
        let failed = begin_attempt(Some(&failed), &BootTargetId("os".into()), Some(2), "a2");
        let failed = begin_attempt(Some(&failed), &BootTargetId("os".into()), Some(3), "a3");
        assert_eq!(failed.consecutive_failures, 3);
        let resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: Some(&failed),
            now_unix: Some(40),
            user_key_held: false,
            remote_refresh_failed: false,
        });
        assert_eq!(resolved.reason, "fallback");
        assert_eq!(resolved.highlight.unwrap().as_str(), "sbm-core:slot-a");
    }

    #[test]
    fn expired_one_shot_and_unknown_preset_are_ignored() {
        let targets = vec![sample("os", BootTargetKind::EfiBinary, true)];
        let mut policy = policy_with(Some("os"), 65535);
        policy.one_shot = Some(OneShotBoot {
            target_id: BootTargetId("os".into()),
            command_id: None,
            expires_at_unix: Some(10),
            consumed: false,
        });
        let resolved = resolve(ResolveInput {
            policy: &policy,
            targets: &targets,
            attempt: None,
            now_unix: Some(11),
            user_key_held: false,
            remote_refresh_failed: false,
        });
        assert_eq!(resolved.reason, "default");
        assert_eq!(resolved.countdown_seconds, Some(65535));
        assert_eq!(
            compose_cmdline(Some("root=LABEL=root"), Some("curl|sh")),
            Some("root=LABEL=root".into())
        );
        assert_eq!(
            compose_cmdline(Some("root=LABEL=root"), Some("quiet")).as_deref(),
            Some("root=LABEL=root quiet")
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
    fn success_report_confirms_the_matching_attempt() {
        let mut record = begin_attempt(
            None,
            &BootTargetId("sbm-core:slot-a".into()),
            Some(10),
            "uefi-1",
        );
        let report = decode_success(
            br#"{"schema_version":"1.0","attempt_id":"uefi-1","target_id":"sbm-core:slot-a","confirmed_by":"sbm-core-uki","confirmed_at_unix":11}"#,
        )
        .unwrap();
        assert!(confirm_attempt(&mut record, &report));
        assert_eq!(record.status, BootAttemptStatus::Confirmed);
        assert_eq!(record.consecutive_failures, 0);
        let other = decode_success(
            br#"{"schema_version":"1.0","attempt_id":"other","target_id":"efi:windows","confirmed_by":"sbm-core-uki","confirmed_at_unix":11}"#,
        )
        .unwrap();
        assert!(!confirm_attempt(&mut record, &other));
    }

    #[test]
    fn embedded_cmdline_is_repeated_before_external_tokens() {
        let (line, mode) = merge_load_options(
            Some("console=tty0 quiet"),
            Some("sbm.handoff_sha256=abc sbm.esp_partuuid=guid"),
        );
        assert_eq!(mode, "embedded-repeated");
        assert_eq!(
            line.as_deref(),
            Some("console=tty0 quiet sbm.handoff_sha256=abc sbm.esp_partuuid=guid")
        );
        let (line, mode) = merge_load_options(None, Some("sbm.handoff_sha256=abc"));
        assert_eq!(mode, "external");
        assert_eq!(line.as_deref(), Some("sbm.handoff_sha256=abc"));
        let command = decode_command(
            br#"{"action":"boot_target","target_id":"sbm-core:slot-a","one_shot":true}"#,
        )
        .unwrap();
        assert_eq!(
            local_request(&command),
            LocalRequest::Boot(BootTargetId("sbm-core:slot-a".into()))
        );
        assert_eq!(
            local_request(&decode_command(br#"{"action":"shutdown"}"#).unwrap()),
            LocalRequest::Shutdown
        );
    }
}
