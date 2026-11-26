# Measurements, 2026-11-26

One successful `cargo run -p xtask --offline -- qemu <arch>` invocation per architecture. These are not physical-hardware samples and not a repeated benchmark.

## Host and firmware

- Apple Silicon M5. QEMU emulator version 11.1.3 (`/opt/homebrew/bin`).
- aarch64 guest: `-machine virt,accel=hvf,gic-version=3 -cpu host`, one `-device ramfb`, `-display none`, USB keyboard on `qemu-xhci`.
- x86_64 guest: `-machine q35,accel=tcg -cpu qemu64`, `-vga std`, `-display none`, debugcon at I/O `0x402`.
- Code firmware is readonly pflash unit 0. Unit 1 is a copy of the variable template.
- aarch64 code and vars: `/opt/homebrew/share/qemu/edk2-aarch64-code.fd` and `edk2-arm-vars.fd`, 67,108,864 bytes each. The aarch64 serial banner was `edk2-stable202408-prebuilt.qemu.org built at 16:28:50 on Sep 12 2024`.
- x86_64 code: `edk2-x86_64-code.fd`, 3,653,632 bytes. Variable template: `edk2-i386-vars.fd`, 540,672 bytes.
- Disk: a 32 MiB FAT16 image (`newfs_msdos -F 16`) on `virtio-blk-pci` with `bootindex=0`. QEMU's `fat:` directory backend was not used. It stored a 2,212-byte handoff as the wrong clusters.
- Release profile: `opt-level = "z"`, LTO, one codegen unit, `panic = "abort"`, strip. `rustc` 1.100.0.

## Guest timer

`first_frame`, `discovery_done`, and `menu_ready` are microseconds on the guest timer from `SBM_MARK uefi_entry`. aarch64 uses `cntpct_el0` / `cntfrq_el0` (24,000,000 Hz in this guest). x86_64 uses `rdtsc` calibrated with a 1 ms `stall` (this run reported 1,018,000,000 Hz). Firmware time before the UEFI entry, and host wall time, are not included. The 250 ms menu goal is a guest-timer goal.

| Arch | GOP | Backbuffer | first_frame | discovery_done | menu_ready | EFI size |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| aarch64 | 1024×768 Bgr, stride 1024 | 3,145,728 | 5,389 µs | 13,846 µs, 6 targets | 20,709 µs | 168,448 |
| x86_64 | 1280×800 Bgr, stride 1280 | 4,096,000 | 28,783 µs | 61,628 µs, 6 targets | 92,312 µs | 187,904 |

Both menu_ready samples are under 250 ms on that guest timer. The x86 number is TCG, so it is not a hardware prediction.

Conventional memory pages after discovery: aarch64 251,873 (32 pages below the pre-discovery sample); x86_64 248,684 (30 pages below). A page is 4,096 bytes. This is the drop in `EfiConventionalMemory`, not a peak-heap trace. The backbuffer above is the RAM copy of the frame.

The fixture EFI is 18,432 bytes on each architecture. Both menu binaries contain the `sbat,1,` CSV and have `IMAGE_DLLCHARACTERISTICS_NX_COMPAT` set. xtask fails the build above 5 MiB. Neither binary is near 1.5 MiB.

## What the run did

Serial listed Windows, Fedora Linux, systemd-boot, Ubuntu, SBM Core Slot A, and SBM Core Slot B. It did not list a target named `sbm`. `remote disabled` followed discovery. Keys `t`, `ret`, and `c` chainloaded Windows and then SBM Core Slot A, both with `chainload method=device-path`. Each target printed `fixture_started` and `chainload_returned status=SUCCESS`.

After quit, the FAT image contained `\EFI\sbm\handoff\current.json` (2,204 bytes, schema 1.0), `\EFI\sbm\state\last_attempt.json` with `"status":"failed"` for `sbm-core:slot-a`, and `\EFI\sbm\test\chainload-ok.txt`. The attempt is `failed` because `StartImage` returned. The fixture returns on purpose.

Host wall time for the warm xtask process, including image setup, firmware, both chainloads, screenshots, and reading the image back: about 10.3 s (aarch64) and 6.3 s (x86_64). That is not menu_ready.

Screenshots from this run: `screenshots/aarch64-menu-dark.png`, `screenshots/aarch64-menu-light.png`, `screenshots/x86_64-menu-dark.png`, `screenshots/x86_64-menu-light.png`.
