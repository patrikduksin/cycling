# Ride recording and export

Rides belong to the VANA application and work through the ordinary USB
terminal. Use device `HELP` for commands and the
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

The record layout and recovery invariants live beside
[ride/log.rs](../apps/vana/src/ride/log.rs) and
[ride/recorder.rs](../apps/vana/src/ride/recorder.rs).

## Export

```sh
cycling ride-export                          # Writes .local/exports/<id>/ by default; see --help
```

- Export is read-only and requires a closed, exportable recorder state.
- The host checks every slot's index, length and transport CRC, then rechecks
  metadata, and renames the partial raw file only after the complete prefix
  passes. It also validates record version, commit, CRC, sequence, the fixed
  byte 7 and flags.
- Invalid occupied slots stay in the raw prefix and manifest. Restart an
  interrupted export from zero.
- Output: raw bytes, SHA-256, manifest, reconstructed ride JSON and GPX. Ride IDs
  restart after clear, so exported identity includes START-slot and raw content hashes.
- Ride JSON lists `laps`: each lap's number, first sample, active time, UTC,
  position and `started_by` (`ride`, `manual`, `distance`, `step`,
  `distance_step` or `position`), or `null` for a ride with no lap marks. GPX
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
cycling ride-export --format fit                        # Also writes ride-<id>.fit, listed in fit.json
cycling ride-export --from .local/exports/<id> --format fit   # Convert an earlier export offline
```

- [ride/fit.rs](../apps/vana/src/ride/fit.rs) turns a ride into a FIT
  activity (cycling): file_id and device_info, which name the device's
  manifest `product` and `fit_product`, timer start, stop and resume
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
  Altitude is the barometric one (below); a ride with no recorded altitude
  has no elevation, and readers such as Strava fill it from their own map.
  Lap and session `total_ascent` and `total_descent` follow the recorded
  altitudes with the ride page's rules, and are left out without any.
- Each lap's `lap_trigger` is `manual` for a lap press, `distance` for an
  automatic lap or a workout step that lasts a distance, `time` for any other
  workout step (FIT has no workout-step trigger), `position_start` for a pass
  of the ride's start, and `session_end` for the last. Laps meet: each starts
  where the last ended, so a pause at a lap boundary belongs to the later
  lap's elapsed time. The laps' timer times add up to the session's. A ride
  with no lap marks is one lap.

## Explicit reclaim

```sh
cycling ride-clear .local/exports/<export-id>
```

- The helper verifies the local export's length and SHA-256, downloads and hashes
  the entire current device prefix, requires identical content, and rechecks
  metadata and the upper-slot bound. It holds the USB lock through verification,
  mutation and completion. Firmware checks the bound and recorder state.
- Clear erases and read-verifies only the owned ride reservation; settings and
  other regions are outside it. Success requires every sector to verify erased.
  Invalid remnants are cleared through the same fresh-export workflow.
- An internal Clear runs FORMAT's wipe of the internal journal
  ([FORMAT](mmc-volumes.md#format)), and refuses with `STATE` while a ride is
  open on either medium. It records the wipe intent before its first erase,
  shows `CHECKING STORAGE` while storage commands answer `BUSY` and
  `RIDE STATUS` reports its token pending, forgets the records `FILES` keeps
  of the journal's rides, and completes once the intent is settled; the first
  page then shows, with no restart. A boot after a power cut finishes it the
  same way, and the host, which got no completion, reads an empty journal. A
  Clear whose write fails, the card's included while its records go, leaves
  `FORMAT` failed until the next boot and completes with `result=FAILED`.
- An internal `RIDE INIT`, which only a journal that needs one or is corrupt
  accepts, still erases through the recorder, with no intent.
- After a bulk Clear's failure or timeout, restart, wait for scanning, inspect
  status and export again before deciding whether to clear. Never automatically
  resend clear; a power interruption can leave part of the old journal intact.

## Bulk MMC storage

When the owned MMC partition is present and initialized, rides record
there instead of internal flash. Each slot sits in its own 512-byte
sector container, so the export format is the same as for internal flash. The
mechanism is [`sector_log`](../packages/services/src/sector_log.rs) and the
VANA format is [ride/bulk.rs](../apps/vana/src/ride/bulk.rs).

```sh
cycling ride-export --medium bulk          # Bulk export; adds store_id, generation, capacity
cycling ride-clear --medium bulk .local/exports/<export-id>
```

- Bulk rides live in the RIDES volume of the MMC's volume table (see
  [MMC volumes](mmc-volumes.md)). `RIDE BULK INIT` formats explicitly: it
  writes the volume table if the region has none (two sectors), then ten log
  metadata sectors. Media that is unformatted, foreign or corrupt is never
  formatted implicitly. A
  corrupt store stays untouched and `corrupt` until someone runs `RIDE BULK INIT`.
  A store whose superblocks were lost while records remain is `corrupt`, not
  unformatted. Export and clear refuse an MMC without a recovered store
  (`STATE`); a store recovered as corrupt exports but never clears. New stores are named from the hardware random source, so a
  reformat never revives an older store's records.
- `RIDE`/`EXPORT` take an optional `BULK` or `INTERNAL` word. Ride control,
  `STATUS` and `HISTORY` default to the medium of the ride on screen, and START
  to the medium the next ride will use; `INIT`, `CLEAR` and `EXPORT` default to
  internal. START is refused while a ride is open on either medium,
  and answers `BUSY` while the FORMAT question is up. `STATUS` reads
  `state=foreign` over a log of another version, where `INIT` and `CLEAR`
  answer `FOREIGN`; only FORMAT erases one. On a foreign card
  `RIDE BULK CLEAR CONFIRM` answers `STATE`, because the export check for a
  recovered store runs first. Bulk status adds `capacity`, `store`,
  `generation`, `boot_reads` and `layout` (`volumes`, `blank`, `lost`
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
  page then shows an amber toast such as `READY INT` or `RECORDING INT`, and `RIDE STATUS`
  reports `fallback=1`. A ride that just filled the MMC still shows as `SAVED`
  until the next one starts. If MMC writes fail during a ride, including on
  pause or resume, the ride continues on internal flash as a new ride with its
  active time and totals carried over (`moved=1`), paused if the rider was
  pausing, and automatically resumed if it was auto-paused. If internal flash
  cannot take the continuation either, the failed MMC ride stays on the ride
  page with the `STORAGE ERROR` toast. A failed finish ends the ride. The MMC part is recovered on the next
  boot. Internal rides are never migrated.
- Bulk export lists invalid sector containers in `invalid_containers`, exports
  them as zero slots and saves each raw sector as `invalid-sector-N.bin`, with
  its SHA-256 in `invalid_sectors`. Clear refuses unless those files are present
  and intact and the device's raw sectors still match them.
- The store is bound to its region size and accepts a region that later grows
  at its end, so an explicit, owner-run migration can extend it.

## ANT diagnostic records

A journal can hold ANT diagnostic records. They are occupied slots that
recovery and raw export keep, and firmware does not write them. Decode a full
raw prefix offline:

```sh
cycling ant-export .local/ant-decoded --input .local/exports/<export-id>/ride-slots.bin
```

`--start-slot N` decodes only the tail. The decoder reports invalid ANT records and
sequence gaps and keeps the selected raw bytes.

Keep raw exports, decoded captures, coordinates, sensor readings and identifiers
in ignored `.local/`.

## What a ride records

[UX flows](ux-flows.md#the-ride) owns the ride's screens and buttons, with
[Sensors](ux-flows.md#sensors), and [UI](ui.md) owns how they look, down to
the [power-on brand](ui.md#motion). This
section keeps what decides the recorded data.

Laps:

- A lap press closes the current lap; another within three seconds is ignored.
  `OPTION AUTO_LAP` starts at every 5 km; its choices are in
  [UX flows](ux-flows.md#recording). A lap press restarts the distance count
  of an automatic lap. During a
  workout each step closes a lap instead, and neither automatic lap runs. All
  laps share one numbered list.
- AT START follows the rules in [UX flows](ux-flows.md#recording). It reads
  the breadcrumb's passes of the start, so it runs wherever the breadcrumb
  does.
- For two seconds a toast shows `LAP n`, the finished lap's time and its
  average power.
- Lap time is recorded time, so a pause adds none. Lap averages follow the ride
  totals: power counts zeros, speed is lap distance over moving time, heart rate
  counts seconds with a reading.
- Laps restart with each ride, stay visible after saving, and keep the last 16
  summaries in RAM. The journal keeps where each lap began, so FIT, JSON and
  GPX exports and saved rides have them, and a ride recovered after power loss
  keeps the laps it committed. See *Lap marks* below.

Sensors:

- VANA holds up to ten saved ANT+ sensors, one per device type: power,
  fitness equipment, shifting, Di2 shifting, bike lights, radar, heart rate,
  speed and cadence, cadence, and speed. It selects one BLE HRS or CSC
  profile at a time.
- A search runs for ten seconds beside the connected sensors, which keep
  reporting through it.
- Saved ANT+ sensors keep their full identity and reconnect by themselves:
  after power-on (without opening Sensors), when a sensor wakes late, after
  signal loss or a sensor power cycle, and after a radio restart or device
  wake. VANA only ever reconnects the saved identity. Each failed search waits
  1, 2, 4, 8, 16, then 30 seconds before the next (on top of the radio's
  ten-second search), one radio request at a time, with saved sensors taken in
  turn. Discovery pauses reconnection briefly; an uncertain radio is not
  retried until it restarts.
- A forgotten sensor's channel closes at once, and its pages are ignored from
  then on, through reconnection, searches and restarts.
- Saved sensors are stored with the training profile (see below). A save that
  fails is retried every five seconds, and the Sensors pages say NOT SAVED; an
  unreadable stored record shows SAVED SENSORS UNREADABLE and is kept.
- BLE transport loss or stale/disconnected measurements invalidate sensor values.

Speed and distance:

- A wheel sensor that measures is the speed source. Without one (none paired,
  lost, or not yet turning) GPS stands in (`ride/gps_speed.rs`): the receiver's
  Doppler ground speed from RMC or VTG when it sends one for the fix, otherwise
  the distance the fix moved over the last 2-4 s (a median of three).
  `speed_mm_s` holds whichever source was used.
- GPS speed needs a fresh fix (at most 2 s old) with HDOP of 5.0 or better and
  at least four satellites when the receiver reports them. A poorer fix gives
  no speed and cannot move or stop the ride. Below 3.6 km/h GPS reads zero:
  standstill jitter adds no distance.
- Without a fix (a tunnel) there is no speed and no distance; the ride keeps
  recording and does not pause. The ride page, the recorded samples and the
  FIT export agree on what was measured; the GPX track shows the gap.
- `POSITION` on the terminal reports `hdop_tenths` and `speed_mm_s`, so an
  outdoor check shows whether the receiver sends Doppler.

Auto-pause (`OPTION AUTO_PAUSE`, on by default; constants in `ride/auto.rs`).
A ride starts only from the rider: riding off on Home or Ride done starts
nothing.

- Only a measured speed pauses or resumes a ride. Without a wheel or GPS speed
  nothing pauses.
- Wheel: pause below 3 km/h for 1 s. Packets that repeat the last count bound
  the speed to one circumference per the time past the sensor's count interval
  plus 300 ms; silence alone does not decay it. A stop pauses the ride about 4 s
  plus that interval after the packet carrying the last new count: about 4.25 s
  for a sensor that counts each revolution at speed, later the slower the wheel
  turned, and about 5 s for one that counts once a second, whose last count
  trails the last revolution. The interval is the latest time between counts by
  the sensor's event times, capped at 1.5 s, and is not updated when the reading
  had already fallen to zero or more than 1 s passed without a packet. Resume:
  at or above 4.5 km/h for 1 s. The debounce stays in active time; time paused
  is excluded like a manual pause.
- GPS: pause at zero (below 3.6 km/h) for 2 s, resume at or above 7.2 km/h for
  2 s. In the simulator the pause takes about 3 s with Doppler and 7 s from
  positions; jitter of 2 m at a light neither resumes the ride nor adds
  distance.
- The journal gets the same Pause and Resume events as the buttons; the
  record format and export do not tell them apart.
- A manual pause never auto-resumes. ACT on an auto-pause keeps the ride paused
  as a manual pause; the next ACT resumes. If every speed source is lost during
  an auto-pause, the ride resumes after 3 s when a wheel measured the stop, or
  10 s when GPS did.

Recording:

- Live rides sample once per second, on the grid of the start or resume press,
  and commit four samples per flash slot. Late service delays a sample but does
  not stretch later periods; wholly skipped periods count as `dropped`.
- Capacity is shared by every ride until an export and explicit clear; nothing is
  reclaimed automatically. The internal journal is as large as the owned
  `Data` reservation's `Geometry.capacity`, in whole 4 KiB sectors. The
  C606's 1 MiB reservation is 4,096 slots: 4,094 hold
  samples once START and the terminal record are reserved, which is 16,376 s
  (4 h 32 min 56 s) at 1 Hz when empty. Each pause/resume costs two event slots
  plus the unused part of the batch it flushes (about 10 s of capacity).
- Ready shows `ROOM FOR n H` (`ROOM FOR n MIN` under an hour). Under one hour
  left, the ride page shows an amber toast, `STORAGE nM LEFT`, `REC nM LEFT` or
  `PAUSED nM LEFT`; `RIDE STATUS` reports `free_s`. START is refused (`STORAGE
  FULL`) without room for one sample batch.
- When the journal fills mid-ride, the last slot closes the ride with a FULL
  record, later samples are dropped and the ride page says `STORAGE FULL`. The
  ride and every earlier ride stay exportable.
- After power loss, the open ride is closed with a RECOVERED record at its last
  committed active time and the ride page says `RIDE RECOVERED`; the next START
  is a new ride. That ride reads the recovered ride's positions back from the
  journal, so the map's track and AT START laps go on from where the cut ride
  began; its totals and laps start from zero. Power lost during the very first
  write of an empty journal leaves one occupied invalid slot and recording
  stays available.
- Sampling continues with missing sensors or GPS; unavailable measurements stay
  absent. Location-free rides are valid, and export does not invent coordinates,
  satellite metadata, speed or distance.
- Optional sample flags 0x40, 0x80 and 0x100 add measured speed in mm/s at byte
  40, power in watts at byte 38 and signed gradient in tenths of a percent at byte
  44.
- Sample bytes 46..48 hold the smoothed barometric altitude (the altitude the
  ride page shows, only with a fresh pressure reading) as FIT's `altitude`
  field: u16 of (metres + 500) * 5, 0.2 m resolution, 0xFFFF for none. It has
  no flag bit. The host JSON has `altitude_m` and the GPX `<ele>`.
- Gradient estimates the pressure change over at least 30 metres of wheel
  distance and needs fresh pressure and moving wheel data.

Rides kept in memory ([UX flows](ux-flows.md#rides-kept-in-memory) has what
the rider sees):

- `Geometry.durable` of the `Data` reservation says whether the internal
  journal survives power-off. A memory-backed reservation (`[owned]` with
  `memory = "psram"` or `"ram"` in the device manifest) is not durable. The
  device fills it with the erased byte at every boot, so the journal keeps the
  same format and scans empty. Rides on the bulk log are always durable.
- A ride is protected from the pass its recording starts on a volatile
  journal, a continuation segment included. The protected set lives in RAM,
  up to 16 rides (`app/sync.rs`).
- Each FIT download of a protected ride carries a random receipt in its
  `X-Receipt` header, and the pass that writes the file keeps the CRC in its
  last two bytes. `POST /api/synced?activity=<id>&receipt=<receipt>&crc=<crc>`
  confirms the ride only when the receipt and the CRC are the newest
  download's: 204 removes the ride from the set, 409 `not_this_export`
  names a mismatch, 404 `not_protected` a ride that is not in the set, and
  400 `acknowledgment` a malformed query. `/api/activities` marks each
  protected ride `"unsaved":true`, and `/api/status` reports `"volatile"`
  for the internal journal and `"clear"` for the page's clear.
- A volatile journal admits 16 starts, continuation segments included, until
  it is cleared. A start past that answers `STATE`. A continuation past it
  does not start, and the bulk part stays the rider's ride.
- An internal `RIDE CLEAR CONFIRM`, or `DELETE /api/activities` from the phone
  page, asks LOSE n UNSAVED RIDES? on the device while rides are protected.
  `RIDE STATUS` reports the token pending until the rider answers. BACK
  completes it with `result=FAILED` and keeps the journal. ACT empties the set
  and runs the clear. `DELETE /api/activities` on a durable journal answers
  409 `durable`, and 409 `indexing` before the page's list is read.
- While rides are protected, every controlled way of losing RAM refuses:
  `RESTART`, `POWER SHUTDOWN`, `POWER SLEEP`, `FORMAT CONFIRM` and the
  C606's `TEST 11` and `TEST 12` answer `UNSYNCED`. The TURN OFF sheet and the
  idle timer refuse too.
- A forced reboot cannot be refused: holding BACK for 10 s, a panic or a
  power cut. The next boot starts the journal empty, so it loses every ride
  in memory, protected or not, and the protected set with them.

Lap marks:

- Sample byte 37 marks the first sample of each lap with why it began: 0 for
  the ride's first sample (the ride keeps its laps), 1 a lap press, 2 an
  automatic lap, 3 a workout step that lasts a time or until marked, 4 a
  workout step that lasts a distance, 5 a pass of the ride's start point (FIT
  `lap_trigger` position_start). 0xFF marks nothing.
- The lap boundary is at the sample before the mark, so a lap is accurate to
  the one-second sample grid. Laps that close within one sample share its
  mark, and a lap closed while paused marks the first sample after resume.
- A lost mark merges two laps: a torn batch, or one a reset left unflushed,
  takes its marks with its samples.
- A ride that moves from the MMC to internal flash mid-ride is two journal
  rides and two FIT files. The lap open at the move is split across them,
  and the internal part starts with a ride mark, not a lap boundary.
- Values 6 to 254 are reserved for later marks; this firmware and
  `ride-export` read them as no mark.

Clock:

- `OPTION ZONE` sets the displayed clock, Chile's summer time rule by
  default (`clock::Zone`); recorded UTC is unmodified.
- The clock comes from Wi-Fi SNTP since boot, else the GNSS receiver's dated
  fixes (carried on through a lost fix); GNSS time of day is a display
  fallback.

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
- The defaults are FTP 215 W and LTHR 185 bpm.
- The main page's power, and the power zone, are the 3 s average: the mean of
  the last reading in each of the last three seconds, zeros included, cleared
  when the meter goes quiet. [UI](ui.md#charts) draws the zone gauges and the
  time-in-zone bars. Time in zone is kept in memory with the other ride
  totals; it is not part of the ride record.
- `PROFILE`, `PROFILE FTP watts` and `PROFILE LTHR bpm`
  read and set it from the terminal. The profile and the saved ANT sensors are
  one versioned record in the application area of the settings record
  (`app::preferences`). A stored record this firmware did not write keeps the
  defaults in force, with no saved sensors, and is never replaced: the
  device offers FORMAT (see [volumes](mmc-volumes.md#format)).

While a radar is paired, the ride pages keep a strip on their left edge for it:

- A white arrow at the top is the rider. Each fresh target is a dot on the
  track below it, lower the farther behind, out to 200 m. A dot is yellow
  (`POWER`), and red when the target closes and is within 40 m or six seconds.
- The track is `ACCENT` while the radar reports, amber while it is
  unavailable, and red when it reports an error or an undefined threat.
- The decoder expires each target page after two seconds.
- The radar's sounds are in [UI](ui.md#sounds).

## Structured training

VANA holds one workout, today's, and ships none of its own. Home's WORKOUT
card starts a ride that plays it, as RIDE starts a free one; with none loaded
it shows NO WORKOUT LOADED. A workout is a list
of steps (warm up, work, recovery, cool down or test), each lasting a time, a
distance or until MARK, with a target: % FTP, watts, heart zone, % LTHR,
cadence, a ramp or none. Repeat blocks play a run of steps several times.
Targets follow the training profile, so a new FTP moves every power band.

While riding a workout, the ride pages gain a workout page first. It
shows the step and its target, the 3 s reading against the band (UNDER, ON
TARGET, OVER), the time or distance left, the next step and the whole
workout as a chart. The main page draws the band under its power or heart
gauge. Three low cues count down a step's last seconds and two high ones mark the
next; a toast names it (`WORK 2/5 3:00`). Each step closes a lap. When the
last step ends, the toast says `WORKOUT DONE` and a rider on the workout page
moves to the main page; one on another page stays there.

| Control | Effect |
| --- | --- |
| MARK (TopLeft) while recording | Closes the lap and ends the step: the next step |
| Tap on the time left | One more minute, or 500 m on a distance step |

`WORKOUT STATUS` reports the step, target, reading, compliance, laps and the
time on target.

### Course

A course is a planned route the rider loads from the phone page (`PUT
/api/course` with a GPX file). VANA keeps one, as `course.dat` on the FILES
volume (stored by [kept/course.rs](../apps/vana/src/kept/course.rs), its bytes
defined in [ride/course.rs](../apps/vana/src/ride/course.rs)), and reads it back at boot and when the card becomes
ready. In memory it is up to 2,048 points in PSRAM beside the breadcrumb. The
upload streams through `ride::course::gpx`, which takes the first track of
two points or more (or, without one, the first such route), joins a track's
segments and refuses the whole file when a
point has no real latitude and longitude, or when fewer than two points
remain. Points closer than 5 m to the last one kept are dropped, and a full
list is thinned in place, keeping a point wherever dropping it would move the
line more than a tolerance (4 m, doubled only when a pass frees too little), so
a course of any length fits and keeps its shape: a 200 km winding road stays
within 26 m of the road. The upload replaces
the file in one atomic commit only once the whole body has read as a course;
an upload that fails, or that the phone cuts off, leaves the stored course,
which VANA then reads back. Body bytes count as activity, so a slow upload
does not end the phone session as idle. `DELETE /api/course` removes it. Without a card, a
FILES volume or a formatted store, no course is kept.

| Bytes | Field |
| --- | --- |
| 0..4 | magic `VCRS` |
| 4 | version, 2 |
| 5 | header length, 34 |
| 6 | point length, 8 |
| 7 | name length, up to 24 |
| 8..10 | points `n`, 2 to 2048 (u16) |
| 10..34 | the name: `A-Z`, `0-9`, space and `-._'`, zero padded |
| 34.. | `n` points of 8 bytes: latitude, then longitude, in 1e-7 degrees (i32 each), little-endian |
| 34+8n.. | CRC32 of every byte before it |

A file of another version or shape reads as damaged and is left alone until
an upload replaces it.

The upload also finds the course's turns, from every point of the file a
metre apart or more, before thinning (`ride::course::stored`): the turn scan runs
as points stream in and keeps only the last 512 of them (4 KiB), all it
reads back. Where the points jitter, as a recorded GPS track's do, each is
first moved to the mean of the points within 6 m along either side, so the
jitter reads as no bend; a drawn course is read as it is. The points jitter
where, from point to point, how far each lies to the side of the line between
its neighbours swings by a metre or more at most of them. Each turn is found
in metres along the full track and moved onto the course as kept, in
proportion between the kept points either side; the gates' turns land within
3 m of where the full track's point projects onto the kept course. The turns
are written as `course-turns.dat` once `course.dat` is committed, and removed
with it. The file store commits one file at a time, so the turns name their
course by its key: the CRC that ends `course.dat`. Turns left by a cut
between the two writes are not that course's. A course without its own turns, or with a turns file that does
not decode, has them found by scanning the course as kept when it is placed;
so does a course whose scan at upload needed more than the 512
points it keeps, and its turns file is removed. Placing a course reads its
turns into the 1,024-turn list and moves them into the ride's frame. The list
holds 1,024 turns at a time: on a course with more, the scan waits once the
list is full until the rider is past half of it, and those turns make way.
A turns file flagged as holding fewer than the course has is followed the
same way by the scan of the course as kept, which lists only turns starting
after the end of the last stored one.

| Bytes | Field |
| --- | --- |
| 0..4 | magic `VTRN` |
| 4 | version, 2 |
| 5 | header length, 16 |
| 6 | turn length, 12 |
| 7 | flags: 1, the course has more turns than are stored; other bits zero |
| 8..12 | the course's key: the CRC that ends `course.dat` |
| 12..14 | turns `n`, up to 1024 (u16) |
| 14..16 | zero |
| 16.. | `n` turns of 12 bytes, in order along the course: from and to in millimetres along the course as kept (u32 each), kind (0 none, 1 left, 2 right, 3 sharp left, 4 sharp right, 5 U-turn), flags (1 two corners, 2 cornered), two zero bytes |
| 16+12n.. | CRC32 of every byte before it |

A file of another version or shape reads as one that does not decode.

FTP tests measure an FTP, which `WORKOUT STATUS` reports and the workout's
record keeps; the profile's FTP changes only when the rider sets it.

- 20 minutes: FTP is 95% of the test step's average power. Skipping the test
  step gives no result.
- Ramp: an open warm-up, then one-minute steps from 50% of FTP, 6% of FTP more
  each, from the MARK that ends the warm-up (the page says LAP TO START). The
  test ends when 3 s power stays under 90% of the step for 10 s, or at MARK,
  and it is the workout's last step, with no cool down. The page says HOLD and
  the step's watts, and the target band, drawn on the gauge, is 95% to 110% of
  them. FTP is 75% of the best full minute.

Load today's workout from JSON with `cycling workout-load <file>`;
`--dry-run` prints the file it would put. Examples are in
`tools/devtools/workouts/` and the format is described in
`cycling_devtools.workouts.load`. The host writes codec version 2
(`apps/vana/src/workout/codec.rs`) to the MMC's FILES volume as
`workouts/today.vw` through the volume transfer session, in place of the one
before. That session checks the CRC on commit and reads the file back; the
tool then sends `WORKOUT LOAD`. `cycling files put workouts/today.vw PATH`
works too, followed by `WORKOUT LOAD`. Format FILES once first if `FILES
STATUS` says it is not formatted.

The phone page sets today's workout too (`PUT /api/workouts`, the same codec file as
the body, at most `ENCODED_MAX` (475) bytes; the page builds it from the JSON
in the browser, byte for byte as `workout-load` does). The file is gathered in
the phone server's PSRAM, then VANA decodes and validates it before
anything is written, so a damaged, cut-off or unridable file changes
nothing. It is stored as `workouts/today.vw` in one atomic commit, so a cut
leaves the old file. Answers: 200 with today's workout, 400 `empty`, 409
`ride`, 413 `too_large`, 422 `format`, `damaged` or `invalid`, 503
`unavailable` or `storage`, 507 `nospace`. `GET /api/workouts` reports today's
workout, read from FILES again, as `{"store":"ok","workout":{"name",
"minutes","steps"}}` (`workout` is `null` without one, and `store` names why a
file could not be read). `DELETE /api/workouts` removes it. The phone session
ends when a ride starts and these routes also refuse while one records.

VANA reads today's workout, steps and all, at boot, on `WORKOUT LOAD` and
after the phone changes it. `WORKOUT LOAD` answers `workout=NAME minutes=M
steps=S files=ok`, or `workout=none files=<why>`. `WORKOUT STATUS` reports
the player during a ride and today's workout before one.

When a ride with a workout is saved, VANA writes how the workout went beside
it as `rides/<i|b><ride id>.workout` on FILES and deletes
`workouts/today.vw`, so a ridden workout stays off Home after a restart. A
free ride leaves today's workout planned. The record holds the name,
test, steps done and skipped, time in, under and over target, the FTP a test
measured, and the ride's start time. Clearing or initializing a journal
deletes its records, as its ride ids start over. It is a versioned 70-byte
record (`apps/vana/src/workout/history.rs`). The newest 32 are kept. A save
never replaces or rotates out a file of another version or shape; it is
refused instead. `WORKOUT HISTORY INTERNAL|BULK id` reports one. A
ride recovered after a power cut has no record.

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
  recorded altitude; only a ride with no recorded altitude estimates ascent
  from the recorded gradient.
- An amber radar track, which means no radar data, does not imply an empty road.
