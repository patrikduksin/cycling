//! `cycling trace` and `cycling replay`: read a ride trace as text, find
//! moments in it, export it to OpenTelemetry, replay a window of it with
//! screenshots, and write a regression test from it.
use shell_simulator::bundle::{Bundle, Bundled, Expect, Run};
use shell_simulator::recording::Recording;
use shell_simulator::replay::{Divergence, Mode, Options as Replaying};
use shell_simulator::session::LeftAtBoot;
use shell_simulator::sim::Sim;
use shell_simulator::trace::find::{Query, find};
use shell_simulator::trace::report::{
    Answers, Artifacts, Diffed, Futures, Header, Listing, Parted, Proved, Replayed, Resolved, Stub,
    Summary, Written, in_window, render,
};
use shell_simulator::trace::story::tell;
use shell_simulator::trace::surgery::{self, Set};
use shell_simulator::trace::timeline::{
    At, Entry, Kind, Timeline, build_name, read, session_clock,
};
use shell_simulator::trace::{card, diff, divergence, otlp, stub};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: cycling trace <command> <trace> [options] [--json]
       cycling replay <trace> [options] [--json]

Read a ride trace (<name>.trace). It holds everything a replay needs but the
TERRAIN pack, which the replay finds by the SHA-256 its header names. Times
are the session clock, mm:ss.mmm since power-on, and the ride clock, which
runs only while a ride records; every line starts
`<session> ride <ride>`. Time options take 40:00, 1:02:03, 90s, 40m or 2500ms,
and r40:00 for a ride time.

--json, on any command, prints one JSON document instead of the text, and
{error} with exit status 2 when the command fails: with left, why, for a
replay that left the recorded world at boot, and no_pack, the pack's
sha256, for a replay this machine lacks the TERRAIN pack of. A moment is
{session_ms, ride_ms}, ride_ms null outside a ride. An entry is a moment's
fields with {kind, text, repeat}, a page's cause and shows, and a packet's
bytes in hex, whatever --hex says. A header's terrain is the TERRAIN pack
the session found, {sha256, length, ready}, or null. summary gives {header,
silences, spans, events, battery, entries (per kind {new, repeats})};
timeline {from_ms, to_ms, entries, hidden}; find {found, total}, each found
an entry with type entry or {type stretch, from, to, first}, at most --limit
of them; replay {header, mode, why, from_ms, to_ms, model, reached,
trace_ended, divergence (with ok and verdict), faithful {ok, physical,
semantic, ended, exclusions}, artifacts, parted, bundle}; replay --set
{sets, recorded, set, diff}; diff {a, b, seen, panel, world, exclusions};
otlp {traces,
metrics}, or {out} with --out; test-stub {fixture_steps, test}. The
summary's battery is prose, the text's own lines, with no fields to parse.

commands:
  summary <trace>
      Sensor silences first (each sensor's last packet before a pause and its
      first after), then spans (session, rides, recording and paused
      stretches, pages, toasts, sensor links and silences, GPS fix) and
      events (boot, laps, warnings, storage, gaps, commands), one line each.
      A warning names what began or which ride ended in the same pass, and
      a ride counts the warnings logged in it. A page names its cause (the
      input, command, ride state or toast in its pass, else by itself) and
      the shown readings it draws (shows=). Then, from the device's activity
      and health records, where the CPU time went per device state (home
      idle, riding, paused, screen off, charging standby): executor wakeups,
      timer ticks, CPU awake, radio drivers and idle, and each task's
      wakeups a second and CPU share; battery flags (a task that keeps
      waking or burning CPU while the screen is dark, a falling heap, an
      error counter that rose, stalled passes); and the health trends.
      Run this first.
  timeline <trace> [--from T] [--to T] [--kinds LIST] [--all] [--hex]
      Every event in the window, inputs and what the rider saw. Kinds: gap,
      input, command, startup, ant, ant.packet, ant.request, ble, ble.packet,
      gps, sensors, battery, time, storage (media faults and VANA's warnings
      about saving or reading), entropy, page, toast, sound, ride, shown,
      log, phone, power, display, buzzer, trace, health, activity, mark;
      groups seen, inputs, default, all (health and activity, every few seconds,
      only by name or all). --all also lists entries
      that only repeat their source (a counter or reading moved); a press,
      a command or a note is never one. A sound names its cue. ANT+ pages
      read as words (power 230 W, hr 142 bpm, cadence 86 rpm, radar targets);
      --hex adds each packet's bytes.
  find <trace> <query> [--where TEXT] [--from T] [--to T] [--limit N|all] [--hex]
      field=value, field!=value or field~text over the shown readings (time,
      speed, power, hr, cad, dist, grade) gives the stretches it held, e.g.
      power=--; a kind or kinds (toast, page,ride) gives those entries; any
      other text gives entries containing it, ignoring case. --where keeps
      entries containing TEXT.
  otlp <trace> [--out FILE]
      OTLP/JSON: a traces request and a metrics request, one per line. The
      metrics hold the shown readings, the battery, the health records and
      each activity window, per task.
  replay <trace> [--from T] [--to T] [--mode auto|faithful|changed] [--model]
         [--every T] [--video] [--out DIR] [--no-bundle]
      Replay the session up to --to (default --from plus 60s), screenshots every
      --every (default 5s) from --from, the screen's text, what the glue
      decided and from what (why.txt: toasts, cues, pages), the replay's own
      trace and where it parted from the recording, into --out (default
      .local/replay/<trace>-<from>/ under the checkout; paths print absolute).
      A faithful replay without --model, by the recording's build or one of
      its sources, that fails also writes failure.bundle there: the trace,
      that build's sources and the first divergence, one file `cycling bundle
      run` reproduces on another machine; the output names it, or why it is
      missing (bundle in JSON: {written} or {failed}). --no-bundle skips it.
      An existing --out is replaced only when a replay or scenario wrote it
      (it holds .simulator-output) or it is empty; anything else is refused.
      Every replay answers VANA from the trace's world records and stops
      where this build leaves them: a call the trace holds no answer for,
      a recorded call it never made, or media the trace does not hold.
      The mode is the verdict. faithful expects the recording build: any
      difference in what it records again is a recording bug. changed
      fails only on a stop, a panic or a broken invariant, and reports the
      first difference as what the change did. auto (default) picks
      faithful when this build recorded the trace, or a build of the same
      VANA sources and features did (by the listings .local/builds archived
      for both hashes), else changed, and says why. --model goes on past
      each stop on a branch labelled with its kind: the nearest recorded
      answer (Modelled), a dropped recorded call (NotRequested), or the
      nearest recorded bytes (Unrecorded); a replay that branched fails.
      Divergences count inside the window, with a total for before it, each
      labelled rider-visible, broken, replay fault, modelled, call-level or
      redacted. A replay whose verdict fails exits 1.
  replay <trace> --set T:<kind>[#n]=drop|<hex> [--set ...]
      Replay two futures, changed with --model: the trace as recorded, and
      as each --set alters it. A set finds the first pass at or after T
      holding the n-th record (#0 when left out) of the kind, named as
      trace timeline names it (ant.request), or of one method, named as a
      replay's stops name it (Ant::request), and drops that record or gives
      it the hex body, arguments first. It prints the pass each set
      resolved to, both verdicts, and the first difference between the two
      futures: the first seen entry (page, toast, sound, ride state, shown
      reading), the first frame, then the first world record. Each
      future's artifacts are in --out/recorded and --out/set; --to
      defaults to the trace's end.
  replay <trace> --against <rev>
      Replay under this build and under <rev>, changed with --model, and
      print where the two replays part. cycling replay builds <rev>'s trace
      tool (tools/devtools/src/cycling_devtools/twin.py) and runs both.
  diff <a.trace> <b.trace>
      The first seen entry, the first frame (the panel's digest) and the
      first world record where two traces of one session part, each at its
      pass. JSON {a, b, seen, panel, world, exclusions}, each difference
      {at_ms, a, b} or null. When either trace replaced the credentials the
      phone page draws, that page's frames are left out and exclusions says
      how many.
  test-stub <trace> --at T [--name NAME] [--expect FIELD=VALUE,...]
      A regression test that replays the trace to T. Its doc comment says
      what the recording showed and what the sensors were doing then; its
      characterization asserts the rest of the moment as recorded; its
      expectation states what the rider should have seen. --expect writes
      the expectation (power!=--, power=230, page=ride/climb, toast~SAVE)
      and refuses one the recording already meets; without it the
      expectation is a todo!() to replace. Either way the test is red on the
      recording. For a trace outside tools/simulator/tests/traces/ it first
      says whether the trace may become a fixture (a device trace or one
      holding a secret may not) and the commands that copy it there.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--build-hash"] {
        println!("{}", env!("CYCLING_BUILD_HASH"));
        return ExitCode::SUCCESS;
    }
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(Ran { printed, held }) => {
            print!("{printed}");
            match held {
                true => ExitCode::SUCCESS,
                false => ExitCode::FAILURE,
            }
        }
        Err(Failed { error, cause }) => {
            if args.iter().any(|a| a == "--json") {
                let mut failed = serde_json::json!({ "error": error });
                if let Some((name, value)) = cause {
                    failed[name] = value.into();
                }
                println!("{failed}");
            }
            eprintln!("cycling trace: {error}");
            ExitCode::from(2)
        }
    }
}

/// A command that failed, and what a caller can tell from it without
/// reading the text: `left`, why a replay left the recorded world at boot,
/// or `no_pack`, the SHA-256 of a TERRAIN pack this machine does not hold.
struct Failed {
    error: String,
    cause: Option<(&'static str, String)>,
}

impl From<String> for Failed {
    fn from(error: String) -> Self {
        Self { error, cause: None }
    }
}

impl From<&str> for Failed {
    fn from(error: &str) -> Self {
        error.to_string().into()
    }
}

impl Failed {
    fn opening(error: std::io::Error) -> Self {
        let inner = error.get_ref();
        let cause = match (
            inner.and_then(|inner| inner.downcast_ref::<LeftAtBoot>()),
            inner.and_then(|inner| inner.downcast_ref::<card::NoPack>()),
        ) {
            (Some(left), _) => Some(("left", format!("at boot, {}", left.0))),
            (_, Some(pack)) => Some(("no_pack", pack.sha256.clone())),
            _ => None,
        };
        Self {
            error: error.to_string(),
            cause,
        }
    }
}

struct Options {
    positional: Vec<String>,
    flags: BTreeMap<String, Vec<Flag>>,
}

enum Flag {
    Switch,
    Value(String),
}

impl Flag {
    fn value(&self) -> Option<&str> {
        match self {
            Flag::Switch => None,
            Flag::Value(value) => Some(value),
        }
    }
}

struct Ran {
    printed: String,
    held: bool,
}

impl Options {
    fn parse(args: &[String], switches: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut flags = BTreeMap::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.strip_prefix("--") {
                Some(name) if switches.contains(&name) => {
                    flags
                        .entry(name.to_string())
                        .or_insert_with(Vec::new)
                        .push(Flag::Switch);
                }
                Some(name) => {
                    let value = args.next().ok_or(format!("--{name} needs a value"))?;
                    let values = flags.entry(name.to_string()).or_insert_with(Vec::new);
                    values.push(Flag::Value(value.clone()));
                }
                None => positional.push(arg.clone()),
            }
        }
        Ok(Self { positional, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name)?.last()?.value()
    }

    fn values(&self, name: &str) -> impl Iterator<Item = &str> {
        (self.flags.get(name).into_iter().flatten()).filter_map(Flag::value)
    }

    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn time(&self, timeline: &Timeline, name: &str) -> Result<Option<u64>, String> {
        self.value(name)
            .map(|text| timeline.session_ms(At::parse(text)?))
            .transpose()
    }

    fn window(&self, timeline: &Timeline) -> Result<(u64, u64), String> {
        let from = self.time(timeline, "from")?.unwrap_or(0);
        let to = self.time(timeline, "to")?.unwrap_or(u64::MAX);
        Ok((from, to))
    }
}

fn run(args: &[String]) -> Result<Ran, Failed> {
    let (command, rest) = args.split_first().expect("arguments checked");
    let options = Options::parse(rest, &["all", "video", "hex", "json", "model", "no-bundle"])?;
    let json = options.has("json");
    let path = PathBuf::from(
        options
            .positional
            .first()
            .ok_or(format!("{command} needs a trace file"))?,
    );
    if command == "diff" {
        let [a, b] = &options.positional[..] else {
            return Err("diff needs two trace files".into());
        };
        let read = |path: &str| std::fs::read(path).map_err(|error| format!("{path}: {error}"));
        let diffed = diff::diff(&read(a)?, &read(b)?)?;
        let printed = render(json, &Diffed { a, b, diff: diffed })?;
        return Ok(Ran {
            printed,
            held: true,
        });
    }
    let trace = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoded = None;
    let mut held = true;
    let printed = match command.as_str() {
        "summary" => {
            let timeline = decode(&mut decoded, &trace)?;
            render(json, &Summary::of(&path, timeline))
        }
        "timeline" => {
            let timeline = decode(&mut decoded, &trace)?;
            let window = options.window(timeline)?;
            let kinds = Kind::parse_list(options.value("kinds").unwrap_or("default"))?;
            let listing = Listing::of(
                timeline,
                window,
                &kinds,
                options.has("all"),
                options.has("hex"),
            );
            render(json, &listing)
        }
        "find" => {
            let timeline = decode(&mut decoded, &trace)?;
            let query = options
                .positional
                .get(1)
                .ok_or("find needs a query: field=value, a kind or text")?;
            let window = options.window(timeline)?;
            let limit = match options.value("limit") {
                None => 50,
                Some("all") => usize::MAX,
                Some(limit) => limit.parse().map_err(|_| "--limit takes a number or all")?,
            };
            let found = find(
                timeline,
                &Query::parse(query),
                window,
                options.value("where"),
            );
            render(json, &Answers::of(found, limit, options.has("hex")))
        }
        "otlp" => {
            let timeline = decode(&mut decoded, &trace)?;
            let requests = otlp::export(timeline, &tell(timeline));
            match options.value("out") {
                Some(out) => {
                    std::fs::write(out, requests.to_string()).map_err(|e| format!("{out}: {e}"))?;
                    let out = std::fs::canonicalize(out).map_err(|e| format!("{out}: {e}"))?;
                    render(json, &Written { out })
                }
                None => render(json, &requests),
            }
        }
        "replay" if options.has("against") => Err(
            "--against builds the trace tool for another revision, which cycling replay does: \
             run `cycling replay <trace> --against <rev>`"
                .into(),
        ),
        "replay" if options.has("set") => {
            let timeline = decode(&mut decoded, &trace)?;
            render(json, &futures(&path, &trace, timeline, &options)?)
        }
        "replay" => {
            let timeline = decode(&mut decoded, &trace)?;
            let asked = asked_mode(&options, timeline)?;
            let from = options.time(timeline, "from")?.unwrap_or(0);
            let dir = out_dir(&path, &options, from);
            let (replayed, _) = replay(&path, &trace, timeline, &options, asked, &dir, 60_000)?;
            held = (replayed.divergence.as_ref()).is_some_and(|divergence| divergence.ok);
            render(json, &replayed)
        }
        "test-stub" => {
            let timeline = decode(&mut decoded, &trace)?;
            let at = options
                .time(timeline, "at")?
                .ok_or("test-stub needs --at")?;
            let fixture = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("the trace needs a file name")?;
            let name = options
                .value("name")
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}_at_{}", fixture.replace('-', "_"), at / 1000));
            let expect: Vec<stub::Expect> = options
                .value("expect")
                .map(|list| list.split(',').map(stub::Expect::parse).collect())
                .transpose()?
                .unwrap_or_default();
            let test = stub::stub(timeline, fixture, &name, at, &expect)?;
            let fixture_steps = stub::to_fixture(&path, &trace);
            render(
                json,
                &Stub {
                    fixture_steps,
                    test,
                },
            )
        }
        other => Err(format!("unknown command {other:?}; see --help")),
    }?;
    // A timeline is millions of small allocations, and the process ends once
    // this is printed: freeing them would only cost time.
    std::mem::forget(decoded);
    Ok(Ran { printed, held })
}

/// `trace` decoded into `slot`, which `run` forgets rather than frees.
fn decode<'a>(slot: &'a mut Option<Timeline>, trace: &[u8]) -> Result<&'a Timeline, String> {
    Ok(slot.insert(read(trace)?))
}

#[derive(Debug, PartialEq)]
enum BuildRelation {
    Same { build: String },
    SameSources { recorded: String, current: String },
    Other { recorded: String, current: String },
    Unknown { why: String },
}

fn short(build: &str) -> String {
    build.chars().take(12).collect()
}

impl BuildRelation {
    fn parse(relate_output: &str, recorded: &str, current: &str) -> Result<Self, String> {
        let (recorded, current) = (short(recorded), short(current));
        match relate_output.trim() {
            "same" => Ok(Self::Same { build: current }),
            "same-sources" => Ok(Self::SameSources { recorded, current }),
            "other" => Ok(Self::Other { recorded, current }),
            other => match other.strip_prefix("unknown ") {
                Some(why) => Ok(Self::Unknown {
                    why: format!(
                        "the trace's build {recorded} and this build {current} cannot be compared: {why}"
                    ),
                }),
                None => Err(format!("build_hash relate printed {other:?}")),
            },
        }
    }
}

fn relation(recorded: &str, current: &str) -> Result<BuildRelation, String> {
    if recorded.is_empty() {
        return Ok(BuildRelation::Unknown {
            why: "the trace names no build".into(),
        });
    }
    let cannot = |why: &str| BuildRelation::Unknown {
        why: format!(
            "the trace's build {} and this build {} cannot be compared: {why}",
            short(recorded),
            short(current)
        ),
    };
    if current.is_empty() {
        return Ok(cannot("this build names none"));
    }
    if recorded == current {
        return Ok(BuildRelation::Same {
            build: short(current),
        });
    }
    match shell_simulator::devtools("build_hash", ["relate", recorded, current]) {
        Ok(out) if out.status.success() => {
            BuildRelation::parse(&String::from_utf8_lossy(&out.stdout), recorded, current)
        }
        Ok(out) => Ok(cannot(&format!(
            "build_hash relate failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
        Err(why) => Ok(cannot(&why)),
    }
}

fn pick_mode(asked: &str, relation: BuildRelation) -> Result<(Mode, String), String> {
    let (matched, builds) = match relation {
        BuildRelation::Same { build } => (true, format!("this build {build} recorded the trace")),
        BuildRelation::SameSources { recorded, current } => (
            true,
            format!(
                "build {recorded} recorded the trace from the VANA sources this build {current} has"
            ),
        ),
        BuildRelation::Other { recorded, current } => (
            false,
            format!(
                "build {recorded} recorded the trace from other VANA sources or features than this build {current}"
            ),
        ),
        BuildRelation::Unknown { why } => (false, why),
    };
    match asked {
        "auto" if matched => Ok((Mode::Faithful, builds)),
        "auto" => Ok((
            Mode::Changed,
            format!("{builds}, so it fails only where it leaves the recorded world"),
        )),
        "faithful" if matched => Ok((Mode::Faithful, format!("asked; {builds}"))),
        "faithful" => Ok((
            Mode::Faithful,
            format!("asked, but {builds}: a difference may be VANA's change, not a recording bug"),
        )),
        "changed" => Ok((Mode::Changed, format!("asked; {builds}"))),
        other => Err(format!(
            "--mode is auto, faithful or changed, not {other:?}"
        )),
    }
}

/// How a replay runs, why, and whether a failure writes a bundle.
struct Asked {
    replaying: Replaying,
    why: String,
    /// A faithful replay by the recording's build, or one of its sources,
    /// without --model: a bundle of its failure reproduces under that build.
    bundle: bool,
}

fn asked_mode(options: &Options, timeline: &Timeline) -> Result<Asked, String> {
    let current = build_name(&firmware_services::trace::origin::build(env!(
        "CYCLING_BUILD_HASH"
    )));
    let asked = options.value("mode").unwrap_or("auto");
    let relation = match relation(&timeline.build, &current) {
        Err(why) if asked != "auto" => BuildRelation::Unknown { why },
        found => found?,
    };
    let own_build = matches!(
        relation,
        BuildRelation::Same { .. } | BuildRelation::SameSources { .. }
    );
    let (mode, why) = pick_mode(asked, relation)?;
    let replaying = Replaying {
        mode,
        model: options.has("model"),
    };
    Ok(Asked {
        replaying,
        why,
        bundle: mode == Mode::Faithful
            && own_build
            && !replaying.model
            && !options.has("no-bundle"),
    })
}

fn out_dir(path: &Path, options: &Options, from: u64) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("trace");
    options.value("out").map_or_else(
        || {
            PathBuf::from(".local/replay").join(format!(
                "{stem}-{}",
                session_clock(from).to_string().replace([':', '.'], "-")
            ))
        },
        PathBuf::from,
    )
}

fn futures<'a>(
    path: &Path,
    trace: &[u8],
    timeline: &'a Timeline,
    options: &Options,
) -> Result<Futures<'a>, Failed> {
    let session_ms = |text: &str| timeline.session_ms(At::parse(text)?);
    let mut altered = trace.to_vec();
    let mut sets = Vec::new();
    for text in options.values("set") {
        let set = Set::parse(text, session_ms)?;
        let (trace, resolved) = surgery::set(&altered, &set)?;
        altered = trace;
        sets.push(Resolved {
            set: text.to_string(),
            at: timeline.moment(resolved),
        });
    }
    let replaying = Replaying {
        mode: Mode::Changed,
        model: true,
    };
    let why = "--set replays both futures changed, with --model".to_string();
    let from = options.time(timeline, "from")?.unwrap_or(0);
    let dir = out_dir(path, options, from);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let run = |trace: Vec<u8>, name: &str| {
        let dir = dir.join(name);
        let asked = Asked {
            replaying,
            why: why.clone(),
            bundle: false,
        };
        replay(path, &trace, timeline, options, asked, &dir, u64::MAX)
    };
    let (recorded, recorded_trace) = run(trace.to_vec(), "recorded")?;
    let (set, set_trace) = run(altered, "set")?;
    Ok(Futures {
        sets,
        diff: diff::diff(&recorded_trace, &set_trace)?,
        recorded,
        set,
    })
}

fn replay<'a>(
    path: &Path,
    trace: &[u8],
    timeline: &'a Timeline,
    options: &Options,
    asked: Asked,
    dir: &Path,
    to: u64,
) -> Result<(Replayed<'a>, Vec<u8>), Failed> {
    let from = options.time(timeline, "from")?.unwrap_or(0);
    let to = options
        .time(timeline, "to")?
        .unwrap_or(from.saturating_add(to));
    let every = match options.value("every") {
        Some(text) => match At::parse(text)? {
            At::Session(ms) if ms > 0 => ms,
            _ => return Err("--every takes a session length such as 1s".into()),
        },
        None => 5_000,
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("trace");
    let dir = dir.to_path_buf();
    shell_simulator::recording::replace_dir(&dir).map_err(|e| e.to_string())?;
    let collecting = firmware_services::why::collect();
    let mut sim = Sim::replaying(trace, asked.replaying, None).map_err(Failed::opening)?;
    let model = sim.device.model;
    let recording = Recording::at(dir.clone(), stem, model.name, model.width, model.height)
        .with_video(options.has("video"));
    let mut ran = sim.replay_until(from);
    let parts = asked.replaying
        == Replaying {
            mode: Mode::Faithful,
            model: false,
        };
    let before_window = parts.then(|| sim.device.report()?.first).flatten();
    let before = sim.device.open_replay_window();
    sim.attach_recording(recording);
    let last = to.min(timeline.end_ms);
    let shots = (0..).map(|i| from + i * every).take_while(|at| *at <= last);
    let changes = in_window(timeline, from, to)
        .filter(|e| e.kind.seen() && e.kind != Kind::Shown && !e.repeat)
        .map(|e| (e.at.session_ms, Some(e)));
    let mut stops: Vec<(u64, Option<&Entry>)> = shots.map(|at| (at, None)).chain(changes).collect();
    stops.sort_by_key(|(at, change)| (*at, change.is_none()));
    let mut screens = String::new();
    for (at, change) in stops {
        ran = ran && sim.replay_until(at);
        if !ran {
            break;
        }
        let moment = timeline.moment(sim.device.now);
        match change {
            Some(entry) => sim.note(
                shell_simulator::sim::NoteKind::Device,
                format!("recorded {} {}", entry.kind.name(), entry.text),
            ),
            None => {
                sim.checkpoint(&moment.to_string());
                let text: Vec<String> = sim.screen().text().lines().map(str::to_string).collect();
                let _ = writeln!(screens, "{moment}  screen  {}", text.join(" | "));
            }
        }
    }
    ran = ran && sim.replay_until(to);
    let reached = timeline.moment(sim.device.now);
    let report = sim.device.report();
    let divergences = (report.as_ref())
        .map(|report| divergence::summarize(timeline, (from, to), &before, report));
    let faithful = report.as_ref().and_then(|report| {
        let ok = report.verdict().is_ok();
        report
            .faithful
            .as_ref()
            .map(|faithful| Proved::of(ok, faithful))
    });
    let own = sim.trace();
    drop(sim);
    let decided = firmware_services::why::take();
    drop(collecting);
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("{}: {e}", dir.display()))
    };
    write("screens.txt", screens.as_bytes())?;
    write("why.txt", why_lines(timeline, &decided).as_bytes())?;
    let text = divergences
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    write("divergence.txt", text.as_bytes())?;
    write("replayed.trace", &own)?;
    let failure = report
        .filter(|_| parts)
        .and_then(|report| first_parting(before_window, report.first));
    let parted = failure.as_ref().map(|first| Parted {
        kind: first.kind.name(),
        at_ms: first.at_ms,
        detail: first.detail.clone(),
    });
    let bundle = failure
        .filter(|_| asked.bundle)
        .map(|first| failure_bundle(path, trace, timeline, &dir, first));
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    files.sort();
    let replayed = Replayed {
        header: Header::of(path, timeline),
        mode: asked.replaying.mode,
        why: asked.why,
        from_ms: from,
        to_ms: to,
        model: asked.replaying.model,
        reached,
        trace_ended: !ran,
        divergence: divergences,
        faithful,
        artifacts: Artifacts { dir, files },
        parted,
        bundle,
    };
    Ok((replayed, own))
}

fn why_lines(timeline: &Timeline, decided: &[firmware_services::why::Why]) -> String {
    let mut lines = String::new();
    for why in decided {
        let _ = write!(lines, "{}  {}", timeline.moment(why.at_ms), why.tag);
        for (i, (name, value)) in why.operands.iter().enumerate() {
            let _ = write!(lines, "{}{name}={value}", if i == 0 { "  " } else { " " });
        }
        lines.push('\n');
    }
    lines
}

fn first_parting(
    before_window: Option<Divergence>,
    in_window: Option<Divergence>,
) -> Option<Divergence> {
    before_window
        .into_iter()
        .chain(in_window)
        .min_by_key(|first| first.at_ms)
}

fn failure_bundle(
    path: &Path,
    trace: &[u8],
    timeline: &Timeline,
    dir: &Path,
    first: Divergence,
) -> Bundled {
    let pack = match card::pack_file(trace, &card::pack_store()) {
        Ok(pack) => pack,
        Err(error) => return Bundled::Failed(format!("the TERRAIN pack: {error}")),
    };
    Bundle {
        out: &dir.join("failure.bundle"),
        trace: path,
        build: &timeline.build,
        run: Run::Replay {
            to_ms: timeline.end_ms,
        },
        expect: Expect::Diverged {
            kind: first.kind.name(),
            at_ms: first.at_ms,
            detail: first.detail,
        },
        course: None,
        pack: pack.as_deref(),
    }
    .write()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn every_mode_the_tool_lists_as_a_capability_is_one_it_replays_in() {
        let listed = include_str!("../../trace-capabilities");
        assert_eq!(
            listed.split_whitespace().collect::<Vec<_>>(),
            ["faithful", "changed"]
        );
        for mode in listed.split_whitespace() {
            let relation = BuildRelation::Unknown {
                why: "a test".into(),
            };
            assert!(pick_mode(mode, relation).is_ok(), "{mode}");
        }
    }

    #[test]
    fn auto_replays_faithful_only_on_a_build_of_the_recording_sources() {
        let builds = || (BUILD.to_string(), BUILD.to_string());
        let auto = |relation| pick_mode("auto", relation).unwrap().0;
        assert_eq!(
            auto(BuildRelation::Same {
                build: BUILD.into()
            }),
            Mode::Faithful
        );
        let (recorded, current) = builds();
        assert_eq!(
            auto(BuildRelation::SameSources { recorded, current }),
            Mode::Faithful,
            "the firmware's build"
        );
        let (recorded, current) = builds();
        assert_eq!(
            auto(BuildRelation::Other { recorded, current }),
            Mode::Changed
        );
        assert_eq!(
            auto(BuildRelation::Unknown {
                why: "not archived".into()
            }),
            Mode::Changed
        );
        let (recorded, current) = builds();
        assert_eq!(
            pick_mode("faithful", BuildRelation::Other { recorded, current })
                .unwrap()
                .0,
            Mode::Faithful,
            "asked for"
        );
        assert!(
            pick_mode(
                "exact",
                BuildRelation::Same {
                    build: BUILD.into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn a_relation_is_what_build_hash_said_and_nothing_else() {
        let parse = |said| BuildRelation::parse(said, BUILD, BUILD);
        assert_eq!(
            parse("same-sources\n"),
            Ok(BuildRelation::SameSources {
                recorded: BUILD[..12].into(),
                current: BUILD[..12].into()
            })
        );
        assert!(matches!(parse("other\n"), Ok(BuildRelation::Other { .. })));
        assert!(matches!(
            parse("unknown build fedcba987654 has no archived sources\n"),
            Ok(BuildRelation::Unknown { why }) if why.ends_with(": build fedcba987654 has no archived sources")
        ));
        assert_eq!(
            parse("Traceback (most recent call last)"),
            Err("build_hash relate printed \"Traceback (most recent call last)\"".into())
        );
    }

    #[test]
    fn equal_builds_are_the_same_without_asking_the_checkout() {
        assert_eq!(
            relation(BUILD, BUILD),
            Ok(BuildRelation::Same {
                build: BUILD[..12].into()
            })
        );
        assert!(matches!(
            relation("", BUILD),
            Ok(BuildRelation::Unknown { .. })
        ));
        assert!(matches!(
            relation(BUILD, ""),
            Ok(BuildRelation::Unknown { .. })
        ));
        let comparison = firmware_services::trace::origin::build(&"0".repeat(40));
        assert_eq!(build_name(&comparison), "", "sim-compare's hash is unknown");
    }

    #[test]
    fn a_divergence_before_the_window_parts_even_when_the_window_holds_none() {
        let at = |at_ms| Divergence {
            at_ms,
            kind: shell_simulator::replay::Kind::Unrecorded,
            detail: format!("pass {at_ms}"),
        };
        assert_eq!(
            first_parting(Some(at(900)), None).map(|d| d.at_ms),
            Some(900)
        );
        assert_eq!(
            first_parting(Some(at(900)), Some(at(4_000))).map(|d| d.at_ms),
            Some(900)
        );
        assert_eq!(
            first_parting(None, Some(at(4_000))).map(|d| d.at_ms),
            Some(4_000)
        );
        assert!(first_parting(None, None).is_none());
    }
}
