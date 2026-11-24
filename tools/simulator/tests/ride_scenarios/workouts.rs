use super::*;

/// The device's beeps since `from` (uptime), as pattern ids.
fn beeps_since(sim: &Sim, from: u64) -> Vec<u8> {
    sim.device
        .sim()
        .speaker
        .played
        .iter()
        .filter(|(at, _)| *at >= from)
        .map(|(_, id)| *id)
        .collect()
}

/// The step's target band from `WORKOUT STATUS`.
fn target(sim: &mut Sim) -> Option<(f64, f64)> {
    let status = sim.workout();
    let (low, high) = status.get("target")?.split_once('-')?;
    Some((low.parse().ok()?, high.parse().ok()?))
}

/// Ride `seconds`, each second at the watts `effort` picks for the target.
fn follow(sim: &mut Sim, seconds: u64, effort: impl Fn(Option<(f64, f64)>) -> f64) {
    for _ in 0..seconds {
        let watts = effort(target(sim));
        sim.world().power_w = watts;
        sim.advance(1000);
    }
}

/// The middle of the band, or an easy spin without one.
fn on_target(band: Option<(f64, f64)>) -> f64 {
    band.map_or(150.0, |(low, high)| ((low + high) / 2.0).round())
}

/// Ride to the start of the next step, following `effort`.
#[track_caller]
fn ride_step(sim: &mut Sim, effort: impl Fn(Option<(f64, f64)>) -> f64) {
    let step = sim.workout()["step"].clone();
    for _ in 0..4000 {
        follow(sim, 1, &effort);
        let status = sim.workout();
        if status["step"] != step || status["done"] == "1" {
            return;
        }
    }
    panic!("step {step} never ended");
}

/// MARK on a recording workout, then wait out the lap debounce.
fn next_step(sim: &mut Sim) {
    sim.press(Role::Back);
    sim.advance(seconds(3));
}

#[test]
fn holding_move_on_the_workout_page_extends_the_step_without_a_mark() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::VO2_MAX);
    follow(&mut sim, 60, on_target);
    let left: u64 = sim.workout()["left_s"].parse().unwrap();
    sim.hold(Role::Move, 1_600);
    sim.expect_text("+1 MIN");
    assert!(
        !sim.screen().text().contains("MARK"),
        "the hold extends, not marks"
    );
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/workout"),
        "the workout page stays"
    );
    let extended: u64 = sim.workout()["left_s"].parse().unwrap();
    assert!(extended >= left + 57, "{left} s then {extended} s");
    assert_eq!(sim.ride_state(), "recording");
    sim.checkpoint("interval extended by a held MOVE");
    sim.ride_page("ride/main");
    sim.hold(Role::Move, 1_600);
    sim.expect_text("MARK 1");
    sim.stop_ride();
}

#[test]
fn a_structured_workout_runs_its_steps_with_targets_beeps_and_laps() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::VO2_MAX);
    // The workout page leads the ride pages.
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/workout"));
    sim.expect_text("LEFT");

    // Warm up to its last seconds: three countdown beeps, then the step's.
    follow(&mut sim, 12 * 60 - 8, on_target);
    assert_eq!(sim.workout()["step"], "1/12");
    let before = sim.now();
    follow(&mut sim, 5, on_target);
    sim.checkpoint("warm up counting down");
    ride_step(&mut sim, on_target);
    assert_eq!(sim.workout()["step"], "2/12");
    sim.advance(500);
    let beeps = beeps_since(&sim, before);
    // Low notes count down; two high notes start the step.
    assert_eq!(
        beeps,
        [21, 21, 21, 10, 10],
        "three countdown beeps, then the step"
    );
    sim.expect_text("3:00");
    sim.expect_text("RECOVER");
    sim.checkpoint("next step announced");
    assert_eq!(sim.workout()["laps"], "1", "the step closed a lap");

    // On target: 106-120 % of 215 W.
    follow(&mut sim, 20, on_target);
    assert_eq!(sim.workout()["target"], "228-258");
    sim.expect_text("258");
    sim.expect_text("228");
    sim.checkpoint("interval on target");
    // The main page draws the same target under its power gauge.
    sim.press(Role::Move);
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.checkpoint("main page with the target band");
    sim.ride_page("ride/workout");
    ride_step(&mut sim, on_target);
    ride_step(&mut sim, on_target);

    assert_eq!(sim.workout()["step"], "4/12");
    follow(&mut sim, 20, |band| band.unwrap().0 * 0.8);
    sim.checkpoint("interval under target");
    ride_step(&mut sim, |band| band.unwrap().0 * 0.8);

    // MARK ends the recovery early: its lap closes and the next step starts.
    follow(&mut sim, 30, on_target);
    let laps: u32 = sim.workout()["laps"].parse().unwrap();
    next_step(&mut sim);
    let status = sim.workout();
    assert_eq!(status["step"], "6/12");
    assert_eq!(status["laps"], (laps + 1).to_string(), "one lap, not two");
    assert_eq!(status["skipped"], "1");

    // A tap on the time left adds a minute to the interval.
    follow(&mut sim, 10, on_target);
    let left: u64 = sim.workout()["left_s"].parse().unwrap();
    let words = sim.screen().run("LEFT").expect("the time left");
    sim.tap(120, words.y as u16);
    sim.expect_text("+1 MIN");
    let extended: u64 = sim.workout()["left_s"].parse().unwrap();
    assert!(extended >= left + 59, "{left} s then {extended} s");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "touch never controls the ride"
    );
    sim.checkpoint("interval extended by a tap");

    // The rest of the workout, then free riding.
    while sim.workout()["done"] == "0" {
        ride_step(&mut sim, on_target);
    }
    sim.expect_text("WORKOUT DONE");
    sim.checkpoint("workout done");
    let status = sim.workout();
    assert_eq!(status["laps"], "12", "one lap per step");
    let in_pct: i32 = status["in_pct"].parse().unwrap();
    assert!((60..100).contains(&in_pct), "{in_pct}% on target");
    sim.stop_ride();
    sim.checkpoint("workout ride saved");
}

#[test]
fn a_thirty_thirty_runs_its_forty_one_steps() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::VO2_THIRTY);
    assert_eq!(sim.workout()["step"], "1/41");
    next_step(&mut sim);
    assert_eq!(sim.workout()["step"], "2/41");
    // Thirty seconds at 120-130 % of the default 215 W FTP.
    assert_eq!(target(&mut sim), Some((258.0, 280.0)));
    follow(&mut sim, 10, on_target);
    sim.checkpoint("thirty thirty hard");
    ride_step(&mut sim, on_target);
    assert_eq!(sim.workout()["step"], "3/41");
    sim.expect_text("RECOVER");
    assert_eq!(target(&mut sim), Some((86.0, 108.0)));
    sim.checkpoint("thirty thirty easy");
}

#[test]
fn twenty_minute_ftp_test_measures_the_ftp() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::FTP_TWENTY);
    // Skip to the twenty minutes with MARK.
    follow(&mut sim, 30, on_target);
    sim.world().power_w = 260.0;
    for _ in 0..10 {
        next_step(&mut sim);
    }
    assert_eq!(sim.workout()["step"], "11/12");
    follow(&mut sim, 5 * 60, |_| 260.0);
    sim.expect_text("AVG");
    sim.expect_text("260");
    sim.checkpoint("twenty minutes all out");
    let before = sim.now();
    ride_step(&mut sim, |_| 260.0);
    assert_eq!(sim.workout()["ftp"], "247");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    assert_eq!(sim.ride_state(), "recording");
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=215 "), "{profile}");
    sim.stop_ride();
}

#[test]
fn ramp_test_ends_at_failure_and_measures_the_ftp() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::FTP_RAMP);
    follow(&mut sim, 6 * 60, on_target);
    assert_eq!(
        sim.workout()["step"],
        "1/2",
        "the warm-up waits for the rider"
    );
    sim.expect_text("LAP TO START");
    sim.checkpoint("ramp warm-up waits for lap");
    next_step(&mut sim);
    assert_eq!(sim.workout()["step"], "2/2");
    // The rider holds every minute up to 300 W, then fades. The band is
    // 95-110 % of the stage, so the stage is its top over 1.1.
    let effort = |band: Option<(f64, f64)>| {
        let stage = band.map_or(100.0, |(_, high)| (high / 1.1).round());
        if stage <= 300.0 { stage + 5.0 } else { 250.0 }
    };
    follow(&mut sim, 87, effort);
    sim.expect_text("121");
    sim.expect_text("LAP TO END");
    assert_eq!(target(&mut sim), Some((115.0, 133.0)), "95-110 % of 121 W");
    sim.checkpoint("ramp early at 121 W");
    follow(&mut sim, 8 * 60 - 87 + 30, effort);
    sim.expect_text("212");
    sim.checkpoint("ramp at 212 W");
    let before = sim.now();
    ride_step(&mut sim, effort);
    // 75 % of the best minute, 295 W.
    assert_eq!(sim.workout()["ftp"], "221");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    assert_eq!(sim.workout()["done"], "1", "the workout ends with the ramp");
    assert_eq!(sim.ride_state(), "recording");
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=215 "), "{profile}");
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/main"),
        "the main ride page, not the workout page"
    );
    sim.checkpoint("main ride page after the ramp");
    sim.stop_ride();
}

#[test]
fn a_host_loaded_workout_is_todays_and_rides() {
    use vana::workout::codec::encode;
    use vana::workout::model::{Block, Intensity, Length, Step, Target, Workout};
    const STEPS: &[Step] = &[
        Step::new(
            Intensity::Warmup,
            Length::Time(300),
            Target::Ftp { low: 50, high: 65 },
        ),
        Step::new(
            Intensity::Work,
            Length::Time(120),
            Target::Ftp { low: 92, high: 96 },
        ),
        Step::new(
            Intensity::Work,
            Length::Time(60),
            Target::Ftp {
                low: 104,
                high: 108,
            },
        ),
        Step::new(
            Intensity::Cooldown,
            Length::Distance(2000),
            Target::HeartZone(1),
        ),
    ];
    const BLOCKS: &[Block] = &[Block {
        first: 1,
        len: 2,
        count: 3,
    }];
    let workout = Workout::new("OVER UNDERS", STEPS, BLOCKS, None);
    let bytes = encode(&workout);
    // Uploads and workout records live on the MMC's FILES volume.
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    let mut damaged = bytes.to_vec();
    damaged[10] ^= 1;
    sim.put_file(vana::workout::today::FILE, &damaged);
    let (_, today) = sim.command("WORKOUT LOAD");
    assert_eq!(today, "workout=none files=decode");
    assert!(sim.on_home());
    sim.expect_no_text("Today's workout");
    sim.advance(seconds(5));
    let (_, today) = sim.command("WORKOUT STATUS");
    assert_eq!(today, "workout=none");
    sim.load_workout(&workout);
    let (_, today) = sim.command("WORKOUT LOAD");
    assert_eq!(today, "workout=OVER_UNDERS minutes=14 steps=8 files=ok");
    sim.start_workout(&workout);
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    follow(&mut sim, 5, on_target);
    next_step(&mut sim);
    follow(&mut sim, 10, on_target);
    assert_eq!(sim.workout()["step"], "2/8");
    // 92-96 % of 215 W.
    assert_eq!(sim.workout()["target"], "198-206");
    sim.checkpoint("uploaded workout riding");
    sim.stop_ride();
    // The saved ride keeps how its workout went on FILES.
    let (_, rides) = sim.command("RIDE HISTORY");
    let ride: u32 = rides
        .rsplit(" ride=")
        .next()
        .and_then(|last| last.split(',').next())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("RIDE HISTORY: {rides}"));
    let saved = ["INTERNAL", "BULK"]
        .iter()
        .map(|journal| sim.command(&format!("WORKOUT HISTORY {journal} {ride}")).1)
        .find(|reply| reply.contains("workout=OVER_UNDERS"))
        .expect("a workout record for the saved ride");
    assert!(saved.contains("steps=1/8 skipped=1 finished=0"), "{saved}");
    sim.done();
    assert_eq!(
        sim.workout()["workout"],
        "none",
        "a ridden workout leaves Home"
    );
    sim.expect_no_text("Today's workout");
    sim.home("RIDE");
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    assert_eq!(sim.workout()["workout"], "none", "a free ride");
}
