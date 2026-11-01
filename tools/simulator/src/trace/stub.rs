//! A regression test, ready to paste, that replays a trace to one moment.
//! It has two sections. The characterization asserts the rest of the moment
//! as the recording showed it, and its doc comment says what was seen and
//! what the sensors were doing. The expectation states what the rider
//! should have seen: from `--expect`, or a `todo!` the author replaces, so
//! the test is red on the recording and never asserts the symptom.
use crate::replay::Capture;
use crate::trace::file::{Media, origin_path};
use crate::trace::find::{Query, Test};
use crate::trace::secret::secrets;
use crate::trace::timeline::{Kind, Silence, Timeline, field, session_clock};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Where `tools/simulator/tests` keeps traces, and how a test names one.
pub const FIXTURES: &str = "tools/simulator/tests/traces";

const UNITS: [(&str, &str); 3] = [("speed", "KM/H"), ("power", "W"), ("hr", "BPM")];
const SLOTS: [&str; 3] = ["page", "toast", "ride"];

/// What the rider should have seen: `power!=--`, `page=ride/climb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expect {
    pub name: String,
    pub test: Test,
    pub value: String,
}

impl Expect {
    /// `field=value`, `field!=value` or `field~text`, for a shown field or
    /// page, toast or ride.
    pub fn parse(text: &str) -> Result<Self, String> {
        match Query::parse(text) {
            Query::Field { name, test, value } => Ok(Self { name, test, value }),
            _ => Err(format!(
                "--expect takes field=value, field!=value or field~text, not {text:?}"
            )),
        }
    }

    fn holds(&self, got: Option<&str>) -> bool {
        match self.test {
            Test::Is => got == Some(self.value.as_str()),
            Test::IsNot => got != Some(self.value.as_str()),
            Test::Contains => got.is_some_and(|got| got.contains(self.value.as_str())),
        }
    }

    fn assert(&self, actual: &str, recorded: Option<&str>) -> String {
        let value = &self.value;
        let message = format!(
            "{}: the recording showed {}",
            self.name,
            recorded.unwrap_or("nothing")
        );
        match self.test {
            Test::Is => format!("    assert_eq!({actual}, Some({value:?}), {message:?});"),
            Test::IsNot => format!("    assert_ne!({actual}, Some({value:?}), {message:?});"),
            Test::Contains => format!(
                "    assert!({actual}.is_some_and(|got| got.contains({value:?})), {message:?});"
            ),
        }
    }
}

/// For a trace outside the fixtures directory: whether it may become a
/// fixture and the steps that make it one, as comment lines to print before
/// the stub, which reads it from there. None for a fixture.
pub fn to_fixture(path: &Path, capture: &Capture, media: &Media) -> Option<String> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    if std::fs::canonicalize(dir).is_ok_and(|dir| dir.ends_with(FIXTURES)) {
        return None;
    }
    let name = path.file_stem()?.to_string_lossy();
    let source = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = format!(
        "// {} is not a fixture: the test below reads {FIXTURES}/{name}.trace.\n",
        source.display()
    );
    let found = secrets(capture);
    if matches!(media, Media::Blank | Media::Rebuilt(_)) {
        out.push_str(
            "// It is a device trace (no .origin, or one `cycling trace pull` rebuilt): it records\n\
             // where the rider rode and is never committed. Write a simulator scenario that gives\n\
             // VANA the same inputs and record it as a fixture instead (ride-trace skill, step 5).\n",
        );
    } else if !found.is_empty() {
        let _ = writeln!(
            out,
            "// It holds secrets, so it is never committed: {}.\n// Record a simulator scenario as the fixture instead (ride-trace skill, step 5).",
            found.join("; ")
        );
    } else {
        let _ = writeln!(
            out,
            "// It holds no Wi-Fi password, phone session token or saved Wi-Fi credentials.\n\
             // To make it a fixture, from the checkout root:\n\
             //   cp {} {FIXTURES}/{name}.trace\n\
             //   cp {} {FIXTURES}/{name}.origin\n\
             //   cargo test -p shell-simulator --features cycling,debug-harness --test trace_tools no_committed_fixture",
            source.display(),
            origin_path(&source).display()
        );
        if Path::new(FIXTURES).join(format!("{name}.trace")).exists() {
            out.push_str(
                "// A fixture of that name is already there: copy under a new name and run\n\
                 // test-stub on the copy.\n",
            );
        }
    }
    Some(out)
}

pub fn stub(
    timeline: &Timeline,
    fixture: &str,
    name: &str,
    at_ms: u64,
    expect: &[Expect],
) -> Result<String, String> {
    let moment = timeline.moment(at_ms);
    let seen = timeline.seen_at(at_ms);
    let shown = seen.shown.as_ref().map(|(_, shown)| shown.as_str());
    let recorded = |name: &str| -> Option<&str> {
        match name {
            "page" => seen.page.as_ref().map(|(_, text)| text.as_str()),
            "toast" => seen.toast.as_ref().map(|(_, text)| text.as_str()),
            "ride" => seen.ride.as_ref().map(|(_, text)| text.as_str()),
            field_name => shown.and_then(|shown| field(shown, field_name)),
        }
    };
    for expected in expect {
        let known = SLOTS.contains(&expected.name.as_str())
            || shown.is_some_and(|shown| field(shown, &expected.name).is_some());
        if !known {
            return Err(format!(
                "--expect {}: the rider saw no {0} then; name page, toast, ride or a field of: {}",
                expected.name,
                shown.unwrap_or("(no readings)")
            ));
        }
        if expected.holds(recorded(&expected.name)) {
            return Err(format!(
                "--expect {}: the recording already shows {}, so the test would pass on the bug",
                expected.name,
                recorded(&expected.name).unwrap_or("nothing")
            ));
        }
    }
    let expected = |name: &str| expect.iter().any(|e| e.name == name);
    let on_main = recorded("page") == Some("ride/main");

    let mut characterization = Vec::new();
    for slot in SLOTS.into_iter().filter(|slot| !expected(slot)) {
        characterization.push(format!(
            "    assert_eq!(text(&seen.{slot}).as_deref(), {});",
            match recorded(slot) {
                Some(value) => format!("Some({value:?})"),
                None => "None".into(),
            }
        ));
    }
    if !expect.is_empty() {
        for (field_name, unit) in UNITS.into_iter().filter(|(f, _)| !expected(f)) {
            let Some(value) = recorded(field_name) else {
                continue;
            };
            characterization.push(format!(
                "    assert_eq!(field(&shown, {field_name:?}), Some({value:?}));"
            ));
            if on_main && value == "--" {
                characterization.push(format!(
                    "    assert_eq!(sim.screen().field({unit:?}).as_deref(), Some(\"--\"), \"{field_name} on screen\");"
                ));
            }
        }
    }

    let mut expectation = Vec::new();
    for expected in expect {
        let name = expected.name.as_str();
        let actual = if SLOTS.contains(&name) {
            format!("text(&seen.{name}).as_deref()")
        } else {
            format!("field(&shown, {name:?})")
        };
        expectation.push(expected.assert(&actual, recorded(name)));
        let unit = UNITS.iter().find(|(f, _)| *f == name).map(|(_, unit)| unit);
        if let Some(unit) = unit
            && on_main
            && expected.value == "--"
            && expected.test != Test::Contains
        {
            let check = if expected.test == Test::Is {
                "assert_eq"
            } else {
                "assert_ne"
            };
            expectation.push(format!(
                "    {check}!(sim.screen().field({unit:?}).as_deref(), Some(\"--\"), \"{name} on screen\");"
            ));
        }
    }
    if expect.is_empty() {
        expectation.push(
            "    // e.g. assert_ne!(shell_simulator::trace::timeline::field(&shown, \"power\"), Some(\"--\"));".into(),
        );
        expectation.push(format!(
            "    todo!(\"state what the rider should have seen at {moment}; the replay showed {{shown}}\");"
        ));
    }
    let uses = |text: &str| {
        characterization
            .iter()
            .chain(&expectation)
            .filter(|line| !line.trim_start().starts_with("//"))
            .any(|line| line.contains(text))
    };

    let mut doc = String::new();
    let _ = writeln!(
        doc,
        "/// Replays {fixture}.trace to {moment}, where the rider reported the problem."
    );
    doc.push_str(
        "/// Written by `cycling trace test-stub`.\n///\n/// What the recording showed then:\n",
    );
    for slot in SLOTS {
        let since = match slot {
            "page" => &seen.page,
            "toast" => &seen.toast,
            _ => &seen.ride,
        };
        let _ = writeln!(
            doc,
            "///   {slot}: {}",
            match since {
                Some((from, text)) => format!("{text} since {}", session_clock(*from)),
                None => "none".into(),
            }
        );
    }
    let _ = writeln!(doc, "///   shown: {}", shown.unwrap_or("nothing"));
    doc.push_str("/// Sensors then:\n");
    for line in sensors(timeline, at_ms) {
        let _ = writeln!(doc, "///   {line}");
    }

    let mut body = String::from("    use shell_simulator::sim::Sim;\n");
    if uses("field(&shown") {
        body.push_str("    use shell_simulator::trace::timeline::field;\n");
    }
    let _ = writeln!(
        body,
        "    let (capture, _) = shell_simulator::trace::file::load(std::path::Path::new(concat!(\n        env!(\"CARGO_MANIFEST_DIR\"),\n        \"/tests/traces/{fixture}.trace\"\n    )))\n    .expect(\"the fixture loads\");"
    );
    let _ = writeln!(
        body,
        "    let mut sim = Sim::replaying(&capture, shell_simulator::replay::Mode::Stimulus, None);"
    );
    let _ = writeln!(
        body,
        "    assert!(sim.replay_until({at_ms}), \"the trace reaches {moment}\");"
    );
    body.push_str("    let seen = sim.seen();\n");
    body.push_str(
        "    let text = |slot: &Option<(u64, String)>| slot.as_ref().map(|(_, text)| text.clone());\n",
    );
    if uses("shown") {
        body.push_str("    let shown = text(&seen.shown).unwrap_or_default();\n");
    }
    body.push_str(
        "\n    // Characterization: the rest of the moment as the recording showed it.\n    // Keep it; a fix should change only what the expectation names.\n",
    );
    for line in &characterization {
        let _ = writeln!(body, "{line}");
    }
    body.push_str(
        "\n    // Expectation: what the rider should have seen. It fails on the\n    // recording and passes once VANA is fixed.\n",
    );
    for line in &expectation {
        let _ = writeln!(body, "{line}");
    }
    Ok(format!(
        "{doc}#[cfg(feature = \"cycling\")]\n#[test]\nfn {name}() {{\n{body}}}\n"
    ))
}

fn sensors(timeline: &Timeline, at_ms: u64) -> Vec<String> {
    let mut links: BTreeMap<String, String> = BTreeMap::new();
    let mut packets: BTreeMap<String, u64> = BTreeMap::new();
    for entry in timeline
        .entries
        .iter()
        .take_while(|e| e.at.session_ms <= at_ms)
        .filter(|e| !e.repeat)
    {
        let since = session_clock(entry.at.session_ms);
        match entry.kind {
            Kind::Ant if entry.text.starts_with("ch") => {
                let channel = entry.text.split(' ').next().unwrap_or_default();
                let key = format!("ant {channel}");
                if entry.text.contains('#') {
                    links.insert(key, format!("ant {} since {since}", entry.text));
                } else {
                    links.remove(&key);
                }
            }
            Kind::Ble if entry.text.starts_with("link ") => {
                links.insert("ble".into(), format!("ble {} since {since}", entry.text));
            }
            Kind::Gps => {
                links.insert("gps".into(), format!("gps {} since {since}", entry.text));
            }
            _ => {}
        }
    }
    for entry in timeline
        .entries
        .iter()
        .take_while(|e| e.at.session_ms <= at_ms)
        .filter(|e| e.kind == Kind::AntPacket)
    {
        let sensor = entry.text.split(' ').next().unwrap_or_default();
        packets.insert(sensor.into(), entry.at.session_ms);
    }
    let mut lines: Vec<String> = links.into_values().collect();
    for (sensor, last_ms) in packets {
        let silence = timeline.silences.iter().find(|s: &&Silence| {
            s.sensor == sensor && s.last_ms <= at_ms && s.next_ms.is_none_or(|next| next > at_ms)
        });
        lines.push(format!(
            "{sensor} last packet {} ({:.1}s before){}",
            session_clock(last_ms),
            (at_ms - last_ms) as f64 / 1000.0,
            match silence {
                Some(Silence {
                    next_ms: Some(next),
                    ..
                }) => format!(", silent until {}", session_clock(*next)),
                Some(_) => ", silent to the end".into(),
                None => String::new(),
            }
        ));
    }
    lines
}
