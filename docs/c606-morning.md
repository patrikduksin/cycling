# C606 morning checks

This session uses the existing device, laptop and owned sensors. No opening the
C606, electrical probes, new equipment or vendor-storage writes are needed. Raw
button, sensor, radio and positioning evidence stays in ignored `.local/`.

## 150-second bench session

Start with the tested harness-enabled base, USB connected, and no other terminal,
logger, exporter or harness holding USB. From the repository root:

```sh
mise run foundation-bench -- --plan
mise run foundation-bench -- .local/foundation-morning-bench
```

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
mise run terminal
```

In the terminal, inspect `INFO`, `STATUS`, `MOTION`, `PRESSURE`, `BATTERY` and
`POSITION`. Confirm the intended firmware revision and SDK/harness modes.

Before selecting any ANT peer, have the coordinator establish the measured free
append tail after the storage scan. Do not use the total partition size as free
capacity. Five minutes requires 608 slots with no selected ANT channels; six
minutes requires 728 and ten minutes requires 1,208. Any selected ANT channel
raises the five-minute reservation to 1,508 slots (six minutes: 1,808; ten minutes:
3,008), even when that sensor is asleep. These are reservations, not a claim about
the device's current free tail. If fewer than 608 slots remain, stop preparation
and preserve the existing data.

ANT peers are optional. Select them only if the measured tail supports their
larger reservation for the chosen duration. Otherwise leave all ANT channels
unselected and capture GPS, pressure, raw motion and battery alone. If capacity
allows and the existing SR mini radar, Polar H10 or Magene PES P515 are awake,
scan before starting capture:

```text
ANT SCAN 10
ANT
ANT DEVICES
```

Wait for scanning to finish. The coordinator compares each discovery's full
identity against the private inventory `.local/foundation/owned-ant.json`, which
comes from the owner's previously confirmed three-sensor ride. Connect only exact
matches, using each discovered decimal device number and transmission type:

```text
ANT CONNECT <device-type> <device-number> <transmission-type>
ANT
```

Known types are 40 for radar, 120 for heart rate and 11 for power. Do not substitute
another nearby identity of the same type. Check selected peers reach connected
and fresh status. Missing or sleeping peers do not block a foundation capture.
The bridge still permits one selected peer per type, and scanning requires closed
channels. Keep identifiers out of tracked notes.

Select the duration explicitly. The example requests five minutes; values from
300 through 600 seconds are accepted. Start the capture:

```text
FOUNDATION LOG START 300
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
plan the outdoor portion within that countdown. Five minutes is the default
example above; request up to ten minutes only when the measured free tail allows.
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

Reconnect USB, then run `mise run terminal` and enter:

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
mise run ant-export -- .local/foundation-morning-export --domain FOUNDATION
```

The exporter checks slot CRCs, commit words and sequence gaps, and preserves the
complete occupied prefix, including unknown or interrupted records. Inspect
`manifest.json` and retain `prefix.bin` and `records.json`. An interrupted export
retains `prefix.partial`; it does not claim a completed manifest. After an
unexpected restart, let the ordinary ride scanner finish before export. Existing
committed captures remain exportable, but a new capture does not start itself.

After export and inspection, restore the required development configuration:

```sh
CYCLING_SDK=0 CYCLING_HARNESS=1 mise run flash
mise run terminal -- INFO
```

Confirm the base revision, SDK disabled and harness enabled. Close readers and
leave USB available. Do not clear or reclaim recordings from this session.

The resulting evidence addresses physical button behavior, raw motion response,
pressure changes, USB/battery status and outdoor acquisition continuity. Absolute
voltage/pressure calibration, current consumption, charging current and electrical
shutdown remain unmeasured. Any shutdown/sleep/wake test needs its own reviewed
sequence and owner-present recovery plan.
