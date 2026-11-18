//! `cycling trace` and `cycling replay`: read a ride trace as text, find
//! moments in it, export it to OpenTelemetry, replay a window of it with
//! screenshots, and write a regression test from it.
use shell_simulator::bundle::{Bundle, Bundled, Expect, Run};
use shell_simulator::recording::Recording;
use shell_simulator::replay::{Divergence, Mode};
use shell_simulator::sim::Sim;
use shell_simulator::trace::file::{self, Media};
use shell_simulator::trace::find::{Query, find};
use shell_simulator::trace::report::{
    Answers, Artifacts, Header, Listing, Parted, Prose, Replayed, Stub, Summary, Written,
    in_window, render,
};
use shell_simulator::trace::story::tell;
use shell_simulator::trace::timeline::{
    At, Entry, Kind, Timeline, build_name, read, session_clock,
};
use shell_simulator::trace::{card, divergence, origin, otlp, stub};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: cycling trace <command> <trace> [options] [--json]
       cycling replay <trace> [options] [--json]

Read a ride trace (<name>.trace; the media it started on are in <name>.origin
beside it when the simulator saved it). Times are the session clock, mm:ss.mmm since power-on,
and the ride clock, which runs only while a ride records; every line starts
`<session> ride <ride>`. Time options take 40:00, 1:02:03, 90s, 40m or 2500ms,
and r40:00 for a ride time.

--json, on any command, prints one JSON document instead of the text, and
{error} with exit status 2 when the command fails. A moment is
{session_ms, ride_ms}, ride_ms null outside a ride. An entry is a moment's
fields with {kind, text, repeat}, a page's cause and shows, and a packet's
bytes in hex, whatever --hex says. A header's terrain is the TERRAIN pack
the session found, {sha256, length, ready}, or null. summary gives {header,
silences, spans, events, battery, entries (per kind {new, repeats})};
timeline {from_ms, to_ms, entries, hidden}; find {found, total}, each found
an entry with type entry or {type stretch, from, to, first}, at most --limit
of them; replay {header, mode, why, from_ms, to_ms, reached, trace_ended,
divergence, artifacts, bundle}; origin {origin, lines}; otlp {traces,
metrics}, or {out} with --out; test-stub {fixture_steps, test}. The summary's
battery and the lines of origin are prose, the text's own lines, with no
fields to parse.

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
  origin <trace>
      The media a replay starts from, in words: power-on number, shell
      settings and VANA's preferences (saved sensors), rides on internal
      flash, and the bulk card. A trace without .origin starts blank.
  upgrade <trace>...
      Rewrite each trace's ORIGIN1 .origin, saved before the intent region,
      as ORIGIN2: a blank intent region and no marginal words, as that
      device had. An ORIGIN2 one is left as it is.
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
  replay <trace> [--from T] [--to T] [--mode auto|stimulus|strict]
         [--every T] [--video] [--out DIR] [--no-bundle]
      Replay the session up to --to (default --from plus 60s), screenshots every
      --every (default 5s) from --from, the screen's text, what the glue
      decided and from what (why.txt: toasts, cues, pages), the replay's own
      trace and where it parted from the recording, into --out (default
      .local/replay/<trace>-<from>/ under the checkout; paths print absolute).
      A strict replay by the recording's build, or one of its sources, that
      parts from the recording also writes failure.bundle there: the trace,
      that build's sources and the first divergence, one file `cycling bundle
      run` reproduces on another machine; the output names it, or why it is
      missing (bundle in JSON: {written} or {failed}). --no-bundle skips it.
      An existing --out is replaced only when a replay or scenario wrote it
      (it holds .simulator-output) or it is empty; anything else is refused.
      stimulus runs this build against the recorded world; strict expects
      the recording build and counts every call it answers differently.
      auto (default) picks strict when this build recorded the trace, or a
      build of the same VANA sources and features did (the firmware's, by
      the listings .local/builds archived for both hashes), else stimulus,
      and says why.
      Divergences count inside the window, with a total for before it, each
      labelled rider-visible, model-only (the stimulus ANT model),
      timing-only, call-level (strict) or replay fault.
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
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::json!({ "error": error }));
            }
            eprintln!("cycling trace: {error}");
            ExitCode::from(2)
        }
    }
}

struct Options {
    positional: Vec<String>,
    flags: BTreeMap<String, Option<String>>,
}

impl Options {
    fn parse(args: &[String], switches: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut flags = BTreeMap::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.strip_prefix("--") {
                Some(name) if switches.contains(&name) => {
                    flags.insert(name.to_string(), None);
                }
                Some(name) => {
                    let value = args.next().ok_or(format!("--{name} needs a value"))?;
                    flags.insert(name.to_string(), Some(value.clone()));
                }
                None => positional.push(arg.clone()),
            }
        }
        Ok(Self { positional, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name)?.as_deref()
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

fn run(args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().expect("arguments checked");
    let options = Options::parse(rest, &["all", "video", "hex", "json", "no-bundle"])?;
    let json = options.has("json");
    let path = PathBuf::from(
        options
            .positional
            .first()
            .ok_or(format!("{command} needs a trace file"))?,
    );
    if command == "upgrade" {
        let mut lines = String::new();
        for trace in &options.positional {
            let path = Path::new(trace);
            let origin = file::origin_path(path);
            let upgraded =
                file::upgrade(path).map_err(|error| format!("{}: {error}", origin.display()))?;
            let done = if upgraded {
                "upgraded from ORIGIN1"
            } else {
                "already ORIGIN2"
            };
            let _ = writeln!(lines, "{}: {done}", origin.display());
        }
        return render(
            json,
            &Prose {
                lines,
                origin: None,
            },
        );
    }
    let (capture, media) =
        file::load(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoded = None;
    let out = match command.as_str() {
        "summary" => {
            let timeline = decode(&mut decoded, &capture.trace)?;
            render(json, &Summary::of(&path, timeline, &media))
        }
        "origin" => {
            let origin = std::fs::canonicalize(file::origin_path(&path))
                .unwrap_or_else(|_| file::origin_path(&path))
                .display()
                .to_string();
            let lines = origin::describe(&capture, media.clone(), &origin);
            render(
                json,
                &Prose {
                    lines,
                    origin: Some(origin),
                },
            )
        }
        "timeline" => {
            let timeline = decode(&mut decoded, &capture.trace)?;
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
            let timeline = decode(&mut decoded, &capture.trace)?;
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
            let timeline = decode(&mut decoded, &capture.trace)?;
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
        "replay" => {
            let timeline = decode(&mut decoded, &capture.trace)?;
            render(json, &replay(&path, &capture, timeline, &media, &options)?)
        }
        "test-stub" => {
            let timeline = decode(&mut decoded, &capture.trace)?;
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
            let fixture_steps = stub::to_fixture(&path, &capture, &media);
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
    Ok(out)
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
        "auto" if matched => Ok((Mode::Strict, builds)),
        "auto" => Ok((
            Mode::Stimulus,
            format!("{builds}, so VANA may ask other things than it did"),
        )),
        "strict" if matched => Ok((Mode::Strict, format!("asked; {builds}"))),
        "strict" => Ok((
            Mode::Strict,
            format!(
                "asked, but {builds}: expect divergences that are VANA's changes, not replay faults"
            ),
        )),
        "stimulus" => Ok((Mode::Stimulus, format!("asked; {builds}"))),
        other => Err(format!("--mode is auto, stimulus or strict, not {other:?}")),
    }
}

fn replay<'a>(
    path: &Path,
    capture: &shell_simulator::replay::Capture,
    timeline: &'a Timeline,
    media: &'a Media,
    options: &Options,
) -> Result<Replayed<'a>, String> {
    let from = options.time(timeline, "from")?.unwrap_or(0);
    let to = options.time(timeline, "to")?.unwrap_or(from + 60_000);
    let every = match options.value("every") {
        Some(text) => match At::parse(text)? {
            At::Session(ms) if ms > 0 => ms,
            _ => return Err("--every takes a session length such as 1s".into()),
        },
        None => 5_000,
    };
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
    let bundling = mode == Mode::Strict && own_build && !options.has("no-bundle");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("trace");
    let dir = options.value("out").map_or_else(
        || {
            PathBuf::from(".local/replay").join(format!(
                "{stem}-{}",
                session_clock(from).to_string().replace([':', '.'], "-")
            ))
        },
        PathBuf::from,
    );
    shell_simulator::recording::replace_dir(&dir).map_err(|e| e.to_string())?;
    let collecting = firmware_services::why::collect();
    let mut sim = Sim::replaying(capture, mode, None).map_err(|e| e.to_string())?;
    let model = sim.device.model;
    let recording = Recording::at(dir.clone(), stem, model.name, model.width, model.height)
        .with_video(options.has("video"));
    let mut ran = sim.replay_until(from);
    let strict = mode == Mode::Strict;
    let before_window = strict.then(|| sim.device.report()?.first).flatten();
    let before = sim.device.open_replay_window();
    sim.attach_recording(recording);
    let shots = (0..).map(|i| from + i * every).take_while(|at| *at <= to);
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
    let reached = timeline.moment(sim.device.now);
    let report = sim.device.report();
    let divergences = report
        .as_ref()
        .map(|report| divergence::summarize(timeline, (from, to), &before, report));
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
        .filter(|_| strict)
        .and_then(|report| first_parting(before_window, report.first));
    let parted = failure.as_ref().map(|first| Parted {
        kind: first.kind.name(),
        at_ms: first.at_ms,
        detail: first.detail.clone(),
    });
    let bundle = failure
        .filter(|_| bundling)
        .map(|first| failure_bundle(path, capture, timeline, &dir, first));
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    files.sort();
    Ok(Replayed {
        header: Header::of(path, timeline, media),
        mode,
        why,
        from_ms: from,
        to_ms: to,
        reached,
        trace_ended: !ran,
        divergence: divergences,
        artifacts: Artifacts { dir, files },
        parted,
        bundle,
    })
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
    capture: &shell_simulator::replay::Capture,
    timeline: &Timeline,
    dir: &Path,
    first: Divergence,
) -> Bundled {
    let spill = dir.join("failure.pack");
    let pack = match card::pack_file(
        &capture.trace,
        &capture.origin.bulk,
        &card::pack_store(),
        &spill,
    ) {
        Ok(pack) => pack,
        Err(error) => return Bundled::Failed(format!("the TERRAIN pack: {error}")),
    };
    let bundled = Bundle {
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
    .write();
    let _ = std::fs::remove_file(&spill);
    bundled
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn auto_replays_strict_only_on_a_build_of_the_recording_sources() {
        let builds = || (BUILD.to_string(), BUILD.to_string());
        let auto = |relation| pick_mode("auto", relation).unwrap().0;
        assert_eq!(
            auto(BuildRelation::Same {
                build: BUILD.into()
            }),
            Mode::Strict
        );
        let (recorded, current) = builds();
        assert_eq!(
            auto(BuildRelation::SameSources { recorded, current }),
            Mode::Strict,
            "the firmware's build"
        );
        let (recorded, current) = builds();
        assert_eq!(
            auto(BuildRelation::Other { recorded, current }),
            Mode::Stimulus
        );
        assert_eq!(
            auto(BuildRelation::Unknown {
                why: "not archived".into()
            }),
            Mode::Stimulus
        );
        let (recorded, current) = builds();
        assert_eq!(
            pick_mode("strict", BuildRelation::Other { recorded, current })
                .unwrap()
                .0,
            Mode::Strict,
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
