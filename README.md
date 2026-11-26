# Native Rust Graphical UEFI Boot Manager (`sbm-uefi`)

Pre-OS fast-path native Rust (`#![no_std]` + `alloc`) graphical UEFI boot manager targeting `x86_64-unknown-uefi` and `aarch64-unknown-uefi` from shared source.

> **Project Status:** Phase 0 initialized — ready for dedicated engineering agent implementation targeting **SBM 0.1**.

## Scope & Responsibilities

- Graphical GOP boot menu (custom `#![no_std]` double-buffered software renderer, integer HiDPI scaling, keyboard-first input with bounded timer polling, serial/text fallback).
- Automatic OS discovery on FAT32 ESP/XBOOTLDR (Windows Boot Manager, UAPI BLS Type #1 `.conf` & Type #2 UKI, Linux/BSD EFI loaders, `sbm-core` A/B recovery slots).
- Deterministic EFI chainloading (`LoadImage`/`StartImage`) & ESP-backed atomic policy/attempt persistence (avoiding UEFI NVRAM wear/bricking).
- Bounded opportunistic remote boot-policy fetch (`smoltcp` + TLS 1.3 over SNP, ≤ 4 s hard timeout, Ed25519 `CommandEnvelope` verification limited ONLY to `boot_select` — zero arbitrary RCE).
- Secure Boot readiness (`.sbat` PE section, `NX_COMPAT`, `shim 16.1` compatibility).

## Canonical Documentation & Contracts

- Central coordination & roadmap: [`smart-boot-menu/sbm`](https://github.com/smart-boot-menu/sbm)
- Normative specifications & ADRs: [`smart-boot-menu/docs`](https://github.com/smart-boot-menu/docs)
- Primary-source technical research: [`smart-boot-menu/research`](https://github.com/smart-boot-menu/research)
- Shared Rust types & JSON Schemas: [`smart-boot-menu/sbm-common`](https://github.com/smart-boot-menu/sbm-common)
- Agent guide: [AGENTS.md](AGENTS.md)

## License

Original code and documentation in this repository are dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), subject to third-party licensing requirements for bundled external components.
