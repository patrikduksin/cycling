use super::*;

#[test]
fn manual_laps_keep_each_effort() {
    manual_laps(&mut rider());
}

#[test]
fn auto_lap_closes_every_five_kilometres() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.advance(minutes(9) + seconds(50));
    sim.expect_no_text("LAP 1 ");
    // 5 km at 30 km/h is ten minutes, plus the seconds sensors take to report.
    sim.wait_for("LAP 1 10:0", seconds(30)).unwrap();
    let toast = sim.screen().find("LAP 1 10:").remove(0).text;
    assert!(
        toast.starts_with("LAP 1 10:0") && toast.ends_with(" 220W"),
        "{toast}"
    );
    sim.checkpoint("auto lap toast at 5 km");
    // The wheel reads 29.9 km/h, so each 5 km takes a few seconds over ten minutes.
    sim.wait_for("LAP 2 10:", minutes(11)).unwrap();
    let toast = sim.screen().find("LAP 2 10:").remove(0).text;
    assert!(toast.starts_with("LAP 2 10:0"), "{toast}");
    sim.advance(seconds(30));
    sim.stop_ride();
    let laps = fit_laps(&mut sim);
    let triggers: Vec<u64> = laps.iter().map(|lap| lap[&24]).collect();
    assert_eq!(triggers, [2, 2, 7]);
    for lap in &laps[..2] {
        assert_near("lap km", lap[&9] as f64 / 100_000.0, 5.0, 0.05);
    }
}

/// A 2 km loop ridden at 30 km/h with AUTO LAP set to START; `jitter_m`
/// is the receiver's error per axis. Ends riding on the main page.
fn at_start_rider(jitter_m: f64) -> Sim {
    auto_lap_rider(jitter_m, "START")
}

fn auto_lap_rider(jitter_m: f64, choice: &str) -> Sim {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_route(Route::loop_around(Point::new(-33.45, -70.66), 2_000.0, 0.0));
    if jitter_m > 0.0 {
        sim.set_gps_quality(true, 1.8, jitter_m);
    }
    sim.option("AUTO_LAP", choice);
    // A fix first, so the lap's start is where the ride starts.
    sim.advance(seconds(10));
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.start_ride();
    sim
}

/// Lap marks the journal kept for a pass of the start.
fn position_laps(sim: &mut Sim) -> usize {
    let rides = sim.rides();
    rides[0]
        .samples()
        .filter(|s| s.lap == Some(vana::ride::log::LapStart::Position))
        .count()
}

#[test]
fn auto_lap_at_start_closes_a_lap_each_time_round_a_loop() {
    let mut sim = at_start_rider(0.0);
    // Two kilometres at 30 km/h is four minutes; the lap closes about 35 m past
    // the start, a few seconds on.
    sim.advance(minutes(3) + seconds(30));
    sim.expect_no_text("LAP 1 ");
    sim.wait_for("LAP 1 04:", seconds(40)).unwrap();
    let toast = sim.screen().find("LAP 1 04:").remove(0).text;
    assert!(toast.ends_with(" 220W"), "{toast}");
    sim.checkpoint("lap toast at the start, first loop");

    sim.wait_for("LAP 2 04:", seconds(300)).unwrap();
    let toast = sim.screen().find("LAP 2 04:").remove(0).text;
    assert!(
        toast.starts_with("LAP 2 04:0") || toast.starts_with("LAP 2 04:1"),
        "{toast}"
    );
    sim.checkpoint("lap toast at the start, second loop");

    sim.wait_for("LAP 3 ", seconds(300)).unwrap();
    let toast = sim.screen().find("LAP 3 ").remove(0).text;
    assert!(
        toast.starts_with("LAP 3 03:5") || toast.starts_with("LAP 3 04:0"),
        "{toast}"
    );
    sim.checkpoint("lap toast at the start, third loop");
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(position_laps(&mut sim), 3);

    let fit = sim.rides()[0]
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
    let laps = fit_messages(&fit, 19);
    let triggers: Vec<u64> = laps.iter().map(|lap| lap[&24]).collect();
    // position_start for each loop, then the ride's end.
    assert_eq!(triggers, [3, 3, 3, 7]);
    let distances: Vec<u64> = laps.iter().map(|lap| lap[&9]).collect();
    for centimetres in &distances[..3] {
        assert!((199_000..=208_000).contains(centimetres), "{distances:?}");
    }
    assert_eq!(saved_session(&mut sim)[&26], 4, "FIT laps");
}

#[test]
fn auto_lap_at_start_counts_each_loop_once_through_gps_jitter() {
    let mut sim = at_start_rider(12.0);
    sim.advance(minutes(12) + seconds(30));
    sim.stop_ride();
    assert_eq!(position_laps(&mut sim), 3);
    assert_eq!(saved_session(&mut sim)[&26], 4, "FIT laps");
}

/// Laps found against true loops, and how far from the start the rider was
/// where each began, for the PR's table.
#[test]
#[ignore = "measurement, run with --ignored --nocapture"]
fn auto_lap_at_start_detection_table() {
    use shell_simulator::route::{distance_m, from_e7};
    use vana::ride::log::LapStart;
    let start = Route::loop_around(Point::new(-33.45, -70.66), 2_000.0, 0.0).at(0.0);
    println!(
        "| GPS jitter per axis | loops ridden | laps detected | lap began this far past the start (m) |"
    );
    println!("| :-- | :-- | :-- | :-- |");
    for jitter in [0.0, 3.0, 8.0, 12.0, 20.0] {
        let mut sim = at_start_rider(jitter);
        sim.advance(minutes(20) + seconds(30));
        sim.stop_ride();
        let rides = sim.rides();
        let off: Vec<f64> = rides[0]
            .samples()
            .filter(|s| s.lap == Some(LapStart::Position))
            .filter_map(|s| s.location_e7)
            .map(|(lat, lon)| distance_m(from_e7(lat, lon), start))
            .collect();
        let (min, max) = off
            .iter()
            .fold((f64::MAX, 0.0_f64), |(lo, hi), &m| (lo.min(m), hi.max(m)));
        println!("| {jitter} m | 5 | {} | {min:.0} to {max:.0} |", off.len());
    }
}

#[test]
fn paused_time_is_not_lap_time() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(2));
    sim.lap(1);
    sim.advance(minutes(2));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pause_ride();
    sim.advance(minutes(5));

    sim.resume_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    let toast = sim.lap(2);
    assert!(
        toast.starts_with("LAP 2 03:0") || toast.starts_with("LAP 2 02:59"),
        "{toast}"
    );
    sim.checkpoint("lap time after resuming");
}

#[test]
fn a_second_lap_press_within_three_seconds_is_ignored() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    sim.lap(1);
    // A double press, as a glove or a pothole makes.
    sim.press(Role::Back);
    sim.press(Role::Back);
    sim.expect_no_text("LAP 2 ");
    sim.checkpoint("double press makes one lap");
    sim.advance(seconds(3));
    sim.lap(2);
    sim.stop_ride();
    assert_eq!(saved_session(&mut sim)[&26], 3, "FIT laps");
}

fn fit_lap_triggers(bytes: &[u8]) -> Vec<u8> {
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut definitions = std::collections::BTreeMap::new();
    let mut triggers = Vec::new();
    let mut at = 14;
    while at < 14 + size {
        let header = bytes[at];
        at += 1;
        let local = header & 0xf;
        if header & 0x40 != 0 {
            let global = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]);
            let count = usize::from(bytes[at + 4]);
            let fields: Vec<(u8, usize)> = (0..count)
                .map(|i| (bytes[at + 5 + 3 * i], usize::from(bytes[at + 6 + 3 * i])))
                .collect();
            at += 5 + 3 * count;
            definitions.insert(local, (global, fields));
            continue;
        }
        let (global, fields) = &definitions[&local];
        for &(number, field_size) in fields {
            if *global == 19 && number == 24 {
                triggers.push(bytes[at]);
            }
            at += field_size;
        }
    }
    triggers
}

#[test]
fn laps_are_kept_with_the_ride_in_its_fit_file() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD", "HEART RATE"]);
    sim.option("AUTO_LAP", "1");

    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.press(Role::Back);
    sim.advance(seconds(1));
    sim.expect_text("LAP 1");
    sim.checkpoint("a lap press");
    sim.advance(minutes(2) + seconds(30));
    sim.stop_ride();
    sim.done();

    let media = sim.ride_media();
    let slots: Vec<Slot> = media
        .as_chunks::<SLOT_SIZE>()
        .0
        .iter()
        .map(|bytes| Slot(*bytes))
        .collect();
    let ride = sim.rides().last().unwrap().id;
    let start = slots
        .iter()
        .position(|slot| decode(slot).is_some_and(|e| e.kind == Kind::Start && e.ride_id == ride))
        .unwrap();
    struct Bytes(Vec<u8>);
    impl firmware_services::fit::Sink for Bytes {
        fn put(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
    }
    let mut fit = Bytes(Vec::new());
    let plan = vana::ride::fit::write_all(
        slots[start..].iter(),
        ride,
        Default::default(),
        &models::C606.product,
        Some(1_789_000_000_000),
        &mut fit,
    )
    .unwrap();
    assert_eq!(plan.laps, 3);
    assert_eq!(
        fit_lap_triggers(&fit.0),
        [0, 2, 7],
        "manual, distance, session end"
    );
}

#[test]
fn a_lap_just_before_the_finish_is_kept_with_the_ride() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD", "HEART RATE"]);
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.lap(1);
    sim.pause_ride();
    sim.stop_ride();
    sim.done();

    let ride = sim.rides().pop().unwrap();
    let marks: Vec<_> = ride.samples().filter_map(|s| s.lap).collect();
    let fit = fit_lap_triggers(
        &ride
            .fit(&models::C606.product, Some(1_789_000_000_000))
            .unwrap(),
    );
    sim.note(NoteKind::Check, format!("laps: FIT {}", fit.len()));
    assert_eq!(
        fit.as_slice(),
        [0, 7].as_slice(),
        "FIT lap triggers (manual, session end)"
    );
    assert_eq!(
        marks,
        [LapStart::Ride, LapStart::Manual],
        "journal lap marks"
    );
}

#[test]
fn a_lap_pressed_just_before_the_start_keeps_both_laps() {
    let pass_ms = {
        let mut sim = at_start_rider(0.0);
        sim.wait_for("LAP 1 04:", minutes(5)).unwrap();
        sim.now()
    };
    let mut sim = at_start_rider(0.0);
    sim.advance(pass_ms - 300 - sim.now());
    sim.report(Role::Back);
    sim.advance(10);
    sim.expect_text("LAP 1 ");
    let second = sim.wait_for("LAP 2 ", seconds(1)).unwrap();
    sim.checkpoint("a lap pressed, then the start closes one");
    sim.advance(seconds(30));
    sim.pause_ride();
    sim.stop_ride();
    sim.done();

    let ride = sim.rides().pop().unwrap();
    let marks: Vec<_> = ride
        .samples()
        .enumerate()
        .filter_map(|(index, s)| s.lap.map(|lap| (index, lap)))
        .collect();
    let fit = fit_lap_triggers(
        &ride
            .fit(&models::C606.product, Some(1_789_000_000_000))
            .unwrap(),
    );
    sim.note(
        NoteKind::Check,
        format!(
            "second lap toast {second} ms after the first; laps: FIT {fit:?}, \
             journal {marks:?}"
        ),
    );
    assert_eq!(
        fit.as_slice(),
        [0, 3, 7].as_slice(),
        "FIT lap triggers (manual, position start, session end)"
    );
    let starts: Vec<_> = marks.iter().map(|&(_, lap)| lap).collect();
    assert_eq!(
        starts,
        [LapStart::Ride, LapStart::Manual, LapStart::Position],
        "journal lap marks"
    );
    assert_eq!(
        marks[2].0,
        marks[1].0 + 1,
        "the two laps begin on consecutive samples"
    );
}
