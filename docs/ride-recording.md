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
- GPX contains only recorded locations and available system UTC. When the clock
  synchronizes mid-ride, earlier samples of the same running stretch (no pause,
  gap or invalid record between) are dated from the first timed sample by active
  time and marked `utc_derived` in JSON. Pause/resume,
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
| Menu | Opens training, sensor status or settings |
| Training | After a short connection check, the ride buttons below apply. |
| Settings | Bottom-left moves to the next field (FTP, LTHR), bottom-right edits it, top-left returns home. While editing, bottom-left lowers and bottom-right raises it by 5 and top-left finishes. Touch: tap a field to edit it, then tap the large minus or plus pad. Edits apply at once and are saved two seconds after the last change. |
| Sensors | Bottom-left moves, bottom-right selects, top-left returns. Choosing a nearby sensor saves it; choosing another of a saved type asks to replace it. A saved sensor opens a page whose only action is Disconnect, which removes it. |

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

The pages are live, totals, lap and climb; the header shows `1/4`. A
confirmed climb (3% over 200 m that gains 20 m) brings up the climb page, and
its end returns to the previous page unless the rider changed page during the
climb.

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
- Selected ANT sensors are saved, one per device type with their full identity,
  and reconnect by themselves: after power-on (without opening Sensors), when a
  sensor wakes late, after signal loss or a sensor power cycle, and after a
  radio restart or device wake. VANA only ever reconnects the saved identity.
  Each failed search waits 1, 2, 4, 8, 16, then 30 seconds before the next
  (on top of the radio's ten-second search), one radio request at a time, with
  saved sensors taken in turn. Discovery pauses reconnection briefly; an
  uncertain radio is not retried until it restarts.
- Sensor tiles keep their positions and show what was observed: live,
  waiting for data, data lost, searching, not found yet (retrying), waiting for
  radio, replacing, radio unavailable or type unsupported.
- Disconnect removes the sensor from the saved sensors at once and closes its
  channel; its pages are ignored from then on. It stays removed through
  reconnection, scans and restarts until selected again.
- Saved sensors are stored with the training profile (see below). A save that
  fails shows NOT SAVED on the Sensors page and is retried every five seconds;
  an unreadable stored record shows SAVED SENSORS UNREADABLE and is kept.
- BLE transport loss or stale/disconnected measurements invalidate sensor values.

Auto-pause and auto-start (on by default, constants in `ride/auto.rs`):

- Wheel speed only; there is no GPS speed. Without a fresh wheel reading nothing
  pauses or starts, and the ride page says `NO SPEED: AUTO PAUSE OFF`.
- Pause: below 3 km/h for 1 s. Wheel speed decays once a revolution is overdue
  (at most one circumference per time since it), so it is below 3 km/h about 3 s
  after the last revolution and the ride pauses about 4 s after standing. Resume: at or
  above 4.5 km/h for 1 s. The 1 s debounce stays in active time; time paused is
  excluded like a manual pause.
- Start: on the ride screen with the ride Ready, at or above 6 km/h for 3 s, through
  the ordinary start request (refused while storage is not ready). Only a Ready
  ride starts itself, not one that is Saved or Recovered.
- Journal: the same Pause, Resume and Start events as the buttons; the record
  format and export are unchanged. The screen says `AUTO PAUSED`.
- A manual pause never auto-resumes. ACT on an auto-pause keeps the ride paused
  as a manual pause; the next ACT resumes. If the wheel sensor is lost during an
  auto-pause for 3 s, the ride resumes.

Recording:

- Live rides sample once per second, on the grid of the start or resume press,
  and commit four samples per flash slot. Late service delays a sample but does
  not stretch later periods; wholly skipped periods count as `dropped`.
- Capacity is shared by every ride until an export and explicit clear; nothing is
  reclaimed automatically. The 1 MiB reservation is 4,096 slots: 4,094 hold
  samples once START and the terminal record are reserved, which is 16,376 s
  (4 h 32 min 56 s) at 1 Hz when empty. Each pause/resume costs two event slots
  plus the unused part of the batch it flushes (about 10 s of capacity).
- The connection check shows `STORAGE h:mm LEFT`. Under one hour left, the ride
  state row turns yellow and reads `STORAGE nM LEFT`, `REC nM LEFT` or
  `PAUSED nM LEFT`; `RIDE STATUS` reports `free_s`. START is refused (`STORAGE
  FULL`) without room for one sample batch.
- When the journal fills mid-ride, the last slot closes the ride with a FULL
  record, later samples are dropped and the screen shows `STORAGE FULL`. The ride
  and every earlier ride stay exportable.
- After power loss, the open ride is closed with a RECOVERED record at its last
  committed active time and the ride screen shows `RECOVERED`; the next START is
  a new ride. Power lost during the very first write of an empty journal leaves
  one occupied invalid slot and recording stays available.
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
| Display clock | Santiago summer time, UTC-3; recorded UTC is unmodified |
| Clock source | Wi-Fi SNTP, with GNSS time of day as a display fallback |

Training profile:

- Power zones are Coggan's seven zones as a percentage of FTP (Z1 up to 55 %,
  then 75, 90, 105, 120 and 150 %); heart rate zones are Friel's five as a
  percentage of LTHR (Z2 from 81 %, then 90, 94 and 100 %). Boundaries round to
  the nearest watt or beat.
- The defaults, FTP 215 W and LTHR 185 bpm, reproduce the earlier fixed power
  table exactly and keep Z5 heart rate from 185 bpm.
- The main ride page shows each zone number in black on a band of the zone
  colour (Z1 grey, Z2 blue, Z3 green, Z4 yellow, Z5 orange, Z6 red, Z7 purple),
  with a gauge of the reading's place within the zone. The stats page shows the
  recorded time in each power and heart rate zone as two bars. Time in zone is
  kept in memory with the other ride totals; it is not part of the ride record.
- `PROFILE`, `PROFILE FTP watts`, `PROFILE LTHR bpm` and `PROFILE OVERWRITE`
  read and set it from the terminal. The profile and the saved ANT sensors are
  one versioned record in the application area of the settings record
  (version 2; version 1 profiles migrate with no saved sensors). A stored record
  this firmware cannot read keeps the defaults in force, with no saved sensors,
  and is never replaced until `PROFILE OVERWRITE`.

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
