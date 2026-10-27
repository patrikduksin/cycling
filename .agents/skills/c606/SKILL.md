---
name: c606
description: Flash, access serial, debug or measure the connected Magene C606 safely, preserving the stock backup, boot metadata and persisted data.
---

# C606

The device-safety rules in [AGENTS.md](../../../AGENTS.md) always apply. This
skill adds the procedure for flashing, device access, testing and reporting.
When the device is on another machine (`CYCLING_DEVICE_HOST`), run these tasks
through the [remote device skill](../remote-device/SKILL.md).

## Firmware: backup, A/B flash, rollback, stock

| Task | Does |
| --- | --- |
| `cycling backup` | One-time full-flash backup into private `.local/device`. Refuses to overwrite an existing backup. |
| `cycling flash` | Rebuilds for the given `CYCLING_SDK`/`CYCLING_HARNESS`, writes the slot that is not selected, verifies it and the reservations, selects and boots it, and requires its console. |
| `cycling rollback` | Validates the cycling image in the other slot and boots it. No slot writes. |
| `cycling stock` | Full restore of the verified 16 MiB backup. Asks for confirmation (`-- --confirm-restore` skips it). |
| `cycling restore-owned <snapshot>` | Writes a saved `.local/device/owned-*.bin` back into the ride/settings reservations while cycling is selected, after saving the current ones; verified readback, then reset. |
| `cycling stock-select` (alias `boot-stock`) | Boots slot A without rewriting flash, only while it still holds the verified stock image. |
| `cycling flash-image` | `flash` for an image built elsewhere: writes the existing `.local/cycling.bin` without building. |
| `cycling monitor` | Reads USB logs without resetting. |

These tasks run [firmware.py](../../../tools/devtools/src/cycling_devtools/device/firmware.py),
which owns the flash geometry. It verifies the device and baseline, validates the
image and ranges, restricts writes to an application slot and OTA selection
(or, for `stock`, the backup itself), and reads back changes.

Stock lives in the verified backup, not on the device: cycling may occupy both
slots (owner decision, 2026-10-17). The backup is `.local/device/flash.bin` with
its `manifest.json`; keep the separate copy too.

- A/B: slot A is `0x20000` (stock's ota_0), slot B `0x760000`. Slot B ends with
  the 1 MiB ride journal and the settings, so images are capped below them in
  both slots (`APP_SIZE`). `flash` writes the slot that is not selected; after a
  full restore that is slot B, and the next flash replaces stock in slot A.
- After the watchdog reset `flash` and `rollback` wait up to 25 s for the
  console to answer `INFO`, whose `slot=` field reports the booted slot. Silence
  or a different slot reselects the previous slot and fails with the reason.
  Firmware without the `slot=` field is reported by OTA selection only.
  `CYCLING_BULK_MAINTENANCE=1` builds mark their descriptor version
  `+maintenance`; for such an image (new, rollback target or previous slot) the
  check uses the maintenance protocol's INFO frame instead (that build has no
  JSON console and needs working MMC). Maintenance images built before the
  marker are probed as JSON and get switched back. A reset
  failure after the selection was written (for example ROM download mode) also
  reselects the previous slot from freshly read metadata.
- `stock` checks every backup hash and the device identity before writing, saves
  the current ride and settings reservations as
  `.local/device/owned-pre-restore-<ns>.bin` (rides at offset 0, settings at
  `0x100000`) and prints the path and the `cycling restore-owned` command
  that puts them back after the next cycling flash, writes only differing sectors (boot metadata
  last, identical bootloader sectors never), reads the whole flash back, and
  resets. It reads 16 MiB twice: allow about five minutes. The reservations
  then hold the backup's bytes, so cycling reports `SETTINGS source=failed
  persistence_error=true` and `EXPORT INFO ... needs_init` and refuses to persist
  until `cycling restore-owned` writes the snapshot back.
- Never use generic flashing tools or write the backup by hand. The recovered
  layout assumes secure boot and flash encryption are disabled.
- Never touch eFuses. Preserve the bootloader, partition table and the ride and
  settings reservations across flashes. No vendor filesystem writes.
- The first backup needs an explicit `--port` or `CYCLING_PORT`, because port
  discovery relies on its manifest. `backup` refuses when slot A holds cycling:
  on a new host, copy the existing verified backup instead. Keep a separate copy of the backup. Never
  overwrite an existing baseline backup.
- The helper resets with the watchdog. RTS hard reset left this device in ROM
  download mode even with stock selected; watchdog reset reached normal stock
  startup, as confirmed by the user.
- Stock can remove the USB port during startup. After reset the helper captures
  briefly with DTR/RTS released, reports unavailable USB and rejects captured ROM
  download mode. Observe physical startup, and record it separately from
  successful attachment.
- If an operation is interrupted, download mode can stay active: inspect the
  private logs and reconnect. ROM download mode stays reachable, so rerun the
  interrupted task, `cycling rollback`, or `cycling stock` for a full restore.
- Stop `cycling monitor` before opening a no-reset reader.

## Device access

- Assign one device owner, including across agents. Terminal, collectors,
  exports, tests and flash share the exclusive `.local/usb.lock`; never run
  simultaneous readers.
- Port selection: `--port`, then `CYCLING_PORT`, then the single connected C606
  matching the backup manifest. `cycling device-port` prints that path without
  opening it. Missing or ambiguous devices fail. Resolve again after flashing,
  reconnecting or renumbering.
- If permissions require sudo, recheck `sudo -n true`; do not assume an old
  authorization window or change security configuration. Reconnection may
  remove a temporary device ACL.
- Use CLI `--help` and device `HELP` for current commands and arguments.
- Ordinary terminal and collector opens do not reset, change DTR/RTS or flush input.
- One pending request is correlated with its reply and sent once. Reconnect never
  replays commands, and a reply from an earlier boot never completes a new request.
- `ACCEPTED` means queued; poll completion where the command requires it. A
  timeout after a mutation is uncertain completion, not permission to retry.
- `cycling screenshot out.png [--scale 2] [--press BUTTON ...]` saves the
  next frame submitted to the panel (harness builds). It is PR proof of rendered
  pixels, not of panel output. A `BUSY power transition` reply after flashing
  means charging standby; `POWER` shows it and `POWER WAKE` leaves it.
- After boot or `RESTART`, `INFO` answers at once but other work is refused
  with `BUSY starting companion=<phase>` until the companion startup check
  ends (3 s, longer after transport faults). It ran nothing, so ask again;
  the harness runner, screenshot and cleanup wait for it with read-only
  `SETTINGS`.
- `cycling latency --press BUTTON` times synthetic presses to the end of the
  next frame submission and reports idle frame rate and time (harness builds).
  It does not observe the panel or physical switches.
- The interactive terminal reads only while awaiting replies. Use `cycling logs`
  for continuous collection. Keep raw bytes and decoded records under `.local/`.

## Build modes

| Variable | `0` | `1` |
| --- | --- | --- |
| `CYCLING_SDK` | Base (default) | Adds VANA cycling workflows |
| `CYCLING_HARNESS` | Removes diagnostic fault controls; keeps terminal, logs, Wi-Fi and the bring-up probe | Diagnostic controls (default) |

- Pass both values to `cycling flash`.
- Record firmware revision, both modes, device timestamps, collection state and
  traffic with results.
- Use harness-disabled builds for performance and power baselines.
- After testing, restore a harness-enabled base with ordinary terminal and logs
  available, and stop readers and owned BLE fixtures.

## Test rules

- Settings mutation tests (for example `cycling persistence-test`) snapshot and
  restore original preferences. Report a failed restoration for inspection; do
  not retry an ambiguous save blindly.
- Preserve existing rides. Never run mutation tests against a ride that must stay
  active. Reclaim only explicitly identified disposable data, after the export
  verification in [ride recording](../../../docs/ride-recording.md).
- Transport starvation (`cycling e2e --transport-recovery`) requires base with
  harness enabled and no ride consumer. It sends a six-second executor stall once;
  allow more than six seconds for acknowledgment. The reply timestamp precedes the
  stall, so judge recovery from later POSITION/INPUT timestamps. Loss is expected
  only when the test establishes it and then observes resumed parser/companion progress.
- GNSS: check transport, parser progress, freshness and recovery. Valid sentences
  without a fix indoors are not a regression by themselves.
- BLE: `cycling bluetooth-echo` verifies reconnect, read/write and notifications.
  A reported MTU is observed, not forced; keep the separate MTU-23 regression.
  BlueZ tasks use system Python. Stop `cycling ble-simulator` afterward to
  unregister its advertisement and GATT application.

## What this does not prove

- Reset completion does not prove boot. A silent or missing port proves neither
  boot failure nor success.
- Logs are bounded and may drop; check sequence gaps and loss counters. ROM and
  panic text may be unstructured. Boot metadata does not prove receipt of earlier
  events, and records lost before attachment cannot be recovered.
- Instrumentation has overhead. Enqueue time excludes facade message formatting;
  USB pump time includes reply traffic. Samples are not worst-case latency, and an
  idle harness is not measured free CPU or power.
- The transport recovery test does not prove every physical overflow path.
- Serial replies and completed LCD transfers prove neither panel output, switch
  behavior nor touch accuracy.
- Battery reports do not establish calibrated capacity or charging behavior.
- Sampled heap minima are not peak memory; reserved stack sizes are not measured
  stack use; event timing is not a power measurement.
- User-reported outdoor success and historical measurements are separate from
  new evidence.

## Reporting

State what was observed and leave unobserved requirements unresolved. Keep raw
coordinates, readings, identifiers, credentials and captures private; publish
sanitized findings with commit-specific provenance in GitHub issues and PRs.
For hardware identity and protocol facts, read the relevant
[C606 research](../../../devices/c606/research) and preserve fitted-part uncertainty.
