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
as `4096 - occupied-upper`; do not subtract only valid records. The most recent
completed five-second device capture reported `INFO 1 256 3090 stopped`, leaving
1,006 slots. After an SDK restart and storage scan, the equivalent expected reply
is `INFO 1 256 3090 idle`. Recheck before departure because another capture reduces
that tail. Existing rides, captures and occupied invalid slots remain preserved.

The prepared session requests **420 seconds, seven minutes, with no selected ANT
peers**. Its reservation is `2 × 420 + 8 = 848` slots, which fits the measured
1,006-slot tail with 158 slots beyond the reservation. If the newly measured tail
is below 848, do not start this duration. Five minutes without ANT requires 608
slots, six requires 728 and ten requires 1,208.

Keep ANT peers unselected for this session. Any selected ANT channel raises the
seven-minute reservation to 2,108 slots; even five minutes with a selected peer
requires 1,508, which exceeds the current tail. Sleeping sensors still count as
selected. Inspect `ANT` and have the coordinator close any selected channel before
starting; do not scan or connect a sensor during this capture.

Start the seven-minute capture:

```text
FOUNDATION LOG START 420
FOUNDATION LOG STATUS
```

`ACCEPTED` means scanning was queued. Poll `FOUNDATION LOG STATUS` until it reports
`Recording`, then confirm `saved_environment` and `saved_positions` increase on
successive checks. Check `remaining_slots`, `required_slots` and error/drop counts
in the same reply. Use these counters to confirm the foundation capture. The
coordinator must establish the available duration from the measured free tail
before departure; this document does not assume unused capacity. The preflight
reserves two slots per second for GPS/environment, an additional three ANT slots
per second if any peer is selected, and eight margin slots. Select peers before
starting; this allowance does not guarantee a duration at arbitrary packet rates.
The countdown begins after the full storage scan reaches ready state, and the
SDK automatically stops at its deadline. `requested_seconds` and
`remaining_seconds` accompany capture status.

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
