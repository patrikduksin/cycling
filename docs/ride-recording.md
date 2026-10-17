# Ride recording and export

Rides belong to the optional VANA application (`CYCLING_SDK=1`) and work in both
harness modes through the ordinary USB terminal. Base firmware leaves existing
rides untouched. Use device `HELP` for commands and the
[C606 skill](../.agents/skills/c606/SKILL.md) for device access.

## Mutations

- `ACCEPTED` is not flash completion. Poll `RIDE STATUS` for the matching
  completed token and result before another mutation.
- After a lost reply or timeout, completion is uncertain; never blindly repeat
  the mutation.
- Startup recovery and writes run independently of terminal requests, one bounded
  media step at a time.
- Nothing is deleted or reused automatically when storage is full.

Persisted compatibility and recovery invariants live beside
[ride/log.rs](../apps/vana/src/ride/log.rs) and
[ride/recorder.rs](../apps/vana/src/ride/recorder.rs). Existing demo records stay
readable; new terminal rides use live observations.

## Export

```sh
mise run ride-export                          # Writes .local/exports/<id>/ by default; see --help
```

- Export is read-only and requires a closed, exportable recorder state.
- The host checks every slot's index, length and transport CRC, then rechecks
  metadata, and renames the partial raw file only after the complete prefix
  passes. It also validates record version, commit, CRC, sequence, source and flags.
- Invalid occupied slots stay in the raw prefix and manifest. Restart an
  interrupted export from zero.
- Output: raw bytes, SHA-256, manifest, reconstructed ride JSON and GPX. Ride IDs
  restart after clear, so exported identity includes START-slot and raw content hashes.
- GPX contains only recorded locations and available system UTC. Pause/resume,
  invalid records, missing locations, sequence gaps and backward UTC split
  segments. A location-free ride keeps raw/JSON and records why GPX is unavailable.

## Explicit reclaim

```sh
mise run ride-clear -- .local/exports/<export-id>
```

- The helper verifies the local export's length and SHA-256, downloads and hashes
  the entire current device prefix, requires identical content, and rechecks
  metadata and the upper-slot bound. It holds the USB lock through verification,
  mutation and completion. Firmware checks the bound and recorder state.
- Clear erases and read-verifies only the owned ride reservation; settings and
  other regions are outside it. Success requires every sector to verify erased.
  Invalid remnants are cleared through the same fresh-export workflow.
- After failure or timeout, restart, wait for scanning, inspect status and export
  again before deciding whether to clear. Never automatically resend clear; a
  power interruption can leave part of the old journal intact.

## Historical ANT captures

Historical ANT diagnostic records remain occupied slots, preserved by recovery and
raw export. The on-device diagnostic recorder has been removed. Decode a full raw
prefix offline:

```sh
mise run ant-export -- .local/ant-decoded --input .local/exports/<export-id>/ride-slots.bin
```

`--start-slot N` decodes only the tail. The decoder reports invalid ANT records and
sequence gaps and keeps the selected raw bytes.

Keep raw exports, decoded captures, coordinates, sensor readings and identifiers
in ignored `.local/`.

## VANA workout build

Controls:

| Screen | Control |
| --- | --- |
| Menu | Opens training or sensor status |
| Training | After a short connection check, the ride buttons below apply. |
| Sensor scan | Bottom-left moves, bottom-right selects, top-left returns. Wait for PICK SENSOR before selecting another device. |

Ride buttons keep the system-wide roles: bottom-right acts, bottom-left moves,
top-left marks or backs out. The ride screen shows no button labels; the
status row under the pages names the ride state, and short toasts confirm a
lap or ask for the end-ride confirmation. A ride ends only once paused, so a
bump of top-left while riding marks a lap instead. Touch never starts, pauses,
laps or ends a ride.

| Ride state | Bottom-right (act) | Bottom-left (move) | Top-left (mark / back) |
| --- | --- | --- | --- |
| Ready, saved or recovered | Start | Next page, wrapping | Home |
| Recording | Pause | Next page, wrapping | Lap |
| Paused | Resume | Next page, wrapping | End ride: a second press within three seconds (toast `PRESS AGAIN TO SAVE`) finishes and commits the final samples; wait for `saved` before powering off |

The pages are live, totals and lap; the header shows `1/3`.

Laps:

- A lap press closes the current lap; another within three seconds is ignored.
  An automatic lap closes every 5 km of lap distance (fixed in this build), and
  a lap press restarts that count. Both share one numbered list.
- For two seconds a toast in the status row shows `LAP n`, the finished lap's
  time and its average power.
- Lap time is recorded time, so a pause adds none. Lap averages follow the ride
  totals: power counts zeros, speed is lap distance over moving time, heart rate
  counts seconds with a reading.
- The lap page shows the open lap's time, number, distance and average power,
  speed and heart rate, then the previous lap's time and average power. Speed
  and power deltas against the previous lap are green with `+` when higher and
  red with `-` when lower.
- Laps restart with each ride, stay visible after saving, and keep the last 16
  summaries. They live in RAM only: the journal has no lap record, and current
  and older readers (firmware recovery and `ride-export`) treat an unknown record
  kind as an invalid slot that marks the ride as gapped. Exports therefore carry
  no laps, and power-off loses them.

Sensors:

- Four ANT channels cover speed, heart rate, power and radar. VANA selects one
  BLE HRS or CSC profile at a time.
- A scan briefly disconnects the selected channels, scans for ten seconds, then
  restores them. Sensor values are unavailable during the scan.
- Explicitly dropped sensors stay disconnected through reconnection and scans
  until selected again.
- BLE transport loss or stale/disconnected measurements invalidate sensor values.

Recording:

- Live rides sample once per second and commit four samples per flash slot.
  Delayed service can reduce sampling.
- The 1 MiB reservation holds about 4.5 hours when empty, less existing rides and
  event records.
- Sampling continues with missing sensors or GPS; unavailable measurements stay
  absent. Location-free rides are valid, and export does not invent coordinates,
  satellite metadata, speed or distance.
- Optional sample flags 0x40, 0x80 and 0x100 add measured speed in mm/s at byte
  40, power in watts at byte 38 and signed gradient in tenths of a percent at byte
  44. Old slots and the version-1 envelope stay readable; use the current exporter
  for these fields.
- Gradient estimates the pressure change over at least 30 metres of wheel
  distance and needs fresh pressure and moving wheel data.

Fixed settings in this build:

| Setting | Value |
| --- | --- |
| Wheel circumference | 2136 mm (stock RC520 700x28c) |
| Power zones | FTP 215 W with the supplied boundaries |
| HR Z5 | From 185 bpm |
| Display clock | Santiago summer time, UTC-3; recorded UTC is unmodified |
| Clock source | Wi-Fi SNTP, with GNSS time of day as a display fallback |

Radar uses the left edge of the workout screen:

- Each fresh target is a dot; lower means farther behind, and it moves toward the
  rider marker as it approaches. Yellow marks an approaching target, red proximity.
- A short sound plays for a closing target within 40 metres or six seconds of
  closing time, at most once per five seconds, on the workout screen including
  its ready and paused states.
- The decoder expires each target page after two seconds. Missing radar data turns
  the rail grey.

VANA branding and the brief glitch animation appear only on the splash screen.

## What this does not prove

- A sudden power loss or reset can lose the uncommitted batch of up to four samples.
- Battery percentage has no recorded age and may be stale.
- Without a synchronized clock, samples have active elapsed time but no calendar
  timestamp, and pause events do not preserve wall-clock pause duration. A
  timestamped upload then needs a supplied start time and cannot recover time
  spent paused.
- Gradient is an approximate barometric grade, not calibrated elevation.
- A grey radar rail does not imply an empty road.
