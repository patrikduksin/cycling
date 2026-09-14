# C606 morning checks

This session uses the existing device, laptop and owned sensors. No opening the
C606, electrical probes, new equipment or vendor-storage writes are needed. Raw
button, sensor, radio and positioning evidence stays in ignored `.local/`.

## 150-second bench session

Start with the tested harness-enabled base, USB connected, and no other terminal,
logger, exporter or harness holding USB. From the repository root:

```sh
export CYCLING_PORT="$(python scripts/device_port.py)"
mise run foundation-bench -- --plan
mise run foundation-bench -- .local/foundation-morning-bench
```

The port helper reads sysfs and matches the private backup manifest without
opening USB. Stop if it reports no unique match. Repeat the export after every
flash, reconnection or USB port renumbering before starting another reader.
`mise run device-port` also prints the current verified path.

Follow the timed prompts. The sequence covers a stationary baseline, isolated and
repeated clicks on all three buttons, two-second lower-button holds and their
combination, device orientations and slow lifting. Top-left holds come last,
first two seconds and then five seconds if responsive. The companion may restart
independently; watch the display and use the normal power button if it needs
physical recovery. Do not pull on the attached cable during rotations.

The script samples `INPUT`, `MOTION`, `PRESSURE`, `BATTERY`, `POSITION`, `GNSS` and
`STATUS`. It records USB gaps and attempts a fresh read-only attachment without
replaying mutations. It finishes automatically after 150 seconds. Ctrl-C stops
early while retaining completed observations. Keep
`.local/foundation-morning-bench/observations.jsonl`, `summary.json` and USB logs.
Record any missed instruction or unexpected screen behavior alongside them.
Software reports establish neither physical releases nor the exact fitted parts.

## Prepare the standalone capture

Close the bench collector before changing firmware. With the owner present and
USB connected, use the protected helper for the tested SDK configuration:

```sh
CYCLING_SDK=1 CYCLING_HARNESS=1 mise run flash
export CYCLING_PORT="$(python scripts/device_port.py)"
mise run terminal
```

In the terminal, inspect `INFO`, `STATUS`, `MOTION`, `PRESSURE`, `BATTERY` and
`POSITION`. Confirm the intended firmware revision and SDK/harness modes.

Establish the append tail before starting. An idle `FOUNDATION LOG STATUS` can
show `remaining_slots: 4096` because the capture has not scanned storage. That
number is not measured free capacity. Let the ordinary recorder scan finish:

```text
RIDE STATUS
FOUNDATION LOG INFO
```

Poll `RIDE STATUS` until `state=ready`. If `FOUNDATION LOG INFO` returns `BUSY`,
wait and inspect again. Its successful reply has this form:

```text
INFO 1 256 <occupied-upper> idle
```

The fourth value is the exclusive occupied upper slot. Compute free append slots
as `4096 - occupied-upper`; do not subtract only valid records. The earlier
completed five-second device capture reported `INFO 1 256 3090 stopped`, leaving
1,006 slots. After an SDK restart and storage scan, the equivalent expected reply
was `INFO 1 256 3090 idle`. The owner subsequently authorized a verified export
and clear, restoring all 4,096 slots. Recheck before departure because another capture reduces
that tail. Existing rides, captures and occupied invalid slots remain preserved.

The SDK boots directly into the physical ANT menu with manual recording.
Reboot returns to that menu without USB; sensor selection is volatile. The base
firmware has a text status screen. A coordinator can reopen the menu without selecting peers or starting recording:

```text
FOUNDATION MENU
```

If the companion is in charging startup, the menu says `HOLD TOP LEFT 2S TO WAKE`.
That physical hold is handled by the companion; synthetic UI events cannot change
its electrical power state. ANT scan/connect stays blocked until startup permits
it. Software restart returns to the menu but does not pretend charging is normal
acquisition. `FOUNDATION MENU STATUS` exposes menu selection and startup state for
diagnosis. The harness can exercise scan/selection through the same input route.

The lower-left button moves the selection; lower-right chooses it. Top-left returns
to the first row, then to ride status. From idle ride status, top-left opens the
menu. The first press after dimming wakes the screen and is consumed. Use short
clicks, not holds. Choose Scan sensors near the bike, then select each owned radar
and power meter by its device number. The menu shows connection progress and
fresh/stale status. It never connects merely because a peer appears in a scan.
Use a selected channel's DROP row to disconnect before choosing a replacement.

Choose Start ride test only when ready outside. Start requires fresh radar/power
channels, a current GPS fix and at least eight free scanned slots. It starts sampled
recording without a deadline. The screen shows elapsed time and measured free slots.
While stationary, press lower-right once for `BR CONFIRM`, then press it again
between 350 ms and five seconds later to stop. Other input cancels confirmation.
Wait for `STOPPED` before switching off. Recording also ends at storage exhaustion;
`STORAGE FULL` is not an active recording state. No erase or automatic reuse occurs.

Keep the device powered while walking to the bike; idle preparation records nothing.
Selection is volatile and does not modify saved preferences. `FOUNDATION SCREEN`
opens only the ride-status preview. The screen shows fresh GPS fix, radar packet
age, fresh decoded watts, verified environment/GPS counts, free slots and drops.
`WAIT SAVE` means no recent verified commit. `RADAR ON` means fresh packets, not
vehicle detection or a safety alert. Do not operate buttons while moving.

The coordinator can start/stop the same manual sampled mode through USB:

```text
FOUNDATION LOG MANUAL
FOUNDATION LOG STATUS
FOUNDATION LOG STOP
```

For explicitly timed diagnostic procedures, the existing `FOUNDATION LOG SAMPLED
420` command remains available. It is separate from the manual on-device Start.

The explicitly timed sampled command reserves `3 × ceil(seconds / 2) + 3 × ceil(seconds / 10) + 8`
slots. Seven minutes requires 764 slots, which fitted the historical 1,006-slot
tail. Manual mode instead requires only eight free slots to start and continues
until confirmed Stop or storage exhaustion. It saves environment, GPS and the latest packet for each selected radar, heart
rate or power type every two seconds, with changed link snapshots at most every ten seconds. Live sensor
decoding continues at full received rate. Intermediate packets and link transitions
are intentionally omitted; this is not a complete RF capture or page sequence.
Other ANT device types are omitted. Link-transition omissions are not counted. `sampled_out_packets`
is separate from loss/drop counters. Each record carries the sampled flag, and
exports retain source timestamps, the two-second interval and omission counts
where recorded. A missing terminal record remains incomplete evidence.

`ACCEPTED` means scanning was queued. Confirm `Recording`, increasing
`saved_environment` and `saved_positions`, fresh selected peers and zero errors or
drops before departure. For timed commands, the countdown starts when the storage scan reaches ready,
so preparation after that scan consumes part of the requested duration. Recording stops at the
deadline. Check the actual free tail before starting; never erase old records to
make room.

The original full-rate `FOUNDATION LOG START 420` remains available. With no
selected ANT peers it reserves 848 slots; with any selected peer it reserves
2,108 slots and does not fit a 1,006-slot tail. Sleeping selected peers still count.
Its allowance is not a guarantee at arbitrary RF rates. Use sampled mode for the
capacity-limited radar/power session, and retain its sampling limitation in analysis.

The capture appends after every occupied slot and never erases. Existing rides,
old captures and invalid occupied slots remain preserved. While it owns storage,
ordinary ride recording and competing storage operations stay excluded until
restart/rescan. `Full` or `Error` is not an active recording state. A failed
capacity preflight also retains storage ownership: do not immediately retry START.
Inspect the failure, restart safely while capture is inactive, let scanning finish,
and reassess the smaller duration or unselected-ANT reservation. Restarting does
not reclaim any slots, and there is no erase step in this procedure.

## Short outdoor session and USB transition

After recording is confirmed, enter `quit` to close the USB terminal. Disconnect
the cable promptly. The requested duration includes preparation after readiness;
plan the outdoor portion within that countdown. Seven minutes is the prepared
session above; it includes preparation after recording becomes ready.
Wait briefly outdoors for positioning to establish a fix. Include a modest
observable elevation change if convenient. Keep the device secured; do not operate
commands or perform button/rotation tests while riding.

Record when the cable was removed, when motion began, the elevation-change
interval and when USB was reconnected. This combines the battery/USB transition
check with the standalone capture. A report gap or missing fix stays missing;
it does not become a repeated current reading. Pressure and temperature use the
companion's compensation. Motion triplets remain unscaled, and reported battery
voltage/percentage do not establish calibration or charging current.

An already-owned phone with an existing track recorder can supply an optional
comparison. Start both recordings while stationary and keep the phone track
private. Align the phone's UTC timeline to a C606 `STATUS` uptime using laptop UTC
readings immediately before and after the USB request, preferably at departure
and return. Retain the bracket width as alignment uncertainty. If the C606
restarts, align each boot separately. Compare only fresh fixes; split tracks at
missing fixes or sample gaps over two seconds, and do not interpolate across those
gaps. A phone track is a functional comparison, not surveyed position or pressure
calibration. The session remains useful without a phone.

## Stop, export and restore base

Reconnect USB, resolve its current port again, then open the terminal:

```sh
export CYCLING_PORT="$(python scripts/device_port.py)"
mise run terminal
```

Enter:

```text
FOUNDATION LOG STOP
FOUNDATION LOG STATUS
BATTERY
POSITION
STATUS
```

The deadline may already have stopped capture. Otherwise poll capture status
until `Stopped`. Note saved counts, drop counters and any
error. Enter `quit`, then export to a new private directory:

```sh
mise run ant-export -- .local/foundation-morning-export --domain FOUNDATION --start-slot 3090
```

The prepared value 3090 is the measured start of the unused tail. Confirm the
start slot again before recording if any additional session has written data.
This exports only the new range, keeping absolute slot and capture identities,
and avoids downloading the old recordings again. The overnight complete export
remains in `.local/foundation/capture-export`.

The exporter checks slot CRCs, commit words, sequence gaps and an unchanged source
bound. Inspect `manifest.json` and retain `range.bin` and `records.json`. An
interrupted range export retains `range.partial`; it does not claim a completed
manifest. Omit `--start-slot` for the complete occupied prefix (`prefix.bin`),
including unknown or interrupted records. Neither operation reclaims data. After an
unexpected restart, let the ordinary ride scanner finish before export. Existing
committed captures remain exportable, but a new capture does not start itself.

After export and inspection, restore the required development configuration:

```sh
CYCLING_SDK=0 CYCLING_HARNESS=1 mise run flash
export CYCLING_PORT="$(python scripts/device_port.py)"
mise run terminal -- INFO
```

Confirm the base revision, SDK disabled and harness enabled. Close readers and
leave USB available. Do not clear or reclaim recordings from this session.

The resulting evidence addresses physical button behavior, raw motion response,
pressure changes, USB/battery status and outdoor acquisition continuity. Absolute
voltage/pressure calibration, current consumption, charging current and electrical
shutdown remain unmeasured. Any shutdown/sleep/wake test needs its own reviewed
sequence and owner-present recovery plan.
