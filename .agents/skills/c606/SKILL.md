---
name: c606
description: Flash, access serial, debug or measure the connected Magene C606 safely, preserving stock firmware and persisted data.
---

# C606

Use [device.py](../../../scripts/device.py) through `mise run backup`,
`mise run flash` and `mise run stock`. The recovered layout assumes secure boot
and flash encryption are disabled. Generic flashing tools can replace protected
boot metadata or stock ota_0. Repository helpers verify the device and baseline,
validate image/ranges, restrict writes to the owned application/OTA selection,
and read back changes. The script owns operative geometry. Preserve ride and
settings reservations, bootloader, partition table and eFuses. No vendor
filesystem writes.

The one-time full-flash backup is private under `.local/device`; keep a separate
copy. Existing baseline backups must not be overwritten. An interrupted operation
can leave download mode active; inspect the private logs, reconnect and use the
stock task with the verified backup. Reset completion alone does not prove boot.
The helper opens USB with DTR/RTS released after reset; stock has stayed dark until
that step. Physical startup still needs observation. If USB is silent immediately after a
safe flash, `mise run monitor -- --seconds 5` can release the control lines and
start/reset the device. Stop monitor before opening a no-reset reader, and record
the startup failure separately from successful attachment.

## Access and collection

Assign one device owner, including across agents. Terminal, collectors, exports,
tests and flash share the exclusive `.local/usb.lock`; never run simultaneous
readers. Use CLI `--help` and device `HELP` for current commands and argument forms.
`CYCLING_PORT` or `--port` selects the port. If permissions require sudo, recheck
`sudo -n true`; do not assume an old authorization window or change security
configuration. Reconnection may remove a temporary device ACL.

Ordinary terminal/collector opens do not reset, change DTR/RTS or flush input.
One pending request is correlated with its reply and sent once. Reconnect must
not replay commands, and replies from an earlier boot must not complete a new
request. `ACCEPTED` means queued; poll completion where the command requires it.
Timeout after a mutation is uncertain completion, not permission to retry.

The interactive terminal reads while awaiting replies, not while waiting for
keyboard input. Use `mise run logs` for continuous collection. Capture raw bytes
and decoded records under ignored `.local/`. Logs are bounded and may drop;
sequence gaps and loss counters matter, ROM/panic text may be unstructured, and
boot metadata does not prove receipt of earlier events. Records lost before
attachment cannot be recovered by the collector. JSON formatting/USB pumping and
instrumentation have overhead. The measured enqueue time excludes facade message
formatting; USB pump time includes reply traffic. These samples are not worst-case
latency, and an idle harness is not measured free CPU or power.

## Build and test choices

`CYCLING_SDK=0` selects base and `1` adds cycling workflows.
`CYCLING_HARNESS=1` enables diagnostic fault controls; `0` removes those controls
while retaining ordinary terminal, logs, Wi-Fi and the public bring-up probe.
Pass both values to `mise run flash`, which rebuilds the image. Record firmware
revision, both modes, device timestamps, collection state and traffic with results.
Use harness-disabled builds for performance/power baselines. Restore a
harness-enabled base after testing, with ordinary terminal/logs available, and
stop readers and owned BLE fixtures.

Settings mutation tests snapshot and restore original preferences. Report failed
restoration for inspection; do not retry an ambiguous save blindly. Preserve
existing rides and never run mutation tests against a ride that must stay active.
Reclaim requires explicitly identified disposable data and the export verification
in [ride recording](../../../docs/ride-recording.md).

For deliberate transport starvation, `base_test.py --transport-recovery` requires
base with harness enabled and no ride consumer. It sends the six-second executor
stall once. Allow more than six seconds for acknowledgment; its reply timestamp
precedes the stall, so use subsequent POSITION/INPUT timestamps for recovery.
Loss is expected only when the test establishes it and then observes resumed
parser/companion progress; this does not prove every physical overflow path.

GNSS can advance valid sentences while reporting no fix indoors. Check transport,
parser progress, freshness and recovery; absence of a current indoor fix alone
is not a regression. User-reported outdoor success and historical measurements
remain separate from new evidence. For BLE, the echo fixture verifies reconnect,
read/write and notification behavior; a reported MTU is observed, not forced.
Retain the separate MTU-23 regression. BlueZ tools use system Python; stop the
simulator to unregister its advertisement and GATT application.

## Evidence

Serial replies and completed LCD transfers prove neither physical panel output
nor switch behavior or touch accuracy. Battery reports do not establish calibrated
capacity or charging behavior. Sampled heap minima are not peak memory; reserved
stack sizes are not measured stack use; event timing is not a power measurement.
State what was observed and leave unobserved requirements unresolved. Keep raw
coordinates, readings, identifiers, credentials and captures private; publish
sanitized findings with commit-specific provenance in GitHub issues/PRs.
For hardware identity and protocol facts, read the relevant
[C606 research](../../../packages/stock/magene-c606) and preserve fitted-part uncertainty.
