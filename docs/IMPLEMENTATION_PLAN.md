# sbm-uefi implementation plan

**Date:** 2026-11-26
**Milestone:** first bootable graphical menu (SBM 0.1 fast path)
**Status:** the 0.1 menu in this plan is implemented. The follow-up local recovery contract, lab markers, and the separate networking plan are in [../HANDOFF.md](../HANDOFF.md). Guest times are in [MEASUREMENTS.md](MEASUREMENTS.md).

## Decisions

### Graphics

`uefi-wm` ([ldmsys/uefi-wm](https://github.com/ldmsys/uefi-wm)) is a real `no_std` GOP window manager (MIT, software framebuffer, keyboard and pointer). It is not the boot menu:

- It targets `uefi` 0.33 and nightly Rust. SBM is on `uefi` 0.41 and stable Rust.
- It is a desktop toolkit (windows, sliders, QR codes) with no tests, and it rejects `BltOnly` GOP modes.
- `fontdue` plus a TTF would dominate the EFI binary for a picker that needs a few dozen widgets.

Slint can draw on UEFI and is GPL-3.0-only for the open-source license, which conflicts with `MIT OR Apache-2.0`. `egui` and `vello_cpu` do not buy a smaller or more deterministic boot picker than a purpose-built renderer. ADR-002 stands: a small `no_std` double-buffered software renderer over GOP.

GOP is a linear framebuffer. An integrated GPU being present does not provide Vulkan, Metal, or a UEFI acceleration API. The renderer writes a RAM backbuffer and presents with a direct framebuffer copy when the pixel format is 32-bit RGB or BGR, otherwise `Blt` (`BltBufferToVideo`), which also covers `Bitmask` and `BltOnly`. Present copies dirty rectangles when the damaged area is under 80% of the frame. There is no native GPU driver in this firmware.

Design tokens are not in `sbm-common` yet (ADR-002 only names them). Colors, spacing, and type sizes for this renderer live in `crates/sbm-gfx` and `docs/DESIGN_TOKENS.md` so `sbm-core` can match them later without a private fork of a wire type.

### Firmware stack

- Rust 2024, `uefi` 0.41.0, `sbm-types` / `sbm-protocol` from `sbm-common` `28d3845` with default features off.
- Host-testable traits for the FAT volume, discovery, policy, and menu state. UEFI calls stay in `sbm-app`.
- Targets: `x86_64-unknown-uefi` and `aarch64-unknown-uefi`. IA32 is documented and not required for this milestone.
- Input is polled. The loop sleeps on `stall`, not `WaitForKeyEx`.
- Configuration, attempt records, and handoff JSON are files under `\EFI\sbm\`. NVRAM is read for `Boot####` and written only for the standard firmware-setup indication and volatile loader variables.
- Remote policy is a fail-closed decision function with a hard 5 s cap. This milestone does not open SNP, DHCP, or TLS, so a missing network adds no delay. Envelopes that are not `boot_select` are rejected with the shared `CommandEnvelope` checks.

### Boot flow

1. Mark entry, open GOP if present, paint a splash (first frame).
2. Detect firmware, GOP, pointer, SNP, and Secure Boot by trying protocols. Missing protocols are skipped.
3. Scan FAT volumes that contain `\EFI` or `\loader\entries`.
4. Resolve `BootPolicy` (local key, one-shot, healthy default, fallback, menu).
5. Interactive menu, or immediate chainload when the policy says so.
6. `LoadImage` / `StartImage` on a device path. On return, show the error or the menu.
7. Recovery entries come from `\EFI\sbm\recovery\manifest.json` and chainload that UKI directly. Linux is not started for ordinary OS selection.

### Testing

- `cargo test` on the host for discovery, policy, NVRAM parsing, layout, input, and pixels.
- QEMU 11.1.3 on the M5: `aarch64` with `hvf`, `x86_64` with TCG, Homebrew EDK2 `edk2-aarch64-code.fd` and `edk2-x86_64-code.fd`. The ESP is a 32 MiB FAT16 image (`newfs_msdos -F 16`), not QEMU's `fat:` directory backend. Vars are copies of the firmware templates.
- A fixture EFI, clearly not SBM Core, stands in for Windows, GRUB, systemd-boot, a UKI, and the recovery slots.

## Non-goals for this milestone

- `smoltcp` / TLS, Ed25519 verification of a live server, Wi-Fi.
- `EFI_LOAD_FILE2` initrd installation. BLS entries that need a separate initrd are discovered; chainload of a single EFI (UKI, GRUB, Windows, recovery) is the path that is tested.
- IA32 images, shim enrollment, and writing `BootOrder`.
- Third-party ext4/NTFS/APFS drivers.
