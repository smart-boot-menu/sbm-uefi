# Native SBM development (SIMULATED)

Run the real Smart Boot Menu UEFI menu and the real SBM Core shell as ordinary
desktop windows, against a simulated computer described in TOML. No QEMU, no
Linux image, no root.

Everything here is a simulation. It never reads or writes a physical disk, an
EFI variable, the host boot configuration, or anything that needs privileges.
Windows, title bars, screens and Core status lines say SIMULATED.

## Commands

From `sbm-uefi/` (or `just preview …` from the workspace root):

```
cargo xtask sim                                  # menu window, dual-boot scenario
cargo xtask sim --scenario multi-disk --theme light
cargo xtask sim --display 1024x768               # fixed virtual resolution, letterboxed
cargo xtask sim --list                           # scenarios
cargo xtask sim --watch                          # relaunch on save (needs watchexec)
cargo xtask core --scenario bitlocker            # SBM Core only (real sbm-ui)
cargo xtask preview --all-scenarios --screen all # headless PNGs in build/preview/
cargo xtask preview --scenario dual-boot --theme both --sizes 800x600,1280x800,1920x1080
cargo xtask preview --scenario multi-disk --theme both --sizes all   # all display presets
cargo xtask preview --scenario corrupt-boot --keys Right,Enter --out shot.png
```

`cargo xtask sim` builds `sbm-preview` here and `sbm-sim` + `sbm-ui
--features native-sim` in the sibling `../sbm-core` checkout in parallel
(override with `SBM_CORE_DIR`). `--no-core` skips the Core build.

## Keys in the menu window

The production keys work as on firmware: arrows, Enter, Tab, Esc, `T` theme,
`C` recovery, `D` details, and the mouse. Simulator keys:

| Key | Action |
| --- | --- |
| 1–4 / F4 | Switch layout (Classic, Bento, Schematic, Storage) |
| F1 | Help overlay |
| F2 / Shift+F2 | Next / previous scenario |
| F3 | Cycle virtual resolutions (800x600 … 2560x1600, then follow window) |
| F5 | Simulated reset of the current scenario |
| F6 | Toggle progressive hardware discovery |
| F7 / Shift+F7 | Cycle typography mode (Atkinson Next+Mono / Next / Bitmap 8×8) |
| F8 | Toggle high-contrast theme |
| + / - / 0 | Adjust UI scale (75%–200%) / reset to 100% |
| F12 | Screenshot to `build/sim/screenshots/` |
| Cmd+Q | Quit |

After an OS "boots", a SIMULATED screen says what firmware would have started.
Enter confirms the boot (as `sbm-agent` would) and resets; `X` resets without
confirming, so the attempt counts as failed; `P` powers off.

## How it fits together

```
sbm-preview (sbm-uefi)                     sbm-sim + sbm-ui (sbm-core)
  scenario TOML -> MemoryVolume ESPs         session dir -> production parsers
  sbm-boot discovery/policy/attempts         lsblk, ip addr, meminfo, handoff verify
  sbm-menu glue shared with sbm-app          answers sbm-ui over a private socket
  sbm-gfx paint, minifb window      ---->    real Iced shell, SIMULATED title
          Manage Computer writes a real handoff, cmdline and selected target
          <---- requested-command.json, boot-success.json, power
```

- The menu code is the production code: `sbm-boot` discovery, policy,
  attempts and handoff builder, `sbm-gfx` state machine and painter, and
  `sbm-menu`, which is the input mapping, card order, diagnostics and failure
  text that `sbm-app` itself uses.
- Firmware services are replaced by in-memory ESPs (`MemoryVolume`) holding
  small synthetic PE headers. `StartImage` outcomes come from the scenario,
  for example Secure Boot rejection or a corrupt loader.
- Choosing SBM Core (or pressing `C`) writes a real handoff, closes the menu
  window, and runs `sbm-sim`. It serves the session to the unmodified `sbm-ui`.
  Rebooting from Core imports its one-shot boot command and success report
  back onto the simulated ESP and powers the menu on again. Closing the Core
  window counts as a reboot.

## Scenarios

`sim/scenarios/*.toml`: `empty-uefi`, `windows-only`, `linux-only`,
`dual-boot`, `multi-disk`, `low-disk-space`, `bitlocker`, `corrupt-boot`,
`secure-boot-reject`, `existing-sbm`. Sizes are metadata only; nothing is
allocated. Unknown keys are rejected. A minimal file:

```toml
name = "My laptop"
summary = "Windows and Fedora on one SSD"
arch = "x86_64"                  # or aarch64

[firmware]
secure_boot = "enabled"          # disabled | setup_mode | unsupported
ram_mib = 16384

[policy]                         # optional; written as \EFI\sbm\state\policy.json
timeout_seconds = 5
default = "fedora"
failed_boots = 0                 # unconfirmed boots already recorded

[[disk]]
name = "nvme0n1"
model = "Example SSD"
size = "1TB"

[[disk.partition]]
name = "EFI System Partition"
kind = "esp"
size = "512MiB"
loaders = ["windows", "fedora", "sbm"]   # see `known_preset` in src/machine.rs

[[disk.partition]]
name = "Fedora"
kind = "linux"
size = "300GiB"
free = "120GiB"
```

Partition `kind`s are `esp`, `msr`, `windows`, `windows_recovery`, `linux`,
`linux_home`, `swap`, `luks`, `data` and `apfs`. Flags: `bitlocker`, plus
`rejected` and `corrupt` (lists of loader presets).

## Not covered

The simulator does not cover GOP modes, input protocol quirks, real
`LoadImage`/`StartImage`, NVRAM, TPM, timing on real firmware, or a Linux
desktop session. Use `cargo xtask qemu` and `sbm-lab` for those.
`measure.py` reproduces the build, startup, memory and CPU numbers.
