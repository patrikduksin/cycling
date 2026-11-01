---
name: ride-trace
description: Turn a rider's account of a ride problem into the moment in the ride trace, a simulator replay of it with screenshots, and a regression test. Use when a rider describes something that went wrong during a ride, when handed a `.trace` file, or when a replay diverges.
---

# Ride trace

The C606 (cycling builds) and the simulator record every power-on session as a
trace: each input VANA read (buttons, ANT and BLE sensors, GPS, barometer,
battery, commands, the phone server, the buzzer and power control) and a thin
**seen** layer, what the rider saw and heard (page, toast, ride state, the
main page's readings once a second on the ride pages, each cue note,
warnings). The seen layer is ground truth for finding the moment; the inputs
drive the replay. The trace holds only
what VANA read: a BLE sensor shows as VANA's link to it and the packets it took,
never as advertisements or anything else on the air, so a sensor that never
appears was never connected, not necessarily absent. Every command below prints
text: start there, and open a PNG only to confirm what the text says.

## 1. Get the trace

- From the simulator: fixtures live in `tools/simulator/tests/traces/`, each
  `<name>.trace` with the media it started on in `<name>.origin`.
- From the device: the C606 writes each session to
  `traces/<sequence>-<boot>.trace` in the card's STORE volume. Connect it
  over USB and run `cycling trace pull` for the newest trace, or
  `cycling trace pull <file>` for another; both copy it to
  `.local/traces/<file>`. By hand: `cycling store list traces/` lists them,
  `cycling store get traces/<file> .local/traces/<file>` copies one. When
  `CYCLING_DEVICE_HOST` is set, the device is on another machine: follow the
  remote device skill. Keep device traces under `.local/` and never commit
  one: it records the rider's location.
- What a device trace holds: the C606 redacts secrets as it records (each
  `WIFI CONFIG` password, the phone session's token and Wi-Fi password), and
  a replay runs on placeholders. Its header names the settings and media at
  power-on, but it has no `.origin`, so its replay starts on blank media:
  rides and settings saved before that power-on are absent, and a strict
  replay reports what differs. The file ends with the pass before VANA
  asked to turn off, or at its last sync (5 s at most) when power was cut.
  A session the companion started for charging records its standby while
  the card stays closed; once the standby screen is dark, a pass is
  recorded only when the startup or power status changes. Warnings
  in a device trace include driver lines that a replay never logs, and
  replays ignore them.

`cycling trace origin <trace>` says in words what a replay starts from: the
power-on number, settings, VANA's saved sensors and setup step, rides on
internal flash, and the bulk card.

Done when `cycling trace summary <trace>` prints a header line, not an error.

## 2. Read the summary

`cycling trace summary <trace>` opens with the build, then the sensor
silences: each sensor's last packet before a pause of over 3 s and its first
after (`power#1234 silent 03:06.010-03:31.260 (25.2s)`). Then the session as
spans and events, one line each, in time order: rides with their recording
and paused stretches, page visits, toasts, sensor links
(`ant power#1234 connected stale`) and silences, GPS fix, companion start-up,
laps, warnings, storage, gaps and commands. A ride counts the warnings logged
in it (`warnings=1`), and a warning names what changed in the same pass
(`(same pass: ride ended saved, page summary began)`). VANA's warnings about
saving or reading media are `storage` events. A page span says what brought
the page up and which of the shown readings it draws:
`page ride/back ... cause="after button BottomLeft code 1" shows=none`. The
cause is the input or command in the pass the page changed in, the ride state
that changed with it (`with ride auto_paused`), a toast, `at power-on`, or
`by itself`: VANA's own rule, such as the climb page coming up on a climb.

Every line opens with two clocks: `03:10.000 ride 02:47` is session time
(mm:ss.mmm since power-on) and ride time (mm:ss of recording; it stops while
paused, `--:--` outside a ride). Ride time follows the ride time VANA
showed, so it carries on when a card failure moves the ride to internal
flash. A rider's "40 minutes in" is ride time:
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
| the screen changed, a page vanished | `find <trace> page`: each page with its cause; then `find <trace> input` |
| a beep or an alarm | `find <trace> sound`: each note names the cue VANA played it for (`pattern 22: radar approach`) |
| a sensor dropped | the summary's silences line; `find <trace> stale`, `find <trace> ant` |
| something about saving | `find <trace> storage` |
| it froze or stuttered | `find <trace> gap` |
| anything else | `find <trace> <text>` (any text, case ignored); `find <trace> log,storage` for warnings |

A field query answers with stretches (`stretch until 03:32.000 (22.0s)`), the
others with entries. Then read the window:
`cycling trace timeline <trace> --from T --to T`. It hides bulky kinds (ANT and
BLE packets, ANT requests, entropy) and entries that only repeat their source;
`--kinds all --all` shows everything, `--kinds seen` or `--kinds ant,shown`
narrows it. `phone`, `power`, `display` and `buzzer` show the phone access
point and server, power control, the backlight and the buzzer; a
`WIFI CONFIG` command shows without its password and a phone request without
its session token (`t=(not shown)`). ANT+ packets read as words (`power#1234 power 200 W, cadence
86 rpm (event 60)`, heart rate, cadence and wheel rates, radar targets);
`--hex` adds the bytes.

Two checks before you trust a reading or a page change:

- **Could the rider see it?** VANA emits a `shown` line once a second on
  every ride page (main, climb, map, back, stats, lap and paused), and it
  always holds the main page's readings, whatever page was up. Its values are
  the ones the page glides to, before the glide, with power as the 3 s
  average: the number on screen can trail a change for a moment, so confirm
  a borderline reading in `screens.txt`. A page line's `shows` lists what that page
  draws (`ride/climb` power,hr,grade; `ride/map` dist; `ride/back` none), so
  `hr=--` while the rider was on `ride/back` is a gap in the data the rider
  never saw. The main page's lower pair is the rider's choice; the tool
  assumes the default, cad and dist.
- **A press or a rule?** A page change `by itself` is VANA's rule. A run of
  `after button ...` changes a second or two apart is the rider paging: two
  presses of BottomLeft step two pages (ride/climb, ride/map, ride/back). The
  climb page comes up and goes back by itself only until the rider pages
  during the climb; then the page stays where they put it. "The page
  vanished" is often that.

Each note names the cue VANA played it for and its place in that cue
(`pattern 0: still paused (2 of 3)`). A cue with fewer notes heard than its
count was cut short by a later cue or the end of the trace. A pattern with no
cue was not VANA's, such as a console `SOUND PLAY`.

Done when you can write one sentence of the form "at `<session> ride <ride>`
the rider saw X, and the inputs then were Y". When Y explains X (the meter
fell silent, so power showed dashes), VANA did what it should: say so, and
write a test only for what VANA got wrong.

## 4. Replay the window

`cycling replay <trace> --from T --to T [--every 1s] [--video]` replays the
session from power-on, records from `--from`, and prints the absolute path of
each artifact. They land in `.local/replay/<trace name>-<from>/` in the
checkout you ran it from (`--out DIR` puts them elsewhere); a rerun with the
same trace and `--from` replaces them. A directory is replaced only when a
replay or scenario wrote it (it holds `.simulator-output`) or it is empty;
any other `--out` is refused, so a mistyped path never loses files. Read them in this order:

- `screens.txt`: the screen's text at each screenshot, one line each.
- `divergence.txt`: where the replay parted from the recording inside the
  window, with one total for before it. Each count has a meaning:
  `rider-visible` (the rider would have seen something else), `model-only`
  (the stimulus ANT model or fresh entropy answered; the rider sees nothing),
  `timing-only`, `call-level` (strict: VANA asked differently) or
  `replay fault`. The `verdict` line sums it up; `rider-visible: none` with
  model-only counts is a clean window, not a replay fault.
- `timeline.txt`: what the recording saw in the window, at its own times; the
  PNGs and `video.mp4` carry the same captions.
- `replayed.trace`: the replay's own trace; `cycling trace timeline` reads it,
  to compare with the recording line by line.

The mode follows the build. A trace names the commit that recorded it (the
summary's `build`). It is `unknown` for a build with uncommitted changes,
which no commit holds, and for traces from before builds were stamped. A
build with uncommitted changes stamps itself `<commit>-dirty`. The default,
`--mode auto`, replays strict when this checkout built that commit with no
uncommitted change under `apps`, `packages`, `tools/simulator`, `vendor`,
`patches`, `.cargo` or the Cargo files, and stimulus otherwise; the `mode`
line says which and why.

- `stimulus` runs this checkout against the recorded world: sensors, GPS and
  buttons arrive when they did, and the ANT radio is a model. It is what a
  changed build, a fix and every regression test use.
- `strict` answers every call VANA makes from the recording and counts any
  difference. It reproduces the recording frame for frame and tells a replay
  fault from a VANA change. To get it for an older trace, check out its
  `build` first.

Done when the screenshot at the moment shows what the rider described, or
`divergence.txt` explains why it does not.

## 5. Write the regression test

1. Make a fixture from a simulator scenario, never from a device trace.
   Write a scenario in `tools/simulator/tests/trace_tools.rs` that gives VANA
   the inputs the timeline showed around the moment (the sensors, the route,
   the silence), add it to `record_trace_fixtures`, and record it with the
   command under Reference. Its summary must show the moment the rider
   described. Keep fixtures short: a trace costs about 40 KB a minute.
   `no_committed_fixture_holds_a_wifi_password_or_a_phone_token` fails on a
   fixture that holds a Wi-Fi password, a phone session token or saved Wi-Fi
   credentials. A sanitizer that turns a real trace into a fixture does not
   exist yet; it is future work.
2. `cycling trace test-stub tools/simulator/tests/traces/<name>.trace --at T --name <what_the_rider_should_see> --expect <field>=<value>`
   prints a test that replays to T in stimulus mode, in two sections. Its doc
   comment says what the recording showed and what the sensors were doing
   (each link, each sensor's last packet and silence). For a trace outside
   the fixtures directory it first prints whether the trace may become a
   fixture (never a device trace or one holding a secret) and the commands
   that copy it there and run the secrets test; the stub already reads the
   path the copy will have. The characterization
   asserts the rest of the moment as recorded. The expectation asserts what
   the rider should have seen: `--expect power!=--`, `power=230`,
   `page=ride/climb` or `toast~SAVED`, several joined with commas, for a
   reading (`time`, `speed`, `power`, `hr`, `cad`, `dist`, `grade`) or `page`,
   `toast` or `ride`. It refuses an expectation the recording already meets.
   Without `--expect`, the expectation is a `todo!()` to replace.
3. Paste it into `tools/simulator/tests/trace_tools.rs`, or a new test file
   beside it: it brings its own imports and `cycling` gate. Run it: it goes red on the expectation, with the recorded value in the
   message. A red in the characterization means the replay itself differs;
   go back to step 4.
4. Fix VANA; the test goes green. The characterization keeps the rest of the
   moment as it was.

Done when the test is red before the fix and green after it.

## Reference

- `cycling trace --help` lists every command, kind and option.
- `cycling trace otlp <trace> --out <file>` writes OTLP/JSON (a traces and a
  metrics request, one per line) for an OpenTelemetry viewer; the collector's
  `otlpjsonfile` receiver reads it.
- Simulator fixtures come from scenarios in `tools/simulator/tests/trace_tools.rs`;
  refresh them after VANA changes what it reads or shows:
  `cargo test -p shell-simulator --features cycling,debug-harness --test trace_tools -- --ignored`.
