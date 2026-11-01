//! The agent's trace tools on recorded rides: the fixtures under
//! `tests/traces/` and what `cycling trace` reads from them.
//!
//! Refresh the fixtures after a change to what VANA reads or shows:
//! `cargo test -p shell-simulator --features cycling,debug-harness --test trace_tools -- --ignored`
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

/// Ride over a hill: the climb page comes up on the way up; past the top
/// and down the descent the power meter drops out for half a minute, then
/// comes back. Paused at a junction, then saved.
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

/// Ride at a steady effort until `metres` along the route.
fn ride_to(sim: &mut Sim, metres: f64, speed_kmh: f64, power_w: f64, cadence_rpm: f64) {
    sim.ride_at(speed_kmh, power_w, 0.0, cadence_rpm);
    while sim.world().distance_m < metres {
        sim.advance(seconds(1));
    }
}

/// Boot, look at the sensors, start a ride and lap it twice.
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

/// Replays power-dropout-after-descent.trace to 03:22.660 ride 03:00, where the rider reported the
/// problem. Recorded by `cycling trace test-stub`; each assert states what the
/// recording showed. Change the wrong ones to what the rider should have seen.
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
    // shown then: time=0:02:59 speed=30.0 power=-- hr=-- cad=86 dist=1.33 grade=0.0%
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
