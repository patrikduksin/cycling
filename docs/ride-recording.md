# Ride recording and export

Ride workflows belong to the optional cycling SDK, selected with `CYCLING_SDK=1`.
Both diagnostic harness modes support them through the ordinary USB terminal.
Base firmware leaves existing rides untouched. Use device `HELP` for commands
and the [C606 skill](../.agents/skills/c606/SKILL.md) for safe access and capture.

A mutation's ACCEPTED reply is not flash completion. Poll `RIDE STATUS` for its
matching completed token and result before another mutation. Lost replies can
leave completion uncertain; do not blindly repeat a mutation. Startup recovery
and writes run independently of terminal requests, one bounded media step at a
time. There is no automatic deletion or reuse when full.

Persisted compatibility and recovery invariants live beside
[ride_log](../packages/os/src/sdk/ride_log.rs) and
[recorder](../packages/os/src/sdk/recorder.rs). Existing demo records stay readable;
new terminal rides use live observations. Delayed service can reduce sampling,
and reset can lose an uncommitted batch.

Location-free rides are valid. Export does not invent coordinates, satellite
metadata, speed or distance. Battery percentage has no recorded age and may be
stale. BLE transport loss or stale/disconnected measurements invalidate usable
sensor values; the SDK currently selects one HRS or CSC profile at a time.

## Export

Run `mise run ride-export`; consult its `--help` for paths and options. Keep raw
exports, coordinates and sensor readings in ignored `.local/`.

Export is read-only and requires a closed, exportable recorder state. The host
checks every slot's index, length and transport CRC, then rechecks metadata.
It renames the partial raw file only after the complete prefix passes. It also
validates record version, commit, CRC, sequence, source and flags. Invalid occupied
slots remain in the raw prefix and manifest. Restart interrupted exports at zero.

The result includes raw bytes, SHA-256, manifest and reconstructed ride JSON.
Ride IDs restart after clear, so exported identity includes START-slot and raw
content hashes. GPX includes only recorded locations and available system UTC.
Pause/resume, invalid records, missing locations, sequence gaps and backward UTC
split segments. A location-free ride retains raw/JSON and a GPX-unavailable reason.

## Explicit reclaim

Retain a complete export, then run `mise run ride-clear -- .local/exports/<export-id>`.
The helper verifies local length and SHA-256, downloads and hashes the entire
current device prefix, requires identical content, and rechecks metadata and the
upper-slot bound. It holds the shared USB lock through verification, mutation and
completion. Firmware checks the bound and recorder state; the host checks content.

Clear erases and read-verifies only the owned ride reservation. Settings and
other flash/storage regions remain outside it. Success requires every sector to
verify erased. Invalid remnants can be cleared through the same fresh-export
workflow. A power interruption can leave part of the old journal intact. After
failure or timeout, restart, wait for scanning, inspect status and export again
before deciding whether to clear. Never automatically resend clear.
