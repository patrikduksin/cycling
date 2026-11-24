use shell_simulator::trace::fixture::{self, Class, Expect, Fixture};
use std::path::{Path, PathBuf};

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn every_fixture_replays_to_what_the_rider_should_have_seen() {
    let public = fixture::under(&manifest().join("tests/traces"));
    let private_root = fixture::primary_checkout()
        .map(|root| root.join(fixture::PRIVATE))
        .unwrap();
    let private = fixture::under(&private_root);
    let mut failed = Vec::new();
    for (dir, kept) in (public.iter().map(|d| (d, Kept::Committed)))
        .chain(private.iter().map(|d| (d, Kept::Private)))
    {
        let verdict = Fixture::read(dir).unwrap().judge();
        match (verdict.class(), kept) {
            (Class::Passing, _) => {}
            (Class::Invalid, Kept::Private) => println!("{}: {verdict}", dir.display()),
            _ => failed.push(format!("{}: {verdict}", dir.display())),
        }
    }
    println!(
        "{} committed fixtures; {} private fixtures from {}",
        public.len(),
        private.len(),
        private_root.display()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[derive(Clone, Copy)]
enum Kept {
    Committed,
    Private,
}

const POWER_SILENT_AT_MS: u64 = 191_700;

fn dropout() -> Vec<u8> {
    std::fs::read(manifest().join("tests/traces/power-dropout-after-descent.trace")).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fixture-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_cut_fixture_fails_on_the_slice_and_reads_back_the_same() {
    let dir = scratch("cut");
    let power_shown = vec![Expect::parse("power!=--").unwrap()];
    let cut = fixture::cut(
        &dropout(),
        POWER_SILENT_AT_MS,
        power_shown,
        None,
        Vec::new(),
        &dir,
        "a test",
    )
    .unwrap();
    assert_eq!(cut.judged[0].got, "--", "the power meter was silent then");
    let fixture = Fixture::read(&dir).unwrap();
    assert_eq!(fixture.expect.at_ms, cut.at_ms);
    assert!(
        cut.at_ms <= POWER_SILENT_AT_MS && cut.at_ms + 1_000 > POWER_SILENT_AT_MS,
        "{}",
        cut.at_ms
    );
    assert_eq!(fixture.judge().class(), Class::Reproduced);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_cut_whose_expectation_already_holds_is_refused() {
    let dir = scratch("holds");
    let dashed = vec![Expect::parse("power=--").unwrap()];
    let refused = fixture::cut(
        &dropout(),
        POWER_SILENT_AT_MS,
        dashed,
        None,
        Vec::new(),
        &dir,
        "a test",
    )
    .unwrap_err();
    assert!(refused.contains("would pass on the slice"), "{refused}");
    let unseen = vec![Expect::parse("watts=200").unwrap()];
    let refused = fixture::cut(
        &dropout(),
        POWER_SILENT_AT_MS,
        unseen,
        None,
        Vec::new(),
        &dir,
        "a test",
    )
    .unwrap_err();
    assert!(refused.contains("saw no watts"), "{refused}");
    assert!(!dir.exists(), "nothing written");
}

#[test]
fn a_shrunk_fixture_is_smaller_still_fails_and_is_what_it_says_it_kept() {
    use shell_simulator::sim::{Sim, seconds};
    use shell_simulator::trace::shrink::{Size, shrink};
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.ant_sensor(shell_simulator::ant::device_type::POWER, 1234);
    sim.pair(&["POWER METER"]);
    sim.ride_at(25.0, 200.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(10));
    sim.set_present(shell_simulator::ant::device_type::POWER, 1234, false);
    sim.advance(seconds(10));
    let dir = scratch("shrink");
    let power_shown = vec![Expect::parse("power!=--").unwrap()];
    let end = sim.device.now;
    fixture::cut(
        &sim.capture(),
        end,
        power_shown,
        None,
        Vec::new(),
        &dir,
        "a test",
    )
    .unwrap();
    let shrunk = shrink(&dir, std::time::Duration::from_secs(4)).unwrap();
    println!("{shrunk}");
    let written = Fixture::read(&dir).unwrap();
    assert!(shrunk.after.bytes < shrunk.before.bytes, "{shrunk}");
    assert_eq!(Size::of(&written.slice), shrunk.after);
    assert_eq!(written.judge().class(), Class::Reproduced);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_lifted_slice_rides_the_sensors_and_track_it_recorded_from_power_on() {
    use shell_simulator::fuzz::lift::{lift, synthetic};
    use shell_simulator::fuzz::plan::{Course, Sensor, Start, Step};
    let lifted = lift(&dropout()).unwrap();
    let setup = &lifted.plan.setup;
    assert!(matches!(setup.start, Start::PowerOn(_)));
    assert!(
        setup.sensors.contains(&Sensor::Power),
        "{:?}",
        setup.sensors
    );
    let Course::Track { points } = &setup.course else {
        panic!("a track: {:?}", setup.course);
    };
    assert!(points.len() > 10);
    let steps = &lifted.plan.steps;
    assert!(
        steps.iter().any(|s| matches!(s, Step::Press(_))),
        "{steps:?}"
    );
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, Step::Ride { kmh_tenths, .. } if *kmh_tenths > 0))
    );
    let Course::Profile { knots, .. } = synthetic(&setup.course) else {
        panic!("a synthetic course");
    };
    let length = shell_simulator::route::Route::track(points).length_m();
    assert_eq!(f64::from(knots[1].0), length.round());
}

fn replayed(plan: &shell_simulator::fuzz::Plan) -> shell_simulator::sim::Sim {
    use shell_simulator::fuzz::script::{Road, calls};
    let mut sim = shell_simulator::sim::Sim::new();
    for call in calls(plan, Road::of(&plan.setup.course, &mut |_| None)) {
        call.run(&mut sim);
    }
    sim
}

#[test]
fn a_slice_from_a_device_set_up_before_lifts_to_one_that_boots_to_home_with_its_sensor() {
    use shell_simulator::ant::device_type;
    use shell_simulator::fuzz::lift::lift;
    use shell_simulator::fuzz::plan::Start;
    use shell_simulator::sim::{Kept, Sim};
    let mut sim = Sim::new();
    sim.ant_sensor(device_type::POWER, 1234);
    sim.wait_usable();
    sim.pair(&["POWER METER"]);
    sim.advance(5_000);
    sim.cut_power();
    sim.wait_usable();
    assert_eq!(sim.vana_page(), "home");
    let lifted = lift(&sim.trace()).unwrap();
    let Start::PowerOn(Some(Kept { sensors, .. })) = &lifted.plan.setup.start else {
        panic!("kept preferences: {:?}", lifted.plan.setup.start);
    };
    assert_eq!(*sensors, vec![(device_type::POWER, 1234)]);
    let mut replay = replayed(&lifted.plan);
    replay.wait_usable();
    assert_eq!(replay.vana_page(), "home", "{}", replay.screen().text());
}

#[test]
fn a_recorded_tap_replays_as_the_same_tap() {
    use shell_simulator::fuzz::lift::lift;
    use shell_simulator::sim::Sim;
    let mut sim = Sim::new();
    sim.wait_usable();
    sim.tap(120, 210);
    sim.expect_text("NEARBY");
    let lifted = lift(&sim.trace()).unwrap();
    assert!(
        !lifted.left_out.contains_key("touch"),
        "{:?}",
        lifted.left_out
    );
    let replay = replayed(&lifted.plan);
    assert!(
        replay.screen().contains("NEARBY"),
        "{}",
        replay.screen().text()
    );
}
