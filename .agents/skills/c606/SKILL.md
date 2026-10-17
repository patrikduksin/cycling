---
name: c606
description: Flash, access serial, debug or measure the connected Magene C606 safely, preserving stock firmware and persisted data.
---

# C606

The device-safety rules in [AGENTS.md](../../../AGENTS.md) always apply. This
skill adds the procedure for flashing, device access, testing and reporting.

## Firmware: backup, flash, stock

| Task | Does |
| --- | --- |
| `mise run backup` | One-time full-flash backup into private `.local/device`. Refuses to overwrite an existing backup. |
| `mise run flash` | Rebuilds for the given `CYCLING_SDK`/`CYCLING_HARNESS`, verifies stock, writes cycling to slot B and boots it. |
| `mise run stock` (alias `boot-stock`) | Verifies and boots preserved stock firmware. |
| `mise run monitor` | Reads USB logs without resetting. |

These tasks run [firmware.py](../../../tools/devtools/src/cycling_devtools/device/firmware.py),
which owns the flash geometry. It verifies the device and baseline, validates the
image and ranges, restricts writes to the owned application and OTA selection,
and reads back changes.

- Never use generic flashing tools; they can replace protected boot metadata or
  stock ota_0. The recovered layout assumes secure boot and flash encryption are
  disabled.
- Preserve the ride and settings reservations, bootloader, partition table and
  eFuses. No vendor filesystem writes.
- The first backup needs an explicit `--port` or `CYCLING_PORT`, because port
  discovery relies on its manifest. Keep a separate copy of the backup. Never
  overwrite an existing baseline backup.
- The helper resets with the watchdog. RTS hard reset left this device in ROM
  download mode even with stock selected; watchdog reset reached normal stock
  startup, as confirmed by the user.
- Stock can remove the USB port during startup. After reset the helper captures
  briefly with DTR/RTS released, reports unavailable USB and rejects captured ROM
  download mode. Observe physical startup, and record it separately from
  successful attachment.
- If an operation is interrupted, download mode can stay active: inspect the
  private logs, reconnect and run `mise run stock` with the verified backup.
- Stop `mise run monitor` before opening a no-reset reader.

## Device access

- Assign one device owner, including across agents. Terminal, collectors,
  exports, tests and flash share the exclusive `.local/usb.lock`; never run
  simultaneous readers.
- Port selection: `--port`, then `CYCLING_PORT`, then the single connected C606
  matching the backup manifest. `mise run device-port` prints that path without
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
- `mise run screenshot -- out.png [--scale 2] [--press BUTTON ...]` saves the
  next frame submitted to the panel (harness builds). It is PR proof of rendered
  pixels, not of panel output. A `BUSY power transition` reply after flashing
  means charging standby; `POWER` shows it and `POWER WAKE` leaves it.
- `mise run latency -- --press BUTTON` times synthetic presses to the end of the
  next frame submission and reports idle frame rate and time (harness builds).
  It does not observe the panel or physical switches.
- The interactive terminal reads only while awaiting replies. Use `mise run logs`
  for continuous collection. Keep raw bytes and decoded records under `.local/`.

## Build modes

| Variable | `0` | `1` |
| --- | --- | --- |
| `CYCLING_SDK` | Base (default) | Adds VANA cycling workflows |
| `CYCLING_HARNESS` | Removes diagnostic fault controls; keeps terminal, logs, Wi-Fi and the bring-up probe | Diagnostic controls (default) |

- Pass both values to `mise run flash`.
- Record firmware revision, both modes, device timestamps, collection state and
  traffic with results.
- Use harness-disabled builds for performance and power baselines.
- After testing, restore a harness-enabled base with ordinary terminal and logs
  available, and stop readers and owned BLE fixtures.

## Test rules

- Settings mutation tests (for example `mise run persistence-test`) snapshot and
  restore original preferences. Report a failed restoration for inspection; do
  not retry an ambiguous save blindly.
- Preserve existing rides. Never run mutation tests against a ride that must stay
  active. Reclaim only explicitly identified disposable data, after the export
  verification in [ride recording](../../../docs/ride-recording.md).
- Transport starvation (`mise run e2e -- --transport-recovery`) requires base with
  harness enabled and no ride consumer. It sends a six-second executor stall once;
  allow more than six seconds for acknowledgment. The reply timestamp precedes the
  stall, so judge recovery from later POSITION/INPUT timestamps. Loss is expected
  only when the test establishes it and then observes resumed parser/companion progress.
- GNSS: check transport, parser progress, freshness and recovery. Valid sentences
  without a fix indoors are not a regression by themselves.
- BLE: `mise run bluetooth-echo` verifies reconnect, read/write and notifications.
  A reported MTU is observed, not forced; keep the separate MTU-23 regression.
  BlueZ tasks use system Python. Stop `mise run ble-simulator` afterward to
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
