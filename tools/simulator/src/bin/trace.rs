//! `cycling trace` and `cycling replay`: read a ride trace as text, find
//! moments in it, export it to OpenTelemetry, replay a window of it with
//! screenshots, and write a regression test from it.
use shell_simulator::recording::Recording;
use shell_simulator::replay::Mode;
use shell_simulator::sim::Sim;
use shell_simulator::trace::file::{self, Media};
use shell_simulator::trace::find::{Query, find};
use shell_simulator::trace::report::{
    Answers, Artifacts, Header, Listing, Otlp, Prose, Rebuilt, Replayed, Stub, Summary, Written,
    in_window, render,
};
use shell_simulator::trace::story::tell;
use shell_simulator::trace::timeline::{At, Entry, Kind, Timeline, read, session_clock};
use shell_simulator::trace::{divergence, origin, otlp, rebuild, stub};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: cycling trace <command> <trace> [options] [--json]
       cycling replay <trace> [options] [--json]

Read a ride trace (<name>.trace; the media it started on are in <name>.origin
beside it: saved by the simulator, or rebuilt for a device trace by pull). Times are the session clock, mm:ss.mmm since power-on,
and the ride clock, which runs only while a ride records; every line starts
`<session> ride <ride>`. Time options take 40:00, 1:02:03, 90s, 40m or 2500ms,
and r40:00 for a ride time.

--json, on any command, prints one JSON document instead of the text, and
{error} with exit status 2 when the command fails. A moment is
{session_ms, ride_ms}, ride_ms null outside a ride. An entry is a moment's
fields with {kind, text, repeat}, a page's cause and shows, and a packet's
bytes in hex, whatever --hex says. summary gives {header, silences, spans,
events, battery, entries (per kind {new, repeats})}; timeline {from_ms,
to_ms, entries, hidden}; find {found, total}, each found an entry with type
entry or {type stretch, from, to, first}, at most --limit of them; replay
{header, mode, why, from_ms, to_ms, reached, trace_ended, divergence,
artifacts}; origin {origin, lines} and media {lines}; rebuild {origin,
not_rebuilt}; otlp {traces, metrics}, or {out} with --out; test-stub
{fixture_steps, test}. The summary's battery and the lines of origin and
media are prose, the text's own lines, with no fields to parse.

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
      flash, and the bulk card; for a rebuilt .origin, what it could not
      rebuild. A trace without .origin starts blank.
  media <trace>
      The media the trace's header names, one line each, as `cycling trace
      pull` reads them: boot N; journal SLOTS, or journal unusable; rides
      log|uncertain STORE HEAD, or rides unusable|uninitialized; files
      STORE GENERATION, or files unreadable card|table|volume; file NAME
      LENGTH CRC per file; terrain LENGTH SHA256 ready|writing TAG, or
      terrain unreadable card|table|volume.
  rebuild <trace> --pulled DIR
      Write <trace>.origin from the header and the media pulled after the
      session into DIR (the layout `cycling trace pull` writes; see
      shell_simulator::trace::rebuild), and say what could not be rebuilt.
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
  find <trace> <query> [--where TEXT] [--from T] [--to T] [--limit N] [--hex]
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
         [--every T] [--video] [--out DIR]
      Replay the session up to --to (default --from plus 60s), screenshots every
      --every (default 5s) from --from, the screen's text, the replay's own
      trace and where it parted from the recording, into --out (default
      .local/replay/<trace>-<from>/ under the checkout; paths print absolute).
      An existing --out is replaced only when a replay or scenario wrote it
      (it holds .simulator-output) or it is empty; anything else is refused.
      stimulus runs this build against the recorded world; strict expects
      the recording build and counts every call it answers differently.
      auto (default) picks strict when this checkout, without uncommitted
      VANA changes, built the trace's commit, else stimulus, and says why;
      a build with uncommitted changes never matches (its stamp ends -dirty,
      and the traces it records name no build).
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
    let options = Options::parse(rest, &["all", "video", "hex", "json"])?;
    let json = options.has("json");
    let path = PathBuf::from(
        options
            .positional
            .first()
            .ok_or(format!("{command} needs a trace file"))?,
    );
    let (capture, media) =
        file::load(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoded = None;
    let out = match command.as_str() {
        "summary" => {
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
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
        "media" => {
            let lines = rebuild::media(&capture.trace)?;
            render(
                json,
                &Prose {
                    lines,
                    origin: None,
                },
            )
        }
        "rebuild" => {
            let dir = options
                .value("pulled")
                .ok_or("rebuild needs --pulled DIR")?;
            let pulled =
                rebuild::Pulled::read(Path::new(dir)).map_err(|e| format!("{dir}: {e}"))?;
            let (origin, not_rebuilt) = rebuild::rebuild(&capture.trace, &pulled)?;
            file::save_rebuilt(&origin, &rebuild::report(&not_rebuilt), &path)
                .map_err(|e| format!("{}: {e}", file::origin_path(&path).display()))?;
            let origin = std::fs::canonicalize(file::origin_path(&path))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            render(
                json,
                &Rebuilt {
                    not_rebuilt,
                    origin,
                },
            )
        }
        "timeline" => {
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
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
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
            let query = options
                .positional
                .get(1)
                .ok_or("find needs a query: field=value, a kind or text")?;
            let window = options.window(timeline)?;
            let limit: usize = options
                .value("limit")
                .map_or(Ok(50), str::parse)
                .map_err(|_| "--limit takes a number")?;
            let found = find(
                timeline,
                &Query::parse(query),
                window,
                options.value("where"),
            );
            render(json, &Answers::of(found, limit, options.has("hex")))
        }
        "otlp" => {
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
            let requests = otlp::export(timeline, &tell(timeline));
            match options.value("out") {
                Some(out) => {
                    std::fs::write(out, requests).map_err(|e| format!("{out}: {e}"))?;
                    let out = std::fs::canonicalize(out).map_err(|e| format!("{out}: {e}"))?;
                    render(json, &Written { out })
                }
                None => render(json, &Otlp(requests)),
            }
        }
        "replay" => {
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
            render(json, &replay(&path, &capture, timeline, &media, &options)?)
        }
        "test-stub" => {
            let timeline: &Timeline = decoded.insert(read(&capture.trace)?);
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

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Stimulus => "stimulus",
        Mode::Strict => "strict",
    }
}

const DIRTY: &str = "-dirty";

fn pick_mode(
    asked: &str,
    recorded: &str,
    current: &str,
    edits: &[String],
) -> Result<(Mode, String), String> {
    let short = |build: &str| build.chars().take(12).collect::<String>();
    let built = current.strip_suffix(DIRTY);
    let commit = built.unwrap_or(current);
    let same = !recorded.is_empty()
        && !commit.is_empty()
        && (recorded.starts_with(commit) || commit.starts_with(recorded));
    let builds = if recorded.is_empty() {
        "the trace names no build (a build with uncommitted changes recorded it, or one from before builds were stamped)".to_string()
    } else if commit.is_empty() {
        format!(
            "the trace's build is {}; this checkout's is unknown",
            short(recorded)
        )
    } else if !same {
        format!(
            "the trace's build {} is not this checkout's {}",
            short(recorded),
            short(commit)
        )
    } else if let Some(first) = edits.first() {
        format!(
            "this checkout's commit {} recorded the trace, but {} files that build VANA have uncommitted changes ({first})",
            short(commit),
            edits.len()
        )
    } else if built.is_some() {
        format!(
            "this checkout's commit {} recorded the trace, but this build was made with uncommitted changes to files that build VANA",
            short(commit)
        )
    } else {
        format!("this checkout's build {} recorded the trace", short(commit))
    };
    let matched = same && built.is_none() && edits.is_empty();
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

fn uncommitted_here() -> Vec<String> {
    std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no", "--"])
        .args(
            shell_simulator::session::build_sources()
                .iter()
                .map(|source| format!(":(top){source}")),
        )
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| line.get(3..).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
    let (mode, why) = pick_mode(
        options.value("mode").unwrap_or("auto"),
        &timeline.build,
        option_env!("CYCLING_BUILD_COMMIT").unwrap_or(""),
        &uncommitted_here(),
    )?;
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
    let recording = Recording::at(dir.clone(), stem, 240, 320).with_video(options.has("video"));
    let mut sim = Sim::replaying(capture, mode, None);
    let mut ran = sim.replay_until(from);
    let before = sim
        .device
        .replay_sources()
        .map(|sources| sources.open_window())
        .unwrap_or_default();
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
    let divergences = sim
        .device
        .report()
        .map(|report| divergence::summarize(timeline, (from, to), &before, &report));
    let own = sim.trace();
    drop(sim);
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("{}: {e}", dir.display()))
    };
    write("screens.txt", screens.as_bytes())?;
    let report = divergences
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    write("divergence.txt", report.as_bytes())?;
    write("replayed.trace", &own)?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    files.sort();
    Ok(Replayed {
        header: Header::of(path, timeline, media),
        mode: mode_name(mode),
        why,
        from_ms: from,
        to_ms: to,
        reached,
        trace_ended: !ran,
        divergence: divergences,
        artifacts: Artifacts { dir, files },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn auto_replays_strict_only_on_the_build_that_recorded_the_trace() {
        let auto = |recorded, current, edits: &[String]| {
            pick_mode("auto", recorded, current, edits).unwrap().0
        };
        assert_eq!(auto(BUILD, BUILD, &[]), Mode::Strict);
        assert_eq!(auto(&BUILD[..12], BUILD, &[]), Mode::Strict);
        assert_eq!(auto("", BUILD, &[]), Mode::Stimulus);
        assert_eq!(auto(BUILD, "", &[]), Mode::Stimulus);
        assert_eq!(auto("fedcba9876", BUILD, &[]), Mode::Stimulus);
        let edited = ["apps/vana/src/app/facts.rs".to_string()];
        assert_eq!(auto(BUILD, BUILD, &edited), Mode::Stimulus);
        let dirty = format!("{BUILD}{DIRTY}");
        assert_eq!(auto(BUILD, &dirty, &[]), Mode::Stimulus, "built dirty");
        assert_eq!(auto(&BUILD[..12], &dirty, &[]), Mode::Stimulus);
        assert_eq!(
            pick_mode("strict", "", BUILD, &[]).unwrap().0,
            Mode::Strict,
            "asked for"
        );
        assert!(pick_mode("exact", BUILD, BUILD, &[]).is_err());
    }
}
