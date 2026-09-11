# Development

This is a Cargo workspace. `packages/os` contains portable rendering and input logic and a
C606 firmware binary. Add workspace members when there is something real to share.

## Tools

Install [mise](https://mise.jdx.dev), then:

```sh
mise install
mise run setup
mise run test
mise run check
mise run build
```

Mise pins Rust, Python, uv, espup and espflash. `setup` installs Espressif's
Xtensa Rust 1.97.0.0 toolchain as `cycling-esp`; the ESP32-S3 requires this compiler
fork. Host tests use Rust 1.98.1. Builds load the generated environment internally.
No global shell changes are needed.

The firmware uses `esp-hal` 1.1.2 and Rust edition 2024. LCD support currently
requires its `unstable` feature, so that dependency is pinned exactly. Cargo.lock
pins the full dependency graph. The published `esp-radio` 1.0.0-beta.0 and
`esp-rtos` 0.3.0 stack requires the HAL 1.1 series. Current releases are preferred, with deliberate
updates rather than moving Git branches.

## Commands

| Command | Purpose |
|---|---|
| `mise run fmt` | Format Rust |
| `mise run test` | Host input, UI, renderer and flash-format tests |
| `mise run check` | Formatting and Clippy |
| `mise run build` | Release ELF and `.local/cycling.bin` |
| `mise run preview` | Render the current 240×320 app preview to `.local/controls.ppm` |
| `mise run bluetooth-echo` | Verify the owned C606 echo peripheral from Linux BlueZ |
| `mise run ble-simulator` | Advertise the owned deterministic HRS/CSC test fixture |

The OS is `no_std`, using esp-rtos and Embassy for asynchronous Wi-Fi alongside
the display loop, with a 160 KiB internal heap. An 80×106 RGB565 canvas is enlarged
3× onto the 240×320 display, using eight-row DMA transfers. A portable `App` owns
the current screen, focus, pressed gesture and the existing controls state. Home
links to Settings, Device, the controls diagnostics, and a disabled future Rides
entry. Pointer release activates a target; timeout, input
failure and debug cleanup cancel it. Physical and injected input call the same app
handlers. The controls diagnostics shows battery percentage, voltage and power
status, counts button events, and tests brightness from 5 to 100 percent.
Target cadence is 24 fps. Touch uses the stock 0x5a report protocol over I2C;
brightness changes the existing backlight PWM duty. It resets to 50 percent on
first use, then restores the last valid saved value before configuring PWM.
Bottom-left and bottom-right short clicks also change brightness by five
points. The companion receiver uses UART2 RX41 at 115200 baud and validates
packet CRCs. Boot sends the single stock-candidate GPS open frame but no button,
power, reset, or update command. Wi-Fi station support uses DHCP and reconnects
after disconnects. The C606's
2 MiB Quad SPI RAM is initialized at 40 MHz, tested on each boot and exposed
through a separate external-only allocator. Bluetooth uses the same radio stack
as Wi-Fi; companion power control is still future work.

The companion UART receiver is interrupt-fed into a 2,048-byte internal-memory
ring. The ISR drains at most 16 128-byte chunks per invocation; the main loop
then consumes bounded 128-byte batches. A FIFO, framing, parity, glitch, or ring
loss clears queued bytes and resumes only at the next `0xa5` frame marker before
the CRC-validating decoder runs. Per-kind loss logs reflect the first error
reported by the pinned HAL, so they do not prove that simultaneous raw hardware
flags were absent.

This replaced one 128-byte polling read per roughly 42 ms display loop. The
agent-runnable `mise run companion-stress` loop reproduced the original symptom:
one instrumented 12.068-second settled run received 412 valid reports while
three UART errors advanced, and all three new errors were reported as FIFO
overflow (ring/glitch/frame/parity/other stayed zero). Matched 12-second USB
tests at temporary brightness 50 and 100 each accumulated four UART errors, so
brightness did not explain the observed difference between earlier sessions.

With interrupt buffering, matched rapid-STATE runs at brightness 100 and 50
received 439 and 445 valid reports with zero UART or CRC growth. A 20.100-second
requested Wi-Fi reconnect received 727, a 10.565-second screen recording
received 389 across 47 captured frames, and a quiet 25.117-second window
received 905; all retained zero companion faults. GPS also progressed with zero
reported faults, HRS remained connected, ride storage stayed at one ride/four
slots, and preferences returned to 100/30/20/-180. These tests isolate the old
main-loop polling cadence as insufficient for observed FIFO bursts; they do not
identify which particular display, USB, or radio stall coincided with every
overflow. Host tests cover explicit loss/resynchronization and a full ring, but
no hardware ring-exhaustion fault was forced.

The final normal harness/HRS build repeated the red-loop workload for 20.059
seconds: companion reports advanced 501 to 1,233 with CRC and UART counters both
unchanged at zero. GPS valid sentences advanced 319 to 780 with all GPS fault
counters unchanged, HRS stayed connected, and free heap remained 80,336 bytes.

Keep allocations in internal RAM if they contain atomics, back task stacks or
must work while the external-memory cache is disabled. The existing radio and
LCD DMA paths use internal memory. New PSRAM DMA use needs the peripheral's
alignment and cache-maintenance requirements checked and validated on hardware.

The original coin renderer remains available through
`mise exec -- cargo run --locked --example preview -- .local/coin.ppm 8 coin`.

### UI foundation validation (2026-09-11)

The host preview now uses the same one-pixel vertical offset as the display and
capture mapping and produces an exact 240×320 image. Both harness-enabled and
harness-disabled target builds passed. The enabled 559,312-byte image was flashed
with the safe slot-B workflow.

On hardware, an injected navigation scenario verified the menu's focused, pressed
and disabled states, cancellation without activation, touch activation, button
selection and return from Controls. Two checksummed captures show the pressed menu
and Controls screens. The existing smoke scenario then passed 48 captured frames
with 27 distinct images after explicitly navigating to Controls. Companion frames
advanced from 934 to 1,748 without new CRC or UART errors; sampled free heap changed
from 116,288 to 116,240 bytes. Ordinary frame work was 20–21 ms. Full-frame capture
raised the run's maximum to 45 ms, so it is evidence of harness cost rather than a
normal rendering bound. A lease-expiry test also canceled a held menu item and
restored screen, focus, brightness and counters without activation.

### Input policy

A Home tap must start and end on the same enabled item and stay within 18 physical
pixels of its initial point on both axes. Moving outside that slop cancels the tap
for the rest of the gesture. Visible Home hit regions include their horizontal and
vertical bounds; moving into a target after starting outside does not acquire it.
Controls keep a separate drag rule: the brightness slider captures only gestures
that start within physical y 231–294, clamps x to 24–216, and keeps the drag until
release or cancellation.

A real or injected release completes the gesture owned by that source. Touch
timeout, I2C failure, source transition, lease expiry and explicit `CANCEL` abandon
it. If a button changes the screen while a finger is held, all pointer reports are
ignored until release, including across repeated button presses. This prevents the
same finger from becoming a new gesture on the destination screen.

Only companion code 1 is assigned an action because it is the physically verified
short click. On Home, bottom-left and bottom-right move focus and top-left
selects an enabled item. Top-left returns toward Home, with Diagnostics returning
through Device; in Settings and Controls the bottom buttons adjust brightness,
and Device bottom-right opens Diagnostics. Other codes remain logged and counted for investigation
but do not change application state. Tap, drag, cancellation and cross-screen
suppression are verified through injection; no new physical hold, repeat or release
semantics are claimed.

The input scenario passed on the C606 with 44 debug commands. It covered two
button navigations while a touch remained held, ignored another report from that
finger, resumed after release, completed and canceled slider drags, rejected a
19-pixel menu movement, exercised the visible hit boundary and left the screen
unchanged for injected code 2. Companion frames advanced from 32 to 164 without
new CRC or UART errors, and sampled minimum heap was 115,988 bytes. The harness
build was 559,632 bytes and ordinary sampled frame work was 19–20 ms; a capture
raised the maximum to 44 ms. The prior navigation scenario also passed unchanged.

One first attempt at that regression did not parse its `BEGIN` reply because a
periodic frame log and debug reply interleaved on USB. The firmware continued
rendering until the test lease expired, and the immediate rerun passed. This is a
test-harness transport limitation already tracked with broader regression work,
not evidence of an input-state failure.

### Home, settings and device screens (2026-09-11)

Home summarizes battery and Wi-Fi state. Settings provides the same live 5–100%
brightness value through a larger touch slider and the two lower buttons. Device
shows battery percentage and voltage, charging/battery/unknown power, the current
Wi-Fi connection state and firmware version. Missing battery data renders `--`
and `--.--V`; missing or unrecognized power renders `UNKNOWN`, and unconfigured
Wi-Fi renders `WIFI NOT SET UP`. Status older than five seconds is already cleared
by the device loop and therefore uses these missing-value forms. Brightness,
idle controls and the fixed timezone offset now share the persistent settings
record described below.

The harness-enabled 562,064-byte image was flashed through the safe slot-B
workflow after both build modes passed. A device scenario navigated through all
four screens, dragged brightness from 5% to 100%, and rendered injected 8%, 3.30 V,
battery-power data. Its 28 checksummed frames contained 17 distinct images, and a
6.2-second transition clip plus Home, Settings and Device PNGs were retained as
private evidence. Companion frames advanced from 36 to 240 without new CRC or UART
errors; sampled minimum heap was 116,240 bytes. Ordinary frames took 20 ms. A
897 ms maximum had already been sampled during startup/USB initialization before
the session and did not increase during it, so it is not attributed to a screen.
The updated Controls smoke test also passed 51 captured frames and restored Home,
brightness, focus and counters at cleanup.

The host USB parser now recovers a valid debug reply that immediately follows a
recognized truncated `CYCLING_FRAME` prefix. Tests cover split reads at three
truncation points, including the `render_ms` field, while reboot, lease-expiry,
malformed reply and recording validation remain strict. This addresses the
specific flaky test start observed during UI work; broader harness reliability
remains tracked separately.

### Persistent preferences (2026-09-11)

Brightness, idle controls and timezone use a validated version 4 settings record
in the two-sector journal. Version 3 records migrate with a UTC timezone; version
2 records migrate their brightness while taking the default idle settings.
The earlier `cycling` version 1 marker migrates to the 50% default. Missing,
malformed and unsupported records also use defaults without overwriting the
unknown record. Live changes save once they are unchanged for one second and no
gesture is active; failures wait five seconds, and an uncertain result is reloaded
before another erase. Temporary debug sessions neither stage nor flush changes.

The hardware persistence test held an injected 65% value beyond the debounce,
reflashed safely and recovered the original 50%. It then explicitly saved 65%,
reflashed and recovered 65%, before saving and reflashing back to the original
50%. The two verified writes took 35 and 34 ms. Companion frames continued, CRC
and UART counts did not increase, and free heap stayed at 116,240 bytes. One save
raised its session maximum frame work from 23 to 55 ms; another session already
contained an unrelated 898 ms startup/Wi-Fi sample before the save.

### Idle dimming and wake (2026-09-11)

Settings includes tap rows for a 15/30/60/120-second or disabled dim timeout and
a 5/10/20/30% dim level. These fields share the debounced versioned preference
record. The selected brightness remains unchanged while the applied PWM becomes
the smaller of selected and dim level. Real and injected touch/button input use
one monotonic idle gate. The first wake button is consumed; a wake touch and all
of its reports through release are consumed. A held contact stays awake, and its
release starts a fresh inactivity interval. USB status, heartbeats, captures,
battery and Wi-Fi activity do not reset the timer.

On the C606, a temporary two-second timeout dimmed from selected 50% to effective
10% while state heartbeats continued. A held wake touch stayed awake for 2.4
seconds, then the first button and first tap each only woke the display after
subsequent dim cycles; the next inputs changed focus and opened Settings. Session
cleanup restored the persisted 30-second/10% configuration and its original idle
anchor. Companion frames advanced from 33 to 305 with no new CRC or UART errors,
and free heap did not decline. Capture work raised the sampled maximum frame time
from 22 to 44 ms. These checks verify PWM state and firmware behavior; they do not
measure panel brightness or power.

### On-device diagnostics (2026-09-11)

Device links to a Diagnostics screen by touch or the bottom-right button. It shows
uptime; previous and maximum frame processing time in milliseconds; free heap and
the sampled minimum in KiB; PSRAM free and capacity; Wi-Fi; valid companion and
CRC counts; UART and touch errors; and harness/recording state. Growing counters
use bounded K/M notation. Runtime minima, maxima and counters are collected every
display cycle in both build modes. The displayed copy updates once per second to
bound redraw churn, while USB state reports retain live per-frame metrics. The
sampled heap minimum is not a peak-allocation or stack high-water measurement.

The harness-disabled image was 554,304 bytes and the restored harness-enabled
image was 564,640 bytes. On the C606, touch navigation, top-button back and
bottom-right reopen all passed. A representative capture showed 19 ms current and
47 ms maximum frame work, 113 KiB free and sampled-minimum heap, 2,048 KiB PSRAM
free of 2,048 KiB, Wi-Fi test success, zero CRC and touch errors and one existing
UART startup overflow. The corresponding live report was 19/47 ms, 116,288 free
and 115,860 sampled-minimum bytes, and 2,097,152 PSRAM bytes free/capacity. The
displayed valid-frame count trailed the live report by less than its one-second
refresh interval. Companion frames advanced from 361 to 476 without new errors or
retained heap loss. The capture itself raised the maximum from 28 to 47 ms, so 47
ms is harness capture timing rather than an ordinary rendering baseline.

The stock ESP-IDF bootloader loads the Rust application; no ESP-IDF application
runtime is linked. A compatible application descriptor is supplied by
`esp-bootloader-esp-idf`.

## Wi-Fi

```sh
mise run wifi-setup
mise run flash
```

`wifi-setup` reads the connected NetworkManager personal Wi-Fi profile and saves
credentials in ignored `.local/wifi/config.json`. The C606 needs a 2.4 GHz network.
WPA2 is supported; WPA3 is configured when the profile uses SAE, but has not been
tested on hardware.

Builds generate private Rust configuration and place all firmware build artifacts
in `.local/firmware`. Configured images contain the Wi-Fi password and must remain
private. Without a configuration file, the firmware builds with Wi-Fi disabled.

The connectivity task resolves `example.com`, fetches its page over HTTP, and
checks for a successful response containing the expected heading. A successful
connection stays up. Association, DHCP and requests have bounded timeouts;
failures retry with delays capped at 30 seconds. Public-server probe failures
retry without tearing down a usable Wi-Fi link, while DHCP failures request a
fresh association. This is a connectivity test, not TLS or authenticated server
verification. The screen shows current progress; USB logs omit SSIDs, passwords,
network addresses and driver-provided identity details.

On the C606, one deliberate reconnect and two controlled public-server failures
completed with association/probe counters advancing from 1/1/0 to 4/4/2. The
injected DNS and request failures each recovered on the existing association
after a one-second retry. Free heap changed from 116,288 to 116,240 bytes;
companion packets advanced from 296 to 604 with no new CRC or UART errors. This
was a short harness-enabled recovery test against one public server, not a
long-duration network or memory soak.

## Network time

Once DHCP is ready, the firmware queries `time.cloudflare.com` over SNTP and
keeps a UTC anchor against Embassy's monotonic clock. DNS and UDP receive each
have five-second limits, the whole attempt is limited to ten seconds, ordinary
failures use the capped retry delay, and a successful clock refreshes hourly.
Server replies must match the selected source and echoed request timestamp and
must have a valid server mode, version, leap state, stratum and timestamps.
`RATE` replies impose a 15-minute deadline that survives reconnects; `DENY` and
`RSTR` disable further requests until restart.

The Device screen shows local `HH:MM` with `SYNCED`, `OFFLINE TIME`, `STALE TIME`,
`SYNCING` or `NO TIME`. Settings cycles a persisted fixed offset in 30-minute
steps from UTC-12:00 through UTC+14:00; UTC is the default. The offset has no
automatic daylight-saving or timezone-rule support. There is no verified RTC,
so every restart begins without time until a valid network reply arrives. A
valid anchor continues advancing while temporarily offline and becomes stale
after six hours without synchronization.

On the C606, the first reply reported stratum 3 and a 32 ms UDP round trip. A
near-simultaneous host comparison observed device UTC 3 ms behind the host
midpoint, within a 194 ms host command window. The client anchors the server's
transmit time at receipt and does not calculate the full four-timestamp NTP
offset, so this observation is not a general accuracy bound. During a deliberate
Wi-Fi reconnect the anchored time continued advancing, then the synchronization
age fell from 29,369 ms to 250 ms after a fresh reply. Companion packets kept
advancing with no new CRC or UART errors.

The parser currently handles NTP era 0. Because it rejects wrapped timestamps
below the Unix epoch offset, it will need era handling before February 2036.

## Simulated ride

Home's `DEMO RIDE` entry opens a prototype with two selectable pages and A/B
field layouts. Every page remains visibly marked as demo data. The right button
starts, pauses and resumes according to the displayed phase; the left button
changes page; the top button returns Home. A separate touch target changes the
layout, and Reset is available only while paused. Navigating or changing the
page/layout does not pause or reset a running ride.

The portable model uses caller-provided monotonic milliseconds. While running it
simulates a constant 5,000 mm/s (18.0 km/h), calculates distance as five times
active milliseconds, and stops both speed and accumulation while paused. It is
independent of render rate and capture stalls. Live debug metrics use the current
monotonic timestamp; the on-screen fields use the existing one-second display
snapshot and can trail them by roughly one second.

On hardware, the scripted run measured 2,231 ms and 11,155 mm, then froze at
2,721 ms and 13,605 mm throughout a 1.1-second pause. After resume it reached
3,888 ms and 19,440 mm before pause/reset returned elapsed and distance to zero.
The run produced 27 verified frames with 10 distinct images while page and layout
changed. Companion packets advanced from 228 to 432, free heap remained 116,288
bytes, and CRC/UART counts did not increase. Debug session cleanup restored the
original screen, ride phase, monotonic anchor, page and layout; a pre-session
running ride therefore includes wall time spent in the temporary session after
its original anchor is restored. This prototype does not read sensors or write
ride records.

## GNSS transport follow-up

The original interrupt-fed UART receiver lost data under bounded load. In a
20-second harness comparison, 451 complete `STATE` replies advanced the loss
counter by 10 while a 20-second closed-USB control advanced it by zero. A normal
heartbeat advanced it by one, 30 short USB reopen sessions by five, and a Wi-Fi
reconnect plus heartbeats by two. These observations support interrupt blackout
during long JTAG output as the leading mechanism. The radio-plus-heartbeat run
also lost data, but it does not isolate a radio-specific contribution or assign
every historical loss to one source.

UART0 RX now feeds an 8 KiB internal-memory UHCI stream on DMA channel 1; the LCD
continues to own DMA channel 0. The main loop copies only currently available
bytes with bounded `peek`/`consume` operations. DMA descriptor-empty/error and
UART FIFO/framing/parity/glitch flags are read separately. Either condition
invalidates the partial NMEA sentence and restarts the receive transfer. A
controlled six-second consumer stall filled the descriptor chain, recorded one
DMA loss plus the resulting UART FIFO overflow, restarted, and resumed parsing
355 valid sentences. This proves the tested recovery path, not indefinite input
retention during an unbounded stall.

On the final harness-enabled firmware, the same rapid test completed 451 state
replies in 20.058 seconds while GNSS advanced 43,264 bytes and 516 valid
sentences; UART, DMA, checksum, parser and line-loss counters were unchanged. A
Wi-Fi reconnect plus 20 seconds of heartbeats advanced 43,008 bytes and 512
valid sentences with those counters unchanged. A separate 15-second recording
captured 118 frames while GNSS advanced 36,096 bytes and 403 valid sentences,
also without an observed counter increase. These bounded runs show the original
reproduction no longer fails; they do not prove loss-free operation under every
load or duration.

The parser associates satellite count with coordinates only when checksum-valid
GGA and coordinate sentences have the same UTC value, including normalized
fractional seconds. A bounded hardware probe sent the two stock-supported,
read-only `PAIR020` and `PDTINFO` queries over recovered UART0 TX1. It produced no
recognized checksum-valid identity response and caused two overlong input lines,
so the probe is absent from normal startup and the fitted receiver remains
unidentified. A separate bounded test sent the exact recovered stock close frame
and restored the open frame four seconds later. Parser counters stayed fixed at
11,008 bytes and 129 valid sentences between the two markers, then advanced by
5,120 bytes and 58 sentences over the next two seconds. This verifies stream
control through the companion command path; it does not establish electrical
power state. The current device location is indoors; earlier indoor positions
near the authorized reference remain valid evidence, while outdoor reception is
still untested.

## Bluetooth

The C606 runs the pinned `esp-radio` controller with a locally patched Trouble
Host 0.6.0. A build selects either the `Cycling Echo` peripheral or one continuous
Heart Rate or Cycling Speed and Cadence peer. Sensor mode performs bounded scans,
matches only its explicit local target, subscribes to the selected standard
measurement characteristic and reconnects with capped backoff. It does not pair,
bond, connect to unknown peers, or acquire two sensors simultaneously.

The host reserves one connection, two L2CAP channels and four 64-byte packets in
internal memory. BLE initialization reduced measured internal free heap from
163,840 to 127,920 bytes; after Wi-Fi and the other device tasks settled, about
80 KiB remained. The unmodified server reproduced a mixed short/long declaration
truncation at ATT MTU 23: BlueZ discovered the long characteristic as the false
short UUID `0x0c06`. The carried patch stops a discovery response before a later
entry would change its element size or be truncated. Two minimum-MTU sessions
then discovered the full UUID and completed notification, exact eight-byte write
and readback, and rejected a short write without changing the value.

A simultaneous startup window observed 22 advertisement reports at the C606
with RSSI -99 to -50 dBm and 27 BlueZ RSSI updates at the laptop with RSSI -100
to -46 dBm. These are controller-specific reports from overlapping scans, not a
claim that either radio saw the same advertisers or that their RSSI values are
directly comparable.

The owned laptop simulator separately verified continuous CSC-only acquisition,
stale-value removal and explicit reconnect. A user-authorized real HRS device
verified HR-only acquisition and reconnect without publishing its identity or
physiological readings. Contact-not-detected removes a heart-rate value; devices
without contact support remain accepted. The parsers reject malformed lengths,
retain bounded information from the first four RR intervals, count additional
intervals, and reject duplicate or implausible cadence updates without refreshing
freshness. Simulator measurements are fixture evidence, not physiological data.

A short explicit live recording from the authorized HRS device stored 18 samples
with heart-rate fields in ride 5, slots 22 through 29. A 45.435-second coexistence
window observed 46 BLE notifications, 975 valid GPS sentences and 1,513 companion
packets while Wi-Fi remained verified. BLE invalid/disconnect, GPS fault and
companion fault counters did not advance; free heap was 80,336 bytes before and
after with a sampled minimum of 79,848 bytes. The record format identifies the
ride as `live`; it does not distinguish a real sensor from the owned simulator,
so fixture recordings must retain that provenance in their test evidence.

During a 30-second running-demo coexistence window, the two-round echo client
passed while 696 display loops, 715 valid GPS sentences and 988 companion packets
advanced. Wi-Fi remained verified, CRC/parser/ring counters stayed unchanged,
and free heap ranged from 80,168 to 80,308 bytes. GPS UART errors increased by
three around the BLE connections and recovered; this is tracked with the existing
UART-loss investigation rather than treated as clean coexistence. All 64 sampled
GPS states were fresh: their positions were 7.6–11.4 m from the issue-authorized
reference with reported ages of 0–941 ms. This is an indoor position comparison,
not a receiver accuracy or satellite-epoch claim.

## Crash and reset diagnostics

The firmware reads the ESP32-S3 reset reason at startup and consumes a seven-word
panic record from RTC fast persistent memory. The record has magic, schema, panic
kind, an eight-byte bounded firmware version, checksum and a commit word written
last. Missing, torn, corrupt and unsupported records are handled without blocking
startup. Consumption clears the commit first and then the body, so an old panic
is reported once. No flash partition, settings record or heap allocation is used.

The existing `esp-backtrace` panic handler calls a small pre-hook that performs
only fixed volatile RTC writes, prints its normal best-effort backtrace, and then
calls a custom halt hook that resets the whole chip. The marker distinguishes a
generic panic from the harness-only controlled test; a software reset reason by
itself is never labeled as a panic. Diagnostics and USB state expose bounded reset
and marker names, while the boot log also includes the version stored with a
valid marker.

On hardware, the controlled command acknowledged `ARMED`, restored its temporary
session, printed a backtrace and rebooted. The next boot reported software reset,
controlled marker and version 0.1.0. Brightness, dim timeout/level and timezone
were unchanged; over the next two seconds display frames advanced from 1 to 49
and companion packets from 0 to 70. A clean requested restart then reported
software reset with no marker, proving the consumed record was not replayed.

RTC retention is verified here only across the tested whole-chip software reset.
It is not power-loss persistence and is not promised across reflashing or booting
stock. The HAL also documents ambiguity in the chip-power-on reason, so that label
is not proof against brownout or a super-watchdog reset. Cache-disabled faults,
corrupt stacks, exceptions and watchdog paths may not reach the Rust panic hook or
finish backtrace output.

## Artwork

The demo uses a rasterized and colored version of the Rust logo. Its attribution
and CC BY 4.0 license are in [the asset directory](../packages/os/assets/README.md).
Our code is MIT licensed.

## Device screenshots

```sh
mise run screenshot
mise run screenshot -- --output .local/screenshots/current.png
```

The tool requests a frame from the running firmware over USB and saves a 240×320
PNG in ignored `.local/screenshots/` by default. It uses the same USB lock as the
flash and monitor tools. Close other serial readers first. `--port` or
`CYCLING_PORT` selects the port; `--timeout` defaults to 20 seconds.

The firmware copies the canvas immediately after drawing it and transmits one
row per display cycle, taking about five seconds. The frozen copy costs 16,960
bytes of RAM. Input handling and Wi-Fi continue during transfer. The PNG expands
RGB565 colors and reproduces the LCD's 3× scaling, including its edge rows.
This captures the pixels sent by firmware, not panel readback or backlight output.

The text command is `SCREENSHOT\n`. `CYCLING_SHOT BEGIN` gives the frame number,
canvas width, height and an FNV-1a checksum of little-endian RGB565 bytes. `ROW`
lines contain the frame number, zero-based row and four hex digits per pixel.
`END` carries the frame number. Ordinary USB logs can appear between these lines.
The host requires ordered rows and a matching checksum before writing a PNG.
Requests during a transfer are ignored. No credentials or memory dumps are exposed.

The host tool uses Linux terminal APIs without changing DTR/RTS and disables
hangup-on-close. This avoids the restart observed when opening this device with
the monitor's serial configuration. Two separate tool invocations captured advancing frame numbers
without restarting the app. Hardware captures showed battery
readings and `WIFI TEST OK`; both HTTP checks passed, rendering remained at
19–20 ms during transfer, and companion reception continued with zero CRC errors.
The previously observed single UART overflow during Wi-Fi startup remained.

See [device debugging](device-debugging.md) for injected input, video recording,
state assertions, Wi-Fi recovery tests and scripted end-to-end runs.
