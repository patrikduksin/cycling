# Testing through the harness

Run named recipes without an interactive prompt:

```sh
mise run harness -- recipes
mise run harness -- run shared --backend virtual
mise run harness -- run input-screen --port /dev/ttyACM0
mise run harness -- run frames --port /dev/ttyACM0
mise run harness -- run acceptance --port /dev/ttyACM0
mise run harness -- run virtual-foundation --backend virtual
mise run harness -- run virtual-sdk --backend virtual --sdk
```

The `acceptance` recipe uses the laptop camera and microphone, plays a known
speaker fixture, checks acquisition progress during input/capture, and exercises
terminal reopen, a targeted Linux USB bus reset and firmware restart. It requires
an idle recording state and access to the selected USB device node. `av` collects a shorter
camera/microphone fixture with screenshots. Inspect the recorded camera frames to
assess physical output. A successful camera capture alone does not assess the panel.
The audio fixture verifies laptop capture, not the C606 buzzer. Buzzer acceptance
belongs to #84.

Use the protected `mise run backup`, `mise run flash` and `mise run stock` workflows
and the [C606 skill](../.agents/skills/c606/SKILL.md) for firmware changes. An existing
verified backup must not be overwritten. Build modes are `CYCLING_SDK=0|1` and
`CYCLING_HARNESS=0|1`. Restore a harness-enabled base after hardware tests.

`HARNESS CAPS` reports supported commands, input controls, geometry and transfer
limits before the runner opens a session. Harness-disabled builds retain ordinary
commands, logs and recovery. They report injection and capture as unsupported.
Real restart/bus-reset recipes require an explicitly idle recording state to avoid
interrupting an existing ride or capture. Default recipes never save preferences or write ride/capture data.
Temporary brightness and all original runtime preferences are restored without
`SAVE`, including unsaved preferences reloaded by a restart.

## Structured scenarios

Pass a JSON file with `--scenario`. Evidence, persistent virtual media and run state
must be under ignored `.local/`. The runner keeps the USB lock through the entire
run, including recovery and cleanup. It sends each mutation once and internally
waits for completion and verifies transfer chunks.

```json
{
  "steps": [
    {"op": "command", "command": "DISPLAY f800"},
    {"op": "input", "gesture": "tap", "x": 30, "y": 30},
    {"op": "capture", "count": 2, "interval_ms": 250},
    {"op": "recover", "mode": "terminal", "seconds": 2}
  ]
}
```

| Step | Fields and behavior |
| --- | --- |
| `command` | `command`, optional `expect` key/value assertions. Read-only foundation commands, temporary `DISPLAY` and `BRIGHTNESS` are allowed on real devices. |
| `wait` | Read-only `command`, `equals` key/value conditions, bounded `timeout_s`. Polling stays inside the runner. |
| `input` | `gesture`: `tap`, `hold`, `swipe`, `drag` or `button`; coordinates, `duration_ms`, destination coordinates, or button index and `repeat`. Alternatively, ordered `events` with `at_ms` and `event`. |
| `capture` | `count`, `interval_ms`; waits, transfers, verifies and writes ordered PNGs with actual frame timestamps. `expected_crc32` asserts a frame; `checksum_only` avoids a matching transfer. Mismatches retain an image. |
| `recover` | `mode`: `terminal`, `restart`, `usb-reset`; `seconds` is the bounded intentional missing-host interval. Recovery has a separate timeout. |
| `delay` | Bounded `seconds`; renews sessions. Virtual execution advances deterministic time. |
| `manual` | Exact physical `action`; stops with `needs-manual-action`. Completed work is not replayed. |
| `fixture` | Virtual-only `command`: positioning, BLE observations/loss, time, display failure or bounded storage failure. |
| `virtual-command` | Production commands against test-owned virtual media, with expected `status` and optional `expect`. Enables settings SAVE and SDK record/recover/export tests. Rejected during real preflight. |

A command can name a `baseline`. A later command uses `progress_from`, `counters`
that must increase and `unchanged` counters that must not change. The acceptance
recipe uses this for GNSS/companion progress and loss checks. Each submitted input
sequence ends with touch released or cancelled; button events represent the
verified companion meanings, not invented arbitrary button down/up or long holds.
Physical observations retain their own counters. Synthetic input enters the normal
shell consumer and cannot establish switch/touch sensing or wake an electrically
off device.

## Results, cancellation and recovery

Standard output is compact JSON with schema `version`, `run_id`, overall `status`,
per-step outcomes, wall time, protocol request count, private report path and any
exact `manual_action`. The full report contains replies, build modes, boot/session
identity, device/host timestamps, loss/timing counters, capture configuration,
artifact paths and integrity hashes. Outcomes distinguish `pass`, `fail`,
`unsupported`, `skipped`, `needs-manual-action` and `inconclusive`. Exit status is 0
for pass, 1 for failure and 2 for unsupported/manual/inconclusive work.

For a longer run:

```sh
mise run harness -- start frames --port /dev/ttyACM0
mise run harness -- wait RUN_ID --seconds 30
mise run harness -- cancel RUN_ID --seconds 10
```

`wait` is bounded to 60 seconds and resumes observation of the same worker. It does
not restart a scenario or resend completed mutations. Cancellation sends a signal
to the owned worker and runs cleanup. A timeout after a mutation remains uncertain;
the runner inspects independently where possible, reports uncertain restoration,
and never blindly resends the mutation. A disconnected device expires the session
and cancels held synthetic touch. Manual-action reports preserve completed steps
for inspection; they do not automatically continue after an unobserved cable action.

`host_invocations` counts the scenario execution. `protocol_requests` includes
transport handshake/recovery and virtual fixtures. `--agent-tool-calls` records an
operator-supplied count, or stays null when unknown; the CLI cannot observe agent
tool calls. Record launch, bounded wait and assessment calls separately when
benchmarking an agent workflow. First virtual execution may include compilation.

USB bus reset uses Linux `USBDEVFS_RESET` on the exact device identified by its USB
identity and never targets a shared hub/controller. Terminal closure, bus reset and
firmware restart are distinct. No supported operation claims host detach or physical
VBUS removal. If rediscovery/recovery fails, the report states the required cable
or power-button action; retries are bounded and contain read-only inspection only.

## Pixel capture and protocol

Ordinary framing is `CMD request_id command`, with one correlated JSON reply.
Harness operations use the same reply scheduling. Examples:

```text
HARNESS CAPS
HARNESS OPEN 123 120000
HARNESS SESSION INPUT BEGIN 1
HARNESS SESSION INPUT ADD 1 0 DOWN 30 30
HARNESS SESSION INPUT ADD 1 100 UP
HARNESS SESSION INPUT RUN 1
HARNESS SESSION INPUT STATUS
HARNESS SESSION CAPTURE START 1 250
HARNESS SESSION CAPTURE STATUS
HARNESS SESSION CAPTURE META FRAME
HARNESS SESSION CAPTURE READ FRAME 0 4096
HARNESS SESSION CLOSE
```

`OPEN` echoes the caller nonce and returns a boot-bound session generation and
lease. Session operations renew the lease. Old sessions, sequence IDs and frame
IDs are rejected. `ACCEPTED` is queued work; input/capture status establishes
completion. Sequences have a fixed event bound and device-relative offsets, with
actual delivery counts, cancellation, losses and maximum lateness in the result.
Normal physical input is drained first; physical input loss cancels synthetic holds.
The host and firmware report limits through discovery rather than promising exact
scheduling under display/flash work.

Capture requests cause another real submission of the current fill or shell
renderer. The capture stores the actual pixels evaluated for that submission,
including its identity, RGB565LE geometry, start/end time and submission result.
It does not reconstruct a past screen or claim the panel displayed those pixels.
C606 capture storage uses external RAM; harness-disabled builds reserve none.
Frames remain bounded even without a reader. Slow readers cannot grow memory or
block acquisition. A new capture replaces the previous retained sequence.

Chunks carry frame identity, decoded offset/length, encoding and CRC32 of decoded
bytes. Payloads are bounded raw bytes or RGB565 run tuples, each tuple a little-endian
16-bit count and 16-bit pixel. Metadata supplies a whole-frame CRC; the host rejects
mixed, incomplete, oversized, stale or corrupt data before emitting a PNG. Frame
sequences retain actual timing and skipped counts instead of inventing a video rate.
Future operations should extend typed operations and capability discovery without
adding arbitrary memory/register access or a general service registry.

## Camera and microphone evidence

`mise run harness -- av-discover` rediscovers camera paths and named PulseAudio /
PipeWire sources. A scenario's optional `captures` list selects `camera` and/or
`microphone`, with `seconds`, camera `device`/`size`/`format`, or microphone `source`.
Only one of each can run. AV processes have independent duration caps and are
terminated on failure/cancellation. Device USB still has one reader.

The tool records camera controls and microphone route/gain. It snapshots the video
format, restores it after capture if changed, and verifies the readback. It retains
microphone gain and routing. Record microphone `position` and keep the source, placement and processing
unchanged for relative comparisons. Host processing is not assumed disabled;
comparisons remain limited when AGC/noise processing is unverified. `fixture: true`
plays two known 1 kHz bursts through `sink` or the default speaker route. Quiet
background before the bursts supplies the analysis noise reference. Thresholds
`floor_db` and `margin_db` are recorded through the scenario and analysis.

Audio analysis returns detected onset, duration, repetition count, dominant FFT
frequency, frequency-bin resolution, recorded dBFS level, noise and clipping.
Clipping or absent events is inconclusive. Fixture checks identify the two sustained
1 kHz bursts while retaining unrelated ambient events in the report. No result is calibrated dB SPL. Camera
recordings include representative frames; inspect these for framing and visible
patterns/backlight changes. Software screenshots remain separate evidence.

Command host send/receive times do not tightly bound a device reply timestamp:
the firmware samples time at loop start, before potentially blocking display work.
AV process launch also precedes first sample by unmeasured backend latency. Retain
both timelines and treat precise cross-device timing as unmeasured until calibrated.

The virtual backend uses production parsing, shared shell handlers and SDK runtime,
with deterministic fixture time and file-backed test media locked against concurrent
access. It exercises restart persistence, torn writes and production export rules;
it does not emulate C606 chips, radio wiring, DMA or physical timing. `mise run test`
and CI run these regressions and both original shell geometries.


## Runtime connectivity

Typed `wifi` and `ble` operations validate bounded commands and wait for their
specific operation sequence to complete or fail. Every scenario using these steps
supplies `connectivity_restore` with the original private profile and connection
intent. Cleanup restores that configuration through ordinary commands and checks
completion. See [runtime connectivity](runtime-connectivity.md) for commands,
scenario generation, private profile paths and radio evidence limits.
