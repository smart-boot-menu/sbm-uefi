use sbm_protocol::{
    AuthScope, BootCommand, CommandEnvelope, CommandPayload, RecoverySlotPreference,
};
use sbm_types::{BootTargetId, SbmErrorCode};

use crate::policy::allowlisted_preset;

pub const NETWORK_HARD_CAP_MS: u32 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    Boot {
        id: BootTargetId,
        preset: Option<&'static str>,
    },
    Recovery(RecoverySlotPreference),
    Firmware,
    Reboot,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteDecision {
    Skip {
        reason: &'static str,
        budget_ms: u32,
    },
    Reject(SbmErrorCode),
    Accept(Applied),
}

pub fn clamp_network_timeout(milliseconds: u32) -> u32 {
    milliseconds.min(NETWORK_HARD_CAP_MS)
}

/// Decide a remote boot command without blocking.
///
/// Passing `envelope: None` is the offline path. It returns immediately.
/// A present envelope is checked with the shared metadata rules and is
/// rejected unless it is a `boot_select` boot command. Signature checking
/// stays outside this function so firmware can fail closed when no pinned
/// key is available.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_remote(
    allow_remote: bool,
    requested_timeout_ms: u32,
    link_up: bool,
    key_pressed: bool,
    envelope: Option<&CommandEnvelope>,
    known_target: impl Fn(&BootTargetId) -> bool,
    device_id: &str,
    last_sequence: u64,
    now_unix: Option<i64>,
) -> RemoteDecision {
    let budget_ms = clamp_network_timeout(requested_timeout_ms);
    if key_pressed {
        return RemoteDecision::Skip {
            reason: "local-key",
            budget_ms,
        };
    }
    if !allow_remote {
        return RemoteDecision::Skip {
            reason: "disabled",
            budget_ms,
        };
    }
    if !link_up {
        return RemoteDecision::Skip {
            reason: "no-link",
            budget_ms,
        };
    }
    let Some(envelope) = envelope else {
        return RemoteDecision::Skip {
            reason: "not-implemented",
            budget_ms,
        };
    };
    if let Err(code) = envelope.validate_metadata(device_id, last_sequence, now_unix, true) {
        return RemoteDecision::Reject(code);
    }
    if envelope.required_scope != AuthScope::BootSelect {
        return RemoteDecision::Reject(SbmErrorCode::UnauthorizedScope);
    }
    match &envelope.payload {
        CommandPayload::Boot(BootCommand::BootTarget {
            target_id,
            extra_cmdline_preset,
            ..
        }) => {
            if !known_target(target_id) {
                return RemoteDecision::Reject(SbmErrorCode::TargetNotFound);
            }
            let preset = extra_cmdline_preset.as_deref().and_then(allowlisted_preset);
            RemoteDecision::Accept(Applied::Boot {
                id: target_id.clone(),
                preset,
            })
        }
        CommandPayload::Boot(BootCommand::BootRecovery { slot, .. }) => {
            RemoteDecision::Accept(Applied::Recovery(*slot))
        }
        CommandPayload::Boot(BootCommand::RebootToFirmwareSetup) => {
            RemoteDecision::Accept(Applied::Firmware)
        }
        CommandPayload::Boot(BootCommand::Reboot) => RemoteDecision::Accept(Applied::Reboot),
        CommandPayload::Boot(BootCommand::Shutdown) => RemoteDecision::Accept(Applied::Shutdown),
        CommandPayload::RunStoragePlan(_)
        | CommandPayload::RunOsInstall(_)
        | CommandPayload::RevokeEnrollment { .. } => {
            RemoteDecision::Reject(SbmErrorCode::UnauthorizedScope)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use sbm_protocol::{BootCommand, CommandPayload};
    use sbm_types::SCHEMA_VERSION_V1;

    fn envelope(scope: AuthScope, payload: CommandPayload, sequence: u64) -> CommandEnvelope {
        CommandEnvelope {
            schema_version: SCHEMA_VERSION_V1.to_string(),
            command_id: "019a0000-0000-7000-8000-000000000009".into(),
            device_id: "dev-01".into(),
            sequence,
            issued_at_unix: 100,
            expires_at_unix: 500,
            required_scope: scope,
            issuer_key_id: "key".into(),
            payload,
            signature_ed25519_hex: "00".repeat(64),
        }
    }

    #[test]
    fn offline_and_rejected_commands_do_not_wait() {
        let skip = evaluate_remote(
            true,
            9_000,
            false,
            false,
            None,
            |_| true,
            "dev-01",
            0,
            Some(120),
        );
        assert_eq!(
            skip,
            RemoteDecision::Skip {
                reason: "no-link",
                budget_ms: 5_000
            }
        );
        let disabled = evaluate_remote(false, 100, true, false, None, |_| true, "dev-01", 0, None);
        assert!(matches!(
            disabled,
            RemoteDecision::Skip {
                reason: "disabled",
                ..
            }
        ));
        let missing = evaluate_remote(true, 100, true, false, None, |_| true, "dev-01", 0, None);
        assert!(matches!(
            missing,
            RemoteDecision::Skip {
                reason: "not-implemented",
                ..
            }
        ));

        let bad = envelope(
            AuthScope::StorageDestructive,
            CommandPayload::Boot(BootCommand::Shutdown),
            1,
        );
        assert_eq!(
            evaluate_remote(
                true,
                100,
                true,
                false,
                Some(&bad),
                |_| true,
                "dev-01",
                0,
                Some(120)
            ),
            RemoteDecision::Reject(SbmErrorCode::UnauthorizedScope)
        );

        let boot = envelope(
            AuthScope::BootSelect,
            CommandPayload::Boot(BootCommand::BootTarget {
                target_id: BootTargetId("os".into()),
                one_shot: true,
                extra_cmdline_preset: Some("ignored;rm".into()),
            }),
            2,
        );
        assert_eq!(
            evaluate_remote(
                true,
                100,
                true,
                true,
                Some(&boot),
                |_| true,
                "dev-01",
                0,
                Some(120)
            ),
            RemoteDecision::Skip {
                reason: "local-key",
                budget_ms: 100
            }
        );
        match evaluate_remote(
            true,
            100,
            true,
            false,
            Some(&boot),
            |_| true,
            "dev-01",
            1,
            Some(120),
        ) {
            RemoteDecision::Accept(Applied::Boot { id, preset }) => {
                assert_eq!(id.as_str(), "os");
                assert_eq!(preset, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
