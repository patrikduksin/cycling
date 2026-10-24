# Ride recording and export

Rides belong to the optional VANA application (`CYCLING_SDK=1`) and work in both
harness modes through the ordinary USB terminal. Base firmware leaves existing
rides untouched. Use device `HELP` for commands and the
[C606 skill](../.agents/skills/c606/SKILL.md) for device access.

## Mutations

- `ACCEPTED` is not flash completion. Poll `RIDE STATUS` for the matching
  completed token and result before another mutation.
- `completed=` reports only the host's own token. Rider buttons and automatic
  actions never replace it, and the host never waits for theirs. `pending=`
  names whichever operation is queued; the recorder takes one at a time, so
  host commands answer `BUSY` and rider presses do nothing until it completes.
  An unread host completion also answers `BUSY` to the next host mutation;
  a status read clears it.
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
- Ride JSON lists `laps`: each lap's number, first sample, active time, UTC,
  position and `started_by` (`ride`, `manual`, `distance`, `step` or
  `distance_step`), or `null` for a ride recorded before laps were kept. GPX
  marks where each lap after the first began with a `wpt` named `LAP n`.
- GPX contains only recorded locations and recorded UTC (Wi-Fi SNTP, else the
  GNSS receiver's clock). When the clock arrives mid-ride, earlier samples of
  the same running stretch (no pause, gap or invalid record between) are dated
  from the first timed sample by active time and marked `utc_derived` in JSON.
  Pause/resume, invalid records, missing locations, sequence gaps and backward
  UTC split segments. A ride whose Wi-Fi time arrives mid-ride switches from
  receiver to network time with a sub-second step, which can split a GPX
  segment (FIT is unaffected). A location-free ride keeps raw/JSON and records
  why GPX is unavailable.

### FIT activities

```sh
mise run ride-export -- --format fit                        # Also writes ride-<id>.fit, listed in fit.json
mise run ride-export -- --from .local/exports/<id> --format fit   # Convert an earlier export offline
```

- [ride/fit.rs](../apps/vana/src/ride/fit.rs) turns a ride into a FIT
  activity (cycling): file_id, device_info, timer start, stop and resume
  events, one record per sample with time, position, distance, speed, heart
  rate, cadence, power, grade and altitude, one lap as each lap ends, then the
  session (with normalized power) and the activity. Laps and session carry
  total calories from the power meter's work (kcal ~ kJ), plus the
  heart-rate estimate for the seconds without power once the rider's body is
  set; invalid without either. The host uses the same encoder as the device (`ride-fit` binary), and `tools/devtools/tests/test_fit_export.py` reads its
  files with Garmin's FIT SDK and fitdecode.
- Samples are dated like GPX: each running stretch from its first UTC sample;
  a stretch without UTC keeps the previous offset, so its pause counts as no
  time. A ride is dated once the receiver reports a dated RMC fix, before or
  during the ride. A ride with no UTC at all has no FIT until `--start-utc-ms`
  gives its start.
- Distance comes from wheel speed only. Without a wheel sensor the file has
  none and readers use GPS.
  Altitude is the barometric one (below); rides recorded before it was kept
  have no elevation, and readers such as Strava fill it from their own map.
  Lap and session `total_ascent` and `total_descent` follow the recorded
  altitudes with the ride page's rules, and are left out without any.
- Each lap's `lap_trigger` is `manual` for a lap press, `distance` for an
  automatic lap or a workout step that lasts a distance, `time` for any other
  workout step (FIT has no workout-step trigger), and `session_end` for the
  last. Laps meet: each starts where the last ended, so a pause at a lap
  boundary belongs to the later lap's elapsed time. The laps' timer times add
  up to the session's. A ride recorded before laps were kept is one lap.

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

## Bulk MMC storage

When the owned MMC partition from #108 is present and initialized, rides record
there instead of internal flash. Each version-1 slot sits in its own 512-byte
sector container, so the export format is the same as for internal flash. The
mechanism is [`sector_log`](../packages/services/src/sector_log.rs) and the
VANA format is [ride/bulk.rs](../apps/vana/src/ride/bulk.rs).

```sh
mise run ride-export -- --medium bulk          # Bulk export; adds store_id, generation, capacity
mise run ride-clear -- --medium bulk .local/exports/<export-id>
```

- Bulk rides live in the RIDES volume of the MMC's volume table (see
  [MMC volumes](mmc-volumes.md)). `RIDE BULK INIT` formats explicitly: it
  writes the volume table if the region has none (two sectors), then ten log
  metadata sectors. Media that is unformatted, foreign or corrupt is never
  formatted implicitly. The whole-region log from before volumes reports
  `state=needs_init layout=legacy` and stays untouched until `RIDE BULK INIT`. A
  corrupt store stays untouched and `corrupt` until someone runs `RIDE BULK INIT`.
  A store whose superblocks were lost while records remain is `corrupt`, not
  unformatted. Export and clear refuse an MMC without a recovered store
  (`STATE`); a store recovered as corrupt exports but never clears. New stores are named from the hardware random source, so a
  reformat never revives an older store's records.
- `RIDE`/`EXPORT` take an optional `BULK` or `INTERNAL` word. Ride control,
  `STATUS` and `HISTORY` default to the medium of the ride on screen, and START
  to the medium the next ride will use; `INIT`, `CLEAR` and `EXPORT` default to
  internal. START is refused while a ride is open on either medium. Bulk status adds `capacity`, `store`,
  `generation`, `boot_reads` and `layout` (`volumes`, `blank`, `legacy`, `lost`
  or `foreign`).
- The 1 GiB RIDES volume holds 2,097,136 records, about 2,330 h at 1 Hz, so the
  Ready screen shows `ROOM FOR 2330 H`.
- Records are append-only and never overwrite a valid record of the current
  store. A checkpoint with the catalog is written every 32 records, after each
  ride ends, and at boot before anything else when one is overdue, so at most
  32 records ever follow the newest checkpoint. Boot reads the volume table, the
  two superblocks, the eight-slot checkpoint ring and 64 sectors past the
  newest checkpoint (77 reads), never the whole region.
- Boot only trusts an end it can prove: the ring holds every checkpoint it
  should, every record after the newest checkpoint is intact and was written
  after it, and no valid record lies further on within those 64 sectors.
  Otherwise the store is `corrupt` but readable: export covers every record
  seen (damaged ones as invalid containers), and nothing is written or cleared
  until `RIDE BULK INIT`. A damaged tail that reads exactly like a power cut
  before those writes is recovered as that cut.
- Rides record to internal flash when the MMC is missing (`NOMEDIA`),
  unformatted, corrupt, failed or without room for another ride. The ride
  screen then shows a yellow `READY INT`/`RECORDING INT`, and `RIDE STATUS`
  reports `fallback=1`. A ride that just filled the MMC still shows as `SAVED`
  until the next one starts. If MMC writes fail during a ride, including on
  pause or resume, the ride continues on internal flash as a new ride with its
  active time and totals carried over (`moved=1`), paused if the rider was
  pausing, and automatically resumed if it was auto-paused. If internal flash
  cannot take the continuation either, the failed MMC ride stays on screen as
  `STORAGE ERROR`. A failed finish ends the ride. The MMC part is recovered on the next
  boot. Internal rides are never migrated.
- Bulk export lists invalid sector containers in `invalid_containers`, exports
  them as zero slots and saves each raw sector as `invalid-sector-N.bin`, with
  its SHA-256 in `invalid_sectors`. Clear refuses unless those files are present
  and intact and the device's raw sectors still match them.
- The store is bound to its region size and accepts a region that later grows
  at its end, so an explicit, owner-run migration can extend it.

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
| Menu | Opens a ride, sensor status, the workout picker (TRAIN) or settings |
| Ride | Opens Ready (GPS, sensors, battery and storage in plain words); ACT or rolling off starts the ride, and the ride buttons below apply. See [UX flows](ux-flows.md). |
| Settings | Bottom-left moves to the next field (FTP, LTHR), bottom-right edits it, top-left returns home. While editing, bottom-left lowers and bottom-right raises it by 5 and top-left finishes. Touch: tap a field to edit it, then tap the large minus or plus pad. Edits apply at once and are saved two seconds after the last change. |
| Sensors | Bottom-left moves, bottom-right selects, top-left returns. Choosing a nearby sensor saves it; choosing another of a saved type asks to replace it. A saved sensor opens a page whose only action is Disconnect, which removes it; a power meter's page also offers Calibrate, which asks it to zero itself. |

Ride buttons keep the system-wide roles: bottom-right acts, bottom-left moves,
top-left marks or backs out. The ride screen shows no button labels; the
status row under the pages names the ride state, and short toasts confirm a
lap. A pause fills the screen. A ride ends only once paused, and only through
the END RIDE? question, so a bump of top-left while riding marks a lap
instead. Touch never starts, pauses, laps or ends a ride.

| Ride state | Bottom-right (act) | Bottom-left (move) | Top-left (mark / back) |
| --- | --- | --- | --- |
| Ready | Start | — | Home |
| Recording | Pause | Next page, wrapping | Lap |
| Paused | Resume | From the paused screen to the ride pages; past the last page, back | Asks END RIDE?: ACT finishes and commits the final samples, then the summary shows `RIDE SAVED`; BACK keeps the ride paused |
| Saved (summary) | Done: Home | Next summary page | Home |

The pages are live, totals, lap and climb; the header shows `1/4`. A
confirmed climb (3% over 200 m that gains 20 m) brings up the climb page, and
its end returns to the previous page unless the rider changed page during the
climb.

Laps:

- A lap press closes the current lap; another within three seconds is ignored.
  Settings AUTO LAP chooses what closes a lap by itself: off, every 1, 2, 5
  (the default) or 10 km of lap distance, or AT START. A lap press restarts the
  distance count. During a workout each step closes a lap instead, and neither
  kind of automatic lap runs. All share one numbered list.
- AT START closes a lap each time the rider rides past where the ride began
  (the first fix, as back to start measures from). A pass comes within 25 m of
  the start and then leaves beyond 35 m while moving, so the lap begins about
  40 m past the start (35 m and the next sample). It counts once the rider has been
  75 m away, which is why a ride never laps at its own start, and GPS jitter
  between 25 m and 75 m cannot fire it twice. More than a minute within 25 m
  is loitering, such as a car park, and is no pass. A pass closes no lap shorter
  than 500 m or 60 s of recorded time; the lap carries on. It needs the
  breadcrumb, so it runs where back to start does.
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
  summaries in RAM. The journal keeps where each lap began, so FIT, JSON and
  GPX exports and saved rides have them, and a ride recovered after power loss
  keeps the laps it committed. See *Lap marks* below.

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

Speed and distance:

- A wheel sensor that measures is the speed source. Without one (none paired,
  lost, or not yet turning) GPS stands in (`ride/gps_speed.rs`): the receiver's
  Doppler ground speed from RMC or VTG when it sends one for the fix, otherwise
  the distance the fix moved over the last 2-4 s (a median of three). The
  record format is unchanged: `speed_mm_s` holds whichever source was used.
- GPS speed needs a fresh fix (at most 2 s old) with HDOP of 5.0 or better and
  at least four satellites when the receiver reports them. A poorer fix gives
  no speed and cannot move or stop the ride. Below 3.6 km/h GPS reads zero:
  standstill jitter adds no distance.
- Without a fix (a tunnel) there is no speed and no distance; the ride keeps
  recording and does not pause. The ride page, the recorded samples and the
  FIT export agree on what was measured; the GPX track shows the gap.
- `POSITION` on the terminal reports `hdop_tenths` and `speed_mm_s`, so an
  outdoor check shows whether the receiver sends Doppler.

Auto-pause and auto-start (on by default, constants in `ride/auto.rs`):

- Only a measured speed pauses, resumes or starts a ride. Without a wheel or
  GPS speed nothing pauses or starts, and the ride page says `AUTO PAUSE OFF`
  for the first 8 s.
- Wheel: pause below 3 km/h for 1 s. Wheel speed decays once a revolution is overdue
  (at most one circumference per time since it), so it is below 3 km/h about 3 s
  after the last revolution and the ride pauses about 4 s after standing. Resume: at or
  above 4.5 km/h for 1 s. Start: at or above 6 km/h for 3 s. The debounce stays
  in active time; time paused is excluded like a manual pause.
- GPS: pause at zero (below 3.6 km/h) for 2 s, resume at or above 7.2 km/h for
  2 s, start at or above 10 km/h for 5 s. In the simulator the pause takes
  about 3 s with Doppler and 7 s from positions; jitter of 2 m at a light
  neither resumes the ride nor adds distance.
- Start happens on the ride screen with the ride Ready, through the ordinary
  start request (refused while storage is not ready). Only a Ready ride starts
  itself, not one that is Saved or Recovered.
- Journal: the same Pause, Resume and Start events as the buttons; the record
  format and export are unchanged. The screen says `AUTO PAUSED`.
- A manual pause never auto-resumes. ACT on an auto-pause keeps the ride paused
  as a manual pause; the next ACT resumes. If every speed source is lost during
  an auto-pause, the ride resumes after 3 s when a wheel measured the stop, or
  10 s when GPS did.

Recording:

- Live rides sample once per second, on the grid of the start or resume press,
  and commit four samples per flash slot. Late service delays a sample but does
  not stretch later periods; wholly skipped periods count as `dropped`.
- Capacity is shared by every ride until an export and explicit clear; nothing is
  reclaimed automatically. The 1 MiB reservation is 4,096 slots: 4,094 hold
  samples once START and the terminal record are reserved, which is 16,376 s
  (4 h 32 min 56 s) at 1 Hz when empty. Each pause/resume costs two event slots
  plus the unused part of the batch it flushes (about 10 s of capacity).
- Ready shows `ROOM FOR n H` (`ROOM FOR n MIN` under an hour). Under one hour left, the ride
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
- Sample bytes 46..48 hold the smoothed barometric altitude (the ride page's
  `Totals` altitude, only with a fresh pressure reading) as FIT's `altitude`
  field: u16 of (metres + 500) * 5, 0.2 m resolution, 0xFFFF for none. It has
  no flag bit: decoders reject unknown flags, so a flag would break reading new
  rides after a firmware rollback. Older samples left the bytes erased, and
  older firmware and host tools never read them, so version 1 is unchanged and
  both directions stay readable. The host JSON has `altitude_m` and the GPX
  `<ele>`.
- Gradient estimates the pressure change over at least 30 metres of wheel
  distance and needs fresh pressure and moving wheel data.

Lap marks:

- Sample byte 37 marks the first sample of each lap with why it began: 0 for
  the ride's first sample (the ride keeps its laps), 1 a lap press, 2 an
  automatic lap, 3 a workout step that lasts a time or until marked, 4 a
  workout step that lasts a distance, 5 a pass of the ride's start point (FIT
  `lap_trigger` position_start). 0xFF, what every earlier sample holds,
  marks nothing. A value this firmware does not know reads as no mark, so
  firmware and host tools from before value 5 keep every sample of a ride with
  position laps and only read those laps as one.
- The lap boundary is at the sample before the mark, so a lap is accurate to
  the one-second sample grid. Laps that close within one sample share its
  mark, and a lap closed while paused marks the first sample after resume.
- A lost mark merges two laps: a torn batch, or one a reset left unflushed,
  takes its marks with its samples.
- A ride that moves from the MMC to internal flash mid-ride is two journal
  rides and two FIT files. The lap open at the move is split across them,
  and the internal part starts with a ride mark, not a lap boundary.
- Values 6 to 254 are reserved for later marks; this firmware and
  `ride-export` read them as no mark. Byte 37 was the last spare sample byte.
- Compatibility decided the place. Firmware recovery, the ride index and
  `ride-export` reject unknown record kinds and unknown sample flag bits, and
  they count a rejected slot as a gap, so a lap record kind or a flag would
  make older firmware and tools mark new rides as gapped or drop their samples.
  Byte 37 lies between battery (36) and power (38); no flag covers it, older
  decoders never read it, and only the slot CRC, which the writer computes,
  includes it. Bytes 46 and 47 were the other spare pair, taken by altitude.
- `ride/log.rs` tests decode marked slots with a frozen copy of the decoder
  from before lap marks, and the host tests decode them with the current
  exporter's unchanged checks.

Fixed settings in this build:

| Setting | Value |
| --- | --- |
| Display clock | Settings TIME ZONE, Chile's summer time rule by default (`clock::Zone`); recorded UTC is unmodified |
| Clock source | Wi-Fi SNTP since boot, else the GNSS receiver's dated fixes (carried on through a lost fix); GNSS time of day as a display fallback |

Wheel circumference starts at 2136 mm (stock RC520 700x28c) and is stored in
the preference record. In WHEEL MODE AUTO, a ride with a speed sensor and a good
GPS fix compares wheel revolutions with GPS distance over windows of about 1 km
(`ride::wheel_size`), and a size that differs by more than 0.5 % and lies within
1500 to 2400 mm replaces the stored one. It applies from the next ride, so a
ride's distance never jumps, and a toast tells the rider once the ride is
saved. MANUAL keeps the size the rider set.

Training profile:

- Power zones are Coggan's seven zones as a percentage of FTP (Z1 up to 55 %,
  then 75, 90, 105, 120 and 150 %); heart rate zones are Friel's five as a
  percentage of LTHR (Z2 from 81 %, then 90, 94 and 100 %). Boundaries round to
  the nearest watt or beat.
- The defaults, FTP 215 W and LTHR 185 bpm, reproduce the earlier fixed power
  table exactly and keep Z5 heart rate from 185 bpm.
- The main ride page shows each zone number in black on a band of the zone
  colour (Z1 grey, Z2 blue, Z3 green, Z4 yellow, Z5 orange, Z6 red, Z7 purple),
  with a gauge of the reading's place within the zone. Its power, and the
  power zone, are the 3 s average: the mean of the last reading in each of the
  last three seconds, zeros included, cleared when the meter goes quiet. The
  stats page shows the recorded time in each power and heart rate zone as two
  bars. Time in zone is kept in memory with the other ride totals; it is not
  part of the ride record.
- The stats page also shows live intensity factor (`IF`, normalized power over
  FTP) and training stress score (`TSS`: seconds with power × NP² / (FTP² ×
  36), so an hour at FTP is 100). Both wait for normalized power's first 30 s
  and use the FTP of the profile.
- The ride index also keeps each saved ride's normalized power and time with
  power (four bytes, in memory, rebuilt after every boot) and works out its TSS
  for the rides list's TSS, at the FTP the ride's facts kept (see Ride facts),
  else at the profile's FTP now. A ride without power has no effort and adds
  nothing.
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

## Structured training

The TRAIN card on Home opens the workout picker: FREE RIDE, the built-ins,
then uploads. MOVE focuses the next workout (held, the next page of four), ACT chooses it and opens Ready,
and BACK returns Home. Ready names the chosen workout under its headline, and
a tap on that line opens the picker again to change it. A chosen workout plays
on the next ride; BACK from Ready drops it.

Built-ins ship in flash, by rising intensity: RECOVERY SPIN 30, ENDURANCE Z2 60
and 90, TEMPO 2X20, SWEET SPOT 3X12 and 2X20, THRESHOLD 4X8, OVER UNDER 2X20,
VO2 MAX 5X3, 6X4 and 30-30S, ANAEROBIC 6X1, SPRINTS 10X10S, OPENERS, FTP TEST
20 MIN and FTP RAMP TEST. A workout is a list
of steps (warm up, work, recovery, cool down or test), each lasting a time, a
distance or until MARK, with a target: % FTP, watts, heart zone, % LTHR,
cadence, a ramp or none. Repeat blocks play a run of steps several times.
Targets follow the training profile, so a new FTP moves every power band.

While riding a workout, the ride pages gain a workout page first (`1/7`). It
shows the step and its target, the 3 s reading against the band (UNDER, ON
TARGET, OVER), the time or distance left, the next step and the whole
workout as a chart. The main page draws the band under its power or heart
gauge. Three low cues count down a step's last seconds and two high ones mark the
next; a toast names it (`WORK 2/5 3:00`). Each step closes a lap.

| Control | Effect |
| --- | --- |
| MARK (TopLeft) while recording | Closes the lap and ends the step: the next step |
| Tap on the time left | One more minute, or 500 m on a distance step |

`WORKOUT STATUS` reports the step, target, reading, compliance, laps and the
time on target; `Runtime::workout_summary()` gives the last workout's summary
to other features.

### Personal bests

The rider's all-time bests (5 s, 1, 5 and 20 min power, longest ride by
distance and by time, most ascent) are ride history, so they live in
`bests.dat` on the FILES volume (`ride::bests`), not in the preference record.
When a live ride finishes, VANA reads the file, merges the ride and writes it
back with the same synchronous write as the workout outcome; a ride that
cannot be stored claims no best. VANA never formats the store: without a
card, a FILES volume or a store the host formatted, bests are not kept (the
host's `files format` erases them). A file that does not decode is never
merged into; only CLEAR BESTS replaces it. A write the store refuses leaves
the merged bests in memory for the next write, and a read that failed only
this time is retried with the ride's outcome waiting (one outcome is kept).
The reader takes at most 84 bytes of the file, so a later version must keep
the whole file within 84 bytes or raise the reader first.

| Bytes | Field |
| --- | --- |
| 0..4 | magic `VBST` |
| 4 | version, 1 |
| 5 | body length `n` before the CRC: 38 in version 1 |
| 6..38 | four power marks (watts and day, u16 each), distance in metres (u32) and day (u16), time in seconds (u32) and day (u16), ascent in metres (u16) and day (u16), little-endian |
| n..n+4 | CRC32 of bytes 0..n |

A day is days since 1970-01-01 in the clock's time zone, 0 when the clock was
unknown; a value of 0 is no best. Later versions only append fields before the
CRC and raise the version, within the 84 bytes above.

FTP tests end on a NEW FTP sheet while the ride keeps recording: ACT saves the
new FTP to the profile, BACK keeps the old one.

- 20 minutes: FTP is 95% of the test step's average power. Skipping the test
  step gives no result.
- Ramp: one-minute steps 20 W apart from 100 W. The test ends when 3 s power
  stays under 90% of the step for 10 s, or at MARK. FTP is 75% of the best
  full minute.

Load a workout from JSON with `mise run workout-load <file> [--name stem]`;
`--dry-run` prints the file it would put. Examples are in
`tools/devtools/workouts/` and the format is described in
`cycling_devtools.workouts.load`. The host writes codec version 1
(`apps/vana/src/workout/codec.rs`) to the MMC's FILES volume as
`workouts/<stem>.vw` through the volume transfer session. That session checks
the CRC on commit and reads the file back. `mise run files -- put
workouts/NAME.vw PATH` works too. Format FILES once first if `FILES STATUS`
says it is not formatted.

VANA lists `workouts/` at boot, each time the picker opens and on
`WORKOUT LIST`. It keeps only each upload's file, name and outline: the first
8 by file name, in name order after the built-ins. Files that do not decode
are counted as `skipped`. A ride reads its workout's steps from FILES when it
starts. If the file is gone by then, the ride runs free and shows WORKOUT
MISSING. `WORKOUT SELECT row` chooses a picker row and opens Ready.
`WORKOUT DELETE n` deletes upload `n`'s file.

When a ride with a workout is saved, VANA writes how the workout went beside
it as `rides/<i|b><ride id>.workout` on FILES. The record holds the name,
test, steps done and skipped, time in, under and over target, the FTP a test
measured, and the ride's start time. Clearing or initializing a journal
deletes its records, as its ride ids start over. It is a versioned 70-byte
record (`apps/vana/src/workout/history.rs`) that later versions only extend.
The newest 32 are kept. `WORKOUT HISTORY INTERNAL|BULK id` reports one. A
ride recovered after a power cut has no record.

### Ride facts

When a ride is saved, VANA writes what the journal does not hold beside it as
`rides/<i|b><ride id>.facts` on FILES (`apps/vana/src/ride/facts.rs`): the FTP
and LTHR in force, the climbs it finished and their gain, and its start time.
Once the summary has measured the heart-rate recovery, the file is written
again with it. The rides list's TSS uses the kept FTP, and a saved ride read
back shows the recovery and the climbs. After boot, and each time the card
becomes ready, VANA reads the files into memory, one directory sector per
tick while no ride is open or being written.

- A file applies to a journal ride only when its journal, ride id and start
  second match. Clearing or initializing a journal deletes its files once the
  clear succeeds, as its ride ids start over; a clear that fails keeps them
  with the rides. Firmware from before facts clears a journal without deleting
  them, and the start time keeps those files off the new rides. So a ride
  whose start time is unknown (no clock) gets no facts.
- A ride that moved from the MMC onto internal flash is two journal rides.
  Its facts go with the part that finished on internal flash; the MMC part
  counts at the FTP now.
- The newest 64 are kept; saving another deletes the oldest. The FILES
  directory holds 128 files: 64 facts, the 32 workout records, `bests.dat` and
  the 8 uploads the picker lists leave 23 for the host. 64 rides cover the two
  weeks the rides list totals at up to four rides a day. Facts give way to
  every other file: a ride saves its workout record first, a new facts file
  never takes the last free directory entry, and one the store refuses as
  full deletes the oldest facts file and tries once more.
- Without a card, a FILES volume or a store the host formatted, nothing is
  written and every reader shows what it showed before facts were kept. A
  write the store refuses is logged and the ride is not held up for it.
- A file is written in one atomic commit. One that is torn, fails its CRC,
  holds a profile out of range or names another ride than its file name is
  ignored, except a profile out of range, which reads as none and leaves the
  other fields. Firmware from before facts lists only `.workout` files under
  `rides/`, so it leaves `.facts` files alone.

It is a versioned 34-byte record that later versions only extend:

| Bytes | Field |
| --- | --- |
| 0..4 | magic `VFCT` |
| 4 | version, 1 |
| 5 | body length `n` before the CRC: 30 in version 1 |
| 6 | journal: `i` internal, `b` bulk |
| 7 | flags: 1 profile, 2 HR recovery, 4 climbs |
| 8..12 | ride id |
| 12..20 | ride start, UTC ms |
| 20..24 | FTP (W) and LTHR (BPM), u16 each |
| 24..26 | HR recovery, BPM fallen (i16) |
| 26..30 | climbs finished and their gain in m, u16 each |
| n..n+4 | CRC32 of bytes 0..n |

The reader takes up to 259 bytes, as much as a one-byte body length allows.

## What this does not prove

- A sudden power loss or reset can lose the uncommitted batch of up to four samples.
- Battery percentage has no recorded age and may be stale.
- Without Wi-Fi time or a GNSS fix, samples have active elapsed time but no calendar
  timestamp, and pause events do not preserve wall-clock pause duration. A
  timestamped upload then needs a supplied start time and cannot recover time
  spent paused.
- Receiver UTC trails true UTC by the receiver's output latency (under a
  second). Whether the C606 receiver reports status A with
  leap-second-uncorrected time before it has UTC parameters is unverified on
  hardware; the clock follows a correction of a second or more, once three
  readings half a second apart confirm it, with one step back. A stall whose
  buffered sentences drain within half a second cannot step it back. Uptime
  drift can still cause one step back of a second after many hours awake.
- Bulk MMC recording is proven on the host and in the simulator only. Endurance,
  latency at 20 MHz, power-cut recovery and coexistence with GNSS, display and
  radios still need hardware validation. The harness `MMC OWNED TEST` writes
  owned sectors and can damage a bulk store.
- Gradient is an approximate barometric grade, not calibrated elevation.
- Altitude is the standard atmosphere, not calibrated to sea level: weather can
  shift absolute altitude by tens of metres, while changes within a ride are
  good. A saved ride's ascent, descent and elevation profile come from the
  recorded altitude; only a ride saved before altitude was recorded estimates
  ascent from the recorded gradient.
- A grey radar rail does not imply an empty road.
