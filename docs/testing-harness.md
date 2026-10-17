# Testing through the harness

`mise run harness` runs named recipes or JSON scenarios against the C606 or the
virtual backend and prints compact JSON. Real runs follow the
[C606 skill](../.agents/skills/c606/SKILL.md), including port selection and
restoring a harness-enabled base afterward.

```sh
mise run harness -- recipes                      # List recipes and their steps
mise run harness -- run input-screen             # Real device; --port is optional
mise run harness -- run shared --backend virtual
mise run harness -- run virtual-sdk --backend virtual --sdk
mise run harness-virtual                         # shared, virtual-foundation and virtual-sdk
```

For a single picture of the panel, use `mise run screenshot`. It saves the next
frame the firmware pushes to the panel, after scaling, as a PNG. The command trace
and USB log stay in `.local/screenshots/`.

```sh
mise run screenshot -- shot.png --scale 2        # Nearest-neighbour 2x upscale
mise run screenshot -- shot.png --press bottom-left --press bottom-right
mise run screenshot -- --backend virtual --sdk   # Simulator; default path in .local/screenshots
```

`--press` wakes a dimmed screen first, then sends the listed synthetic button
presses in order, waiting `--settle-ms` after each. The JSON summary reports render
time, capture wait, transfer time and the number of chunk reads. Render time
includes copying the frame into capture memory; STATUS `display_ms` is the latest
frame without that copy.

For responsiveness, `mise run latency` times button presses to the end of the next
frame on the C606 and reports the idle frame rate and frame time. Presses fall due
at random phases, so some wait for a frame already rendering.

```sh
mise run latency -- --press top-left --count 20        # On Home
mise run latency -- --press bottom-left --count 21     # During a ride: steps through the pages
```

| Recipe | Checks |
| --- | --- |
| `input-screen` | Tap, hold, swipe and button input, then a screenshot |
| `frames` | Four screenshots at 250 ms |
| `recovery` | Terminal reopen and firmware restart |
| `shared` | Settings, input, capture and restart; runs on both backends |
| `av` | Short camera and microphone fixture with screenshots |
| `acceptance` | Camera, microphone and speaker fixture; display, brightness and acquisition progress during input/capture; terminal reopen, Linux USB bus reset and restart |
| `virtual-foundation` | Settings persistence, storage failure, positioning freshness and BLE loss fixtures |
| `virtual-sdk` | Ride start, BLE heart rate, restart recovery and export info |

Real runs:

- `HARNESS CAPS` reports supported commands, input controls, geometry and transfer
  limits before a session opens. Harness-disabled builds keep ordinary commands,
  logs and recovery, and report injection and capture as unsupported.
- Restart and bus-reset recipes require an explicitly idle recording state, rechecked
  immediately before the disruptive step. Terminal reopen works during acquisition.
- `acceptance` needs access to the selected USB device node.
- Default recipes never save preferences or write ride or capture data. Temporary
  brightness and all original runtime preferences are restored without `SAVE`,
  including unsaved preferences reloaded by a restart.

## Structured scenarios

Pass a JSON file with `--scenario`. Evidence, persistent virtual media and run
state must be under ignored `.local/`. The runner holds the USB lock for the whole
run, including recovery and cleanup, sends each mutation once, waits for its
completion and verifies transfer chunks.

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
| `command` | `command`, optional `expect` key/value assertions. Real devices allow read-only foundation commands and temporary `DISPLAY` and `BRIGHTNESS`. |
| `wait` | Read-only `command`, `equals` conditions, bounded `timeout_s`. Polling stays inside the runner. |
| `input` | `gesture`: `tap`, `hold`, `swipe`, `drag` or `button`, with coordinates, `duration_ms`, destination, or button index and `repeat`. Or ordered `events` with `at_ms` and `event`. |
| `capture` | `count`, `interval_ms`; writes verified, ordered PNGs with actual frame timestamps. `expected_crc32` asserts a frame; `checksum_only` skips the transfer. Mismatches retain an image. |
| `recover` | `mode`: `terminal`, `restart` or `usb-reset`; `seconds` is the intentional missing-host interval. Recovery has its own timeout. |
| `delay` | Bounded `seconds`; renews sessions. Virtual runs advance deterministic time. |
| `manual` | Exact physical `action`; stops with `needs-manual-action` without replaying completed work. |
| `fixture` | Virtual only: positioning, BLE observations/loss, time, display failure or bounded storage failure. |
| `virtual-command` | Production commands against test-owned virtual media, with expected `status` and optional `expect`. Enables settings `SAVE` and SDK record/recover/export tests. Rejected on real devices. |
| `wifi`, `ble` | Typed connectivity operations; see [runtime connectivity](runtime-connectivity.md). |
| `sound`, `gnss`, `mmc`, `companion`, `ant` | Sound play/stop (four finite patterns), GNSS pause/resume (with an on-device resume deadline), read-only MMC read/clock/recover, companion query, bounded ANT scan/stop. |

A command can name a `baseline`; a later command uses `progress_from`, `counters`
that must increase and `unchanged` counters that must not. `acceptance` uses this
for GNSS/companion progress and loss.

Each input sequence ends with touch released or cancelled. Button events carry the
verified companion meanings, not arbitrary down/up or long holds.

Cleanup checks sound settling, resumed GNSS progress, the original MMC clock and
completed ANT scanning, independently of preference and radio restoration.

## Results

Standard output has schema `version`, `run_id`, overall `status`, per-step
outcomes, wall time, protocol request count, private report path and any exact
`manual_action`. The private report adds replies, build modes, boot/session
identity, device and host timestamps, loss/timing counters, capture configuration,
artifact paths and integrity hashes.

| Status | Exit |
| --- | --- |
| `pass` | 0 |
| `fail` | 1 |
| `unsupported`, `needs-manual-action`, `inconclusive` | 2 |

`skipped` marks steps not reached. `host_invocations` counts the scenario
execution; `protocol_requests` includes handshake, recovery and virtual fixtures.
`--agent-tool-calls` records an operator-supplied count and stays null otherwise;
when benchmarking an agent, record launch, wait and assessment calls separately.
The first virtual run may include compilation.

## Long runs, cancellation and recovery

```sh
mise run harness -- start frames
mise run harness -- wait RUN_ID --seconds 30
mise run harness -- cancel RUN_ID --seconds 10
```

- `wait` is bounded to 60 seconds and observes the same worker; it never restarts
  a scenario or resends completed mutations.
- `cancel` signals the owned worker, which runs cleanup.
- After a timed-out mutation the runner inspects independently where possible,
  reports uncertain restoration and never resends the mutation.
- A disconnected device expires the session and cancels held synthetic touch.
- Manual-action reports keep completed steps and do not continue automatically
  after an unobserved cable action.
- `usb-reset` uses Linux `USBDEVFS_RESET` on the exact device, identified by USB
  identity, never a shared hub or controller. Terminal closure, bus reset and
  firmware restart are distinct. If rediscovery fails, the report names the
  required cable or power-button action; retries are bounded and read-only.

## Protocol and pixel capture

Ordinary framing is `CMD request_id command` with one correlated JSON reply.
Harness operations share that scheduling:

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

- `OPEN` echoes the caller nonce and returns a boot-bound session generation and
  lease; session operations renew it. Old sessions, sequence IDs and frame IDs
  are rejected.
- `ACCEPTED` is queued work; input and capture status establish completion.
- Input sequences have a fixed event bound and device-relative offsets. Results
  report delivery counts, cancellation, losses and maximum lateness. Physical
  input drains first, and physical input loss cancels synthetic holds.
- A capture triggers a real submission of the current fill or shell renderer and
  stores the pixels evaluated for it: identity, RGB565LE geometry, start/end time
  and submission result.
- C606 capture storage uses external RAM; harness-disabled builds reserve none.
  Frames stay bounded without a reader, slow readers cannot grow memory or block
  acquisition, and a new capture replaces the retained sequence.
- Chunks carry frame identity, decoded offset/length, encoding and CRC32 of the
  decoded bytes. Payloads are raw bytes or RGB565 runs (little-endian 16-bit count,
  16-bit pixel). The host checks the whole-frame CRC and rejects mixed, incomplete,
  oversized, stale or corrupt data before writing a PNG. Sequences keep actual
  timing and skipped counts.

Extend this through typed operations and capability discovery, not arbitrary
memory/register access or a general service registry.

## Camera and microphone

`mise run harness -- av-discover` lists camera paths and PulseAudio/PipeWire
sources. A scenario's `captures` list selects at most one `camera` and one
`microphone`, with `seconds`, camera `device`/`size`/`format` or microphone
`source`. Each has its own duration cap and stops on failure or cancellation.

- The tool records camera controls and microphone route and gain. It restores a
  changed video format and verifies the readback; microphone gain and routing are
  left unchanged.
- For relative comparisons, record microphone `position` and keep source,
  placement and processing unchanged.
- `fixture: true` plays two 1 kHz bursts through `sink` or the default speaker;
  the quiet lead-in is the noise reference. `floor_db` and `margin_db` are recorded.
- Analysis reports onset, duration, repetitions, dominant frequency and bin
  resolution, dBFS level, noise and clipping. Clipping or no detected events is
  inconclusive. Unrelated ambient events stay in the report.
- Inspect the representative camera frames for framing and visible pattern or
  backlight changes.

## Virtual backend

The virtual backend uses production parsing, shared shell handlers and the SDK
runtime with deterministic time and file-backed test media locked against
concurrent access. It covers restart persistence, torn writes and production
export rules. `mise run test` and CI run these regressions and both original
shell geometries.

## What this does not prove

- Screenshots show pixels evaluated for a submission, not what the panel
  displayed, and cannot reconstruct a past screen. Camera frames are the physical
  evidence; a successful camera capture alone does not assess the panel.
- Synthetic input enters the normal shell consumer. It cannot establish switch or
  touch sensing or wake an electrically off device. Physical observations keep
  their own counters.
- The audio fixture verifies laptop capture, not the C606 buzzer (see #84). No
  result is calibrated dB SPL, and comparisons are limited while host AGC or
  noise processing is unverified.
- Timing is reported, not promised: scheduling under display or flash work is
  bounded only by discovered limits. Host send/receive times do not tightly bound
  a device reply timestamp, because firmware samples time at loop start before
  blocking display work, and AV launch precedes the first sample by unmeasured
  latency. Treat cross-device timing as unmeasured until calibrated.
- No operation claims host detach or physical VBUS removal.
- The virtual backend does not emulate C606 chips, radio wiring, DMA or physical timing.
- The CLI cannot observe agent tool calls.
