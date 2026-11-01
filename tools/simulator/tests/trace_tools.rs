#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::ant::device_type;
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

#[test]
fn each_note_names_the_cue_it_played() {
    let timeline = timeline("laps-and-a-menu");
    let heard: Vec<&str> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Sound)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        heard[..6],
        [
            "pattern 0: start (1 of 2)",
            "pattern 10: start (2 of 2)",
            "pattern 0: lap",
            "pattern 0: lap",
            "pattern 10: pause (1 of 2)",
            "pattern 0: pause (2 of 2)",
        ],
        "{heard:#?}"
    );
    assert!(
        heard
            .last()
            .unwrap()
            .contains("the start of saved or low battery"),
        "{heard:#?}"
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
    assert_eq!(to_fixture(&fixture, &capture, media), None);

    let dir = std::env::temp_dir().join(format!("trace-stub-{}", std::process::id()));
    let outside = dir.join("morning.trace");
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.advance(seconds(2));
    file::save(&sim.capture(), &outside).unwrap();
    let (capture, media) = file::load(&outside).unwrap();
    let steps = to_fixture(&outside, &capture, media).unwrap();
    assert!(steps.contains("holds no Wi-Fi password"), "{steps}");
    assert!(
        steps.contains("tools/simulator/tests/traces/morning.trace\n"),
        "{steps}"
    );
    assert!(
        steps.contains("--test trace_tools no_committed_fixture"),
        "{steps}"
    );

    std::fs::remove_file(file::origin_path(&outside)).unwrap();
    let (capture, media) = file::load(&outside).unwrap();
    let steps = to_fixture(&outside, &capture, media).unwrap();
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
    let mine = dir.join("notes.txt");
    std::fs::write(&mine, "the rider's notes").unwrap();
    let refused = cycling_replay(&dir, Some(&dir));
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

include!("../src/build_sources.rs");

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
    let edited = !git(&[
        &["status", "--porcelain", "--untracked-files=no", "--"][..],
        BUILD_SOURCES,
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
