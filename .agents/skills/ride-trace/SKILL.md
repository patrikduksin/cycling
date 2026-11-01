---
name: ride-trace
description: Turn a rider's account of a ride problem into the moment in the ride trace, a simulator replay of it with screenshots, and a regression test. Use when a rider describes something that went wrong during a ride, when handed a `.trace` file, or when a replay diverges.
---

# Ride trace

The device records every power-on session as a trace: each input VANA read
(buttons, ANT and BLE sensors, GPS, barometer, battery, commands) and a thin
**seen** layer, what the rider saw and heard (page, toast, ride state, the
main page's readings once a second, sounds, warnings). The seen layer is ground
truth for finding the moment; the inputs drive the replay. Every command below
prints text: start there, and open a PNG only to confirm what the text says.

## 1. Get the trace

- From the device: `cycling store get traces/<file>` (arguments in
  `cycling store --help`). A device trace has no `.origin`, so its replay
  starts on blank media: rides and settings saved before that power-on are
  absent.
- From the simulator: fixtures live in `tools/simulator/tests/traces/`, each
  `<name>.trace` with the media it started on in `<name>.origin`.

Done when `cycling trace summary <trace>` prints a header line, not an error.

## 2. Read the summary

`cycling trace summary <trace>` gives the session as spans and events, one line
each, in time order: rides with their recording and paused stretches, page
visits, toasts, sensor links (`ant power#1234 connected stale`), GPS fix,
companion start-up, laps, warnings, gaps and commands.

Every line opens with two clocks: `03:10.000 ride 02:47` is session time
(mm:ss.mmm since power-on) and ride time (mm:ss of recording; it stops while
paused, `--:--` outside a ride). A rider's "40 minutes in" is ride time:
`r40:00`. Time options take `40:00`, `1:02:03`, `90s`, `40m`, `2500ms`, with a
leading `r` for ride time.

Done when you have named the window the rider's account points at, as two
times, and the spans that overlap it.

## 3. Find the moment

Look for what the rider described in the seen layer, then for the input that
explains it:

| The rider said | Query |
| --- | --- |
| a reading showed dashes | `cycling trace find <trace> power=--` (also `hr`, `speed`, `cad`, `dist`, `grade`, `time`; `!=` and `~` work too) |
| a page did or did not come up | `find <trace> page --where climb` |
| a message flashed | `find <trace> toast` |
| a sensor dropped | `find <trace> stale`, `find <trace> ant` |
| it froze or stuttered | `find <trace> gap` |
| anything else | `find <trace> <text>` (any text, case ignored); `find <trace> log` for warnings |

A field query answers with stretches (`stretch until 03:32.000 (22.0s)`), the
others with entries. Then read the window:
`cycling trace timeline <trace> --from T --to T`. It hides bulky kinds (ANT and
BLE packets, ANT requests, entropy) and entries that only repeat their source;
`--kinds all --all` shows everything, `--kinds seen` or `--kinds ant,shown`
narrows it.

Done when you can write one sentence of the form "at `<session> ride <ride>`
the rider saw X, and the inputs then were Y".

## 4. Replay the window

`cycling replay <trace> --from T --to T [--every 1s] [--video]` replays the
session from power-on, records from `--from`, and prints its artifact paths.
Read them in this order:

- `screens.txt`: the screen's text at each screenshot, one line each.
- `divergence.txt`: where the replay parted from the recording (`none`, or the
  first difference and counts by kind). A `Seen` count means the rider would
  have seen something else than in the recording.
- `timeline.txt`: what the recording saw in the window, at its own times; the
  PNGs and `video.mp4` carry the same captions.
- `replayed.trace`: the replay's own trace; `cycling trace timeline` reads it,
  to compare with the recording line by line.

Pick the mode by the build:

- `--mode stimulus` (default) runs this checkout against the recorded world:
  sensors, GPS and buttons arrive when they did, and the ANT radio is a model.
  Use it for a changed build, for a fix and for every regression test.
- `--mode strict` expects the build that recorded the trace (the summary's
  `build`; check it out first). Every call VANA makes is answered from the
  recording and any difference is a divergence. Use it to reproduce the
  recording frame for frame, or to tell a replay fault from a VANA change.

Done when the screenshot at the moment shows what the rider described, or
`divergence.txt` explains why it does not.

## 5. Write the regression test

1. Copy the trace (and its `.origin`) to
   `tools/simulator/tests/traces/<name>.trace`. Keep fixtures small: a trace
   costs about 30 KB a minute.
2. `cycling trace test-stub tools/simulator/tests/traces/<name>.trace --at T --name <what_the_rider_should_see>`
   prints a test that replays to T in stimulus mode and asserts what the
   recording showed: page, toast, ride state and readings, plus the screen
   where a reading showed dashes.
3. Paste it into `tools/simulator/tests/trace_tools.rs`, or a test file beside
   it. Change the asserts that describe the bug to what the rider should have
   seen. Run it: it goes red on the bug.
4. Fix VANA; the test goes green. The stub's untouched asserts keep the rest of
   the moment as it was.

Done when the test is red before the fix and green after it.

## Reference

- `cycling trace --help` lists every command, kind and option.
- `cycling trace otlp <trace> --out <file>` writes OTLP/JSON (a traces and a
  metrics request, one per line) for an OpenTelemetry viewer; the collector's
  `otlpjsonfile` receiver reads it.
- Simulator fixtures come from scenarios in `tools/simulator/tests/trace_tools.rs`;
  refresh them after VANA changes what it reads or shows:
  `cargo test -p shell-simulator --features cycling,debug-harness --test trace_tools -- --ignored`.
