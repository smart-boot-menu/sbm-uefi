# AGENTS.md — `sbm-uefi`

Follow the universal Smart Boot Menu agent rules in [`smart-boot-menu/.github/AGENTS.md`](https://github.com/smart-boot-menu/.github/blob/main/AGENTS.md) (locally `../.github/AGENTS.md`).

## Assigned Repository & Role

- **Repository:** `smart-boot-menu/sbm-uefi`
- **Primary Dependencies & Specs:** `sbm-common` (`sbm-types`, `sbm-protocol` with `--no-default-features`), `docs/specifications/01..03, 06..07`, `ADR-001`, `ADR-002`, `ADR-004`

## Responsibilities (What You Own)

- Graphical GOP boot menu (custom `#![no_std]` double-buffered software renderer, integer HiDPI scaling, keyboard-first input with bounded timer polling, serial/text fallback).
- Automatic OS discovery on FAT32 ESP/XBOOTLDR (Windows Boot Manager, UAPI BLS Type #1 `.conf` & Type #2 UKI, Linux/BSD EFI loaders, `sbm-core` A/B recovery slots).
- Deterministic EFI chainloading (`LoadImage`/`StartImage`) & ESP-backed atomic policy/attempt persistence (avoiding UEFI NVRAM wear/bricking).
- Bounded opportunistic remote boot-policy fetch (`smoltcp` + TLS 1.3 over SNP, ≤ 4 s hard timeout, Ed25519 `CommandEnvelope` verification limited ONLY to `boot_select` — zero arbitrary RCE).
- Secure Boot readiness (`.sbat` PE section, `NX_COMPAT`, `shim 16.1` compatibility).

## Strict Boundaries (Do NOT Duplicate or Violate)

Do NOT implement Linux disk/OS installation logic (`sbm-core`), Apple `m1n1` bootstrap (`sbm-as`), or copy `sbm-common` structs. Do NOT allow arbitrary remote code execution or destructive storage actions in UEFI.

## Current milestone

0.1 is the graphical menu, discovery, chainload, and the fail-closed remote decision function. It does not open SNP. Read [HANDOFF.md](HANDOFF.md) before changing the recovery JSON or the QEMU disk. Guest times from the 2026-11-26 run are in [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md).

Host checks: `cargo fmt --all`, `cargo clippy -p sbm-boot -p sbm-gfx -p xtask --all-targets -- -D warnings`, `cargo test`. UEFI checks are `cargo clippy` and `cargo build` for `sbm-app` and `sbm-fixture` with `--release` on both UEFI targets. Do not set `profile.dev` `panic = "abort"`.

## Engineering & Performance Targets

Startup-to-first-frame ≤ 250 ms; stripped EFI binary ≤ 1.5 MiB; peak heap ≤ 32 MiB; network hard cap ≤ 5 s (default 3 s, instant keypress cancel).

*(Targets are goals, not claims; follow `../docs/testing/benchmarks.md` and archive real measurements in `../research/validation/evidence/`.)*

## Security, Testing & Interface Coordination

- Enforce all invariants in `../docs/threat-model/README.md` (offline-first, no secrets committed, bounded timeouts, safe storage plans).
- Run formatting, clippy, unit tests, and integration hooks before committing.
- Never silently diverge from `sbm-common` types or `docs/specifications/`; follow the Interface Change Protocol in `../.github/agent-reference.md`.
