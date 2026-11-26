# Handoff

The 0.1 UEFI menu builds and boots for `x86_64-unknown-uefi` and `aarch64-unknown-uefi`. Shared types come from `sbm-common` `28d38450954f0b93815e12f31da92370b13a7462` (`sbm-types`, `sbm-protocol`, default features off). This tree does not modify `sbm-common`, `sbm-core`, or `sbm-lab`.

Numbers and the QEMU command shape are in [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md). The plan is in [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md).

## Layout

- `crates/sbm-boot` — discovery, policy, handoff JSON, volume trait. Host tests.
- `crates/sbm-gfx` — software framebuffer, themes, input state. Host tests. The 8×8 font is public domain (`crates/sbm-gfx/src/font8x8.rs`).
- `crates/sbm-app` — UEFI entry, GOP, chainload. Release builds only.
- `crates/sbm-fixture` — tiny EFI that prints `SBM_MARK fixture_started`, writes `\EFI\sbm\test\chainload-ok.txt`, and returns. It is not Windows, Linux, or SBM Core.
- `crates/xtask` — `build`, `qemu aarch64`, `qemu x86_64`.
- `artifacts/` — the release `sbm-*.efi` and fixture binaries from the measured run.

`cargo test` covers the default members (`sbm-boot`, `sbm-gfx`, `xtask`): 14 + 4 + 5 tests. UEFI clippy and builds use `--release`. There is no `profile.dev` panic abort.

## Recovery contract for sbm-core

Ordinary OS entries are chainloaded with `LoadImage` / `StartImage` and no Linux step. Recovery is the same path, plus a handoff written only when the chosen target's kind is `SbmCore`.

Read `\EFI\sbm\recovery\manifest.json` (`RecoveryManifest`, schema `1.0`). Each slot uses `slot_name`, `uki_efi_path`, `sha256`, `version`, and `healthy`. The active slot is preferred. A slot is selectable only when `healthy` is true and the file hash matches. The fixture manifest points both slots at the fixture EFI and marks them healthy, so the menu shows Slot A and Slot B. That fixture is not a recovery environment.

Before `StartImage` of an `SbmCore` target the menu writes `\EFI\sbm\handoff\current.json` (`EfiHandoffPayload`, schema `1.0`). The x86_64 run serialized these keys: `schema_version`, `attempt_id`, `sbm_uefi_version` (`0.1.0`), `platform`, `launch_mode` (`{"mode":"interactive"}`), `discovered_targets` (six `BootTarget` values), `boot_timing_us`, `network_hint`. `pending_command_id` was omitted. `platform` on that run had `arch` `x86_64`, `firmware` `uefi-native`, `secure_boot_state` `unsupported`, `gop_framebuffer` true, `uefi_networking` false, `ram_total_mib` 1021.

Load options for that launch are UCS-2 and space-separated: the target cmdline when it has one, `sbm.handoff_sha256=<sha256 of the JSON bytes>`, and `sbm.esp_partuuid=<GPT partition GUID>` when the boot volume has one. The FAT16 test image has no partition GUID, so the partuuid token is omitted there.

`LoaderEntrySelected` is also published as a volatile UCS-2 variable (boot-service and runtime access, not non-volatile) under `d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40` and under the systemd vendor `4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`. The value is the target id (`sbm-core:slot-a`, `efi:<guid>:<12 hex>`, or `bls:...`).

`\EFI\sbm\state\last_attempt.json` is `started` before `StartImage`. If `StartImage` returns, the same record is written again as `failed`. A UKI that calls `ExitBootServices` does not return, so this menu will not mark that boot failed. The fixture returns `SUCCESS` and is recorded as failed. Two returning launches in the x86 run left `consecutive_failures` at 2.

Initrd `LoadFile2` is not installed. A recovery artifact has to be one EFI file.

## Lab interface for sbm-lab

`cargo run -p xtask -- qemu aarch64` and `cargo run -p xtask -- qemu x86_64` build both release binaries, assemble an ESP tree under `build/`, copy it into a 32 MiB FAT16 image, and boot it. `cargo run -p xtask -- build` only compiles and copies `artifacts/`.

Use the FAT image (`-drive file=<image>,format=raw,if=none,id=esp` and `virtio-blk-pci,drive=esp,bootindex=0`). Do not use `-drive file=fat:rw:<dir>`. On aarch64 that backend returned a 2 KB file whose bytes were firmware status strings rather than the JSON the guest wrote.

Graphics: one device. aarch64 is `ramfb`. x86_64 is `-vga std`. aarch64 also needs `qemu-xhci` and `usb-kbd` or `sendkey` has no keyboard. The monitor is a unix socket. The runner sends `t`, `ret`, and `c`, and dumps PPM screenshots. Serial is a file of `SBM_MARK` lines. x86 also writes debugcon at `0x402`. `-nic none` and `-no-reboot`.

The primary attempt is the one that passed on 2026-11-26. The runner still has a second disk device (`virtio-blk-device` on aarch64, `ide-hd` on x86) and a Cocoa display if the primary attempt fails. Those fallbacks were not used for the recorded numbers.

## Limits

- No SNP, DHCP, TLS, or Ed25519. `evaluate_remote` returns immediately and fail-closed. `allow_remote` false produces `remote disabled` and zero network wait. There is no path that runs a remote script.
- IA32 is documented in [docs/IA32.md](docs/IA32.md) and is not a built target.
- Design tokens live in [docs/DESIGN_TOKENS.md](docs/DESIGN_TOKENS.md) and `sbm-gfx`. They are not in `sbm-common`.
- At 1024×768 the six cards ellipsize (`Windo…`). At 1280×800 the Windows title fits. Keyboard works without a mouse: arrows, Enter, Tab, `T` theme, `C` recovery, `D` diagnostics. Escape on the menu does not reboot.
- The menu does not write `BootOrder`. Firmware setup sets `OsIndications` bit 0 and cold-resets only when `OsIndicationsSupported` says the firmware offers it.
- `uefi-wm` (ldmsys/uefi-wm) is a real project and was not used. GOP has no GPU acceleration API. The renderer is a RAM backbuffer. `research/hardware/gpu-and-graphics-acceleration.md` still says no repository named `uefi-wm` exists. That research tree was dirty on 2026-11-26, so this repo did not add a correction or an evidence file there.

## Coordination

- Recovery consumer: https://github.com/smart-boot-menu/sbm-core/issues/1
- QEMU disk and guest setup: https://github.com/smart-boot-menu/sbm-lab/issues/1
