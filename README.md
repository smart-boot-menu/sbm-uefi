# Native Rust Graphical UEFI Boot Manager (`sbm-uefi`)

Pre-OS fast-path native Rust (`#![no_std]` + `alloc`) graphical UEFI boot manager targeting `x86_64-unknown-uefi` and `aarch64-unknown-uefi` from shared source.

> **Project Status:** SBM 0.1 fast path boots in QEMU on `x86_64-unknown-uefi` and `aarch64-unknown-uefi`. Measurements are in [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md). The recovery contract, lab markers, and the separate networking plan are in [HANDOFF.md](HANDOFF.md).

## What 0.1 does

- Graphical GOP menu: software double buffer, light and dark themes, keyboard, mouse pointer, firmware / restart / shut down.
- ESP discovery for Windows Boot Manager, GRUB, systemd-boot, BLS Type #1, a Linux EFI, and `sbm-core` A/B recovery slots. The menu skips its own path.
- `LoadImage` / `StartImage` chainload. Recovery launches that UKI directly and writes `\EFI\sbm\handoff\current.json`.
- Policy and attempt JSON on the ESP. The remote decision function is fail-closed and does not open a network.
- `.sbat` section and `IMAGE_DLLCHARACTERISTICS_NX_COMPAT` on the release EFI.

SNP, DHCP, TLS, and Ed25519 verification of a live server are not in this tree. IA32 is documented and not built.

```
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi
cargo test
cargo run -p xtask -- build
cargo run -p xtask -- qemu aarch64
cargo run -p xtask -- qemu x86_64
```

`cargo test` is the host suites. UEFI bins are release builds. QEMU is local; CI does not boot a guest.

### Native development (SIMULATED)

```
cargo xtask sim --scenario dual-boot      # real menu code in a desktop window; Manage Computer opens real sbm-ui
cargo xtask preview --all-scenarios       # headless screenshots
```

The simulator runs the production `sbm-boot`, `sbm-gfx` and `sbm-menu` code against scenario files in `sim/scenarios/`. It never touches host disks, NVRAM or boot settings, and it is not part of `sbm-app`. See [sim/README.md](sim/README.md).

## Canonical Documentation & Contracts

- Central coordination & roadmap: [`smart-boot-menu/sbm`](https://github.com/smart-boot-menu/sbm)
- Normative specifications & ADRs: [`smart-boot-menu/docs`](https://github.com/smart-boot-menu/docs)
- Primary-source technical research: [`smart-boot-menu/research`](https://github.com/smart-boot-menu/research)
- Shared Rust types & JSON Schemas: [`smart-boot-menu/sbm-common`](https://github.com/smart-boot-menu/sbm-common)
- Agent guide: [AGENTS.md](AGENTS.md)

## License

Original code and documentation in this repository are dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), subject to third-party licensing requirements for bundled external components.
