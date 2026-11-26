# Measurements, 2026-11-26

One successful `cargo run -p xtask --offline -- qemu` invocation per architecture after the recovery-contract changes. These are guest-timer samples from QEMU, not physical hardware, and not an average.

The 0.1 menu, before this integration, measured `menu_ready` at 20,709 µs (aarch64) and 92,312 µs (x86_64), with EFI sizes 168,448 and 187,904. The binaries below are larger because discovery now reads PE `.cmdline` sections and hashes recovery files in 64 KiB chunks.

## Host and firmware

- Apple Silicon M5. QEMU emulator version 11.1.3 (`/opt/homebrew/bin`).
- aarch64 guest: `-machine virt,accel=hvf,gic-version=3 -cpu host`, one `-device ramfb`, `-display none`, USB keyboard on `qemu-xhci`.
- x86_64 guest: `-machine q35,accel=tcg -cpu qemu64`, `-vga std`, `-display none`, debugcon at I/O `0x402`.
- Code firmware is readonly pflash unit 0. Unit 1 is a copy of the variable template. The host NVRAM is not written.
- aarch64 code and vars: `/opt/homebrew/share/qemu/edk2-aarch64-code.fd` and `edk2-arm-vars.fd`, 67,108,864 bytes each. The aarch64 serial banner was `edk2-stable202408-prebuilt.qemu.org built at 16:28:50 on Sep 12 2024`.
- x86_64 code: `edk2-x86_64-code.fd`, 3,653,632 bytes. Variable template: `edk2-i386-vars.fd`, 540,672 bytes.
- Disk: a 32 MiB FAT16 image (`newfs_msdos -F 16`) on `virtio-blk-pci` with `bootindex=0`. QEMU's `fat:` directory backend is not used.
- Release profile: `opt-level = "z"`, LTO, one codegen unit, `panic = "abort"`, strip. `rustc` 1.100.0.

## Guest timer

`first_frame`, `discovery_done`, and `menu_ready` are microseconds on the guest timer from `SBM_MARK uefi_entry`. aarch64 uses `cntpct_el0` / `cntfrq_el0` (24,000,000 Hz in this guest). x86_64 uses `rdtsc` calibrated with a 1 ms `stall` (this run reported 1,050,000,000 Hz). Firmware time before the UEFI entry, and host wall time, are not included. The 250 ms menu goal is a guest-timer goal.

| Arch | GOP | Backbuffer | first_frame | discovery_done | menu_ready | EFI size |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| aarch64 | 1024×768 Bgr, stride 1024 | 3,145,728 | 5,347 µs | 14,286 µs, 6 targets | 21,765 µs | 206,336 |
| x86_64 | 1280×800 Bgr, stride 1280 | 4,096,000 | 63,780 µs | 139,751 µs, 6 targets | 238,073 µs | 230,912 |

Both `menu_ready` samples are under 250 ms. Other x86_64 TCG samples from the same session, before the final fixture `.cmdline` section, were 193,880 µs and 198,654 µs. The x86 number moves around under TCG and is not a hardware prediction.

Conventional memory pages after discovery: aarch64 251,864 (32 pages below the pre-discovery sample); x86_64 248,674 (30 pages below). A page is 4,096 bytes. This is the drop in `EfiConventionalMemory`, not a peak-heap trace.

The fixture EFI is 18,944 bytes on each architecture. It carries an unsigned `.cmdline` section whose text is `console=tty0`. Both menu binaries contain the `sbat,1,` CSV and have `IMAGE_DLLCHARACTERISTICS_NX_COMPAT` set. xtask fails the build above 5 MiB. Neither binary is near 1.5 MiB.

## What the acceptance run did

Serial listed Windows, Fedora Linux, systemd-boot, Ubuntu, SBM Core Slot A, and SBM Core Slot B. It did not list a target named `sbm`. `remote disabled` followed discovery. Keys `t`, `ret`, and `c` chainloaded Windows and then SBM Core Slot A, both with `chainload method=device-path`.

Windows had `uki cmdline=embedded` and `load-options embedded` (the fixture section, no extra tokens). Slot A had `uki cmdline=embedded` and `load-options embedded-repeated` (the same section plus `sbm.handoff_sha256=`). Each target printed `fixture_started` and `chainload_returned status=SUCCESS`.

Lab lines from the aarch64 serial:

```
SBM-LAB-MARKER: menu-ready us=21765 arch=aarch64 targets=6
SBM-LAB-MARKER: chainload-returned status=SUCCESS
```

The x86_64 lines are the same shape with `us=238073` and `arch=x86_64`. xtask writes `build/acceptance-<arch>.json` from those results. `build/` is gitignored.

After quit, the FAT image contained:

- `\EFI\sbm\handoff\current.json`, schema 1.0, 2,226 bytes on aarch64 and 2,218 bytes on x86_64. `launch_mode` was `{"mode":"interactive"}`. `platform.secure_boot_state` was `unsupported`. `platform.uefi_networking` was false. `platform.page_size_bytes` was 4096. `platform.ram_total_mib` was 1021. `network_hint.link_up` and `dhcp_acquired` were false. Six targets.
- `\EFI\sbm\state\last_attempt.json` with `"status":"failed"` and `consecutive_failures` 2 for `sbm-core:slot-a`. The attempt is `failed` because `StartImage` returned. The fixture returns on purpose.
- `\EFI\sbm\recovery\manifest.json` pointing both slots at `sbm-core-aarch64.efi` or `sbm-core-x86_64.efi`.
- `\EFI\sbm\test\chainload-ok.txt`.

Screenshots from this run: `screenshots/aarch64-menu-dark.png`, `screenshots/aarch64-menu-light.png`, `screenshots/x86_64-menu-dark.png`, `screenshots/x86_64-menu-light.png`. At 1024×768 the six titles fit on three columns. Subtitles still end in an ellipsis.

## Debian GRUB

Separate from the acceptance ESP. Packages `grub-efi-arm64-unsigned_2.12-9+deb13u4_arm64.deb` and `grub-efi-amd64-unsigned_2.12-9+deb13u4_amd64.deb` from `http://deb.debian.org/debian/pool/main/g/grub2/`. The monolithic images are `usr/lib/grub/arm64-efi/monolithic/grubaa64.efi` (2,854,912 bytes, machine `0xAA64`) and `usr/lib/grub/x86_64-efi/monolithic/grubx64.efi` (2,682,880 bytes, machine `0x8664`).

Each was placed at `\EFI\ubuntu\grub*.efi` with a `grub.cfg` that echoes `SBM_MARK grub_started` and exits. The menu discovered one Ubuntu target and `LoadImage` / `StartImage` started it. Serial on both architectures contained `SBM_MARK grub_started` and then `SBM_MARK chainload_returned status=SUCCESS`.

No licensed Windows virtual disk was on this machine. The acceptance Windows card is still the fixture at `\EFI\Microsoft\Boot\bootmgfw.efi`. No `sbm-core` UKI or raw image was present under `sbm-core`, so Slot A in the acceptance run is the fixture under the real filename.

## Secure Boot

`virt-firmware` 26.10 enrolled a locally generated development certificate into copies of the two variable templates (`PK`, `KEK`, `db`, `SecureBootEnable=1`). `osslsigncode` 2.16 signed the menu and the fixture, and `osslsigncode verify` accepted those signatures. The code pflash stayed readonly. The host NVRAM was not written. Keys and enrolled variable copies stayed under `build/`.

The aarch64 firmware still started the unsigned menu and the unsigned fixture. After that boot the variable copy still had no `SecureBoot` or `SetupMode` variable. The x86_64 firmware loaded `SecurityStubDxe`, logged `GetVariable SecureBoot` as not found on the first enrolled store, and logged `[Security] 3rd party image[0] can be loaded after EndOfDxe` for the unsigned menu and the unsigned fixture. Writing `SecureBoot=01` and `SetupMode=00` into that copy did not change the allow line. The acceptance handoff therefore still reports `secure_boot_state` `unsupported`, which is what the menu reads when those two variables are absent.
