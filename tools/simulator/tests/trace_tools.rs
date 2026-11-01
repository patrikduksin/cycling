#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::recording;
use shell_simulator::replay::Mode;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::trace::file;
use shell_simulator::trace::find::{Found, Query, find};
use shell_simulator::trace::story::tell;
use shell_simulator::trace::timeline::{At, Kind, Timeline, read};
use std::path::{Path, PathBuf};

const POWER: u16 = 1234;
const SPEED: u16 = 777;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/traces")
        .join(format!("{name}.trace"))
}

fn power_dropout_after_descent(sim: &mut Sim) {
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ride_route(Route::profile(
        Point::new(-33.45, -70.66),
        &[
            (0.0, 600.0),
            (200.0, 600.0),
            (800.0, 640.0),
            (1200.0, 600.0),
            (3000.0, 600.0),
        ],
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    ride_to(sim, 200.0, 32.0, 220.0, 88.0);
    ride_to(sim, 800.0, 20.0, 300.0, 78.0);
    ride_to(sim, 1200.0, 45.0, 0.0, 0.0);
    sim.ride_at(30.0, 200.0, 0.0, 86.0);
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(25));
    sim.set_present(device_type::POWER, POWER, true);
    sim.advance(seconds(15));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(6));
    sim.pause_ride();
    sim.advance(seconds(3));
    sim.stop_ride();
}

fn ride_to(sim: &mut Sim, metres: f64, speed_kmh: f64, power_w: f64, cadence_rpm: f64) {
    sim.ride_at(speed_kmh, power_w, 0.0, cadence_rpm);
    while sim.world().distance_m < metres {
        sim.advance(seconds(1));
    }
}

fn laps_and_a_menu(sim: &mut Sim) {
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(30.0, 210.0, 0.0, 90.0);
    sim.advance(seconds(40));
    sim.press(Button::TopLeft);
    sim.advance(seconds(30));
    sim.press(Button::TopLeft);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.advance(seconds(5));
    sim.stop_ride();
}

#[test]
#[ignore = "writes tests/traces; run after VANA changes what it reads or shows"]
fn record_trace_fixtures() {
    type Scenario = fn(&mut Sim);
    let scenarios: [(&str, Scenario); 2] = [
        ("power-dropout-after-descent", power_dropout_after_descent),
        ("laps-and-a-menu", laps_and_a_menu),
    ];
    for (name, scenario) in scenarios {
        let mut sim = Sim::new().boot();
        sim.discard_recording();
        scenario(&mut sim);
        file::save(&sim.capture(), &fixture(name)).expect("the fixture saves");
    }
}

#[test]
fn no_committed_fixture_holds_a_wifi_password_or_a_phone_token() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("the fixtures directory") {
        let path = entry.expect("a fixture").path();
        if path.extension().is_some_and(|ext| ext == "trace") {
            let (capture, _) = file::load(&path).expect("the fixture loads");
            let found = shell_simulator::trace::secret::secrets(&capture);
            assert!(
                found.is_empty(),
                "{} holds secrets: {found:?}. Record fixtures from a simulator scenario, never from a device",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixtures in {}", dir.display());
}

#[test]
fn secrets_finds_a_wifi_password_a_phone_token_and_saved_credentials() {
    use shell_simulator::trace::secret::secrets;
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.advance(seconds(5));
    sim.device
        .command(
            b"CMD 1 WIFI CONFIG WPA2 54657374 70617373776f7264",
            &mut String::new(),
        )
        .expect("the command parses");
    sim.advance(seconds(1));
    sim.connect_phone();
    sim.phone_request(device_api::server::Method::Get, "/api/workouts", b"");
    let found = secrets(&sim.capture());
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].ends_with("a Wi-Fi password"), "{found:?}");
    assert!(found[1].ends_with("a phone session token"), "{found:?}");

    sim.device
        .power_cycle()
        .expect("the device powers on again");
    sim.advance(seconds(5));
    assert_eq!(
        secrets(&sim.capture()),
        ["the origin's settings: saved Wi-Fi credentials"]
    );
}

fn timeline(name: &str) -> Timeline {
    let (capture, _) = file::load(&fixture(name)).expect("the fixture loads");
    read(&capture.trace).expect("the fixture decodes")
}

#[test]
fn the_dropout_shows_as_a_stretch_of_dashed_power_after_the_descent() {
    let timeline = timeline("power-dropout-after-descent");
    let found = find(
        &timeline,
        &Query::parse("power=--"),
        (
            timeline.session_ms(At::Ride(seconds(10))).unwrap(),
            u64::MAX,
        ),
        None,
    );
    let [
        Found::Stretch {
            from, to: Some(to), ..
        },
    ] = found.as_slice()
    else {
        panic!("one stretch of dashes after the ride got going: {found:?}");
    };
    let held = to.session_ms - from.session_ms;
    assert!((15_000..=35_000).contains(&held), "dashes for {held} ms");
    let descent_end = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Shown)
        .find(|e| e.text.contains("speed=45"))
        .expect("the descent shows 45 km/h");
    assert!(from.session_ms > descent_end.at.session_ms);
}

#[test]
fn the_meter_out_of_range_is_one_silence_between_its_last_and_first_packet() {
    let timeline = timeline("power-dropout-after-descent");
    let [silence] = timeline.silences.as_slice() else {
        panic!("only the power meter went quiet: {:?}", timeline.silences);
    };
    assert_eq!(silence.sensor, format!("power#{POWER}"));
    let next_ms = silence.next_ms.expect("the meter came back");
    let silent = next_ms - silence.last_ms;
    assert!(
        (24_500..=25_500).contains(&silent),
        "silent for {silent} ms"
    );
    let story = tell(&timeline);
    let span = story
        .spans
        .iter()
        .find(|s| s.name == format!("ant power#{POWER} silent"))
        .expect("a span for the silence");
    assert_eq!(
        (span.start.session_ms, span.end.session_ms),
        (silence.last_ms, next_ms)
    );
}

#[test]
fn the_save_warning_is_storage_and_belongs_to_the_ride_it_ended_with() {
    let timeline = timeline("power-dropout-after-descent");
    let found = find(
        &timeline,
        &Query::parse("storage"),
        (0, u64::MAX),
        Some("save_failed"),
    );
    let [Found::Entry(warning)] = found.as_slice() else {
        panic!("one save warning under storage: {found:?}");
    };
    let story = tell(&timeline);
    let event = story
        .events
        .iter()
        .find(|e| e.at == warning.at && e.name == "storage")
        .expect("the warning is an event");
    let ride = &story.spans[event.span];
    assert_eq!(ride.name, "ride");
    assert!(ride.attributes.contains(&("warnings".into(), "1".into())));
    assert!(
        event.alongside.contains(&"ride ended saved".to_string()),
        "{:?}",
        event.alongside
    );
}

#[test]
fn the_origin_of_a_later_power_on_names_the_ride_and_sensors_saved_before_it() {
    let mut sim = Sim::new();
    sim.fit_bulk(shell_simulator::bulk::Presence::Ready);
    let mut sim = sim.boot();
    sim.discard_recording();
    laps_and_a_menu(&mut sim);
    sim.device
        .power_cycle()
        .expect("the device powers on again");
    sim.advance(seconds(5));
    let text =
        shell_simulator::trace::origin::describe(&sim.capture(), file::Media::Saved, "x.origin");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].ends_with("power-on 2"), "{text}");
    assert!(
        lines[2].contains(&format!("sensors power#{POWER}")),
        "{text}"
    );
    assert!(lines[2].ends_with("setup done"), "{text}");
    assert!(
        lines[3].starts_with("rides on internal flash: ride 1 (1:"),
        "{text}"
    );
    assert!(lines[4].starts_with("bulk card: ready, "), "{text}");
}

#[test]
fn the_story_holds_the_ride_its_pause_and_the_climb_page() {
    let timeline = timeline("power-dropout-after-descent");
    let story = tell(&timeline);
    let names: Vec<&str> = story.spans.iter().map(|s| s.name.as_str()).collect();
    for name in [
        "session",
        "ride",
        "ride recording",
        "ride paused",
        "page ride/climb",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    let ride = story.spans.iter().find(|s| s.name == "ride").unwrap();
    assert!(!ride.open && ride.attributes.contains(&("ended".into(), "saved".into())));
    assert!(
        story
            .spans
            .iter()
            .any(|s| s.name.starts_with("ant power#1234") && s.name.contains("stale")),
        "{names:?}"
    );
}

#[test]
fn lap_toasts_are_events_and_ride_time_stops_while_paused() {
    let timeline = timeline("laps-and-a-menu");
    let story = tell(&timeline);
    let laps: Vec<&str> = story
        .events
        .iter()
        .filter(|e| e.name == "lap")
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(laps.len(), 2, "{laps:?}");
    let ride = story.spans.iter().find(|s| s.name == "ride").unwrap();
    assert!(ride.attributes.contains(&("laps".into(), "2".into())));
    let paused = story
        .spans
        .iter()
        .find(|s| s.name == "ride paused")
        .unwrap();
    let before_saving = timeline.moment(paused.end.session_ms - 1);
    assert!(paused.end.session_ms - paused.start.session_ms > 1_000);
    assert_eq!(
        paused.start.ride_ms, before_saving.ride_ms,
        "the ride clock stops"
    );
}

#[test]
fn every_press_of_the_same_button_is_found() {
    let timeline = timeline("laps-and-a-menu");
    let laps: Vec<u64> = tell(&timeline)
        .events
        .iter()
        .filter(|e| e.name == "lap")
        .map(|e| e.at.session_ms)
        .collect();
    let presses: Vec<u64> = find(
        &timeline,
        &Query::parse("input"),
        (0, u64::MAX),
        Some("TopLeft"),
    )
    .iter()
    .filter_map(|found| match found {
        Found::Entry(entry) => Some(entry.at.session_ms),
        Found::Stretch { .. } => None,
    })
    .collect();
    assert_eq!(laps.len(), 2);
    for lap in laps {
        assert!(presses.contains(&lap), "a press at {lap} in {presses:?}");
    }
}

#[test]
fn a_page_names_what_brought_it_up_and_the_readings_it_shows() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(2));
    sim.press(Button::BottomLeft);
    sim.advance(1_400);
    sim.press(Button::BottomLeft);
    sim.advance(seconds(1));
    let pressed = read(&sim.capture().trace).unwrap();
    let pages: Vec<String> = pressed
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Page)
        .map(|e| {
            e.to_string()
                .split_once("page        ")
                .unwrap()
                .1
                .to_string()
        })
        .collect();
    let last_two = &pages[pages.len() - 2..];
    assert_eq!(
        last_two,
        [
            "ride/stats  (after button BottomLeft code 1; shows dist)",
            "ride/lap  (after button BottomLeft code 1; shows none)"
        ],
        "{pages:?}"
    );
    assert!(pages[0].ends_with("(at power-on)"), "{pages:?}");

    let climb = tell(&timeline("power-dropout-after-descent"))
        .spans
        .into_iter()
        .find(|s| s.name == "page ride/climb")
        .unwrap();
    assert!(
        climb
            .attributes
            .contains(&("cause".into(), "by itself".into())),
        "{climb}"
    );
}

#[cfg(feature = "debug-harness")]
#[test]
fn a_page_a_harness_press_brought_up_names_the_press() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(2));
    let (status, opened) = sim.command("HARNESS OPEN 1 60000");
    assert_eq!(status, "OK", "{opened}");
    let session = opened
        .split_whitespace()
        .find_map(|word| word.strip_prefix("session="))
        .unwrap()
        .to_string();
    for command in ["INPUT BEGIN 1", "INPUT ADD 1 0 BUTTON 1", "INPUT RUN 1"] {
        let (status, out) = sim.command(&format!("HARNESS {session} {command}"));
        assert!(
            matches!(status, "OK" | "ACCEPTED"),
            "{command}: {status} {out}"
        );
    }
    sim.advance(seconds(1));
    let trace = read(&sim.capture().trace).unwrap();
    let page = trace
        .entries
        .iter()
        .rfind(|e| e.kind == Kind::Page)
        .unwrap()
        .to_string();
    assert!(
        page.ends_with("ride/stats  (after synthetic button BottomLeft code 1; shows dist)"),
        "{page}"
    );
}

#[test]
fn each_note_names_the_cue_vana_played_it_for() {
    let timeline = timeline("laps-and-a-menu");
    let heard: Vec<&str> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Sound)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        heard,
        [
            "pattern 0: start (1 of 2)",
            "pattern 10: start (2 of 2)",
            "pattern 0: lap",
            "pattern 0: lap",
            "pattern 10: pause (1 of 2)",
            "pattern 0: pause (2 of 2)",
            "pattern 0: still paused (1 of 3)",
            "pattern 0: still paused (2 of 3)",
            "pattern 21: saved (1 of 3)",
        ]
    );
}

#[test]
fn otlp_export_is_one_traces_and_one_metrics_request() {
    let timeline = timeline("power-dropout-after-descent");
    let json = shell_simulator::trace::otlp::export(&timeline, &tell(&timeline));
    let lines: Vec<&str> = json.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0]
            .starts_with(r#"{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name""#)
    );
    assert!(lines[1].starts_with(r#"{"resourceMetrics":"#));
    assert!(lines[1].contains(r#""name":"vana.shown.power""#));
    assert!(
        lines[1].contains(r#""flags":1"#),
        "dashed power has no value"
    );
}

#[test]
fn power_shows_dashes_while_the_meter_is_out() {
    let (capture, _) = shell_simulator::trace::file::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/traces/power-dropout-after-descent.trace"
    )))
    .expect("the fixture loads");
    let mut sim = Sim::replaying(&capture, shell_simulator::replay::Mode::Stimulus, None);
    assert!(
        sim.replay_until(202660),
        "the trace reaches 03:22.660 ride 03:00"
    );
    let seen = sim.seen();
    use shell_simulator::trace::timeline::field;
    assert_eq!(
        seen.page.map(|(_, page)| page).as_deref(),
        Some("ride/main")
    );
    assert_eq!(seen.toast, None);
    assert_eq!(
        seen.ride.map(|(_, state)| state).as_deref(),
        Some("recording")
    );
    let shown = seen.shown.map(|(_, shown)| shown).unwrap_or_default();
    assert_eq!(field(&shown, "speed"), Some("30.0"));
    assert_eq!(field(&shown, "power"), Some("--"));
    assert_eq!(
        sim.screen().field("W").as_deref(),
        Some("--"),
        "power on screen"
    );
    assert_eq!(field(&shown, "hr"), Some("--"));
    assert_eq!(
        sim.screen().field("BPM").as_deref(),
        Some("--"),
        "hr on screen"
    );
}

#[test]
fn a_test_stub_never_asserts_the_symptom_it_is_told_to_expect_away() {
    use shell_simulator::trace::stub::{Expect, stub};
    let timeline = timeline("power-dropout-after-descent");
    let at = timeline.session_ms(At::Ride(seconds(170))).unwrap();
    let not_dashed = [Expect::parse("power!=--").unwrap()];
    let test = stub(&timeline, "dropout", "power_shows", at, &not_dashed).unwrap();
    let (characterization, expectation) = test.split_once("// Expectation").unwrap();
    assert!(
        !characterization.contains(r#"field(&shown, "power")"#),
        "{test}"
    );
    assert!(
        expectation.contains(r#"assert_ne!(field(&shown, "power"), Some("--")"#),
        "{test}"
    );
    assert!(
        characterization.contains(&format!("power#{POWER} last packet")),
        "{test}"
    );
    let unchanged = [Expect::parse("power=--").unwrap()];
    assert!(stub(&timeline, "dropout", "power_shows", at, &unchanged).is_err());
    let open = stub(&timeline, "dropout", "power_shows", at, &[]).unwrap();
    assert!(open.contains("todo!("), "{open}");
}

#[test]
fn a_trace_outside_the_fixtures_gets_the_steps_that_make_it_one() {
    use shell_simulator::trace::stub::to_fixture;
    let fixture = fixture("laps-and-a-menu");
    let (capture, media) = file::load(&fixture).unwrap();
    assert_eq!(to_fixture(&fixture, &capture, &media), None);

    let dir = std::env::temp_dir().join(format!("trace-stub-{}", std::process::id()));
    let outside = dir.join("morning.trace");
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.advance(seconds(2));
    file::save(&sim.capture(), &outside).unwrap();
    let (capture, media) = file::load(&outside).unwrap();
    let steps = to_fixture(&outside, &capture, &media).unwrap();
    assert!(steps.contains("holds no Wi-Fi password"), "{steps}");
    assert!(
        steps.contains("tools/simulator/tests/traces/morning.trace\n"),
        "{steps}"
    );
    assert!(
        steps.contains("--test trace_tools no_committed_fixture"),
        "{steps}"
    );

    file::save_rebuilt(&capture.origin, "file bests.dat: gone\n", &outside).unwrap();
    let (capture, media) = file::load(&outside).unwrap();
    assert_eq!(media, file::Media::Rebuilt("file bests.dat: gone\n".into()));
    let steps = to_fixture(&outside, &capture, &media).unwrap();
    assert!(steps.contains("device trace"), "a rebuilt origin: {steps}");

    std::fs::remove_file(file::origin_path(&outside)).unwrap();
    let (capture, media) = file::load(&outside).unwrap();
    let steps = to_fixture(&outside, &capture, &media).unwrap();
    assert!(steps.contains("device trace"), "{steps}");
    assert!(!steps.contains("cp "), "{steps}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stimulus_replay_of_a_fixture_shows_what_the_recording_showed() {
    let (capture, _) = file::load(&fixture("laps-and-a-menu")).unwrap();
    let sim = Sim::replay_as(&capture, Mode::Stimulus, |_| {});
    let report = sim.device.report().unwrap();
    assert_eq!(
        report.count(shell_simulator::replay::Kind::Seen),
        0,
        "{:?}",
        report.first
    );
}

fn cycling_replay(cwd: &Path, out: Option<&Path>) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_trace"));
    command
        .current_dir(cwd)
        .arg("replay")
        .arg(fixture("laps-and-a-menu"))
        .args(["--to", "3s", "--every", "1s"]);
    if let Some(out) = out {
        command.arg("--out").arg(out);
    }
    command.output().expect("the trace tool runs")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn replay_never_deletes_a_directory_it_did_not_write() {
    let dir = scratch("replay-out");
    let theirs = dir.join("theirs");
    std::fs::create_dir(&theirs).unwrap();
    let mine = theirs.join("notes.txt");
    std::fs::write(&mine, "the rider's notes").unwrap();
    let refused = cycling_replay(&dir, Some(&theirs));
    assert!(!refused.status.success(), "{refused:?}");
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(error.contains("did not write"), "{error}");
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), "the rider's notes");

    let empty = dir.join("empty");
    std::fs::create_dir(&empty).unwrap();
    for run in ["into an empty directory", "over its own output"] {
        let replayed = cycling_replay(&dir, Some(&empty));
        assert!(
            replayed.status.success(),
            "{run}: {}",
            String::from_utf8_lossy(&replayed.stderr)
        );
        assert!(empty.join("divergence.txt").exists(), "{run}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn replay_never_claims_or_replaces_the_directory_it_runs_in() {
    let cwd = scratch("replay-cwd");
    let parent_of_cwd = cwd.join("inside");
    std::fs::create_dir(&parent_of_cwd).unwrap();
    let mine = cwd.join("notes.txt");
    std::fs::write(&mine, "the rider's notes").unwrap();

    let empty = cycling_replay(&cwd, Some(Path::new("")));
    assert!(!empty.status.success(), "{empty:?}");
    assert!(!cwd.join(recording::MARKER).exists());

    std::fs::write(cwd.join(recording::MARKER), "").unwrap();
    for out in [Path::new("."), cwd.as_path()] {
        let refused = cycling_replay(&cwd, Some(out));
        assert!(!refused.status.success(), "{}: {refused:?}", out.display());
    }
    for out in ["..", "nope/../..", "gone/../../inside/.."] {
        let refused = cycling_replay(&parent_of_cwd, Some(Path::new(out)));
        assert!(!refused.status.success(), "{out}: {refused:?}");
    }
    std::fs::remove_file(cwd.join(recording::MARKER)).unwrap();
    let refused = cycling_replay(&cwd, Some(Path::new("nope/..")));
    assert!(!refused.status.success(), "nope/..: {refused:?}");
    assert!(
        !cwd.join(recording::MARKER).exists(),
        "nope/.. marked the working directory"
    );
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), "the rider's notes");
    std::fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn replay_guards_its_default_directory_the_same_way() {
    let cwd = scratch("replay-default");
    let first = cycling_replay(&cwd, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let replay = cwd.join(".local/replay");
    let out = std::fs::read_dir(&replay)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    for entry in std::fs::read_dir(&out).unwrap() {
        let path = entry.unwrap().path();
        if !path.ends_with("divergence.txt") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let refused = cycling_replay(&cwd, None);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(out.join("divergence.txt").exists());
    std::fs::remove_dir_all(&cwd).unwrap();
}

#[test]
fn the_build_stamp_is_this_checkouts_commit_and_says_when_it_was_edited() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let head = git(&["rev-parse", "HEAD"]);
    let sources = shell_simulator::session::build_sources();
    let edited = !git(&[
        &["status", "--porcelain", "--untracked-files=no", "--"][..],
        &sources,
    ]
    .concat())
    .is_empty();
    let expected = format!("{head}{}", if edited { "-dirty" } else { "" });
    assert_eq!(option_env!("CYCLING_BUILD_COMMIT"), Some(expected.as_str()));
}

#[test]
fn timeline_and_find_never_print_the_phone_token() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.connect_phone();
    let token = sim
        .device
        .sdk
        .phone_status()
        .unwrap()
        .token
        .text()
        .to_string();
    let (status, _) = sim.phone_request(
        device_api::server::Method::Get,
        &format!("/?t={token}&start=5"),
        b"",
    );
    assert_eq!(status, 200);
    let timeline = read(&sim.capture().trace).unwrap();
    let asked: Vec<&str> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Phone && e.text.contains("server holds #"))
        .map(|e| e.text.as_str())
        .collect();
    assert!(
        asked
            .iter()
            .any(|text| text.contains("/?t=(not shown)&start=5")),
        "{asked:#?}"
    );
    let printed: Vec<&str> = timeline
        .entries
        .iter()
        .map(|e| e.text.as_str())
        .filter(|text| text.contains(&token))
        .collect();
    assert!(printed.is_empty(), "{printed:#?}");
    let found = find(&timeline, &Query::parse(&token), (0, u64::MAX), None);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn the_ride_clock_reads_what_the_rider_saw_after_the_card_fails() {
    let mut sim = Sim::new();
    sim.fit_bulk(shell_simulator::bulk::Presence::Ready);
    let mut sim = sim.boot();
    sim.discard_recording();
    sim.ant_sensor(device_type::HEART_RATE, 4242);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.advance(seconds(120));
    sim.fail_bulk(0, 100);
    sim.advance(seconds(60));
    let timeline = read(&sim.capture().trace).unwrap();
    use shell_simulator::trace::timeline::field;
    let off: Vec<String> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Shown)
        .filter_map(|e| {
            let shown = field(&e.text, "time")?;
            let ride = e.at.ride_ms.map(|ms| {
                let s = ms / 1000;
                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            });
            (ride.as_deref() != Some(shown)).then(|| format!("{e}"))
        })
        .collect();
    assert!(
        off.is_empty(),
        "{} off, first {:#?}",
        off.len(),
        &off[..off.len().min(4)]
    );
    let states: Vec<&str> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Ride)
        .map(|e| e.text.as_str())
        .collect();
    assert!(
        states.len() > 1,
        "the fallback shows in the ride states: {states:?}"
    );
}

fn fresh_file_faults(test: &str) -> Vec<String> {
    let imported: Vec<&str> = test
        .lines()
        .filter_map(|line| line.trim().strip_prefix("use ")?.strip_suffix(';'))
        .filter_map(|path| path.rsplit("::").next())
        .collect();
    let code: String = test
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    let used = |name: &str| match name {
        "Sim" => code.contains("Sim::"),
        "field" => code.contains("field(&shown"),
        other => panic!("the stub imports {other}"),
    };
    let mut faults: Vec<String> = imported
        .iter()
        .filter(|name| !used(name))
        .map(|name| format!("imports {name} unused"))
        .collect();
    for name in ["Sim", "field"] {
        if used(name) && !imported.contains(&name) {
            faults.push(format!("uses {name} without importing it"));
        }
    }
    if !test.contains("#[cfg(feature = \"cycling\")]\n#[test]") {
        faults.push("runs in builds without VANA".into());
    }
    faults
}

#[test]
fn a_test_stub_compiles_in_a_fresh_file_whatever_it_expects() {
    use shell_simulator::trace::stub::{Expect, stub};
    let timeline = timeline("power-dropout-after-descent");
    let riding = timeline.session_ms(At::Ride(seconds(170))).unwrap();
    let before_the_ride = 2_000;
    for (at, expect) in [
        (riding, "page=ride/climb"),
        (before_the_ride, "page=ride/main"),
        (before_the_ride, "toast~SAVED,ride=recording"),
        (riding, "power!=--"),
    ] {
        let expect: Vec<Expect> = expect
            .split(',')
            .map(|e| Expect::parse(e).unwrap())
            .collect();
        let test = stub(&timeline, "dropout", "stub", at, &expect).unwrap();
        assert_eq!(fresh_file_faults(&test), Vec::<String>::new(), "{test}");
    }
    let open = stub(&timeline, "dropout", "stub", riding, &[]).unwrap();
    assert_eq!(fresh_file_faults(&open), Vec::<String>::new(), "{open}");
}

#[test]
fn the_build_stamp_and_replay_watch_every_checkout_path_cargo_builds_from() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sources = shell_simulator::session::build_sources();
    let covered = |path: &str| {
        sources
            .iter()
            .any(|source| path == *source || path.starts_with(&format!("{source}/")))
    };
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let mut read: Vec<String> = manifest
        .lines()
        .filter_map(|line| line.split_once("path = \"")?.1.split_once('"'))
        .map(|(path, _)| path.to_string())
        .collect();
    assert!(
        !read.is_empty(),
        "the workspace patches a crate from the checkout"
    );
    read.push(".cargo/config.toml".into());
    read.extend(
        std::fs::read_dir(root.join("patches"))
            .unwrap()
            .map(|entry| format!("patches/{}", entry.unwrap().file_name().to_string_lossy())),
    );
    let tree = std::process::Command::new(env!("CARGO"))
        .current_dir(&root)
        .args([
            "tree",
            "-p",
            "shell-simulator",
            "--locked",
            "--offline",
            "--no-default-features",
            "--features",
            "cycling,debug-harness",
            "--prefix",
            "none",
            "-e",
            "normal,build",
        ])
        .output()
        .unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    for line in String::from_utf8_lossy(&tree.stdout).lines() {
        if let Some((path, _)) = line
            .split_once(" (")
            .and_then(|(_, rest)| rest.split_once(')'))
            && let Ok(local) = Path::new(path).strip_prefix(&root)
        {
            read.push(local.to_string_lossy().into_owned());
        }
    }
    let unwatched: Vec<&String> = read.iter().filter(|path| !covered(path)).collect();
    assert!(
        unwatched.is_empty(),
        "{unwatched:?} are not in build.rs's BUILD_SOURCES"
    );
}

fn source_files(dir: &Path, out: &mut String) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push_str(&std::fs::read_to_string(&path).unwrap());
        }
    }
}

#[test]
fn the_ride_trace_skill_matches_where_traces_come_from_and_what_shown_holds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skill = std::fs::read_to_string(root.join(".agents/skills/ride-trace/SKILL.md")).unwrap();
    let mut firmware = String::new();
    source_files(&root.join("devices/c606/src"), &mut firmware);
    if !firmware.contains("report_seen") {
        assert!(
            skill.contains("The device does not record traces yet") && skill.contains("phase 4"),
            "the C606 firmware records no trace, and the skill must say so"
        );
    }
    let mut pages = std::collections::BTreeSet::new();
    for name in ["power-dropout-after-descent", "laps-and-a-menu"] {
        let timeline = timeline(name);
        for entry in timeline.entries.iter().filter(|e| e.kind == Kind::Shown) {
            if let Some((_, page)) = timeline.seen_at(entry.at.session_ms).page {
                pages.insert(page);
            }
        }
    }
    assert!(
        pages.len() > 1,
        "shown is emitted on more than the main page: {pages:?}"
    );
    let listed = skill
        .split_once("every ride page (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map_or("", |(list, _)| list);
    for page in &pages {
        let page = page.strip_prefix("ride/").unwrap_or(page);
        assert!(
            listed.contains(page),
            "the skill lists {page} among the pages shown is emitted on: {listed:?}"
        );
    }
    assert!(
        skill.contains("before the glide"),
        "shown holds pre-glide values"
    );
}

/// Home, a ride, a pause and the end of the ride, in the simulator.
fn a_short_ride() -> Timeline {
    let mut sim = Sim::new().boot();
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(10));
    sim.start_ride();
    sim.ride_at(30.0, 210.0, 0.0, 90.0);
    sim.advance(seconds(20));
    sim.pause_ride();
    sim.advance(seconds(10));
    sim.stop_ride();
    read(&sim.trace()).expect("the trace decodes")
}

#[test]
fn the_summary_splits_activity_by_what_the_device_was_doing() {
    let timeline = a_short_ride();
    let summary = shell_simulator::trace::battery::summary(&timeline);
    let windows = timeline.activity.len();
    assert!(windows >= 8, "{windows} activity windows");
    for state in ["home idle", "riding", "paused"] {
        assert!(
            summary.contains(&format!("  {state} for ")),
            "{state} in {summary}"
        );
    }
    let console = summary
        .lines()
        .find_map(|line| line.trim().strip_prefix("task console: "))
        .unwrap_or_else(|| panic!("the console's wakeups in {summary}"));
    let rate = |state: &str| -> u32 {
        console
            .split(", ")
            .find_map(|part| {
                part.strip_prefix(state)?
                    .trim()
                    .strip_suffix("/s")?
                    .parse()
                    .ok()
            })
            .unwrap_or_else(|| panic!("{state} in {console}"))
    };
    // A ride keeps the busy pace. Home rests between passes once the
    // pairing scan is over, as in the last window before the ride.
    assert!(rate("riding") >= 95 && rate("paused") >= 95, "{console}");
    let ride = timeline
        .clock
        .iter()
        .find(|anchor| anchor.ride_ms.is_some())
        .expect("a ride");
    let home = timeline
        .activity
        .iter()
        .rev()
        .find(|window| window.end_ms <= ride.session_ms)
        .expect("a window on Home");
    let still = home
        .tasks
        .iter()
        .find(|task| task.name == "console")
        .map_or(0, |task| task.polls)
        * 1000
        / home.activity.window_ms;
    assert!((15..=25).contains(&still), "{still}/s on a still Home");
    assert!(summary.contains("battery flags: none"), "{summary}");
}

#[test]
fn dark_charging_standby_rests_between_passes() {
    use vana::app::{STANDBY_DARK_PASS_MS, STANDBY_LIT_MS};
    let mut sim = Sim::new().boot();
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "turned off into charging standby");
    sim.advance(STANDBY_LIT_MS + seconds(150));
    let timeline = read(&sim.trace()).expect("the trace decodes");
    let summary = shell_simulator::trace::battery::summary(&timeline);
    assert!(summary.contains("  charging standby for "), "{summary}");
    assert!(summary.contains("battery flags: none"), "{summary}");
    let dark: Vec<_> = timeline
        .activity
        .iter()
        .filter(|window| window.activity.consumers.backlight == 0)
        .collect();
    assert!(
        (2..=4).contains(&dark.len()),
        "dark standby records once a minute, {} windows",
        dark.len()
    );
    let (_going_dark, dark_minutes) = dark.split_first().expect("dark windows");
    for window in dark_minutes {
        let passes = window
            .tasks
            .iter()
            .find(|task| task.name == "console")
            .map_or(0, |task| task.polls);
        let most = u64::from(window.activity.window_ms) / STANDBY_DARK_PASS_MS + 1;
        assert!(
            u64::from(passes) <= most,
            "{passes} passes in a dark {} ms window",
            window.activity.window_ms
        );
    }
    sim.turn_on();
    sim.wait_for("READY TO RIDE", seconds(5)).unwrap();
}

#[test]
fn activity_and_health_are_found_by_kind_and_exported_per_task() {
    let timeline = a_short_ride();
    let found = find(
        &timeline,
        &Query::parse("activity"),
        (0, u64::MAX),
        Some("console 100/s"),
    );
    assert!(found.len() >= 2, "{found:?}");
    let health = find(&timeline, &Query::parse("health"), (0, u64::MAX), None);
    assert!(
        matches!(health.first(), Some(Found::Entry(entry)) if entry.text.starts_with("simulated, gps ")),
        "{health:?}"
    );
    let json = shell_simulator::trace::otlp::export(&timeline, &tell(&timeline));
    let metrics = json.lines().nth(1).expect("a metrics request");
    assert!(
        metrics.contains(
            r#""name":"vana.activity.task.wakeups","unit":"1/s","gauge":{"dataPoints":[{"#
        ),
        "{metrics}"
    );
    assert!(
        metrics.contains(
            r#""asDouble":100,"attributes":[{"key":"task","value":{"stringValue":"console"}}]"#
        ),
        "{metrics}"
    );
    assert!(
        metrics.contains(r#""name":"vana.health.gps.overflows""#),
        "{metrics}"
    );
    assert!(
        !metrics.contains(r#""name":"vana.health.heap_free""#),
        "the simulator knows no heap"
    );
}
