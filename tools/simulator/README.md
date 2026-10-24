# Shell simulator

The `shell-simulator` package runs the shared shell and optional VANA application
on a host, composed like C606 firmware, inside a simulated world. It owns its fake
devices because they serve simulation and test control rather than physical hardware.

Start with `src/sim.rs`, the scenario driver: press buttons, change the world, let
time pass and read the screen. `session.rs` composes the device in the firmware
loop's order and serves the terminal backend. The world and its observers:

- `world.rs`: rider speed, power, heart rate and cadence, distance, route,
  elevation and vehicles behind the rider, in simulated wall time.
- `ant.rs`: a simulated ANT companion under the production `Channels`, and
  ANT+ heart-rate, power, speed/cadence and radar sensors that encode the world.
  The companion starts 1.5 s after power-on, closes a channel whose sensor has
  been silent for its search timeout, takes acknowledged sends (a power meter
  answers a zero request with an offset, a failure or silence, as its `zero`
  field says), and survives a CPU-only `RESTART`; power
  loss restarts it too. Those timings are inferred, not measured on C606.
- `route.rs` and `gps.rs`: GPX or synthetic routes and a receiver that emits NMEA.
- `screen.rs`: reads text from the frame buffer with the shared 5x7 font.
- `devices.rs`, `storage.rs`: display, input, power, buzzer, battery, and file or
  memory media with injected torn writes; memory media survives a power loss.
- `rides.rs`: reads recorded rides back with VANA's journal decoder.

Scenarios live in `tests/ride_scenarios.rs`. `mise run sim` runs them with
`SIM_ARTIFACTS` set: `recording.rs` writes a captioned video, checkpoint
screenshots (`sim.checkpoint(..)`), a chart of the world, recorded rides as GPX
and JSON, a timeline and `result.json` for each scenario under `.local/sim/latest`,
including failed ones. `artifacts/` holds the PNG, GIF, MP4 (through `ffmpeg`)
and chart encoders. `mise run sim-pr <number>` posts the run inline on a pull
request through `cycling_devtools.sim.report`. `mise run sim-compare [--base <ref>]
[--filter <test name>] [--pr <number>]` also runs the base ref (default `origin/main`, fetched first) in a temporary
worktree and reports only screens whose pixels changed, Before | After
(`cycling_devtools.sim.compare`).

`mise run sim-live` opens a window (`src/bin/sim-live.rs`, behind the `live`
feature) onto `live.rs`: the device powers on beside a rider at 25 km/h on a
loop route with GPS and ANT heart-rate, power, speed/cadence and radar sensors
in range. Time runs in 10 ms firmware passes at real time or faster:

| Keys | Action |
| --- | --- |
| `Q` `A` `D` | Top-left, bottom-left, bottom-right button; a key held past 0.3 s repeats like a held button, and the device runs at 1x while it is held; holding `Q` while off turns it on |
| `H` `P` `C` `R` | Heart-rate, power, speed/cadence, radar sensor in or out of range |
| `B` / `G` | BLE heart strap in range / GPS fix |
| `L` / `K` | Next battery level (80, 40, 15, 5%) / charging |
| `V` / `X` | A vehicle approaches from behind / power loss |
| `S` | Save a checkpoint screenshot |
| `Space`, `1` `2` `3` | Pause; 1x, 10x, 60x |
| `Esc` | Quit and write the recording |

Each session is written as `live_session-<unix time>` under `.local/sim/live`
when the window closes or on `Esc`; Ctrl-C in the terminal discards it. `sim-live --frames N --screenshot out.png` renders N
frames at 30 fps without a window.

`mise run simulate` runs the base demonstration and `mise run harness-virtual`
the shared terminal scenarios. Terminal mode (`--terminal`) requires an explicit
disposable `--state-dir` and prints a `ready` line once booted; the harness builds
the simulator first and starts its reply timeouts only after that line.

Simulation checks presentation, input routing, sensor and ride behaviour, command
compatibility and recovery from storage faults and power loss. Its companion
mirrors the C606 adapter's admission rules but not its UART framing. It does not
model DMA timing, radio coexistence, electrical behavior or power sequencing.
Device changes still need the relevant hardware validation.
