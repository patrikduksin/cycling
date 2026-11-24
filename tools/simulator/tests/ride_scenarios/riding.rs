use super::*;

#[test]
fn ride_shows_live_metrics_and_records_the_gps_track() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(5));
    // Wheel events arrive in quarter-second packets, so the last digit wobbles.
    let speed = sim.screen().field("SPD KM/H").unwrap();
    assert!(
        ["27.9", "28.0", "28.1"].contains(&speed.as_str()),
        "{speed}"
    );
    sim.expect_field("PWR 3S W", "210");
    sim.expect_field("HEART BPM", "142");
    assert_eq!(sim.screen().elapsed().as_deref(), Some("0:05:00"));
    use vana::compose::data::Metric;
    assert_eq!(sim.reading(Metric::Power).as_deref(), Some("210"));
    assert_eq!(sim.reading(Metric::Heart).as_deref(), Some("142"));
    assert_eq!(
        sim.reading(Metric::AvgPower),
        None,
        "the shown line has no averages"
    );
    sim.checkpoint("five minutes riding");
    sim.stop_ride();
    sim.checkpoint("ride saved");

    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    let ride = &rides[0];
    assert_eq!(ride.kinds().first(), Some(&Kind::Start));
    assert_eq!(ride.kinds().last(), Some(&Kind::Finish));
    let samples: Vec<_> = ride.samples().collect();
    assert!(samples.len() >= 290, "{} samples", samples.len());
    let riding = &samples[samples.len() / 2];
    assert_eq!(riding.heart_bpm, Some(142));
    assert_eq!(riding.power_watts, Some(210));
    let kmh = f64::from(riding.speed_mm_s.unwrap()) * 3.6 / 1000.0;
    assert!((kmh - 28.0).abs() < 0.5, "{kmh} km/h");
    // Five minutes at 28 km/h is 2.33 km along the route.
    let track = ride.track_m();
    assert!((track - 2_333.0).abs() < 120.0, "{track:.0} m tracked");
    assert!(ride.gpx().contains("<trkpt"));
}

#[test]
fn gps_tunnel_records_samples_without_a_position() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(30.0, 0.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(30));
    ride_page(&mut sim, "ride/map");
    let gps = sim.screen().run("GPS").unwrap();
    assert_eq!(gps.color, GREEN, "GPS shown as fixed");
    sim.checkpoint("gps fixed");

    sim.set_gps_fix(false);
    sim.advance(seconds(30));
    let gps = sim.screen().run("GPS").unwrap();
    assert_ne!(gps.color, GREEN, "GPS still shown as fixed in the tunnel");
    ride_page(&mut sim, "ride/main");
    // Wheel speed keeps working without satellites. The truncated decimal
    // depends on where the sample lands between wheel revolutions.
    expect_near(&mut sim, "SPD KM/H", 30.0, 0.15);
    sim.checkpoint("in the tunnel");

    sim.set_gps_fix(true);
    sim.advance(seconds(30));
    sim.stop_ride();
    let ride = &sim.rides()[0];
    let samples: Vec<_> = ride.samples().collect();
    let blind = samples.iter().filter(|s| s.location_e7.is_none()).count();
    assert!(
        (25..=35).contains(&blind),
        "{blind} samples without position"
    );
    // One unbroken gap: the tunnel, not scattered dropouts.
    let first = samples
        .iter()
        .position(|s| s.location_e7.is_none())
        .unwrap();
    assert!(
        samples[first..first + blind]
            .iter()
            .all(|s| s.location_e7.is_none())
    );
    assert!(samples.iter().all(|s| s.speed_mm_s.is_some()));
    // The tracked distance still matches the ride around the gap.
    let track = ride.track_m();
    assert!(
        (track - 750.0).abs() < 60.0,
        "{track:.0} m tracked of 750 m"
    );
}

#[test]
fn heart_rate_band_and_eat_and_drink_alerts_sound_and_show() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.option("HR_HIGH", "170");
    sim.option("FUEL", "20");
    sim.ride_at(28.0, 220.0, 150.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.expect_no_text("HR HIGH");
    sim.ride_at(28.0, 320.0, 172.0, 90.0);
    let latency = sim.wait_for("HR HIGH 172", seconds(20)).unwrap();
    assert!(latency >= seconds(10), "a surge alerted after {latency} ms");
    // Two high notes: the band cue.
    let ids = played_since(&sim, 0);
    assert!(ids.windows(2).any(|pair| pair == [10, 10]), "{ids:?}");
    sim.checkpoint("alert: heart rate above the band");
    sim.ride_at(28.0, 220.0, 150.0, 90.0);
    sim.wait_for("EAT AND DRINK", minutes(19)).unwrap();
    let active = elapsed_secs(&sim);
    assert!((1199..=1202).contains(&active), "reminder at {active} s");
    let ids = played_since(&sim, 0);
    assert!(ids.windows(2).any(|pair| pair == [0, 0]), "{ids:?}");
    sim.checkpoint("alert: eat and drink after 20 minutes");
}

#[test]
fn pause_excludes_time_and_two_hour_ride_keeps_every_sample() {
    two_hour_ride_with_a_pause(&mut rider());
}

#[test]
fn main_page_power_is_a_three_second_average() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.expect_field("PWR 3S W", "200");
    // A sprint: the reading climbs over three seconds instead of jumping.
    sim.ride_at(30.0, 500.0, 140.0, 90.0);
    let mut seen = Vec::new();
    for _ in 0..8 {
        sim.advance(500);
        seen.push(reading(&sim, "PWR 3S W") as u32);
    }
    sim.checkpoint("3 s power during a sprint");
    assert!(
        seen.iter().any(|w| (250..450).contains(w)),
        "no step between 200 and 500 W: {seen:?}"
    );
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(seen.last(), Some(&500), "{seen:?}");
}

#[test]
fn a_steady_ten_minutes_reads_on_the_main_page_and_totals_on_the_summary() {
    let mut sim = rider();
    sim.need(Need::Barometer);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    // Sensors take a few seconds to report after the start.
    expect_near(&mut sim, "DIST KM", 5.0, 0.05);
    sim.expect_field("CAD RPM", "90");
    sim.expect_field("NP W", "200");
    sim.checkpoint("stats page after ten minutes");

    // Ride done keeps the totals; TopLeft is Home.
    sim.stop_ride();
    assert_eq!(done_stat(&sim, "distance").as_deref(), Some("5.0"));
    assert_eq!(done_stat(&sim, "time").as_deref(), Some("0:10"));
    sim.checkpoint("ride done after saving");
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn a_saved_ride_keeps_the_calories_of_its_work() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 170.0, 90.0);
    sim.advance(minutes(10));
    sim.stop_ride();
    // 200 W for ten minutes is 120 kJ, about 120 kcal.
    assert_eq!(saved_session(&mut sim)[&11], 120, "FIT kcal");
}

#[test]
fn pause_leaves_the_totals_untouched() {
    const TOTALS: [&str; 3] = ["DIST KM", "NP W", "CLIMB M"];
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    ride_page(&mut sim, "ride/main");
    expect_near(&mut sim, "DIST KM", 2.5, 0.05);

    // Paused but still rolling hard: nothing the wheel or pedals do counts.
    sim.pause_ride();
    sim.show_ride_pages();
    let screen = sim.screen();
    let before: Vec<_> = TOTALS.iter().map(|label| screen.field(label)).collect();
    sim.checkpoint("stats when paused");
    sim.ride_at(30.0, 400.0, 170.0, 100.0);
    sim.advance(minutes(5));
    let screen = sim.screen();
    for (label, before) in TOTALS.iter().zip(&before) {
        assert_eq!(&screen.field(label), before, "{label} changed while paused");
    }
    sim.expect_field("NP W", "250");
    sim.checkpoint("stats after five paused minutes");

    sim.resume_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    // Past the auto-lap toast at 5 km, which covers the distance.
    sim.wait_gone("LAP 1", seconds(5)).unwrap();
    expect_near(&mut sim, "DIST KM", 5.0, 0.08);
    sim.expect_field("NP W", "250");
    sim.checkpoint("stats after resuming");
    // The pause is not ride time.
    let active = sim.screen().elapsed().unwrap();
    assert!(active.starts_with("0:10:0"), "{active}");
    sim.checkpoint("main page after resuming");
}

#[test]
fn numbers_jump_to_each_reading() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 200.0, 130.0, 88.0);
    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("HEART BPM", "130");
    sim.ride_at(28.0, 200.0, 170.0, 88.0);
    let mut numbers = vec![String::from("130")];
    for _ in 0..100 {
        sim.advance(10);
        let bpm = sim.screen().field("HEART BPM").unwrap();
        if numbers.last() != Some(&bpm) {
            numbers.push(bpm);
        }
    }
    assert_eq!(numbers, ["130", "170"], "the number jumps");
    sim.checkpoint("the number jumped to the reading");
}
