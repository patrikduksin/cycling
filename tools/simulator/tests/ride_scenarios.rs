//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::log::Kind;

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;
const RADAR: u16 = 31;

/// Choose a Home tile; the selected one is drawn black on cyan.
fn home(sim: &mut Sim, label: &str) {
    for _ in 0..3 {
        if sim.screen().run(label).is_some_and(|run| run.color == 0) {
            break;
        }
        sim.press(Button::BottomLeft);
    }
    sim.press(Button::BottomRight);
}

/// Step through nearby tiles to a sensor kind and connect it.
fn connect(sim: &mut Sim, kind: &str) {
    for _ in 0..12 {
        let screen = sim.screen();
        if screen.run(kind).is_some() && screen.run("CONNECT").is_some() {
            sim.press(Button::BottomRight);
            sim.wait_for("LIVE DATA", seconds(15)).unwrap();
            return;
        }
        sim.press(Button::BottomLeft);
    }
    panic!(
        "no {kind} tile to connect; screen:\n{}",
        sim.screen().text()
    );
}

/// Pair sensors from the Sensors page, then return Home.
fn pair(sim: &mut Sim, kinds: &[&str]) {
    home(sim, "SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    for kind in kinds {
        connect(sim, kind);
    }
    for _ in 0..3 {
        if sim.screen().contains("READY TO RIDE") {
            return;
        }
        sim.press(Button::TopLeft);
    }
    panic!(
        "Sensors did not return Home; screen:\n{}",
        sim.screen().text()
    );
}

/// Train from Home, through the connection check, onto a recording ride page.
fn start_ride(sim: &mut Sim) {
    home(sim, "TRAIN");
    sim.expect_text("CONNECTION CHECK");
    sim.wait_for("ACTIVE TIME", seconds(5)).unwrap();
    sim.press(Button::BottomLeft);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
}

fn stop_ride(sim: &mut Sim) {
    sim.press(Button::BottomRight);
    sim.expect_text("STOP TO SAVE");
    sim.press(Button::BottomRight);
    sim.wait_for("SAVED", seconds(5)).unwrap();
}

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
    pair(&mut sim, &["HEART RATE", "POWER METER", "SPEED/CAD"]);
    home(&mut sim, "TRAIN");
    let screen = sim.screen();
    for line in [
        "SPEED 777 CONNECTED",
        "HEART 4242 CONNECTED",
        "POWER 1234 CONNECTED",
        "GPS FIX OK",
    ] {
        assert!(screen.contains(line), "{line}; screen:\n{}", screen.text());
    }
}

#[test]
fn ride_shows_live_metrics_and_records_the_gps_track() {
    let mut sim = rider();
    pair(&mut sim, &["HEART RATE", "POWER METER", "SPEED/CAD"]);
    start_ride(&mut sim);
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.advance(minutes(5));
    sim.expect_field("SPEED", "28.0");
    sim.expect_field("WATTS", "210");
    sim.expect_field("BPM", "142");
    sim.expect_field("ACTIVE TIME", "00:05:00");
    stop_ride(&mut sim);

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
    pair(&mut sim, &["POWER METER"]);
    start_ride(&mut sim);
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("WATTS", "190");

    // A short loss keeps the channel tracking; readings blank, then resume.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("WATTS", "--");

    sim.set_present(device_type::POWER, POWER, true);
    let waited = sim.wait_for("190", seconds(10)).unwrap();
    assert!(waited < seconds(3), "recovered after {waited} ms");
    sim.expect_field("WATTS", "190");
    sim.advance(seconds(10));
    stop_ride(&mut sim);
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
    pair(&mut sim, &["HEART RATE"]);
    start_ride(&mut sim);
    sim.ride_at(30.0, 0.0, 150.0, 0.0);
    sim.advance(minutes(2));
    let before = sim.rides()[0].samples().count();
    assert!(before >= 110, "{before} samples before loss");

    sim.power_loss();
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
    pair(&mut sim, &["RADAR"]);
    start_ride(&mut sim);
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
    pair(&mut sim, &["POWER METER"]);
    start_ride(&mut sim);
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("WATTS", "190");

    // Beyond the companion's search timeout the channel closes.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(40));
    sim.expect_field("WATTS", "--");

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
    home(&mut sim, "SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    connect(&mut sim, "HEART RATE");
}
