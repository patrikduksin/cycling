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

The OS is `no_std`, using esp-rtos and Embassy for asynchronous Wi-Fi alongside
the display loop, with a 160 KiB internal heap. An 80×106 RGB565 canvas is enlarged
3× onto the 240×320 display, using eight-row DMA transfers. A portable `App` owns
the current screen, focus, pressed gesture and the existing controls state. The
initial menu exposes the controls diagnostics and shows a disabled placeholder
for later device screens. Pointer release activates a target; timeout, input
failure and debug cleanup cancel it. Physical and injected input call the same app
handlers. The controls diagnostics shows battery percentage, voltage and power
status, counts button events, and tests brightness from 5 to 100 percent.
Target cadence is 24 fps. Touch uses the stock 0x5a report protocol over I2C;
brightness changes the existing backlight PWM duty. It resets to 50 percent on
boot. Bottom-left and bottom-right short clicks also change brightness by five
points. The companion receiver uses UART2 RX41 at 115200 baud and validates
packet CRCs; it sends no commands. Wi-Fi station support uses DHCP and reconnects after disconnects. The C606's
2 MiB Quad SPI RAM is initialized at 40 MHz, tested on each boot and exposed
through a separate external-only allocator. BLE and companion power control are
still future work.

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

A menu tap must start and end on the same enabled item and stay within 18 physical
pixels of its initial point on both axes. Moving outside that slop cancels the tap
for the rest of the gesture. Visible menu hit regions include their horizontal and
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
short click. On the menu, bottom-left and bottom-right move focus and top-left
selects an enabled item. In Controls, top-left returns to the menu and the bottom
buttons adjust brightness. Other codes remain logged and counted for investigation
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

The bring-up task resolves `example.com`, fetches its page over HTTP, and checks
for a successful response containing the expected heading. It disconnects the
C606 once after success, reconnects, and repeats the check. This is a connectivity
test, not TLS or authenticated server verification. The screen shows progress;
USB logs omit SSIDs, passwords and network addresses.

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
