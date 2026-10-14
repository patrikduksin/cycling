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
  been silent for its search timeout, and survives a CPU-only `RESTART`; power
  loss restarts it too. Those timings are inferred, not measured on C606.
- `route.rs` and `gps.rs`: GPX or synthetic routes and a receiver that emits NMEA.
- `screen.rs`: reads text from the frame buffer with the shared 5x7 font.
- `devices.rs`, `storage.rs`: display, input, power, buzzer, battery, and file or
  memory media with injected torn writes; memory media survives a power loss.
- `rides.rs`: reads recorded rides back with VANA's journal decoder.

Scenarios live in `tests/ride_scenarios.rs`; run them with `mise run sim`.
`mise run simulate` runs the base demonstration and `mise run harness-virtual`
the shared terminal scenarios. Terminal mode (`--terminal`) requires an explicit
disposable `--state-dir`.

Simulation checks presentation, input routing, sensor and ride behaviour, command
compatibility and recovery from storage faults and power loss. Its companion
mirrors the C606 adapter's admission rules but not its UART framing. It does not
model DMA timing, radio coexistence, electrical behavior or power sequencing.
Device changes still need the relevant hardware validation.
