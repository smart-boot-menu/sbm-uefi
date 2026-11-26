# Authenticated UEFI networking plan

This is a plan for a later milestone. The 0.1 menu does not open SNP, DHCP, DNS, or TLS. Ordinary boot and recovery launch stay offline. No remote script or remote command text is executed.

The signing profile has to be agreed with `sbm-server` and `sbm-common` before any bytes are parsed from the network. The types already exist on `sbm-common` `28d3845` and this plan does not add new ones.

## What the firmware may accept

`CommandEnvelope` (`sbm-protocol`) is the only remote payload. `validate_metadata` already requires:

- `device_id` matches the enrolled device
- `sequence` is greater than the last accepted sequence (replay)
- `expires_at_unix` is after `issued_at_unix`, and after `now` when the firmware clock is available
- in UEFI, `required_scope` is `read_only` or `boot_select`
- in UEFI, `payload` is `Boot` (`BootCommand` only)

`signature_ed25519_hex` is verified against `server_command_verify_key_ed25519_hex` from enrollment. A failed signature is a rejection, not a boot. `RunStoragePlan`, `RunOsInstall`, and `RevokeEnrollment` stay out of the firmware even if a future envelope carries them.

`BootCommand` is limited to the variants the menu already performs locally: boot one known target, boot a discovered recovery slot, reboot to firmware setup, reboot, or shut down. `extra_cmdline_preset` stays on the existing allowlist (`quiet`, `verbose`, `diagnostic`). Unknown presets are dropped.

## Transport, after the local path

1. SNP only. No Wi-Fi stack in the firmware.
2. DHCP, then DNS, then one HTTPS GET or POST to the enrolled server URL. TLS 1.3, system time or a pinned certificate set shipped with the build. No custom certificate download.
3. Hard cap 5 seconds, default from `network_policy_timeout_ms` (3 seconds). A keypress cancels the wait and the menu is already on screen.
4. The last accepted `sequence` is stored in `\EFI\sbm\state\command-sequence.json` on the ESP, not in NVRAM. A missing or corrupt file means sequence 0 and fail closed for anything that does not validate.
5. The network is skipped when `allow_remote_boot_select` is false, when SNP is absent, or when the operator is already holding a key. That is the current `evaluate_remote` behavior.

## What this plan does not include

- Arbitrary command strings, shell scripts, or URLs that are fetched and executed.
- A second protocol beside `CommandEnvelope`.
- Blocking the splash or the recovery chainload on DHCP.
- Changing `sbm-common` until server and firmware both review the same sequence file and the same signature field.

`sbm-server` issues the envelopes. `sbm-core` may display a verified or unverified handoff. The firmware remains the component that decides whether a `Boot` payload changes the next boot.
