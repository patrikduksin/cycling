use super::*;

const SPEED_ONLY: u16 = 778;

#[test]
fn traffic_light_stop_auto_pauses_and_excludes_the_wait() {
    traffic_light_stop(&mut rider());
}

#[test]
fn riding_off_on_home_waits_for_the_rider_to_start() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(20.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(30));
    assert!(sim.rides().is_empty());
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("rolling on Home, no ride");
    sim.start_ride();
    assert!(recording(&mut sim));
    sim.checkpoint("started by the rider");
    sim.ride_page("ride/map");
    let rec = sim.screen().run("REC").expect("REC in the status bar");
    assert_eq!((rec.y < 24, rec.color), (true, RED));
}

#[test]
fn a_manual_pause_does_not_resume_when_the_bike_moves() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.advance(seconds(60));
    assert!(sim.screen().run("Paused").is_some());
    sim.expect_no_text("AUTO");
    sim.expect_text("for 01:00");
    assert!(!recording(&mut sim));
    sim.checkpoint("manual pause while riding");
}

#[test]
fn a_manually_paused_rider_who_rides_on_hears_a_reminder_every_half_minute() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.advance(seconds(1));
    let from = sim.device.sim_mut().speaker.played.len();
    sim.advance(seconds(60));
    assert_eq!(
        played_since(&sim, from),
        [0, 0, 10, 0, 0, 10],
        "two reminders in a minute"
    );
    assert!(sim.screen().run("Paused").is_some());
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    let from = sim.device.sim_mut().speaker.played.len();
    sim.advance(seconds(60));
    assert_eq!(
        played_since(&sim, from),
        Vec::<u8>::new(),
        "quiet once stopped"
    );
    assert!(!recording(&mut sim));
}

#[test]
fn a_speed_sensor_dropout_leaves_auto_pause_to_gps() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    sim.set_present(device_type::SPEED_CADENCE, SPEED, false);
    sim.advance(minutes(2));
    assert!(recording(&mut sim));
    sim.expect_no_text("Auto paused");
    sim.checkpoint("speed sensor lost, still recording");
    let km = reading(&sim, "DIST KM");
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("Auto paused", seconds(20)).unwrap();
    sim.advance(seconds(30));
    sim.expect_text("Auto paused");
    sim.ride_at(36.0, 0.0, 0.0, 0.0);
    let latency = sim.wait_for("AUTO RESUME", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-resume took {latency} ms");
    sim.advance(minutes(1));
    assert!(recording(&mut sim));
    expect_near(&mut sim, "SPD KM/H", 36.0, 0.2);
    let ridden = reading(&sim, "DIST KM") - km;
    assert!(
        (ridden - 0.6).abs() <= 0.05,
        "{ridden:.2} km after resuming"
    );
    sim.set_present(device_type::SPEED_CADENCE, SPEED, true);
    sim.advance(seconds(10));
    assert!(recording(&mut sim));
}

#[test]
fn a_silent_paired_speed_sensor_leaves_auto_pause_to_gps() {
    let mut sim = rider();
    sim.ant_sensor(device_type::SPEED, SPEED_ONLY);
    sim.pair(&["SPEED"]);
    sim.set_present(device_type::SPEED, SPEED_ONLY, false);
    sim.advance(seconds(10));
    sim.start_ride();
    sim.wait_for("Auto paused", seconds(15)).unwrap();
    sim.advance(seconds(30));
    sim.expect_text("Auto paused");
    sim.ride_at(36.0, 0.0, 0.0, 0.0);
    let latency = sim.wait_for("AUTO RESUME", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-resume took {latency} ms");
    sim.advance(minutes(1));
    assert!(recording(&mut sim));
    expect_near(&mut sim, "SPD KM/H", 36.0, 0.2);
    expect_near(&mut sim, "DIST KM", 0.6, 0.05);
}

#[test]
fn riding_without_wheel_speed_or_gps_says_auto_pause_is_off() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    // Indoors: no sky view, so GPS cannot stand in for a wheel sensor.
    sim.set_gps_fix(false);
    sim.ride_at(25.0, 0.0, 140.0, 0.0);
    sim.start_ride();
    sim.wait_for("AUTO PAUSE OFF", seconds(3)).unwrap();
    sim.expect_field("SPD KM/H", "--");
    sim.checkpoint("no wheel speed or GPS, auto-pause off");
    // A note, not a standing banner over the readings.
    sim.wait_gone("AUTO PAUSE OFF", seconds(15)).unwrap();
    assert!(recording(&mut sim));
}

#[test]
fn slow_steep_climbing_never_auto_pauses() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(5.0, 180.0, 0.0, 50.0);
    // Let the wheel report the climbing speed before starting: started
    // while it still reads standstill, a ride pauses at once, rightly.
    sim.advance(seconds(3));
    sim.start_ride();
    sim.advance(minutes(5));
    assert!(recording(&mut sim));
    sim.checkpoint("five minutes at 5 km/h, still recording");
    sim.ride_at(3.6, 180.0, 0.0, 40.0);
    sim.advance(minutes(2));
    assert!(recording(&mut sim));
    sim.expect_no_text("Auto paused");
    sim.stop_ride();
    // Only the pause that stop_ride itself presses.
    let pauses = sim.rides()[0]
        .kinds()
        .iter()
        .filter(|kind| **kind == Kind::Pause)
        .count();
    assert_eq!(pauses, 1, "{:?}", sim.rides()[0].kinds());
}

/// A rider with a heart strap and GPS but no wheel sensor, the most common
/// setup: speed, distance and auto-pause all come from GPS.
fn gps_rider(doppler: bool) -> Sim {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    if !doppler {
        // A receiver that sends no ground speed, with a metre of jitter.
        sim.set_gps_quality(false, 1.2, 1.0);
    }
    sim
}

#[test]
fn riding_with_gps_only_shows_speed_and_distance() {
    for doppler in [true, false] {
        let mut sim = gps_rider(doppler);
        sim.ride_at(25.0, 0.0, 140.0, 0.0);
        sim.advance(seconds(10));
        sim.start_ride();
        sim.advance(minutes(10));
        // Doppler is exact; positions jitter, so their speed wobbles a little.
        let tolerance = if doppler { 0.15 } else { 1.5 };
        expect_near(&mut sim, "SPD KM/H", 25.0, tolerance);
        expect_near(&mut sim, "DIST KM", 4.17, 0.08);
        sim.expect_no_text("AUTO PAUSE OFF");
        if doppler {
            sim.checkpoint("GPS only: speed and distance");
        }
        sim.stop_ride();
        let ride = &sim.rides()[0];
        let measured = ride.samples().filter(|s| s.speed_mm_s.is_some()).count();
        assert!(measured >= 595, "{measured} samples with speed");
        let pauses = ride.kinds().iter().filter(|k| **k == Kind::Pause).count();
        assert_eq!(pauses, 1, "only the pause stop_ride pressed");
    }
}

#[test]
fn gps_only_traffic_light_stop_auto_pauses_through_jitter() {
    for doppler in [true, false] {
        let mut sim = gps_rider(doppler);
        if !doppler {
            // A poor sky at the light: two metres of position jitter.
            sim.set_gps_quality(false, 1.8, 2.0);
        }
        sim.ride_at(25.0, 0.0, 140.0, 0.0);
        sim.start_ride();
        sim.advance(seconds(60));
        sim.ride_at(0.0, 0.0, 120.0, 0.0);
        let latency = sim.wait_for("Auto paused", seconds(20)).unwrap();
        println!("GPS doppler={doppler} auto-pause latency {latency} ms");
        assert!(latency <= seconds(8), "auto-pause took {latency} ms");
        if doppler {
            sim.checkpoint("GPS only: auto paused at the light");
        }
        let km = paused_km(&sim);
        // Two minutes at the light: jitter neither resumes nor adds distance.
        sim.advance(minutes(2));
        sim.expect_text("Auto paused");
        assert!(!recording(&mut sim));
        assert_eq!(paused_km(&sim), km, "distance crept while standing");
        if !doppler {
            sim.checkpoint("GPS jitter at the light: still paused");
        }
        sim.ride_at(25.0, 0.0, 140.0, 0.0);
        let latency = sim.wait_for("AUTO RESUME", seconds(15)).unwrap();
        println!("GPS doppler={doppler} auto-resume latency {latency} ms");
        assert!(latency <= seconds(8), "auto-resume took {latency} ms");
        assert!(recording(&mut sim));
        // Stopping right after the resume: Paused and Ride saved still show.
        sim.stop_ride();
    }
}

/// The ride distance the paused screen shows, as in "0.42 km".
#[track_caller]
fn paused_km(sim: &Sim) -> String {
    let text = sim.screen().text();
    text.lines()
        .find_map(|line| line.strip_suffix(" km"))
        .unwrap_or_else(|| panic!("no distance; screen:\n{text}"))
        .to_owned()
}

#[test]
fn gps_only_tunnel_keeps_recording_without_inventing_distance() {
    let mut sim = gps_rider(true);
    sim.ride_at(30.0, 0.0, 140.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(60));
    sim.set_gps_fix(false);
    sim.advance(seconds(20));
    // No speed in the tunnel, and no pause: GPS loss is not a stop.
    sim.expect_field("SPD KM/H", "--");
    assert!(recording(&mut sim));
    sim.expect_no_text("Auto paused");
    sim.checkpoint("GPS only: in the tunnel");
    let km = reading(&sim, "DIST KM");
    sim.advance(seconds(20));
    assert_eq!(reading(&sim, "DIST KM"), km, "distance without a speed");
    sim.set_gps_fix(true);
    sim.advance(seconds(20));
    // 80 of the 120 s measured at 30 km/h: 0.67 km. The track shows the gap.
    expect_near(&mut sim, "DIST KM", 0.67, 0.03);
    expect_near(&mut sim, "SPD KM/H", 30.0, 0.2);
    sim.checkpoint("GPS only: speed back after the tunnel");
    assert!(recording(&mut sim));
    sim.stop_ride();
    let ride = &sim.rides()[0];
    let blind = ride.samples().filter(|s| s.speed_mm_s.is_none()).count();
    assert!((38..=45).contains(&blind), "{blind} samples without speed");
}

#[test]
fn without_auto_pause_or_sounds_a_stop_keeps_recording_quietly_and_laps_follow_the_choice() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.option("AUTO_PAUSE", "OFF");
    sim.option("AUTO_LAP", "1");
    sim.option("SOUNDS", "OFF");

    sim.ride_at(30.0, 0.0, 0.0, 0.0);
    let from = sim.device.sim_mut().speaker.played.len();
    sim.start_ride();
    // 1 km at 30 km/h is two minutes.
    sim.advance(minutes(2));
    sim.wait_for("LAP 1 02:", seconds(20)).unwrap();
    sim.checkpoint("auto lap at 1 km");
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(30));
    assert!(recording(&mut sim), "a stop does not pause");
    sim.expect_no_text("Auto paused");
    sim.checkpoint("stopped, still recording");
    assert_eq!(played_since(&sim, from), Vec::<u8>::new(), "muted");
}
