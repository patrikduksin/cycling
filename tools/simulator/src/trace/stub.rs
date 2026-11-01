//! A regression test, ready to paste, that replays a trace to one moment and
//! asserts what the recording showed then. The asserts state what the rider
//! saw; the author changes the wrong ones to what the rider should have
//! seen, and the test fails until the fix lands.
use crate::trace::timeline::{Timeline, field};

/// Where `tools/simulator/tests` keeps traces, and how a test names one.
pub const FIXTURES: &str = "tools/simulator/tests/traces";

/// Units the main ride page writes after a reading, by shown field.
const UNITS: [(&str, &str); 3] = [("speed", "KM/H"), ("power", "W"), ("hr", "BPM")];

pub fn stub(timeline: &Timeline, fixture: &str, name: &str, at_ms: u64) -> String {
    let moment = timeline.moment(at_ms);
    let seen = timeline.seen_at(at_ms);
    let mut asserts = Vec::new();
    if let Some((_, page)) = &seen.page {
        asserts.push(format!(
            "    assert_eq!(seen.page.map(|(_, page)| page).as_deref(), Some({page:?}));"
        ));
    }
    match &seen.toast {
        Some((_, toast)) => {
            asserts.push(format!(
                "    assert_eq!(seen.toast.map(|(_, toast)| toast).as_deref(), Some({toast:?}));"
            ));
            asserts.push(format!(
                "    assert!(sim.screen().contains({toast:?}), \"toast on screen\");"
            ));
        }
        None => asserts.push("    assert_eq!(seen.toast, None);".into()),
    }
    if let Some((_, state)) = &seen.ride {
        asserts.push(format!(
            "    assert_eq!(seen.ride.map(|(_, state)| state).as_deref(), Some({state:?}));"
        ));
    }
    let on_main = seen
        .page
        .as_ref()
        .is_some_and(|(_, page)| page == "ride/main");
    if let Some((_, shown)) = &seen.shown {
        asserts
            .push("    let shown = seen.shown.map(|(_, shown)| shown).unwrap_or_default();".into());
        for (name, unit) in UNITS {
            let Some(value) = field(shown, name) else {
                continue;
            };
            asserts.push(format!(
                "    assert_eq!(field(&shown, {name:?}), Some({value:?}));"
            ));
            // A number glides on screen between samples; dashes do not.
            if on_main && value == "--" {
                asserts.push(format!(
                    "    assert_eq!(sim.screen().field({unit:?}).as_deref(), Some(\"--\"), \"{name} on screen\");"
                ));
            }
        }
        asserts.push(format!("    // shown then: {shown}"));
    }
    format!(
        r#"/// Replays {fixture}.trace to {moment}, where the rider reported the
/// problem. Recorded by `cycling trace test-stub`; each assert states what the
/// recording showed. Change the wrong ones to what the rider should have seen.
#[test]
fn {name}() {{
    let (capture, _) = shell_simulator::trace::file::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/traces/{fixture}.trace"
    )))
    .expect("the fixture loads");
    let mut sim = Sim::replaying(&capture, shell_simulator::replay::Mode::Stimulus, None);
    assert!(sim.replay_until({at_ms}), "the trace reaches {moment}");
    let seen = sim.seen();
    use shell_simulator::trace::timeline::field;
{asserts}
}}
"#,
        asserts = asserts.join("\n")
    )
}
