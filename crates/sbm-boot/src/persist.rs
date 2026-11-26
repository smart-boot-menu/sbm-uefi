use alloc::vec::Vec;
use sbm_protocol::{BootAttemptRecord, BootPolicy, EfiHandoffPayload, RecoveryManifest};
use sbm_types::SCHEMA_VERSION_V1;

use crate::pathutil::normalize;
use crate::volume::{VolError, Volume};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecError;

pub fn atomic_write(vol: &dyn Volume, path: &str, data: &[u8]) -> Result<(), VolError> {
    let path = normalize(path);
    let tmp = alloc::format!("{path}.tmp");
    vol.write(&tmp, data)?;
    if vol.exists(&path) {
        let _ = vol.remove(&path);
    }
    vol.rename(&tmp, &path)
}

pub fn encode_policy(policy: &BootPolicy) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(policy).map_err(|_| CodecError)
}

pub fn decode_policy(bytes: &[u8]) -> Result<BootPolicy, CodecError> {
    let policy: BootPolicy = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(policy.schema_version.as_str())?;
    Ok(policy)
}

pub fn encode_attempt(record: &BootAttemptRecord) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(record).map_err(|_| CodecError)
}

pub fn decode_attempt(bytes: &[u8]) -> Result<BootAttemptRecord, CodecError> {
    let record: BootAttemptRecord = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(record.schema_version.as_str())?;
    Ok(record)
}

pub fn encode_manifest(manifest: &RecoveryManifest) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(manifest).map_err(|_| CodecError)
}

pub fn decode_manifest(bytes: &[u8]) -> Result<RecoveryManifest, CodecError> {
    let manifest: RecoveryManifest = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(manifest.schema_version.as_str())?;
    Ok(manifest)
}

pub fn encode_handoff(payload: &EfiHandoffPayload) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(payload).map_err(|_| CodecError)
}

pub fn decode_handoff(bytes: &[u8]) -> Result<EfiHandoffPayload, CodecError> {
    let payload: EfiHandoffPayload = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(payload.schema_version.as_str())?;
    Ok(payload)
}

fn schema(version: &str) -> Result<(), CodecError> {
    if version == SCHEMA_VERSION_V1 {
        Ok(())
    } else {
        Err(CodecError)
    }
}
