# Handoff

## Visual architecture consolidation & Agent 13 release qualification (2026-11-27)

Consolidated Agent 20's accessible typography (`Atkinson Hyperlegible Next` Regular/SemiBold + `Atkinson Hyperlegible Mono` Regular) and unified CPU vector rendering backend (`sbm_gfx::vector`) into `main` alongside the LBA-ordered paragraph-style partition dashboards (`Bento`, `Schematic`, `Storage`) and all Agent 15 UEFI security/performance/handoff fixes.

### What changed
- **Single unified rendering engine (`sbm-gfx`):** Removed `crates/sbm-gfx/src/text.rs` (`noto-sans-mono-bitmap`), `crates/sbm-gfx/src/assets.rs`, duplicate pixel polygon fill/stroke loops in `disk_wrap.rs`, hard-aliased rounded-panel helpers in `draw.rs`, and the ~330-line duplicate `paint_*_legacy` engine in `paint.rs`. Both Classic and experimental layouts now render through one `TextRenderer` + `sbm_gfx::vector` pipeline with unconditional viewport clipping and scroll-bias translation (`Framebuffer::with_viewport`).
- **Accessibility, themes, and scaling:** Unified `Theme::resolve(dark, high_contrast)` and `Palette::resolve(dark, high_contrast)` across Classic, experimental dashboards, and simulator overlays (Light, Dark, High-Contrast Dark, High-Contrast Light with 2 px borders and high-contrast badge ink). `UiScale` (`75%`–`200%`) dynamically scales both Classic and experimental metrics.
- **Production layout policy:** Classic remains the sole production layout compiled into `sbm-app`. Experimental dashboards stay gated behind `sbm-gfx/experimental-layouts` and selectable in `sbm-preview` (`1`–`4` / `F4`, `--layout`).
- **Safe countdown & click-to-boot semantics:** Removed `arm_card` / click-triggered countdown arming in `sbm-preview`. Only policy-authorized startup autoboot (`Picker::initial_state`) may run a countdown timer. Clicking an unselected partition selects it and cancels any running policy countdown without arming a new timer; clicking the already-selected partition or pressing `Enter`/`Space` boots immediately; `[×]`, `Esc`, and arrow keys abort countdown without booting.
- **Agent 15 fixes preserved intact:** `sbm-common@abe3029`, lazy `OnceCell` capabilities, 1 MiB streaming SHA-256 with `sha2` `opt-level = 3`, all five Spec 03 volatile EFI variables, UUIDv4 `attempt_id`, bound `LocalBootRequest`, `sbm-menu::flow::Picker`, SBAT `.sbat`, and PE `NX_COMPAT`.

### Coordination for Agent 13 (Release Qualification)
- **Qualification candidate:** Use this consolidation commit on `main` (see `git rev-parse HEAD` / commit log for the exact SHA). Built release artifacts `artifacts/sbm-aarch64.efi` (`410,112 B`) and `artifacts/sbm-x86_64.efi` (`465,920 B`) carry `.sbat` and `NX_COMPAT`.
- **Completed locally (native):** All host unit/integration tests (`88` tests across default + experimental feature sets), clippy `-D warnings` across host + window + both UEFI targets, `bench_typography` latency/memory profiling (Classic `2.60 ms` warm / `7.5 KiB` cache at `1280×800`; experimental `6.90–9.93 ms` warm / `13.8–16.8 KiB` cache), and visual screenshot audit (`build/sim/consolidation/`).
- **Remaining for Agent 13 in QEMU/lab:** Run `cargo run -p xtask -- qemu all` (deferred here while `sbm-lab` guest memory budget was occupied) plus real recovery media verification against the existing Slot A UKI (`menu_ready` guest-timer + `SBM_MARK glyph_cache` telemetry + refreshed QEMU captures in `screenshots/`).

## Agent 15 update

The picker performs metadata-only Slot A discovery and streams the selected recovery UKI through SHA-256 before launch. It writes the Spec 03 handoff and attempt to the identified boot ESP, publishes all five volatile variables, and starts the UKI by its exact device path with empty external options. Core-issued one-shot requests bind to a confirmed attempt, and a rejected request leaves the picker interactive with an explanation.

The native simulator and firmware use the same picker. The GPT smoke fixture validates the handoff from inside the launched EFI image. See the [Agent 15 measurements](docs/MEASUREMENTS.md) for evidence and remaining firmware checks.

## Earlier handoff


The 0.1 UEFI menu still builds and boots for `x86_64-unknown-uefi` and `aarch64-unknown-uefi`. This follow-up wires it to the recovery files `sbm-core` already writes, records boot success and local boot commands, and keeps the menu offline. Shared types come from `sbm-common` `28d38450954f0b93815e12f31da92370b13a7462` (`sbm-types`, `sbm-protocol`, default features off). This tree does not modify `sbm-common`, `sbm-core`, or `sbm-lab`.

There is no Agent 10. The agent roster is Agents 1–9. Recovery belongs to Agent 02 (`sbm-core`). The lab belongs to Agent 03 (`sbm-lab`).

Numbers, the GRUB chainload, and the Secure Boot check are in [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md). The networking work that is not in this firmware is in [docs/NETWORKING_PLAN.md](docs/NETWORKING_PLAN.md).

## Layout

- `crates/sbm-boot` — discovery, policy, handoff JSON, volume trait. Host tests.
- `crates/sbm-gfx` — software framebuffer, themes, input state. Host tests. The 8×8 font is public domain (`crates/sbm-gfx/src/font8x8.rs`).
- `crates/sbm-app` — UEFI entry, GOP, chainload. Release builds only.
- `crates/sbm-fixture` — tiny EFI that prints `SBM_MARK fixture_started`, writes `\EFI\sbm\test\chainload-ok.txt`, and returns. It now has an unsigned `.cmdline` section (`console=tty0`) so the guest exercises the load-option merge. It is not Windows, Linux, or SBM Core.
- `crates/xtask` — `build`, `qemu aarch64`, `qemu x86_64`. After a passing guest it writes `build/acceptance-<arch>.json`.
- `artifacts/` — the release `sbm-*.efi` and fixture binaries from the measured run.

`cargo test` covers the default members (`sbm-boot`, `sbm-gfx`, `xtask`): 18 + 5 + 5 tests. UEFI clippy and builds use `--release`. There is no `profile.dev` panic abort.

## Recovery contract

Ordinary OS entries are chainloaded with `LoadImage` / `StartImage` and no Linux step. Recovery is the same path, plus a handoff written only when the chosen target's kind is `SbmCore`.

Read `\EFI\sbm\recovery\manifest.json` (`RecoveryManifest`, schema `1.0`). Each slot uses `slot_name`, `uki_efi_path`, `sha256`, `version`, and `healthy`. The active slot is preferred. A slot is selectable only when `healthy` is true and the file hash matches. The hash is streamed in 64 KiB chunks, up to 2 GiB, so a real UKI is not skipped for size. `mkosi.postoutput` in `sbm-core` copies the UKI to `\EFI\sbm\recovery\slot-a\sbm-core-<arch>.efi` (`sbm-core-aarch64.efi` or `sbm-core-x86_64.efi`). The QEMU fixture manifest uses those names. The bytes in the fixture image are still the fixture EFI.

Before `StartImage` of an `SbmCore` target the menu writes `\EFI\sbm\handoff\current.json` (`EfiHandoffPayload`, schema `1.0`). On the measured runs `launch_mode` was `{"mode":"interactive"}`, `platform.firmware` was `uefi-native`, `platform.secure_boot_state` was `unsupported`, `platform.gop_framebuffer` was true, `platform.uefi_networking` was false, and `network_hint` had `link_up` false and `dhcp_acquired` false.

Load options are UCS-2. When the UKI has a `.cmdline` section, the menu repeats that line and appends external tokens. systemd-stub replaces the embedded line with the EFI load options when Secure Boot is off, and ignores the EFI load options when Secure Boot is on and `.cmdline` is present. Repeating the embedded line keeps `root=` and `console=` in the first case. The acceptance serial showed `load-options embedded` for the Windows fixture (section only) and `load-options embedded-repeated` for Slot A (section plus `sbm.handoff_sha256=`). A file with no `.cmdline` and with handoff tokens uses `load-options external`. The FAT16 test image has no partition GUID, so `sbm.esp_partuuid=` is omitted there.

`LoaderEntrySelected` is also published as a volatile UCS-2 variable (boot-service and runtime access, not non-volatile) under `d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40` and under the systemd vendor `4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`.

`\EFI\sbm\state\last_attempt.json` is `started` before `StartImage`. If `StartImage` returns, the same record is written again as `failed`. A UKI that calls `ExitBootServices` does not return, so this menu will not mark that boot failed. The fixture returns `SUCCESS` and is recorded as failed. Two returning launches left `consecutive_failures` at 2.

`\EFI\sbm\state\boot-success.json` (`BootSuccessReport`) confirms that record when the schema is `1.0`, the target id matches, and `attempt_id` matches or is the Core fallback `"local"`. A match sets status `confirmed`, sets `consecutive_failures` to 0, and deletes the report. A report for a different target is left in place. Rejected JSON is deleted.

`\EFI\sbm\handoff\requested-command.json` (`BootCommand`) is deleted before it is acted on, so a reboot or shutdown cannot loop. The menu then boots a known target, boots a discovered recovery slot, tries firmware setup, reboots, or shuts down. The acceptance image does not contain this file, so the measured menu still waits for a key.

Initrd `LoadFile2` is not installed. A recovery artifact has to be one EFI file.

## What was actually started

The acceptance run chainloaded the fixture, twice per architecture, through a device path. That is the same `LoadImage` / `StartImage` path a UKI uses. It is not SBM Core. No `sbm-core-*.raw` or `sbm-core-*.efi` image was in the `sbm-core` tree, and mkosi is not available on this Mac.

Debian GRUB 2.12 was started for real. `grubaa64.efi` and `grubx64.efi` from the unsigned Debian packages printed `SBM_MARK grub_started` from `grub.cfg` and returned `SUCCESS` to the menu. Details and the package names are in the measurements note. No licensed Windows disk image was available, so Windows Boot Manager itself was not started.

## Lab

`sbm-lab` parses `SBM-LAB-MARKER:` lines. The menu emits `menu-ready` and `chainload-returned` in that form, in addition to the existing `SBM_MARK` lines. xtask writes `build/acceptance-aarch64.json` and `build/acceptance-x86_64.json`. `sbm-lab` was not changed. Its own smoke test still boots its probe, not `sbm.efi`.

## Secure Boot

A development certificate was enrolled only into copies of the QEMU variable templates, and the menu and fixture were signed with `osslsigncode`. Both Homebrew firmware builds still started the unsigned images. The x86 build logs that a third-party image can be loaded. The stock acceptance handoff reports `secure_boot_state` `unsupported`. The host boot configuration was not changed. The full log lines are in the measurements note.

## Menu behavior that changed

- At 1024×768 the six cards use three columns. The titles Windows, Fedora Linux, systemd-boot, Ubuntu, SBM Core Slot A, and SBM Core Slot B fit. Subtitles still ellipsize the filename. The footer hint is short enough to fit on that width. This is not a new renderer.
- Removable targets sort after internal targets of the same OS family. A removable card's subtitle says `Removable`.
- A failed `LoadImage` or a non-success `StartImage` names Secure Boot rejection, a missing file, or an unsupported image. Enter and Esc return to the menu.
- Keyboard: arrows, Enter, Tab, `T` theme, `C` recovery, `D` diagnostics. Escape on the menu does not reboot.

## Limits

- No SNP, DHCP, TLS, or a live Ed25519 exchange. `evaluate_remote` returns immediately and fail-closed when `allow_remote` is false. The serial line is `remote disabled`. There is no path that runs a remote script or a remote command string. The later networking design is [docs/NETWORKING_PLAN.md](docs/NETWORKING_PLAN.md). It uses the existing `CommandEnvelope` and does not add types. It should not be implemented until `sbm-server` and `sbm-common` agree on the same signature field and the same sequence file.
- IA32 is documented in [docs/IA32.md](docs/IA32.md) and is not a built target.
- The menu does not write `BootOrder`. Firmware setup sets `OsIndications` bit 0 and cold-resets only when `OsIndicationsSupported` says the firmware offers it.
- `uefi-wm` (ldmsys/uefi-wm) is a real project and was not used.

## Reproduce

```
cargo test
cargo run -p xtask --offline -- qemu
cargo run -p xtask --offline -- qemu aarch64
cargo run -p xtask --offline -- qemu x86_64
```

`cargo test` is the host suites. The QEMU command builds both release binaries, copies them to `artifacts/`, and boots a 32 MiB FAT16 image. Use `-drive file=<image>,format=raw,if=none,id=esp` with `virtio-blk-pci,drive=esp,bootindex=0`. Do not use `-drive file=fat:rw:<dir>`.

## Coordination

- Recovery consumer: https://github.com/smart-boot-menu/sbm-core/issues/1
- QEMU disk and guest setup: https://github.com/smart-boot-menu/sbm-lab/issues/1
