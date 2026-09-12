# Development

`packages/os` contains the portable base services, optional cycling SDK and C606
firmware. Board ownership lives in `src/device`; Embassy acquisition tasks live
in `src/services`; `main` composes them with the ordinary terminal. The former
App, coin renderer, preview example and graphical test scenarios are removed.

## Tools and checks

```sh
mise install
mise run setup
mise run test
mise run check
mise run build
CYCLING_HARNESS=0 mise run build
CYCLING_SDK=1 mise run build
CYCLING_SDK=1 CYCLING_HARNESS=0 mise run build
```

Mise pins Rust, Python, uv, espup and espflash. `setup` installs Espressif's
Xtensa Rust 1.97.0.0 toolchain as `cycling-esp`; host tests use Rust 1.98.1.
The build script loads its environment without changing the user's shell.
Keep Cargo.lock committed. HAL 1.1.2, esp-radio 1.0.0-beta.0 and esp-rtos 0.3.0
remain pinned; no alternative runtime or moving vendor Git dependency is needed.

`test` runs host base and `cycling` suites, the licensed Trouble Host tests and
Python tooling tests. `check` runs formatting and Clippy for both host feature
sets. CI builds all four combinations of `CYCLING_SDK=0/1` and
`CYCLING_HARNESS=0/1`. Base and SDK both use the same board drivers and services.
Harness selection only controls test operations, not ordinary terminal access.

| Task | Purpose |
| --- | --- |
| `fmt`, `test`, `check`, `build` | Format, verify and build |
| `terminal`, `logs` | Interactive/one-shot commands and continuous private JSON capture |
| `e2e`, `regression` | Framing, no-reset opens and acquisition without rendering |
| `gps-stress`, `companion-stress` | Bounded read-only acquisition/error checks |
| `persistence-test` | Explicit temporary save/restart and original-settings restoration |
| `crash-test` | Harness-controlled panic and one-shot reset marker verification |
| `bluetooth-echo`, `ble-simulator` | Owned Linux BlueZ interoperability fixtures |
| `ride-export`, `ride-clear` | SDK export and explicitly verified reclaim |

Use `mise run terminal -- STATUS` for one JSON reply, or `mise run terminal`
for an interactive prompt. Use `mise run logs -- --seconds 30 --command INFO`
for continuous capture; run only one USB owner at a time.

## Resource and service ownership

`device::c606::init` consumes HAL singleton resources. LCD uses DMA_CH0; GNSS
uses UART0, UHCI0 and DMA_CH1; companion UART2 retains its interrupt-fed 2 KiB
ring. The LEDC timer has stable board-owned storage so the backlight channel's
borrow survives moving resources. Clocks remain 160 MHz CPU and 40 MHz quad
PSRAM. The configured internal heap remains 160 KiB; the 2 MiB PSRAM has a
separate allocator and a full startup integrity test.

Position acquisition consumes at most 2,048 bytes per nominal 10 ms poll.
It publishes only changed parser state; callers calculate freshness at their
own read time. Copying the original parser preserves its fix, satellite-epoch
and loss rules. Host sizes are 648 bytes for the parser and 696 for acquisition
state; these are type sizes, not a device stack measurement. No consumer needs
to acknowledge updates.

Input acquisition polls touch and consumes companion reports independently of
terminal/display work. Its 16-entry edge queue never blocks acquisition. On
queue overflow, old queued history is discarded and Cancel precedes the newest
edge. Buttons retain raw report codes; gesture/navigation policy is absent.
Battery/power observations retain unavailable or stale values and receipt times.
Stock interprets power zero as charging; other values remain uninterpreted.

Display submission accepts an explicit 80x106 RGB565 frame or a solid native
240x320 fill. The existing 3x mapping, top-row offset and eight-row DMA strips
are retained. Exclusive mutable ownership prevents simultaneous submissions;
return follows final DMA completion. The HAL's blocking wait has no newly
established timeout or worst-case timing guarantee.

BLE core transport discovers one explicitly selected name/address and UUID pair,
then publishes complete notifications through a four-packet queue. Connection,
sequence and drop stamps let the SDK discard obsolete samples and reset cadence
continuity. HRS/CSC decoding and sensor freshness live in the optional SDK.
An unselected base build exposes the owned echo fixture. Wi-Fi association,
DHCP, public probe and SNTP retain bounded waits and recovery generation checks.

Normal allocation, radio buffers, task state and DMA stay internal. PSRAM must
not hold atomics or memory needed while its cache is disabled. New PSRAM DMA
use requires separate alignment/cache validation on hardware.

## Historical measurements

These measurements came from the retired graphical firmware on September 11,
2026. They are useful regression evidence, not performance claims for the new
base firmware. Detailed run history remains in [the execution log](overnight-plan.md).

The former 42 ms main-loop UART polling lost companion FIFO reports. The bounded
interrupt receiver passed a subsequent 20.059-second workload with 732 companion
reports, advancing GPS, no new companion/GPS errors and 80,336 bytes free heap.
No physical ring-exhaustion fault was forced. The current services retain that
receiver and move its consumer outside the graphical cadence.

A prior 602.013-second shell soak sampled 562 times. Heap started and settled at
116,240 bytes and sampled a 116,192-byte minimum. Such sampled minima are not
peak allocation or stack-use measurements. The older redraw optimization reduced
observed unchanged-screen loop work from roughly 20 ms to 3 ms while retaining a
16,960-byte external history canvas. That history and its benchmark loop are
removed; do not treat those figures as current idle CPU or power results.

GPS transport/parser progression and no-fix handling can be checked indoors.
The user reported successful outdoor acquisition; no new reference-accuracy or
fitted-receiver identification claim follows from this refactor. Native MMC
CLK13/CMD14/D016 identification and repeated reads were verified; D1-D3 remain
candidates. No vendor filesystem writes are permitted.
