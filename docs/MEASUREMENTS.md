# Measurements, 2026-11-27

## Visual architecture consolidation (2026-11-27)

Consolidated Agent 20's accessible vector typography (`Atkinson Hyperlegible Next` Regular/SemiBold + `Atkinson Hyperlegible Mono` Regular, `ttf-parser` 0.25.1 + `ab_glyph_rasterizer` 0.1.10) and unified CPU vector backend (`sbm_gfx::vector`) with `main`'s LBA-ordered paragraph-style partition dashboards (`Bento`, `Schematic`, `Storage`) and Agent 15's UEFI security/performance/handoff fixes. Removed redundant text rendering (`text.rs` / `noto-sans-mono-bitmap`), legacy raster assets (`assets.rs`), duplicate polygon fill/stroke loops (`disk_wrap.rs`), hard-aliased rounded-panel helpers (`draw.rs`), and the ~330-line duplicate `paint_*_legacy` engine (`paint.rs`). Classic remains the sole production layout compiled into `sbm-app` (`experimental-layouts` stays disabled in `sbm-app`).

### Release UEFI binary sizes (`cargo run -p xtask -- build all`)

Profile: `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, `sha2` at `opt-level = 3`, SBAT `.sbat` section (`sbat,1,`) and `IMAGE_DLLCHARACTERISTICS_NX_COMPAT` verified on both binaries.

| Artifact | Pre-consolidation (`60e7c42`, Agent 15 + 8×8) | Consolidated (`Atkinson Next + Mono` + Unified Vector) | Delta | % of 1 MiB Target |
| --- | ---: | ---: | ---: | ---: |
| `artifacts/sbm-aarch64.efi` | 230,912 B (225.5 KiB) | **410,112 B (400.5 KiB)** | +179,200 B (+175.0 KiB) | **39.1%** |
| `artifacts/sbm-x86_64.efi` | 261,632 B (255.5 KiB) | **465,920 B (455.0 KiB)** | +204,288 B (+199.5 KiB) | **44.4%** |
| Bundled TTF fonts (3 files) | 0 B | **101,836 B (99.4 KiB)** | +101,836 B | 9.7% |

Compared with Agent 20's unmerged branch (`+185,344 B` on `aarch64`, `+207,872 B` on `x86_64`), removing the duplicate legacy paint engine reduced net renderer overhead by `6,144 B` (`aarch64`) and `3,584 B` (`x86_64`).

### Native host latency and glyph-cache memory (`cargo run -p sbm-gfx --release --features experimental-layouts --example bench_typography`)

Measured on Apple Silicon M5 (macOS, release build, single-threaded software framebuffer):

#### Classic production layout (`paint_menu_with`, 6 targets)

| Resolution | Cold `Bitmap8x8` | Cold `AtkinsonNext` | Cold `AtkinsonNextMono` | Warm `AtkinsonNextMono` (50f) | Cache Footprint (cap: 192 / 64 KiB) |
| --- | ---: | ---: | ---: | ---: | --- |
| `800×600` | 1,471.6 µs | 2,282.7 µs | **2,306.4 µs** | — | 79 glyphs / 6,218 B (6.1 KiB) |
| `1024×768` | — | — | — | **2,287.6 µs** (1.41× vs uncached) | 79 glyphs / 7,727 B (7.5 KiB) |
| `1280×800` | 1,991.5 µs | 3,009.4 µs | **2,782.0 µs** | **2,603.8 µs** (1.34× vs uncached) | 79 glyphs / 7,727 B (7.5 KiB) |
| `1920×1080` | 3,276.4 µs | 4,240.9 µs | **4,084.6 µs** | **4,079.6 µs** (1.23× vs uncached) | 79 glyphs / 10,492 B (10.2 KiB) |
| `3840×2160` | 9,509.3 µs | 10,589.1 µs | **10,758.0 µs** | — | 79 glyphs / 24,713 B (24.1 KiB) |

Cold splash (`paint_splash_with`): `524.8 µs` (`1024×768`), `621.8 µs` (`1280×800`), `1,160.0 µs` (`1920×1080`). Isolated text pass (6 mixed-role lines, 243 glyphs at `1280×800`): `1,077.5 µs` uncached vs **406.0 µs** cached (**2.65× speedup**, 114 entries / 13,464 B, 0 evictions).

#### Experimental LBA paragraph dashboards (`paint_experimental_with`, `AtkinsonNextMono`, 5-region NVMe with unallocated gap & multi-target partition)

| Layout | Resolution | Cold First Frame | Warm Frame (30f) | Cache Footprint |
| --- | --- | ---: | ---: | --- |
| `bento` | `1280×800` | **10.00 ms** (10,000.2 µs) | **9.36 ms** (9,358.7 µs) | 192 glyphs / 16,841 B (16.4 KiB) |
| `bento` | `1920×1080` | **18.87 ms** (18,871.6 µs) | **17.50 ms** (17,496.1 µs) | 192 glyphs / 22,642 B (22.1 KiB) |
| `schematic` | `1280×800` | **7.54 ms** (7,538.9 µs) | **6.90 ms** (6,898.6 µs) | 189 glyphs / 17,204 B (16.8 KiB) |
| `schematic` | `1920×1080` | **13.58 ms** (13,576.0 µs) | **12.39 ms** (12,391.0 µs) | 184 glyphs / 22,509 B (22.0 KiB) |
| `storage` | `1280×800` | **10.71 ms** (10,710.1 µs) | **9.93 ms** (9,926.4 µs) | 168 glyphs / 14,156 B (13.8 KiB) |
| `storage` | `1920×1080` | **19.40 ms** (19,398.9 µs) | **17.37 ms** (17,369.0 µs) | 164 glyphs / 18,616 B (18.2 KiB) |

### Verification status & QEMU deferral

- Host test suites: `sbm-boot` (26), `sbm-gfx` default (22) & `--features experimental-layouts` (28), `sbm-menu` (3), `sbm-preview` (2 unit + 24 integration scenarios), `xtask` (5) — all passing.
- `cargo clippy -- -D warnings` clean across host, `sbm-gfx --no-default-features`, `sbm-preview --features window`, `sbm-app --target aarch64-unknown-uefi`, and `sbm-app --target x86_64-unknown-uefi`.
- `xtask qemu` was deferred during this consolidation because `sbm-lab`'s host guest-memory reservation was active (`4096 MiB` reserved against `3584 MiB` budget) and recovery images were intentionally not rebuilt. Agent 13 should execute QEMU OVMF qualification (`cargo run -p xtask -- qemu all`) on this revision once the lab slot is free.

## Agent 15 real recovery media

On ARM64 HVF, every measured menu-ready sample was under 27 ms across ten runs with real recovery media. The median was 16.2 ms. The selected 74,513,752-byte UKI took 393.5 ms median to stream and verify at launch. The baseline median menu-ready time under the same test configuration was 2,073.8 ms. [Raw samples and configuration](https://github.com/smart-boot-menu/research/blob/main/validation/evidence/sbm-uefi-agent15-2026-11-27.md)

The ARM64 EFI is 230,912 bytes and the x86-64 EFI is 261,632 bytes. The ARM64 SHA-256 speed setting adds 5,120 bytes and reduces median launch verification from 615.5 to 393.5 ms in this configuration. Read-and-hash timings include firmware storage I/O.

## Earlier measurements


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
