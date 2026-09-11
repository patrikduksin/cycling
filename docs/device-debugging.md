# Device debugging

The USB toolkit drives the running C606 firmware and collects state, captured
pixels and logs. It uses the same input handlers as physical touch and buttons.
Captures represent the pixels sent to the LCD, not panel readback or backlight
measurements. Tests do not prove touch-controller, button-switch or battery-sensor
hardware behavior.

## Build with or without the harness

```sh
mise run build                         # Harness enabled, development default
CYCLING_HARNESS=0 mise run build         # Compile out the harness
CYCLING_HARNESS=0 mise run flash         # Build and flash without the harness
mise run flash                         # Restore the development harness
```

The build helper enables Cargo's `debug-harness` feature by default. Setting
`CYCLING_HARNESS=0` omits it. Other values fail the build. Both modes use the same
optimized release profile. Direct Cargo builds must explicitly enable
`c606,debug-harness` to include the harness; `c606` alone excludes it.

Disabling the feature removes USB command reception, input injection, session
management, debug state telemetry, both capture buffers and recording output.
Physical touch, buttons, battery reporting and the UI remain enabled. The
portable display scaling helper is still needed by the LCD. Normal USB logs,
Wi-Fi and its public HTTP bring-up test remain enabled. This option therefore
provides a baseline without the test harness, not a silent production firmware.
The boot log reports `harness=true` or `harness=false`.

The build directories are `.local/firmware` and `.local/firmware-no-harness`.
Both produce `.local/cycling.bin`; that image always reflects the last successful
build. `mise run flash` rebuilds using the environment passed to that invocation,
so pass `CYCLING_HARNESS=0` to **flash**, not only to an earlier build command.
Screenshot/debug commands cannot work in the disabled mode and will time out.
Restore the enabled build after baseline testing on the development device.

## Interpreting overhead

The screenshot and recording canvases reserve 16,960 bytes each, or 33.125 KiB
together, even with no host attached. They are static storage, not allocations
from the 160 KiB heap. Removing them will not automatically increase reported
free heap; it makes internal RAM available to the linker. Code and session state
add smaller costs. PNG conversion and video encoding run on the computer.

Idle harness CPU overhead has not been isolated experimentally. The enabled
firmware checks USB and session state each frame, even when no recording runs.
Recording adds canvas comparisons, checksumming, compression and USB output.
State polling and injected inputs also generate acknowledgment traffic.

Use disabled firmware for frame-timing and power baselines. With the harness
enabled, report whether it was idle, polling state, taking a screenshot or
recording, along with requested fps, measured timestamps and scene complexity.
A recording can itself cause a missed frame budget; it cannot establish the
performance of firmware without recording. The hardware observations below
cover the current controls screen, not every future animation. Power consumption
and idle overhead are unmeasured. A short successful run is not a soak-duration
or worst-case memory guarantee.

## Run a test

```sh
mise run debug -- state
mise run debug -- capture
mise run debug -- record --seconds 10 --fps 5
mise run e2e
mise run debug -- run scripts/scenarios/controls.json
mise run debug -- lease-test
mise run debug -- wifi-recovery
mise run debug -- soak --seconds 60
```

Each invocation creates a directory in ignored `.local/tests/` and prints its
location. `--output` selects a new directory; existing directories are not
replaced. `--port` or `CYCLING_PORT` selects USB. Global flags go before the
subcommand:

```sh
mise run debug -- --output .local/tests/my-run run scripts/scenarios/controls.json
```

The tool requires Linux terminal APIs and Python from mise. If `ffmpeg` is on
PATH, it also produces MP4 clips. PNGs, timestamps, assertions and logs remain
available without ffmpeg. The older `mise run screenshot` still works without
opening a test session.

Close other USB readers first. All repository device tools share `.local/usb.lock`.
The toolkit does not toggle DTR/RTS or intentionally restart the device. If USB
is silent immediately after flashing, the existing `mise run monitor -- --seconds 5`
can release the control lines and start the firmware. Do this before testing;
it can restart the device. A reboot during a session fails the test.

## Evidence

- `report.json` records success or failure, command IDs, acknowledgments, device
  frame numbers, timestamps, assertions, baseline state and cleanup state.
- `frame-*.png` are 240×320 images reconstructed from checksummed RGB565 frames.
- `stream-*.mp4` preserve device timing to millisecond resolution. Separate
  recordings become separate clips; gaps are not represented as frozen frames.
- `index.html` shows clips and a frame gallery. Open it locally.
- `usb.log` contains the original device output, including diagnostics.

A capture is rejected if a frame or row is missing, duplicated or malformed,
its checksum fails, or the stream ends without the expected frame count.
Failures exit nonzero and retain collected evidence. Session cleanup runs on
ordinary Python exceptions and Ctrl-C. If the process is killed or USB goes away,
the firmware's lease timeout provides cleanup.

The parser accepts a valid debug reply after a recognized, truncated periodic
`CYCLING_FRAME` prefix. This handles the observed case where the USB printer lost
part of a normal frame line before emitting a complete reply. Arbitrary prefixes,
malformed replies, reboot notices, lease expiry and incomplete recordings remain
errors.

## Input and state

JSON scenarios contain sequential actions. See
[controls.json](../scripts/scenarios/controls.json) for a complete example.

| Action | Fields | Behavior |
|---|---|---|
| `command` | `value` | Send a protocol command and require its acknowledgment |
| `tap` | `point`, optional `seconds` | Press, wait, release |
| `drag` | `from`, `to`, optional `seconds`, `steps` | Move a held touch, then release |
| `wait` | `seconds` | Keep processing USB and renewing the lease |
| `expect` | `state`, optional `timeout` | Poll until all named state values match |
| `capture` | none | Save one complete frame; recording must be stopped |
| `pixel` | `point`, `rgb565` hex string | Assert a pixel in the latest captured frame |

Coordinates use the physical 240×320 display. Use `TOUCH x y`, a `wait`, and
`RELEASE` for a held touch. Physical reports continue to be read; injected touch
takes precedence until release. Recording alone allows physical touch input.

Python scenarios can import `Device` from `scripts/debug.py`. Its `command`,
`tap`, `drag`, `capture`, `expect`, `pixel` and `wait` methods share one connection.
`pixel(..., after_ms=ack['ms'] + 1)` waits for a recorded frame after an input
acknowledgment. This links a visual assertion to a displayed input result.

State reports include the current screen, focused and pressed control, whether
pointer input is blocked pending release, brightness,
touch point, button counters, battery values, whether battery data is simulated, Wi-Fi state, physical
touch availability, companion packet/error counters, free heap, sampled minimum
heap, PSRAM capacity and free space, previous frame processing time and maximum
observed frame processing time, the touch error counter, selected/effective
brightness, dim state, idle age, timeout and dim level.
A missing touch coordinate, battery reading or power status is `-1`.

Wi-Fi values are 0 unconfigured, 1 connecting, 2 awaiting DHCP, 3 connected,
4 public HTTP test passed, 5 retrying and 6 failed initialization. Debug state
also reports association, successful-probe and failed-probe counters plus the
active injected fault. Heap minima are sampled once per display
cycle, not allocator high-water marks. The on-device Diagnostics screen caches a
copy for one second to bound visual refresh, while these USB values remain live.
Stack usage is not measured. Frame time
includes recording work but excludes the wait for the next display cycle.

## Protocol and cleanup

Commands are ASCII lines, `DBG <request-id> <command>\n`, with a maximum of 96
bytes before the newline. Replies are `CYCLING_DEBUG <id> <result> <JSON>`.
Protocol version 1 is reported in the JSON. Acknowledgments follow LCD drawing,
so their frame numbers identify displayed results. Malformed commands produce
`CYCLING_DEBUG 0 INVALID {}` and execute nothing.

| Command | Purpose |
|---|---|
| `BEGIN` | Start a test session and save navigation, brightness and button counters |
| `PING` | Renew the session lease |
| `STATE` | Inspect state; also allowed outside a session |
| `TOUCH x y` / `RELEASE` | Inject touch or complete the gesture |
| `CANCEL` | Abandon the injected gesture without activating it |
| `BUTTON id code` | Inject a companion button event, IDs 0 top, 1 left, 2 right |
| `BATTERY percent millivolts power` | Override displayed battery values; power 0 charging, 1 battery, 2 unknown |
| `LIVE` | Resume current physical battery readings |
| `WIFI` | Request a real station disconnect followed by normal reconnection |
| `WIFI_FAULT 0/1/2` | Clear faults or inject a DNS/request failure for recovery tests |
| `CAPTURE` | Capture one compressed frame |
| `RECORD milliseconds fps` | Record 100–30000 ms at a requested 1–10 fps |
| `PERSIST brightness` | End and restore the temporary session, then explicitly save a validated 5–100% brightness |
| `IDLE seconds brightness` | Temporarily set timeout (`0` disables) and dim level for the current session |
| `STOP` | Stop recording |
| `END` | Stop recording, cancel touch, clear battery override and restore saved UI values |

The host opens a session automatically and sends heartbeats every second.
The firmware expires a session after three seconds without an accepted command.
Ordinary injected state is temporary and never written to flash. `PERSIST` is the
explicit exception: it restores and ends the session, commits and verifies the
supplied brightness, applies PWM, redraws, then acknowledges. Further injected
commands require a new `BEGIN`. Battery injection affects
the displayed status; it does not change charging or send companion commands.
Button code 1 is the verified short-click action. Other codes can be injected,
but their physical long-press meanings are not verified. There is no invented
button-down/button-up protocol.

Session cleanup clears injected Wi-Fi faults and restores saved button counters, including when they changed
through physical presses during the test. Brightness returns to its initial
value. Wi-Fi follows its normal reconnect loop; ending a test does not cancel
an in-progress reconnect.

Recordings send a full initial frame followed by changed rows. Rows contain
hex runs of a two-digit count and four-digit RGB565 color. Every frame carries
its stream ID, sequence, display frame number, device timestamp and checksum.
The checksum is FNV-1a over little-endian canvas pixels. A STOP record gives the
total frame count. Legacy screenshots and recordings cannot run together.

## Hardware validation

The smoke test verified the touch marker in captured pixels, dragged the slider
to both endpoints, checked the white knob at each endpoint, injected all three
short button events, exercised battery overrides and returned to live readings.
It forced a Wi-Fi disconnect and waited for another successful public HTTP test.
Companion reception continued without additional UART or CRC errors. Cleanup
restored brightness and counters. The JSON scenario and lease-timeout test also
passed on the device.

The first smoke recording contained 50 verified frames across two streams and
26 distinct images. A five-second recording requested at 10 fps produced 40
frames, about 7.8 fps. The current display loop rounds capture deadlines to frame
boundaries, so requested fps is an upper target. Most sampled frame-processing
times during the smoke run were 20–30 ms; the full initial frame reached 44 ms,
slightly above the 42 ms display target. More complex future screens may cost
more to encode. This is a debug recorder, not a lossless 24 fps panel trace.

The recorder adds a 16,960-byte canvas alongside the existing screenshot buffer.
The internal heap remained about 113.5 KiB free after testing. The short stability
check is useful for regressions; it does not establish overnight reliability.

With the build option added, comparing both release ELFs showed that disabling
it reduced `.bss` by 34,136 bytes and `.data` by 24 bytes. The linker increased
the main stack region from 109,740 to 143,900 bytes. These are reserved regions,
not measured stack usage. The configured 160 KiB heap was unchanged.

Hardware validation of the disabled mode showed 20 ms sampled frame processing,
a successful touch probe, continuing companion reception and both public HTTP
checks passing. Sending debug and screenshot commands produced no response while
frame logs continued. No physical touch/button retest was performed for this
build-option change. These samples are not a controlled measurement of idle
harness overhead.
