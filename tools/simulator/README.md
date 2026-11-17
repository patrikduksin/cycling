# Shell simulator

The `shell-simulator` package runs the shared shell and the VANA application
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

Scenarios live in `tests/ride_scenarios.rs`. `cycling sim` runs them with
`SIM_ARTIFACTS` set: `recording.rs` writes checkpoint
screenshots (`sim.checkpoint(..)`), a chart of the world, recorded rides as GPX
and JSON, a timeline and `result.json` for each scenario under `.local/sim/latest`,
including failed ones. `SIM_VIDEO=1` adds a captioned video; encoding it is most
of a run's time, so only `cycling sim-pr` and `cycling sim-live` set it. Every
scenario also stores the frames its video would show in `frames.bin`
(`recording/tape.rs`), so
`shell-simulator --video <scenario dir>...` renders that video later without
running the scenario again. `artifacts/` holds the PNG, GIF, MP4 (through `ffmpeg`)
and chart encoders. `cycling sim-pr <number>` posts the run inline on a pull
request through `cycling_devtools.sim.report`: the comment posts at once and its
media upload in the background, filling it in place; media that fail to upload are
marked, with gh's errors in `.local/sim/publish/<job>.log`. `cycling sim-compare [--base <ref>]
[--filter <test name>] [--pr <number>] [--video]` also runs the merge-base with
the base ref (default `origin/main`, fetched first) in a temporary worktree and
reports only screens whose pixels changed, Before | After
(`cycling_devtools.sim.compare`). Both sides run once, without video; scenarios
new in the PR get a video from their stored frames, and with `--video` the
scenarios whose screens changed too, on both sides. A complete base run is
cached by commit, rustc and filter in the primary checkout's
`.local/sim/screens-cache`, so a later comparison against the same base, from
any worktree, runs only the current checkout. `cycling sim-compare --fill <commit>`
caches a commit's run ahead of time, for a merge to call on each new main;
concurrent misses on one base build it once.

Comparison mode (`src/compare.rs`) proves that a refactor changed nothing the
screens cannot show. It is a switch at run time: a `Sim` made while
`SIM_COMPARE=1` is set compares, and so does one made with `Sim::comparing`.
A `Sim` that does not compare has no tap, and its logger keeps warnings and
errors only, at most 64 a pass. Each comparing scenario writes
`hashes.json` with three SHA-256 hashes, and its run directory gets a
`comparison-mode` marker:

- `trace`: every event the recorder takes, before the spool can refuse its
  block, the BLE requests the trace passes through unrecorded, and every log
  line as it is logged, VANA's `info` lines included, with its pass time. A
  line belongs to the device whose code ran last on the thread, so one a test
  logs between passes counts after that device's last pass. A line logged
  before any comparing device exists on the thread is not hashed. Each
  session is folded in with its ordinal when it ends, so a difference before
  a reboot counts.
- `storage`: every operation on internal flash and the card through the
  device's handles, with medium, offset, length and result. A scenario's setup
  through those handles counts too, such as `sim.device.media.program`. Host
  handles (`host_bulk`, `put_file`, `file`), `untapped` handles and copies of
  the media do not, and neither do the trace spool's own card writes, which go through host
  handles.
- `media`: what the media hold at the end. Internal regions and card sectors
  count as bytes, FILES and STORE by file name and bytes, and the trace spool
  is left out. A FILES or STORE volume with a file that does not read whole,
  a lost record included, counts as its raw sectors instead, all but the
  spool's chunks.

Every value is hashed as explicit bytes in the trace's own codes, never as
Rust `Debug` text, so a rename changes no hash. A passing scenario whose
hashes cannot be taken or written fails. `cycling sim-compare` runs both sides
with `SIM_COMPARE=1` and `CYCLING_BUILD_COMMIT` set to forty zeros, so the
header's commit and the `INFO` reply match. A side has comparison mode when its run wrote the marker. The report
lists every scenario whose hashes differ from the base, and every scenario
that wrote no hashes on a side that has comparison mode. It reports and never
fails: a PR that changes behavior on purpose names the hashes it changes.

`cycling sim-live` opens a window (`src/bin/sim-live.rs`, behind the `live`
feature) onto `live.rs`: the device powers on beside a rider at 25 km/h on a
loop route with GPS and ANT heart-rate, power, speed/cadence and radar sensors
in range. Time runs in 10 ms steps at real time or faster, with a firmware
pass each step while the device is busy and every 50 ms on a still page:

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

`cycling fuzz` (`src/fuzz/`, `src/bin/fuzz.rs`) runs random sessions in the
twin. A seed draws a plan blind to the device (`plan.rs`): a course, synthetic
or the GPS track of a world log under the primary checkout's
`.local/ride-trace`, the sensors and the card, then steps of button presses,
rides, sensor dropouts, GPS and barometer loss, radar vehicles, battery
changes, host ride commands, power cuts, torn flash writes and card refusals.
The executor plays each step with raw inputs, and the checks in `check.rs`
judge the device after it: rides decode, committed samples survive, a riding
ride keeps recording, the boot page clears, and no panic. At the end the
settled device must boot and record and save a ride. A finding prints the seed
and the command that reproduces it, and writes the plan and the world log under
`.local/fuzz/<seed>`. `--seed S` prints the session step by step, `--minutes N`
runs fresh seeds on several threads and prints coverage of pages by button and
the skipped guards, and `--fixed` runs the seed set that `cycling verify --deep`
runs. `cycling fuzz-planted` proves the fuzzer: behind the `planted-bug`
feature, VANA's recovery forgets a ride that lost power while paused, and the
test finds that bug from a seed and finds it again from the same seed.
`broken()` (#397 0b) joins the checks through the hook in `check.rs`.

`cycling simulate` runs the shell alone on two display geometries and `cycling harness-virtual`
the shared terminal scenarios. Terminal mode (`--terminal`) requires an explicit
disposable `--state-dir` and prints a `ready` line once booted; the harness builds
the simulator first and starts its reply timeouts only after that line.

Simulation checks presentation, input routing, sensor and ride behaviour, command
compatibility and recovery from storage faults and power loss. Its companion
mirrors the C606 adapter's admission rules but not its UART framing. It does not
model DMA timing, radio coexistence, electrical behavior or power sequencing.
Device changes still need the relevant hardware validation.
