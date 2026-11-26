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
mod volume;

pub use bls::{BlsEntry, bls_arch_ok, parse_bls};
pub use clock::unix_timestamp;
pub use discover::{
    DiscoveredTarget, add_load_options, best_recovery, discover_volume, recovery_for_slot,
    sort_discovered,
};
pub use nvram::{ParsedLoadOption, parse_load_option};
pub use pathutil::{
    efi_target_id, hex_encode, normalize, recovery_target_id, sanitize_id_part, sha256,
};
pub use pe::{MACHINE_AA64, MACHINE_IA32, MACHINE_X64, machine_matches, pe_machine};
pub use policy::{
    CodecError, ResolveInput, Resolved, allowlisted_preset, atomic_write, begin_attempt,
    build_handoff, compose_cmdline, decode_attempt, decode_handoff, decode_manifest, decode_policy,
    effective_failures, encode_attempt, encode_handoff, encode_manifest, encode_policy,
    find_target, fresh_policy, handoff_sha256_hex, linux_load_options, mark_one_shot_consumed,
    resolve, selectable, visible,
};
pub use remote::{Applied, NETWORK_HARD_CAP_MS, RemoteDecision, evaluate_remote};
pub use volume::{DirEntry, FileMeta, MemoryVolume, VolError, Volume};
