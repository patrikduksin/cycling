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

The [ANT outdoor diagnostic capture](../packages/stock/magene-c606/ant.md) appends
its own committed records to unused tail slots in this reservation without erasing.
It excludes ordinary ride writes for the rest of that boot. Restart rescans the
occupied prefix. Raw exports preserve these records; use `mise run ant-export` to
decode ANT captures. Existing rides and their record format remain unchanged.

## VANA workout build

The foreground menu opens training or sensor status. Training shows a short
connection check, then bottom-left starts, pauses or resumes the ride.
Bottom-right twice within five seconds finishes and commits the final samples.
Wait for `saved` before powering off. Top-left returns home when no ride is active.
Sensor scan uses bottom-left to move, bottom-right to select, top-left to return.
Four ANT channels accommodate speed, heart rate, power and radar. Starting a scan
briefly disconnects the selected channels, scans for ten seconds, then restores
those selections. Explicitly dropped sensors stay disconnected through automatic
reconnection and scans until selected again. Wait for PICK SENSOR before selecting an additional device.
Sensor values are unavailable during this scan.

Live rides sample once per second and commit four samples per flash slot.
A sudden power loss can lose the uncommitted batch. The 1 MiB reservation holds
about 4.5 hours when empty, less existing rides and event records. Nothing is
automatically erased. Sampling continues with missing sensors or GPS; unavailable
measurements remain absent. The full read-only export retains pause/resume events
and UTC when available. Without a synchronized clock, samples have active elapsed
time but no calendar timestamp, and pause events do not preserve wall-clock pause
duration. A timestamped upload then needs a supplied start time and cannot recover
the time spent paused.

Optional sample flags 0x40, 0x80 and 0x100 add measured speed in mm/s at byte 40,
power in watts at byte 38 and signed gradient in tenths of a percent at byte 44.
Old slots and the version-1 envelope stay readable; use the updated exporter for
these optional fields. Gradient estimates the pressure change over at least
30 metres of wheel distance. It needs fresh pressure and moving wheel data.
This is an approximate barometric grade, not calibrated elevation.

Wheel circumference is 2136 mm for stock RC520 700x28c tires. Power zones use
FTP 215 W and the supplied boundaries; HR Z5 starts at 185 bpm. The clock uses
Santiago summer time UTC-3 for this September workout build; recorded UTC is
unmodified. Clock synchronization uses existing Wi-Fi SNTP, with GNSS time of
day as a display fallback.

The workout screen reserves the left edge for radar. Each fresh target has a dot:
farther down means farther behind; the dot moves toward the rider marker as it
approaches. Yellow marks an approaching target and red marks proximity. A short
sound plays for a closing target within 40 metres or six seconds of closing time,
at most once per five seconds. Missing radar data turns the rail grey; it does
not imply an empty road. Radar alert playback is active on the workout screen,
including its ready and paused states. The existing decoder expires each target
page after two seconds. VANA branding and the brief glitch animation appear only
on the splash screen.
