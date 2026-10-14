//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
#![cfg(feature = "cycling")]
use device_api::ble_transport::Link;
use shell_simulator::ant::device_type;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::log::Kind;

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;
const RADAR: u16 = 31;

fn rider() -> Sim {
    let mut sim = Sim::new().boot();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

#[test]
fn paired_sensors_pass_the_connection_check() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.home("TRAIN");
    let screen = sim.screen();
    for line in [
        "SPEED 777 CONNECTED",
        "HEART 4242 CONNECTED",
        "POWER 1234 CONNECTED",
        "GPS FIX OK",
    ] {
        assert!(screen.contains(line), "{line}; screen:\n{}", screen.text());
    }
    sim.checkpoint("connection check passes");
}

#[test]
fn ride_shows_live_metrics_and_records_the_gps_track() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.advance(minutes(5));
    sim.expect_field("SPEED", "28.0");
    sim.expect_field("WATTS", "210");
    sim.expect_field("BPM", "142");
    sim.expect_field("ACTIVE TIME", "00:05:00");
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
fn power_meter_dropout_blanks_watts_and_recovers() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("WATTS", "190");
    sim.checkpoint("power reading");

    // A short loss keeps the channel tracking; readings blank, then resume.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("WATTS", "--");
    sim.checkpoint("power meter out of range");

    sim.set_present(device_type::POWER, POWER, true);
    let waited = sim.wait_for("190", seconds(10)).unwrap();
    assert!(waited < seconds(3), "recovered after {waited} ms");
    sim.expect_field("WATTS", "190");
    sim.checkpoint("power reading recovered");
    sim.advance(seconds(10));
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(ride.samples().any(|s| s.power_watts.is_none()));
    assert!(
        ride.samples()
            .filter(|s| s.power_watts == Some(190))
            .count()
            > 15
    );
}

#[test]
fn power_loss_mid_ride_keeps_the_recorded_prefix() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(30.0, 0.0, 150.0, 0.0);
    sim.advance(minutes(2));
    let before = sim.rides()[0].samples().count();
    assert!(before >= 110, "{before} samples before loss");
    sim.checkpoint("riding before power loss");

    sim.power_loss();
    sim.checkpoint("booted after power loss");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recovered"), "{status}");
    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    assert!(rides[0].samples().count() >= before);
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Recovered));
}

#[test]
fn approaching_vehicle_is_drawn_and_sounds_an_alert() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(5));
    assert!(sim.device.speaker.played.is_empty());

    sim.vehicle(Vehicle {
        range_m: 120.0,
        closing_kmh: 40.0,
        fast: true,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
    assert!(sim.device.speaker.played.is_empty(), "alert while far away");
    // Six seconds from reaching the rider, VANA beeps.
    sim.advance(seconds(4));
    assert_eq!(sim.device.speaker.played.len(), 1, "no alert when close");
    sim.checkpoint("vehicle close, alert sounded");
    sim.advance(seconds(15));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 0, "vehicle passed");
}

/// Acceptance for #130: VANA retries only `Disconnected` channels, so a
/// channel that timed out while the sensor was away is never reopened.
#[test]
#[ignore = "VANA does not retry timed-out ANT channels yet (#130)"]
fn long_power_meter_loss_reconnects_when_it_returns() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("WATTS", "190");

    // Beyond the companion's search timeout the channel closes.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(40));
    sim.expect_field("WATTS", "--");
    sim.checkpoint("power meter gone for 40 s");

    sim.set_present(device_type::POWER, POWER, true);
    sim.wait_for("190", seconds(30)).unwrap();
    sim.expect_field("WATTS", "190");
}

#[test]
fn sensors_wait_for_the_companion_after_power_on() {
    let mut sim = Sim::new();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    // Ask for sensors as soon as Home appears; the radio is still starting.
    sim.advance(seconds(7));
    sim.home("SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("HEART RATE");
    sim.checkpoint("heart rate connected");
}

#[test]
fn storage_failure_mid_ride_is_shown_and_the_prefix_recovers_after_reboot() {
    let mut sim = rider();
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 120.0, 0.0);
    sim.advance(seconds(30));
    sim.checkpoint("recording");
    let before = sim.rides()[0].samples().count();
    let last_active = sim.rides()[0].samples().last().map(|s| s.active_ms);

    // The next journal write tears after ten bytes and every later one fails.
    sim.fail_storage(0, 10);
    sim.advance(seconds(10));
    sim.expect_text("STORAGE ERROR");
    sim.checkpoint("storage error shown");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=error"), "{status}");

    sim.power_loss();
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recovered"), "{status}");
    let ride = &sim.rides()[0];
    // Exactly the committed prefix: the torn write adds no garbage sample.
    assert_eq!(ride.samples().count(), before);
    assert_eq!(ride.samples().last().map(|s| s.active_ms), last_active);
    assert_eq!(ride.kinds().last(), Some(&Kind::Recovered));
    sim.checkpoint("booted with the ride recovered");
}

#[test]
fn gps_tunnel_records_samples_without_a_position() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 0.0, 0.0, 85.0);
    sim.advance(seconds(30));
    let gps = sim.screen().run("GPS").unwrap();
    assert_eq!(gps.color, 0x07ff, "GPS shown as fixed");
    sim.checkpoint("gps fixed");

    sim.set_gps_fix(false);
    sim.advance(seconds(30));
    let gps = sim.screen().run("GPS").unwrap();
    assert_ne!(gps.color, 0x07ff, "GPS still shown as fixed in the tunnel");
    // Wheel speed keeps working without satellites.
    sim.expect_field("SPEED", "30.0");
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
fn ble_heart_strap_drives_heart_rate_and_drops_out_honestly() {
    let mut sim = rider();
    sim.ble_heart_strap(true);
    sim.start_ride();
    sim.ride_at(22.0, 0.0, 131.0, 0.0);
    // A strap in range is not used until it is selected.
    sim.advance(seconds(15));
    sim.expect_field("BPM", "--");

    // Selected: a 10 s scan, then connection and discovery.
    sim.select_ble_heart_strap();
    sim.advance(seconds(9));
    sim.expect_field("BPM", "--");
    sim.wait_for_field("BPM", "131", seconds(5));
    sim.checkpoint("ble heart rate");

    // The link survives a short gap; the reading goes stale, then the link drops.
    sim.ble_heart_strap(false);
    sim.advance(seconds(3));
    assert_eq!(sim.device.ble.snapshot.link, Link::Connected);
    sim.advance(seconds(4));
    sim.expect_field("BPM", "--");
    assert_ne!(sim.device.ble.snapshot.link, Link::Connected);
    sim.checkpoint("strap out of range");

    // Back in range, it reconnects on the next scan after the backoff.
    sim.advance(seconds(20));
    sim.ble_heart_strap(true);
    sim.wait_for_field("BPM", "131", seconds(30));

    // Forgotten, it is never used again even in range.
    let (status, _) = sim.command("BLE FORGET");
    assert_eq!(status, "ACCEPTED");
    sim.advance(seconds(20));
    sim.expect_field("BPM", "--");
}

#[test]
fn radar_tracks_a_group_of_six_vehicles_until_they_pass() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    for (i, side) in [
        Side::Behind,
        Side::Behind,
        Side::Left,
        Side::Behind,
        Side::Right,
        Side::Behind,
    ]
    .into_iter()
    .enumerate()
    {
        sim.vehicle(Vehicle {
            range_m: 60.0 + 25.0 * i as f64,
            closing_kmh: 25.0,
            fast: false,
            side,
        });
    }
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 6, "{radar:?}");
    sim.checkpoint("six vehicles behind");
    // They pass one by one: the count never rises and reaches zero.
    let mut counts = Vec::new();
    for _ in 0..40 {
        sim.advance(seconds(1));
        let radar = sim.device.sdk.radar(sim.device.now);
        counts.push(radar.targets.iter().flatten().count());
    }
    assert!(counts.windows(2).all(|w| w[1] <= w[0]), "{counts:?}");
    assert!(counts.iter().any(|&n| (1..6).contains(&n)), "{counts:?}");
    assert_eq!(counts.last(), Some(&0), "{counts:?}");
    assert!(sim.device.speaker.played.len() >= 5, "alerts sounded");
    sim.checkpoint("all vehicles passed");

    // The radar is still connected: a new vehicle is detected.
    sim.vehicle(Vehicle {
        range_m: 80.0,
        closing_kmh: 20.0,
        fast: false,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
}

#[test]
fn pause_excludes_time_and_two_hour_ride_keeps_every_sample() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 220.0, 145.0, 90.0);
    sim.advance(minutes(60));
    sim.checkpoint("one hour in");

    sim.ride_at(0.0, 0.0, 95.0, 0.0);
    sim.pause_ride();
    sim.advance(minutes(10));
    sim.expect_field("ACTIVE TIME", "01:00:00");
    sim.checkpoint("cafe stop, paused");
    sim.resume_ride();

    sim.ride_at(27.0, 190.0, 138.0, 86.0);
    sim.advance(minutes(60));
    sim.expect_field("ACTIVE TIME", "02:00:00");
    sim.checkpoint("two hours riding");
    sim.stop_ride();

    let ride = &sim.rides()[0];
    let kinds = ride.kinds();
    assert!(kinds.contains(&Kind::Pause) && kinds.contains(&Kind::Resume));
    let samples: Vec<_> = ride.samples().collect();
    assert!(
        (7_195..=7_205).contains(&samples.len()),
        "{} samples",
        samples.len()
    );
    // One sample per active second; the pause leaves no gap in active time.
    let widest = samples
        .windows(2)
        .map(|w| w[1].active_ms - w[0].active_ms)
        .max()
        .unwrap();
    assert!(widest <= 1_100, "{widest} ms between samples");
    // 30 km in the first hour and 27 km in the second.
    let km = ride.track_m() / 1000.0;
    assert!((km - 57.0).abs() < 1.0, "{km:.2} km");
}
