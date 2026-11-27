use alloc::vec::Vec;
use sbm_protocol::{
    BootAttemptRecord, BootPolicy, BootSuccessReport, EfiHandoffPayload, LocalBootRequest,
    RecoveryManifest,
};
use sbm_types::SCHEMA_VERSION_V1;

use crate::pathutil::normalize;
use crate::volume::{VolError, Volume};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecError;

/// Spec 02 §7: write `<name>.tmp` and flush it, remove the old file, rename
/// the temporary file in place and flush again. `Volume::write` and
/// `Volume::rename` flush before they return.
pub fn atomic_write(vol: &dyn Volume, path: &str, data: &[u8]) -> Result<(), VolError> {
    let path = normalize(path);
    let tmp = alloc::format!("{path}.tmp");
    vol.write(&tmp, data)?;
    match vol.remove(&path) {
        Ok(()) | Err(VolError::NotFound) => {}
        Err(err) => return Err(err),
    }
    vol.rename(&tmp, &path)
}

/// Read a record written by [`atomic_write`]. When the record is missing but
/// a complete temporary copy survived an interrupted rename, use that copy.
/// `decode` decides what complete means.
pub fn read_durable<T>(
    vol: &dyn Volume,
    path: &str,
    decode: impl Fn(&[u8]) -> Result<T, CodecError>,
) -> Option<Result<T, CodecError>> {
    let path = normalize(path);
    match vol.read(&path) {
        Ok(bytes) => Some(decode(&bytes)),
        Err(VolError::NotFound) => {
            let bytes = vol.read(&alloc::format!("{path}.tmp")).ok()?;
            Some(decode(&bytes))
        }
        Err(VolError::Io) => Some(Err(CodecError)),
    }
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

pub fn decode_success(bytes: &[u8]) -> Result<BootSuccessReport, CodecError> {
    let report: BootSuccessReport = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(report.schema_version.as_str())?;
    Ok(report)
}

pub fn encode_success(report: &BootSuccessReport) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(report).map_err(|_| CodecError)
}

/// Decode a Spec 02 §5a request. A bare `BootCommand` from an older Core
/// does not have the binding fields and fails here.
pub fn decode_boot_request(bytes: &[u8]) -> Result<LocalBootRequest, CodecError> {
    let request: LocalBootRequest = serde_json::from_slice(bytes).map_err(|_| CodecError)?;
    schema(request.schema_version.as_str())?;
    Ok(request)
}

pub fn encode_boot_request(request: &LocalBootRequest) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(request).map_err(|_| CodecError)
}

fn schema(version: &str) -> Result<(), CodecError> {
    if version == SCHEMA_VERSION_V1 {
        Ok(())
    } else {
        Err(CodecError)
    }
}
