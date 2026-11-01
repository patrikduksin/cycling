//! `cycling trace` and `cycling replay`: read a ride trace as text, find
//! moments in it, export it to OpenTelemetry, replay a window of it with
//! screenshots, and write a regression test from it.
use shell_simulator::recording::Recording;
use shell_simulator::replay::{Kind as Divergence, Mode};
use shell_simulator::sim::Sim;
use shell_simulator::trace::file::{self, Media};
use shell_simulator::trace::find::{Query, find};
use shell_simulator::trace::story::tell;
use shell_simulator::trace::timeline::{
    At, Entry, Kind, SILENCE_MS, Timeline, duration, read, session_clock,
};
use shell_simulator::trace::{otlp, stub};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: cycling trace <command> <trace> [options]
       cycling replay <trace> [options]

Read a ride trace (<name>.trace; a simulator capture keeps its media in
<name>.origin beside it). Times are the session clock, mm:ss.mmm since power-on,
and the ride clock, which runs only while a ride records; every line starts
`<session> ride <ride>`. Time options take 40:00, 1:02:03, 90s, 40m or 2500ms,
and r40:00 for a ride time.

commands:
  summary <trace>
      Sensor silences first (each sensor's last packet before a pause and its
      first after), then spans (session, rides, recording and paused
      stretches, pages, toasts, sensor links and silences, GPS fix) and
      events (boot, laps, warnings, storage, gaps, commands), one line each.
      A warning names what began or which ride ended in the same pass, and
      a ride counts the warnings logged in it. Run this first.
  timeline <trace> [--from T] [--to T] [--kinds LIST] [--all] [--hex]
      Every event in the window, inputs and what the rider saw. Kinds: gap,
      input, command, startup, ant, ant.packet, ant.request, ble, ble.packet,
      gps, sensors, battery, time, storage (media faults and VANA's warnings
      about saving or reading), entropy, page, toast, sound, ride, shown,
      log, phone, power, display, buzzer, trace; groups seen, inputs,
      default, all. --all also lists entries
      that only repeat their source (a counter or reading moved). ANT+ pages
      read as words (power 230 W, hr 142 bpm, cadence 86 rpm, radar targets);
      --hex adds each packet's bytes.
  find <trace> <query> [--where TEXT] [--from T] [--to T] [--limit N] [--hex]
      field=value, field!=value or field~text over the shown readings (time,
      speed, power, hr, cad, dist, grade) gives the stretches it held, e.g.
      power=--; a kind or kinds (toast, page,ride) gives those entries; any
      other text gives entries containing it, ignoring case. --where keeps
      entries containing TEXT.
  otlp <trace> [--out FILE]
      OTLP/JSON: a traces request and a metrics request, one per line.
  replay <trace> [--from T] [--to T] [--mode auto|stimulus|strict]
         [--every T] [--video] [--out DIR]
      Replay the session up to --to (default --from plus 60s), screenshots every
      --every (default 5s) from --from, the screen's text, the replay's own
      trace and where it parted from the recording. stimulus runs this build
      against the recorded world; strict expects the recording build and
      counts every call it answers differently. auto (default) picks strict
      when this checkout, without uncommitted VANA changes, built the trace's
      commit, else stimulus, and prints which and why.
  test-stub <trace> --at T [--name NAME]
      A regression test that replays the trace to T and asserts what the
      rider saw then. Put the trace under tools/simulator/tests/traces/.
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
            eprintln!("cycling trace: {error}");
            ExitCode::from(2)
        }
    }
}

/// The parsed options after a command and its trace.
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
    let options = Options::parse(rest, &["all", "video", "hex"])?;
    let path = PathBuf::from(
        options
            .positional
            .first()
            .ok_or(format!("{command} needs a trace file"))?,
    );
    let (capture, media) =
        file::load(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let timeline = read(&capture.trace)?;
    match command.as_str() {
        "summary" => Ok(summary(&path, &timeline, media)),
        "timeline" => timeline_text(&timeline, &options),
        "find" => find_text(&timeline, &options),
        "otlp" => {
            let json = otlp::export(&timeline, &tell(&timeline));
            match options.value("out") {
                Some(out) => {
                    std::fs::write(out, json).map_err(|e| format!("{out}: {e}"))?;
                    Ok(format!("{out}\n"))
                }
                None => Ok(json),
            }
        }
        "replay" => replay(&path, &capture, &timeline, media, &options),
        "test-stub" => {
            let at = options
                .time(&timeline, "at")?
                .ok_or("test-stub needs --at")?;
            let fixture = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("the trace needs a file name")?;
            let name = options
                .value("name")
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}_at_{}", fixture.replace('-', "_"), at / 1000));
            Ok(stub::stub(&timeline, fixture, &name, at))
        }
        other => Err(format!("unknown command {other:?}; see --help")),
    }
}

fn header(path: &Path, timeline: &Timeline, media: Media) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "trace {}", path.display());
    let _ = writeln!(
        out,
        "build {}, boot {}, format {}, {} over {} passes, {}; media {}",
        if timeline.build.is_empty() {
            "unknown"
        } else {
            &timeline.build
        },
        timeline.boot,
        timeline.version,
        duration(timeline.end_ms),
        timeline.passes,
        if timeline.torn {
            "torn: the last block was cut short"
        } else {
            "complete"
        },
        match media {
            Media::Saved => "saved beside it (.origin)",
            Media::Blank => "not saved: replays start on blank media",
        }
    );
    out
}

fn summary(path: &Path, timeline: &Timeline, media: Media) -> String {
    let mut out = header(path, timeline, media);
    let silences: Vec<String> = timeline
        .silences
        .iter()
        .map(|silence| {
            let until = silence.next_ms.unwrap_or(timeline.end_ms);
            format!(
                "{} silent {}-{} ({}{})",
                silence.sensor,
                session_clock(silence.last_ms),
                session_clock(until),
                duration(until - silence.last_ms),
                if silence.next_ms.is_none() {
                    ", to the end"
                } else {
                    ""
                }
            )
        })
        .collect();
    let _ = writeln!(
        out,
        "sensor silences (no ANT+ packet for over {}): {}",
        duration(SILENCE_MS),
        if silences.is_empty() {
            "none".into()
        } else {
            silences.join("; ")
        }
    );
    let story = tell(timeline);
    let mut lines: Vec<(u64, String)> = story
        .spans
        .iter()
        .map(|span| (span.start.session_ms, span.to_string()))
        .chain(
            story
                .events
                .iter()
                .map(|event| (event.at.session_ms, event.to_string())),
        )
        .collect();
    lines.sort_by_key(|(at, _)| *at);
    for (_, line) in lines {
        let _ = writeln!(out, "{line}");
    }
    let mut counts: BTreeMap<Kind, (usize, usize)> = BTreeMap::new();
    for entry in &timeline.entries {
        let count = counts.entry(entry.kind).or_default();
        if entry.repeat {
            count.1 += 1;
        } else {
            count.0 += 1;
        }
    }
    let counts: Vec<String> = counts
        .iter()
        .map(|(kind, (new, repeats))| match repeats {
            0 => format!("{} {new}", kind.name()),
            _ => format!("{} {new}+{repeats}", kind.name()),
        })
        .collect();
    let _ = writeln!(out, "entries: {}", counts.join(", "));
    let _ = writeln!(
        out,
        "next: timeline --from T --to T, find <field=value|kind|text>, replay --from T"
    );
    out
}

fn timeline_text(timeline: &Timeline, options: &Options) -> Result<String, String> {
    let (from, to) = options.window(timeline)?;
    let kinds = Kind::parse_list(options.value("kinds").unwrap_or("default"))?;
    let all = options.has("all");
    let mut out = String::new();
    let mut hidden: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in in_window(timeline, from, to) {
        if !kinds.contains(&entry.kind) {
            *hidden.entry(entry.kind.name()).or_default() += 1;
        } else if entry.repeat && !all {
            *hidden.entry("repeats").or_default() += 1;
        } else if options.has("hex") {
            let _ = writeln!(out, "{entry:#}");
        } else {
            let _ = writeln!(out, "{entry}");
        }
    }
    if !hidden.is_empty() {
        let hidden: Vec<String> = hidden.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let _ = writeln!(out, "hidden: {} (--kinds all, --all)", hidden.join(", "));
    }
    Ok(out)
}

fn in_window(timeline: &Timeline, from: u64, to: u64) -> impl Iterator<Item = &Entry> {
    timeline
        .entries
        .iter()
        .filter(move |e| (from..=to).contains(&e.at.session_ms))
}

fn find_text(timeline: &Timeline, options: &Options) -> Result<String, String> {
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
    let mut out = String::new();
    for line in found.iter().take(limit) {
        if options.has("hex") {
            let _ = writeln!(out, "{line:#}");
        } else {
            let _ = writeln!(out, "{line}");
        }
    }
    match found.len() {
        0 => out.push_str("found nothing\n"),
        n if n > limit => {
            let _ = writeln!(out, "... {} more (--limit)", n - limit);
        }
        _ => {}
    }
    Ok(out)
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Stimulus => "stimulus",
        Mode::Strict => "strict",
    }
}

/// The mode a replay runs in, and why. `auto` is strict when this checkout
/// built the trace's commit and has no uncommitted change to what builds
/// VANA, else stimulus.
fn pick_mode(
    asked: &str,
    recorded: &str,
    current: &str,
    edits: &[String],
) -> Result<(Mode, String), String> {
    let short = |build: &str| build.chars().take(12).collect::<String>();
    let same = !recorded.is_empty()
        && !current.is_empty()
        && (recorded.starts_with(current) || current.starts_with(recorded));
    let builds = if recorded.is_empty() {
        "the trace names no build (recorded before builds were stamped)".to_string()
    } else if current.is_empty() {
        format!(
            "the trace's build is {}; this checkout's is unknown",
            short(recorded)
        )
    } else if !same {
        format!(
            "the trace's build {} is not this checkout's {}",
            short(recorded),
            short(current)
        )
    } else if let Some(first) = edits.first() {
        format!(
            "this checkout's commit {} recorded the trace, but {} files that build VANA have uncommitted changes ({first})",
            short(current),
            edits.len()
        )
    } else {
        format!(
            "this checkout's build {} recorded the trace",
            short(current)
        )
    };
    let matched = same && edits.is_empty();
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

/// Files with uncommitted changes that go into this binary's VANA, from the
/// checkout that built it; none when git cannot say.
fn uncommitted() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--",
            "apps",
            "packages",
            "tools/simulator",
            "Cargo.toml",
            "Cargo.lock",
        ])
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

fn replay(
    path: &Path,
    capture: &shell_simulator::replay::Capture,
    timeline: &Timeline,
    media: Media,
    options: &Options,
) -> Result<String, String> {
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
        &uncommitted(),
    )?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("trace");
    let dir = options.value("out").map_or_else(
        || {
            PathBuf::from(".local/replay").join(format!(
                "{stem}-{}",
                session_clock(from).replace([':', '.'], "-")
            ))
        },
        PathBuf::from,
    );
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut recording = Recording::at(dir.clone(), stem, 240, 320);
    if !options.has("video") {
        recording = recording.without_video();
    }
    let mut sim = Sim::replaying(capture, mode, None);
    let mut ran = sim.replay_until(from);
    sim.attach_recording(recording);
    // Stop at each screenshot and at each change the recording saw, which
    // becomes a caption at its own time.
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
    let report = sim.device.report().map(|report| {
        let mut text = String::new();
        match &report.first {
            None => text.push_str("none\n"),
            Some(first) => {
                let _ = writeln!(
                    text,
                    "first at {}: {:?}: {}",
                    timeline.moment(first.at_ms),
                    first.kind,
                    first.detail
                );
                let counts: Vec<String> = report
                    .counts
                    .iter()
                    .map(|(kind, n)| format!("{kind:?} {n}"))
                    .collect();
                let _ = writeln!(text, "counts: {}", counts.join(", "));
                if mode == Mode::Stimulus && report.count(Divergence::Seen) == 0 {
                    text.push_str(
                        "the rider saw what the recording saw; the rest is how this build asked\n",
                    );
                }
            }
        }
        text
    });
    let report = report.unwrap_or_default();
    let own = sim.trace();
    drop(sim);
    let write = |name: &str, bytes: &[u8]| {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("{}: {e}", dir.display()))
    };
    write("screens.txt", screens.as_bytes())?;
    write("divergence.txt", report.as_bytes())?;
    write("replayed.trace", &own)?;
    let mut out = header(path, timeline, media);
    let _ = writeln!(out, "mode {}: {why}", mode_name(mode));
    let _ = writeln!(
        out,
        "replayed {} from {} to {}; reached {}{}",
        mode_name(mode),
        session_clock(from),
        session_clock(to),
        reached,
        if ran { "" } else { " (the trace ended)" }
    );
    let _ = write!(out, "divergence: {report}");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    files.sort();
    let _ = writeln!(out, "artifacts in {}:", dir.display());
    for file in files {
        let _ = writeln!(out, "  {}", file.display());
    }
    let _ = writeln!(
        out,
        "screens.txt has the screen text at each screenshot; replayed.trace is this replay's own trace (diff it with timeline)"
    );
    Ok(out)
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
        // The device stamps 12 hex digits.
        assert_eq!(auto(&BUILD[..12], BUILD, &[]), Mode::Strict);
        assert_eq!(auto("", BUILD, &[]), Mode::Stimulus);
        assert_eq!(auto(BUILD, "", &[]), Mode::Stimulus);
        assert_eq!(auto("fedcba9876", BUILD, &[]), Mode::Stimulus);
        let edited = ["apps/vana/src/app/facts.rs".to_string()];
        assert_eq!(auto(BUILD, BUILD, &edited), Mode::Stimulus);
        assert_eq!(
            pick_mode("strict", "", BUILD, &[]).unwrap().0,
            Mode::Strict,
            "asked for"
        );
        assert!(pick_mode("exact", BUILD, BUILD, &[]).is_err());
    }
}
