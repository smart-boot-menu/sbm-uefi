#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod bls;
mod clock;
mod discover;
mod nvram;
mod pathutil;
mod pe;
mod persist;
mod policy;
mod remote;
mod verify;
mod volume;

pub use bls::{BlsEntry, bls_arch_ok, parse_bls};
pub use clock::unix_timestamp;
pub use discover::{
    DiscoveredTarget, Integrity, RECOVERY_MANIFEST_PATH, RECOVERY_SLOT, RECOVERY_TARGET_ID,
    add_load_options, best_recovery, canonical_recovery_path, compare_targets, discover_recovery,
    discover_volume, embedded_cmdline, recovery_for_slot, sort_discovered,
};
pub use nvram::{ParsedLoadOption, parse_load_option};
pub use pathutil::{
    efi_target_id, hex_encode, normalize, recovery_target_id, sanitize_id_part, sha256,
};
pub use pe::{
    MACHINE_AA64, MACHINE_IA32, MACHINE_X64, machine_matches, pe_machine, pe_section_span,
    section_text,
};
pub use policy::{
    AcceptedRequest, CodecError, Confirmation, RequestProblem, ResolveInput, Resolved,
    allowlisted_preset, atomic_write, begin_attempt, build_handoff, check_boot_request,
    compose_cmdline, confirm_attempt, decode_attempt, decode_boot_request, decode_handoff,
    decode_manifest, decode_policy, decode_success, effective_failures, encode_attempt,
    encode_boot_request, encode_handoff, encode_handoff_with_digest, encode_manifest,
    encode_policy, encode_success, find_target, fresh_policy, mark_one_shot_consumed, menu,
    merge_load_options, new_attempt_id, read_durable, resolve, selectable, unique_selectable,
    visible,
};
pub use remote::{Applied, NETWORK_HARD_CAP_MS, RemoteDecision, evaluate_remote};
pub use verify::{VERIFY_CHUNK_BYTES, Verified, VerifyError, verify_image};
pub use volume::{DirEntry, FileMeta, MemoryVolume, Streamed, VolError, Volume};
