use super::*;
use crate::ride::log::transport_checksum;
use std::string::String;

fn run(
    runtime: &mut Runtime,
    journal: &mut Journal,
    now: u64,
    words: &str,
) -> (&'static str, String) {
    let mut output = String::new();
    let status = runtime.command(words, journal, &mut NoBulk, now, &mut output);
    (status, output)
}

/// A reply buffer with no room at all.
struct Full;
impl core::fmt::Write for Full {
    fn write_str(&mut self, _: &str) -> core::fmt::Result {
        Err(core::fmt::Error)
    }
}

#[test]
fn ride_commands_are_accepted_once_and_report_completion_once() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);

    // Initializing an already usable journal is refused without using the token.
    assert_eq!(
        run(&mut runtime, &mut journal, 0, "RIDE INIT"),
        ("STATE", "token=1".into())
    );
    assert_eq!(
        run(&mut runtime, &mut journal, 0, "RIDE START"),
        ("ACCEPTED", "token=1".into())
    );
    assert!(runtime.recording());
    assert_eq!(run(&mut runtime, &mut journal, 0, "RIDE PAUSE").0, "BUSY");

    tick(&mut runtime, &mut journal, &mut radio, 100);
    let (status, output) = run(&mut runtime, &mut journal, 100, "RIDE STATUS");
    assert_eq!(status, "OK");
    assert!(
        output.starts_with("state=recording slot=1 rides=0 "),
        "{output}"
    );
    assert!(
        output.ends_with("pending=0 completed=1 result=OK"),
        "{output}"
    );
    // Reading the status consumed the completion.
    let (_, output) = run(&mut runtime, &mut journal, 100, "RIDE STATUS");
    assert!(output.ends_with("pending=0"), "{output}");

    assert_eq!(
        run(&mut runtime, &mut journal, 200, "RIDE PAUSE"),
        ("ACCEPTED", "token=2".into())
    );
    let (_, output) = run(&mut runtime, &mut journal, 200, "RIDE STATUS");
    assert!(output.ends_with("pending=2"), "{output}");
}

#[test]
fn undersized_reply_leaves_the_command_unqueued() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    assert_eq!(
        runtime.command("RIDE START", &mut journal, &mut NoBulk, 0, &mut Full),
        "OUTPUT"
    );
    assert!(!runtime.recording());
    assert_eq!(
        run(&mut runtime, &mut journal, 0, "RIDE START"),
        ("ACCEPTED", "token=1".into())
    );
}

#[test]
fn malformed_commands_are_invalid_without_side_effects() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    for words in [
        "",
        "RIDE",
        "RIDE STATUS NOW",
        "RIDE START A B C",
        "RIDE CLEAR CONFIRM",
        "RIDE CLEAR CONFIRM many",
        "RIDE CLEAR NOW 3",
        "RADAR SENSORS 1",
        "RADAR NOW",
        "EXPORT SLOT first",
        "EXPORT INFO 1",
    ] {
        assert_eq!(
            run(&mut runtime, &mut journal, 0, words),
            ("INVALID", String::new()),
            "{words:?}"
        );
    }
    assert!(!runtime.recording());
}

#[test]
fn export_returns_checksummed_slots_within_the_journal() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    assert_eq!(
        run(&mut runtime, &mut journal, 0, "EXPORT INFO"),
        ("OK", "INFO 1 256 0 ready".into())
    );
    assert_eq!(
        run(&mut runtime, &mut journal, 0, "EXPORT SLOT 0").0,
        "BOUNDS"
    );

    run(&mut runtime, &mut journal, 0, "RIDE START");
    tick(&mut runtime, &mut journal, &mut radio, 100);
    // An open ride is still being written.
    assert_eq!(
        run(&mut runtime, &mut journal, 100, "EXPORT INFO").0,
        "BUSY"
    );
    run(&mut runtime, &mut journal, 100, "RIDE STATUS");
    run(&mut runtime, &mut journal, 2000, "RIDE FINISH");
    tick(&mut runtime, &mut journal, &mut radio, 2000);

    let (status, info) = run(&mut runtime, &mut journal, 2000, "EXPORT INFO");
    assert_eq!(status, "OK");
    let slots = info
        .strip_prefix("INFO 1 256 ")
        .and_then(|rest| rest.strip_suffix(" saved"))
        .and_then(|slots| slots.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("{info}"));
    assert!(slots >= 2, "start and finish are separate records");
    let (status, output) = run(&mut runtime, &mut journal, 2000, "EXPORT SLOT 0");
    assert_eq!(status, "OK");
    let slot = journal.slot(0);
    let mut expected = std::format!("SLOT 0 {:08x} ", transport_checksum(&slot.0));
    for byte in slot.0 {
        expected.push_str(&std::format!("{byte:02x}"));
    }
    assert_eq!(output, expected);
    let past_end = std::format!("EXPORT SLOT {slots}");
    assert_eq!(run(&mut runtime, &mut journal, 2000, &past_end).0, "BOUNDS");
}

#[test]
fn radar_sensors_reports_only_recent_ant_readings() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(120, 0);
    radio.data(120, [0, 0, 0, 0, 0x34, 0x12, 7, 72], 1000);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(
        run(&mut runtime, &mut journal, 3999, "RADAR SENSORS"),
        (
            "OK",
            "heart=Some((HeartRate { beat_event_time: 4660, beat_count: 7, bpm: Some(72) }, 1000)) power=None".into()
        )
    );
    assert_eq!(
        run(&mut runtime, &mut journal, 4000, "RADAR SENSORS"),
        ("OK", "heart=None power=None".into())
    );
}

#[test]
fn ride_status_reports_the_page_and_home_selection() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = crate::screens::workout::Page::Home;
    runtime.navigation.home_cursor = 2;
    let mut out = std::string::String::new();
    assert_eq!(
        runtime.command("RIDE STATUS", &mut journal, &mut NoBulk, 0, &mut out),
        "OK"
    );
    assert!(out.contains(" page=home home_cursor=2 pending=0"), "{out}");
    // A power sheet covers Home: a harness must not press through it.
    runtime.input(
        Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
        1_000,
        &mut radio,
    );
    out.clear();
    runtime.command("RIDE STATUS", &mut journal, &mut NoBulk, 1_000, &mut out);
    assert!(out.contains(" page=power "), "{out}");
}
