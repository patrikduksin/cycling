---
name: ride-trace
description: Turn a rider's account of a ride problem into the moment in the ride trace, a simulator replay of it with screenshots, and a regression test; or find the task or radio that drains the battery. Use when a rider describes something that went wrong during a ride, when the battery runs down faster than it should, when handed a `.trace` file, or when a replay diverges.
---

# Ride trace

The C606 and the simulator record every power-on session as a
trace: each input VANA read (buttons, ANT and BLE sensors, GPS, barometer,
battery, commands, the phone server, the buzzer and power control) and a thin
**seen** layer, what the rider saw and heard (page, toast, ride state, the
main page's readings once a second on the ride pages, each cue note,
warnings, a digest of the whole panel's pixels each time they change). The seen
layer is ground truth for finding the moment; the inputs drive the replay.
Every 5 s (every minute in dark charging standby) the trace
also holds the device's **health** (memory, slowest frame and pass, error
counters, signal) and **activity** (where the CPU time went, per task, and
what drew power); replays read past both. For a battery problem, go to
[Battery drain or a background loop](#battery-drain-or-a-background-loop). The trace holds only
what VANA read: a BLE sensor shows as VANA's link to it and the packets it took,
never as advertisements or anything else on the air, so a sensor that never
appears was never connected, not necessarily absent. Every command below prints
text: start there, and open a PNG only to confirm what the text says. Add
`--json` to any of them, `trace pull` too, for one JSON document to parse
instead. A failure the tool reports, such as bad arguments, a trace that does
not read or a tool that did not build, is `{"error": ...}` with exit status 2;
`cycling trace --help` gives each command's fields.

## 1. Get the trace

- From the simulator: fixtures live in `tools/simulator/tests/traces/`, each
  a `<name>.trace` and nothing beside it.
- From the device: the C606 writes each session to
  `traces/<sequence>-<boot>.trace` in the card's STORE volume. Connect it
  over USB and run `cycling trace pull` for the newest trace, or
  `cycling trace pull <file>` for another; both copy it to
  `.local/traces/<file>`. The pull only reads the device, and `timing`
  says how long its transfer session paused VANA and where the device
  spent that time. A device trace needs nothing beside it: it is a world
  log that holds every byte of media VANA read, each once, so a replay
  takes its media from the trace. The pull then replays the trace
  faithfully under the twin of the build that recorded it, rebuilt from
  `.local/builds/<build>/` and cached by trace and build, and adds
  `faithful: {ok, ended, exclusions}`, or `{skipped: why}` when that build
  is not archived here, its sources list no faithful mode in
  `tools/simulator/trace-capabilities` (checked before anything is built),
  its twin did not build, or the TERRAIN pack is missing
  (`no TERRAIN pack <sha256>`). A divergence, or a replay that leaves the
  recorded world at boot, is a recording bug (`ok: false`), and the pull's
  output leads with it: fix it first.
  A build reads only its own trace format and refuses any other with
  both formats named; read such a trace with the build that recorded it.
  When `CYCLING_DEVICE_HOST` is set, the device is on another machine:
  follow the remote device skill; `cycling device trace pull` brings
  `.local/` back with the kept pack, so the first pull of a pack copies it
  across. Keep device traces under `.local/` and never commit them: they
  record the rider's location.
- What a replay of a device trace reads beside the trace: the
  terrain pack's data, which the trace holds only digests of. The header
  names the pack by its SHA-256, and the replay finds the pack file in
  `.local/terrain/` of the primary checkout (`CYCLING_TERRAIN_PACKS` names
  another directory). Without that file the replay stops and names the
  pack. The replay's report lists the reads it served by digest.
- What a device trace holds: the C606 redacts secrets as it records (each
  `WIFI CONFIG` password, the phone session's token and Wi-Fi password), and
  a replay runs on placeholders. The file ends with the pass before VANA
  asked to turn off, or at its last sync (5 s at most) when power was cut.
  A session the companion started for charging records its standby while
  the card stays closed; once the standby screen is dark, a pass is
  recorded only when the startup or power status changes. Warnings
  in a device trace include driver lines that a replay never logs, and
  replays ignore them.

A replay starts from blank flash at the header's power-on number, with the
card the trace's volume table describes; every byte VANA reads comes from the
trace.

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
`page ride/lap ... cause="after button BottomLeft code 1" shows=none`. The
cause is the input or command in the pass the page changed in, the ride state
that changed with it (`with ride auto_paused`), a toast, `at power-on`, or
`by itself`: VANA's own rule, such as the climb page coming up on a climb. A
press a diagnostic harness made (`cycling screenshot --press`, harness
recipes) reads `after synthetic button BottomLeft code 1`.

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
| the rider marked it (MOVE held 1.5 s) | `find <trace> mark`: each mark names when MOVE went down and the page shown before; `cycling trace pull` lists them on stderr after its JSON |
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
  the readings the page's numbers show, never glided, with power as the 3 s
  average. A reading reaches the screen on the next frame, so confirm a
  borderline one in `screens.txt`. A page line's `shows` lists what that page
  draws (`ride/climb` power,hr,grade; `ride/map` dist; `ride/lap` none), so
  `hr=--` while the rider was on `ride/lap` is a gap in the data the rider
  never saw. The main page's lower pair is the rider's choice; the tool
  assumes the default, cad and dist.
- **A press or a rule?** A page change `by itself` is VANA's rule. A run of
  `after button ...` changes a second or two apart is the rider paging: two
  presses of BottomLeft step two pages (ride/lap, ride/climb, ride/map). The
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
  `rider-visible` (the rider would have seen something else), `broken`
  (VANA panicked or broke an invariant), `replay fault` (the build left the
  recorded world, or the trace is damaged), `modelled` (`--model` answered
  where the trace held nothing), `call-level` (VANA asked differently) or
  `redacted`. The `seen` line says what the rider saw; the `verdict` line
  says whether the mode's verdict holds, and a replay whose verdict fails
  exits 1.
- `timeline.txt`: what the recording saw in the window, at its own times; the
  PNGs and `video.mp4` carry the same captions.
- `replayed.trace`: the replay's own trace; `cycling trace timeline` reads it,
  to compare with the recording line by line.

A trace names the build that recorded it (the
summary's `build`): a hash of what built it, uncommitted changes included.
The build archives what it hashed in the primary checkout's
`.local/builds/<hash>/` on the machine that built it, and keeps it. An unstamped trace
names `unknown`. A device build hashes its whole tree. A
simulator build hashes only `apps`, `packages`, `tools/simulator`, the
vendored `trouble-host` manifest, `patches`, `.cargo` and the Cargo files, with its features and toolchain, so
an edit anywhere else keeps its hash.

Every replay runs the same twin: each call VANA makes is answered from the
trace's world records, and media from the twin's model of them. A read is
served from the bytes the trace holds, whenever VANA makes it, so a build
that caches or reorders reads replays; a write is the twin's own, so a build
that writes something else reads back what it wrote, never the device's
bytes. The replay stops where the build leaves the recorded world: a request
or take the trace holds no answer for, a recorded call the build never made
in its pass, or media the trace does not hold. The stop names the session
time and what was asked, as `left the recorded world at 12:30.250: unknown
media Bulk 4096+1`.

The mode is only the verdict. The default, `--mode auto`, picks `faithful`
when this build recorded the trace, or when the two archived builds name the
same sources and VANA features, as a device build and a simulator build of
one tree do. Otherwise it picks `changed`. The `mode` line says which and
why; `uv run --project tools/devtools python -m cycling_devtools.build_hash
relate <hash> <hash>` prints the same relation.

- `faithful` expects the build that recorded the trace. Anything it records
  otherwise than the trace did, a world record or a panel digest, is a
  recording bug, and the verdict fails. To replay an older trace faithfully,
  rebuild its tree: check out the commit in `.local/builds/<build>/commit`,
  then apply the `diff.patch` beside it, if there is one, with `git apply
  --binary`. A frame sends only the rows that changed, but the digest
  (`Panel`) is of the whole panel, so a frame the device sent whole, such
  as one a capture asked for, still matches.
- `changed` runs this checkout's build. It fails only on a stop, a panic or
  a broken invariant. The first difference in what it recorded again is
  what the change did, and "re-recorded the trace byte for byte" shows that
  a refactor changed nothing.
- `--model` goes on past each stop on a branch labelled with its kind: the
  nearest recorded answer of the same call (`Modelled`), a recorded call
  dropped with what it observed kept (`NotRequested`), or the nearest
  recorded bytes, erased where none exist (`Unrecorded`). Every later pass
  is on the branch, and a replay that branched fails.

To ask what one input changes, `cycling replay <trace> --set
T:<kind>[#n]=drop|<hex>` replays the trace as recorded and as altered, both
`changed` with `--model`. The kind is the one `trace timeline` prints, such
as `ant.request`; a method as a stop names it, such as `Ant::request`,
also works. The set alters the n-th record of that kind in the first pass at
or after T that holds it, and the output prints that pass. To ask what a
change of VANA did, `cycling replay <trace> --against <rev>` replays under
this checkout and under `<rev>`; a rev whose sources lack `changed` is
refused before it builds. Both print where the two replays part, as
`cycling trace diff <a.trace> <b.trace>` does for any two traces: the first
seen entry, the first frame (the panel's digest) and the first world record
that differ, each at its pass. When either trace replaced the phone page's
credentials, that page's frames are left out and the diff says how many.

A replay says what it proved: the media records and the settings records
that matched, and what it left out (redacted frames, the terrain pack read
by digest, and anything after records the device lost). A device trace keeps its
secrets redacted, so the phone page, which draws the session's network name,
password and code, draws other pixels. A replay counts these frames
as `redacted`, never as the first divergence, so a later difference still
shows as first. Read what the replay left out before you call a divergence a
replay fault.

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
   credentials. No tool turns a device trace into a fixture.
2. `cycling trace test-stub tools/simulator/tests/traces/<name>.trace --at T --name <what_the_rider_should_see> --expect <field>=<value>`
   prints a test that replays to T as a changed build, in two sections. Its doc
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

## Battery drain or a background loop

Use this when the battery ran down faster than it should, the device was
warm, or it drained while dark. A trace cannot measure current: it says what
kept the CPU and the radios busy, and a USB power meter on the bench says
what that costs in each device state.

1. Get the trace of that session (step 1).
2. Run `cycling trace summary <trace>` and read three blocks:
   - `activity by device state`: for each state (home idle, riding, paused,
     screen off, charging standby), executor wakeups and timer interrupts a
     second, and the share of the CPU awake in the executor, in the radio
     drivers and idle (the CPU waiting for an interrupt). Then one line per
     task: `task gps: home idle 100/s 0.4%, screen off 100/s 0.3%`.
   - `battery flags`: a task that wakes 10 times a second or more, or takes
     1% of the CPU or more, while the screen is dark; the radio drivers over
     1% while dark; a falling heap; an error counter that rose; passes over
     100 ms.
   - `health`: memory, the slowest pass, frame and card write, error
     counters, the spool's fullest point and the signal range.
3. Read the window: `cycling trace timeline <trace> --kinds activity,health,power,display,ride --from T --to T`.
   Follow one task with `cycling trace find <trace> activity --where gps`.
   `cycling trace otlp` exports the same values as metrics, one series per
   task.
4. Check each number before you blame a task:
   - A task's time includes the interrupts and radio threads that ran during
     its poll. The radio drivers' time includes the interrupts they took.
   - `counting` is what the hooks themselves cost, estimated from the
     startup log's `hook_cycles=`. Subtract it first.
   - A simulator trace counts its passes as task `console` (every 10 ms
     while something needs passes, up to 50 ms apart on a still page or a
     recording ride, every 200 ms in dark charging standby) and has no CPU times, memory or
     signal. Use it to test the tools, never to judge the battery.
   - These tasks wake on a timer: `console` 5 ms after a pass while
     something needs passes (a finger, a sheet, a cue, storage work, a host
     on the terminal), up to 50 ms after one on a still page or a recording
     ride, sooner for its next frame (a new ride number, or a gauge or radar
     marker gliding onto another pixel, at most every 40 ms), a
     button or touch, or a radar page, and every 200 ms in dark charging
     standby;
     `io`, `gps` and `power` every 10 ms in every state. `io` reads the
     touch panel only while the panel is lit. `ble` wakes once a second
     while a sensor is connected, and `wifi` every 5 s while the station is
     associated. They show up as flags while the screen is dark until they
     wait on an event instead.
5. Find the code. `main` and `console` are in `devices/c606/src/main.rs`;
   VANA picks the `console` rest after each pass in
   `apps/vana/src/app/pace.rs`.
   `power`, `gps` and `io` are in `devices/c606/src/capabilities/` (`power.rs`,
   `positioning.rs`, `io.rs`). `ble` is in `drivers/bluetooth.rs`. `wifi`,
   `wifi.station`, `wifi.ap`, `wifi.verify`, `time` and `dhcp` are in
   `drivers/wifi.rs`, and `http` is in `drivers/http.rs`. `task` is a task
   spawned without a name in `devices/c606/src/activity.rs`. Change the loop
   to wait on an event, or to wake less often while the screen is dark. Then
   record a new session in the same state and compare its line with the old
   one.

Done when you can name the task or radio that kept the CPU awake in the
drained state, with its wakeups a second and CPU share, and the code that
wakes it.

## Reference

- `cycling trace --help` lists every command, kind and option, and the
  fields of each `--json` document.
- `cycling trace` and `cycling replay` run the trace tool built for the
  checkout's commit, kept in the primary checkout's `.local/tools/trace/`
  for every worktree. The first query after a commit builds it, which takes
  a minute; later ones answer a two-hour trace in under a second. A checkout
  with uncommitted changes to the files that build VANA builds its own tree
  each time, as `cargo run` would.
- `cycling trace otlp <trace> --out <file>` writes OTLP/JSON (a traces and a
  metrics request, one per line) for an OpenTelemetry viewer; the collector's
  `otlpjsonfile` receiver reads it.
- Simulator fixtures come from scenarios in `tools/simulator/tests/trace_tools.rs`;
  refresh them after VANA changes what it reads or shows:
  `cargo test -p shell-simulator --test trace_tools -- --ignored`.
