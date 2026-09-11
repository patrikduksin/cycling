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

The firmware uses `esp-hal` 1.2.1 and Rust edition 2024. LCD support currently
requires its `unstable` feature, so that dependency is pinned exactly. Cargo.lock
pins the full dependency graph. Current releases are preferred, with deliberate
updates rather than moving Git branches.

## Commands

| Command | Purpose |
|---|---|
| `mise run fmt` | Format Rust |
| `mise run test` | Host input, UI, renderer and flash-format tests |
| `mise run check` | Formatting and Clippy |
| `mise run build` | Release ELF and `.local/cycling.bin` |
| `mise run preview` | Render `.local/controls.ppm` on the host |

The OS is `no_std`, with no allocator or RTOS yet. An 80×106 RGB565 canvas is enlarged
3× onto the 240×320 display, using eight-row DMA transfers. The current screen shows battery percentage, voltage and power status, counts
button events, tests touch and controls brightness from 5 to 100 percent.
Target cadence is 24 fps. Touch uses the stock 0x5a report protocol over I2C;
brightness changes the existing backlight PWM duty. It resets to 50 percent on
boot. Bottom-left and bottom-right short clicks also change brightness by five
points. The companion receiver uses UART2 RX41 at 115200 baud and validates
packet CRCs; it sends no commands. Radio, PSRAM and companion power control are
still future work.

The original coin renderer remains available through
`mise exec -- cargo run --locked --example preview -- .local/coin.ppm 8 coin`.

The stock ESP-IDF bootloader loads the Rust application; no ESP-IDF application
runtime is linked. A compatible application descriptor is supplied by
`esp-bootloader-esp-idf`.

## Artwork

The demo uses a rasterized and colored version of the Rust logo. Its attribution
and CC BY 4.0 license are in [the asset directory](../packages/os/assets/README.md).
Our code is MIT licensed.
