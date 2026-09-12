# Device debugging

Use the ordinary terminal and JSON log stream for base and SDK firmware. USB
has one owner on the device and an exclusive `.local/usb.lock` on the host.
Do not run a collector, export, test or flash session concurrently. Raw evidence,
identifiers, physiological readings and credentials stay in ignored `.local/`.

## Build modes and collection

```sh
mise run build
CYCLING_HARNESS=0 mise run build
CYCLING_SDK=1 mise run build
CYCLING_SDK=1 CYCLING_HARNESS=0 mise run build
mise run terminal -- STATUS           # Print one correlated JSON reply
mise run terminal                     # INFO, then an interactive prompt
mise run logs -- --seconds 30 --command INFO
```

The terminal accepts commands without the `CMD id` prefix. One-shot mode exits
after printing the reply; interactive mode prints initial `INFO` metadata, then
accepts commands until `quit`, EOF or Ctrl-C. `ACCEPTED` means queued, not complete.
It sends each command once and writes private raw USB bytes under `.local/terminal`
or `--output`. It reads USB while awaiting replies, not while waiting for keyboard
input; use `logs` for continuous collection. Both use the same exclusive USB lock.

`CYCLING_SDK=0` is the base default; `CYCLING_SDK=1` enables cycling workflows.
`CYCLING_HARNESS=1` is the development default; zero removes optional test faults,
log saturation and controlled panic. Both modes retain terminal reception,
ordinary logs, position/input/status/settings, Wi-Fi and the public bring-up
probe. Harness-disabled firmware is not a quiet production build.

The collector opens Linux tty without flushing input or changing DTR/RTS. It
records raw bytes, decoded records and a summary, reconnects with bounded backoff,
and sends each requested command once. Reconnection never replays mutations.
Pass `--port` or `CYCLING_PORT` if needed. Add `sudo` only when USB permissions
require it. Do not change system security for a test. See [safe device tasks](device.md)
for flashing and stock restoration; always pass the desired build mode to
`mise run flash`, since it rebuilds `.local/cycling.bin`.

## Ordinary protocol

Requests are ASCII `CMD <id> <command>\n`, up to 128 bytes before newline.
Replies are JSON Lines with `type="reply"`, `id`, `status`, device `ms` and a
bounded `data` string. Malformed/overlong lines produce `id=0`, `status="INVALID"`
and execute nothing. The parser discards the entire overlong line through its
newline. A client must correlate replies and keep at most one request pending.

| Command | Meaning |
| --- | --- |
| `HELP`, `INFO`, `STATUS` | Supported commands, build mode and general diagnostics |
| `POSITION` | Fix/no-fix/staleness, transport state and parser/loss counters |
| `INPUT`, `BATTERY` | Physical input counters and available/stale battery/power observations |
| `TIME` | Network time status and synchronization age |
| `SETTINGS` | Selected preferences and effective brightness |
| `BRIGHTNESS 5..100` | Change selected brightness |
| `TIMEZONE minutes` | Offset -720..840, in 30-minute increments |
| `IDLE seconds level` | Timeout 0..3600, zero disables; dim level 5..100 |
| `SAVE` | Explicitly persist current validated preferences |
| `ACTIVITY` | Reset idle activity time |
| `WIFI`, `WIFI RECONNECT` | Connectivity diagnostics and bounded recovery request |
| `BLE`, `BLE RECONNECT` | Generic link/notification/drop state and selected-client reconnect |
| `STORAGE` | Owned flash reservation geometry |
| `DISPLAY rgb565hex` | Synchronous solid fill, e.g. `DISPLAY f800` |
| `RESTART` | Acknowledge and request a software reboot |
| `TEST n` | Optional harness operations; disabled builds return UNSUPPORTED |

SDK builds additionally expose ride/sensor/history/export operations described in
[ride recording](ride-recording.md). Base firmware rejects SDK commands with INVALID. Read `HELP` and build metadata before choosing feature-specific tests.
Test operations include Wi-Fi fault controls, log saturation, a controlled panic
and `TEST 20`, a six-second executor stall. The stall blocks executor acquisition
while the existing UART interrupts/DMA continue; buffers may exhaust and report
loss. It is restricted by the host test to base firmware with no cycling/ride
consumer. Its acknowledgment arrives after the stall, so allow more than six
seconds for that one request and never retry it automatically. The terminal's
reply `ms` was sampled before dispatch and is not the end of the stall; use
subsequent POSITION/INPUT timestamps for recovery measurements.

Logs and replies are serialized as complete records. Log buffering is bounded;
dropped records create sequence gaps/loss counts rather than growing memory or
blocking acquisition behind an absent USB host. ROM/panic text may remain
unstructured and is retained in the raw capture. A record from a fresh boot or a
sequence gap is not proof that all earlier events were received. Details and
queue limits are in [logging](logging.md).

## Bounded tests

```sh
mise run e2e
mise run gps-stress
mise run companion-stress
mise run regression
mise run persistence-test       # Explicit save/restart; restores original settings
mise run crash-test             # Harness only; controlled reboot and marker check
```

`base_test.py` checks invalid/overlong requests, repeated no-reset USB opens and
acquisition while display submission stays unchanged. Optional `--restart`,
`--write-settings`, `--wifi-recovery` and `--panic` exercise those paths.
`--transport-recovery` issues TEST 20 once, records pre/post loss counters and
requires resumed parser/companion progression plus sustained receiving state.
Loss increments are allowed in this deliberate starvation test; a pass does
not establish electrical receiver control, overflow on every buffer, or recovery
from faults that were not observed. Default progress tests still reject new loss.
Run it only on the base harness-enabled build:

```sh
python scripts/base_test.py .local/tests/transport-recovery --transport-recovery
```
 Settings
writes snapshot original values and restore them in cleanup. An ambiguous save
is not retried blindly; a failed restoration is reported for inspection.
Never run mutation tests against a ride that must remain active.

The GPS and companion tools now use ordinary `POSITION`/`INPUT` requests and work
without the harness. They require progress, compare error counters and preserve
preferences, storage-operation counts and SDK ride inventory when enabled.
They use device timestamps and report sampled heap minima. An indoor no-fix
state is permitted when bytes and valid sentences advance. Longer windows can
be requested through the scripts, up to 600 seconds; a short run is not an
unmeasured overnight reliability guarantee.

For BLE echo, run `mise run bluetooth-echo` against base echo mode or an SDK build
selected with `CYCLING_BLE_MODE=echo`. It verifies two reconnect cycles, exact
eight-byte notifications/readback and short-write refusal. Its reported MTU is
observed, not forced. Use `btgatt-client --mtu 23` separately for the retained
minimum-MTU check. SDK `CYCLING_BLE_MODE=sim-heart` or `sim-csc` selects the owned
laptop fixture; `CYCLING_SIM_PROFILE=heart` or `csc` controls its exposed services.
Stop the simulator after testing so it unregisters its advertisement and GATT
application. These tools use system Python for BlueZ D-Bus/GLib bindings.

## Interpreting results and historical evidence

There is no graphical screenshot/injection session or recording protocol in the
base terminal. The old `DBG`, `SCREENSHOT`, preview, scene and visual-comparison
tools are retired. Display fill checks firmware submission and DMA completion;
physical panel output still requires observation. Input counters or injected
software tests do not establish switch behavior or touch accuracy. Battery
reports do not establish calibrated capacity or charging hardware behavior.

The retired shell recorder reserved two 16,960-byte canvases, 33.125 KiB combined,
even when idle. Those buffers are removed. Its requested 10 fps capture achieved
about 7.8 fps in one run; device timestamps, not requested fps, were the evidence.
Observed processing was mostly 20–30 ms, and a full-frame capture reached 44 ms
against its former 42 ms frame target. These are historical recording costs,
not baseline measurements of the current services. Idle harness CPU overhead
and power savings remain unmeasured. Use harness-disabled firmware for new
performance/power baselines and record SDK mode, harness mode, recording state,
traffic and elapsed time with results.

Keep sampled heap minima distinct from peak usage and linker reservations from
measured stack consumption. September 11 shell soak, companion-loss fixes,
physical touch/button observations and prior reset/persistence evidence remain
recorded in [the execution history](overnight-plan.md) and
[C606 research](../packages/stock/magene-c606/README.md). Their UI commands are
historical and are not current entry points.
