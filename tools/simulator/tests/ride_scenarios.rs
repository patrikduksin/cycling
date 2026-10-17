//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
#![cfg(feature = "cycling")]
use device_api::ant::{LinkState, Request};
use device_api::ble_transport::Link;
use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::log::{Entry, Kind, SLOT_SIZE, SLOTS, Sample, Source, commit_word, encode};
use vana::screens::chrome::{AMBER, GREEN, RED, SECONDARY, ZONES};

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;
const RADAR: u16 = 31;

fn rider() -> Sim {
    equip(Sim::new().boot())
}

fn equip(mut sim: Sim) -> Sim {
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
    sim.home("RIDE");
    let screen = sim.screen();
    for line in ["READY", "GPS LOCKED", "3 SENSORS LIVE", "HR  PWR  SPD/C"] {
        assert!(screen.contains(line), "{line}; screen:\n{}", screen.text());
    }
    sim.checkpoint("ready with every sensor live");
}

#[test]
fn ride_shows_live_metrics_and_records_the_gps_track() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(5));
    // Wheel events arrive in quarter-second packets, so the last digit wobbles.
    let speed = sim.screen().field("KM/H").unwrap();
    assert!(
        ["27.9", "28.0", "28.1"].contains(&speed.as_str()),
        "{speed}"
    );
    sim.expect_field("W", "210");
    sim.expect_field("BPM", "142");
    assert_eq!(sim.screen().elapsed().as_deref(), Some("0:05:00"));
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
    sim.expect_field("W", "190");
    sim.checkpoint("power reading");

    // A short loss keeps the channel tracking; readings blank, then resume.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("W", "--");
    sim.checkpoint("power meter out of range");

    sim.set_present(device_type::POWER, POWER, true);
    let waited = sim.wait_for("190", seconds(10)).unwrap();
    assert!(waited < seconds(3), "recovered after {waited} ms");
    sim.expect_field("W", "190");
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

    // Ready tells the rider the interrupted ride was kept, then they ride on.
    sim.home("RIDE");
    sim.expect_text("LAST RIDE RECOVERED");
    sim.checkpoint("recovered ride named on Ready");
    sim.press(device_api::input::Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(seconds(20));
    sim.stop_ride();
    let rides = sim.rides();
    assert_eq!(rides.len(), 2);
    assert_ne!(rides[0].id, rides[1].id);
    assert_eq!(rides[1].kinds().last(), Some(&Kind::Finish));
}

/// Radar alerts sounded: the 300 ms pattern, which ride chimes never use.
fn alerts(sim: &Sim) -> usize {
    sim.device
        .speaker
        .played
        .iter()
        .filter(|(_, id)| *id == 22)
        .count()
}

/// A finished ride that leaves `free` sample slots before the terminal slot.
fn old_ride_leaving(free: usize) -> Vec<u8> {
    let used = SLOTS - 2 - free;
    let mut bytes = Vec::with_capacity(used * SLOT_SIZE);
    let sample = |s: u64| Sample {
        active_ms: s * 1_000,
        ..Sample::default()
    };
    for index in 0..used {
        let sequence = index as u32;
        let entry = if index == 0 {
            Entry::event(Kind::Start, Source::Live, 1, 0, 0)
        } else if index == used - 1 {
            Entry::event(
                Kind::Finish,
                Source::Live,
                1,
                sequence,
                index as u64 * 4_000,
            )
        } else {
            let first = (index as u64 - 1) * 4 + 1;
            let batch = [
                sample(first),
                sample(first + 1),
                sample(first + 2),
                sample(first + 3),
            ];
            Entry::batch(Source::Live, 1, sequence, &batch).unwrap()
        };
        let mut slot = encode(&entry);
        slot.0[SLOT_SIZE - 4..].copy_from_slice(&commit_word().0);
        bytes.extend_from_slice(&slot.0);
    }
    bytes
}

#[test]
fn nearly_full_storage_warns_and_a_full_journal_closes_the_ride() {
    use device_api::storage::{OwnedFlash, Region};
    let mut sim = Sim::new();
    // An earlier, un-exported ride leaves ten minutes of 1 Hz recording.
    sim.device
        .media
        .program(Region::Data, 0, &old_ride_leaving(150))
        .unwrap();
    let mut sim = equip(sim.boot());
    sim.pair(&["HEART RATE"]);
    // Rolling with GPS from the start, so no auto-pause interrupts the count.
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.home("RIDE");
    sim.expect_text("ROOM FOR 10 MIN");
    sim.expect_text("EXPORT OLD RIDES");
    sim.checkpoint("ready shows ten minutes of storage");
    sim.press(device_api::input::Button::BottomRight);
    sim.wait_for("REC 9M LEFT", seconds(2)).unwrap();
    sim.advance(minutes(5));
    sim.expect_text("REC 4M LEFT");
    sim.checkpoint("recording with low storage");

    sim.advance(minutes(6));
    sim.expect_text("STORAGE FULL");
    sim.checkpoint("journal full, ride closed");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(
        status.contains("state=full") && status.contains("free_s=0"),
        "{status}"
    );
    let rides = sim.rides();
    assert_eq!(rides.len(), 2, "the earlier ride is never reclaimed");
    let ride = &rides[1];
    assert_eq!(ride.kinds().last(), Some(&Kind::Full));
    // Exactly the ten minutes the warning promised; later samples are dropped.
    assert_eq!(ride.samples().count(), 600);
    let (reply, info) = sim.command("EXPORT INFO");
    assert_eq!(reply, "OK", "{info}");
    assert!(info.contains(&format!(" {SLOTS} full")), "{info}");

    // Rebooting a full journal keeps both rides and still refuses to record.
    sim.power_loss();
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=full"), "{status}");
    assert_eq!(sim.rides().len(), 2);
}

#[test]
fn power_lost_during_the_first_write_of_a_fresh_journal_still_records() {
    let mut sim = rider();
    sim.home("RIDE");
    sim.expect_text("READY");
    // Power fails ten bytes into the very first START record.
    sim.fail_storage(0, 10);
    sim.press(device_api::input::Button::BottomRight);
    sim.wait_for("STORAGE ERROR", seconds(2)).unwrap();
    sim.checkpoint("first write torn");

    sim.power_loss();
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=ready"), "{status}");
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 120.0, 0.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.checkpoint("ride saved after the torn first write");
    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Finish));
    assert!(rides[0].samples().count() >= 29);
}

#[test]
fn approaching_vehicle_is_drawn_and_sounds_an_alert() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(5));
    // Starting chimed once; no alert yet.
    assert_eq!(alerts(&sim), 0);
    assert!(!sim.device.speaker.played.is_empty(), "the start chime");

    sim.vehicle(Vehicle {
        range_m: 120.0,
        closing_kmh: 40.0,
        fast: true,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
    assert_eq!(alerts(&sim), 0, "alert while far away");
    // Six seconds from reaching the rider, VANA beeps.
    sim.advance(seconds(4));
    assert_eq!(alerts(&sim), 1, "no alert when close");
    sim.checkpoint("vehicle close, alert sounded");
    sim.advance(seconds(15));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 0, "vehicle passed");
}

/// The channel times out while the sensor is away; VANA keeps searching for
/// the saved identity with bounded backoff, so watts return by themselves.
#[test]
fn long_power_meter_loss_reconnects_when_it_returns() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("W", "190");

    // Beyond the companion's search timeout the channel closes.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(40));
    sim.expect_field("W", "--");
    sim.checkpoint("power meter gone for 40 s");

    sim.set_present(device_type::POWER, POWER, true);
    sim.wait_for("190", seconds(30)).unwrap();
    sim.expect_field("W", "190");
    sim.checkpoint("power meter back without visiting sensors");
    sim.advance(seconds(10));
    sim.stop_ride();
    // The gap is recorded as missing power, never bridged with old watts.
    let ride = &sim.rides()[0];
    let gap = ride.samples().filter(|s| s.power_watts.is_none()).count();
    assert!(gap >= 35, "{gap} samples without power");
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
    sim.ride_at(30.0, 0.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(30));
    let gps = sim.screen().run("GPS").unwrap();
    assert_eq!(gps.color, GREEN, "GPS shown as fixed");
    sim.checkpoint("gps fixed");

    sim.set_gps_fix(false);
    sim.advance(seconds(30));
    let gps = sim.screen().run("GPS").unwrap();
    assert_ne!(gps.color, GREEN, "GPS still shown as fixed in the tunnel");
    // Wheel speed keeps working without satellites. The truncated decimal
    // depends on where the sample lands between wheel revolutions.
    expect_near(&mut sim, "KM/H", 30.0, 0.15);
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
    assert!(alerts(&sim) >= 5, "alerts sounded");
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
    sim.ride_at(30.0, 220.0, 145.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(60));
    sim.checkpoint("one hour in");

    sim.ride_at(0.0, 0.0, 95.0, 0.0);
    sim.pause_ride();
    sim.advance(minutes(10));
    assert_eq!(sim.screen().elapsed().as_deref(), Some("1:00:00"));
    sim.checkpoint("cafe stop, paused");
    sim.resume_ride();

    sim.ride_at(27.0, 190.0, 138.0, 86.0);
    sim.advance(minutes(60));
    assert_eq!(sim.screen().elapsed().as_deref(), Some("2:00:00"));
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

/// The number under `label`, for readings that may differ in the last digit.
#[track_caller]
fn reading(sim: &Sim, label: &str) -> f64 {
    let screen = sim.screen();
    let value = screen
        .field(label)
        .unwrap_or_else(|| panic!("no {label}; screen:\n{}", screen.text()));
    value
        .parse()
        .unwrap_or_else(|_| panic!("{label} shows {value:?}; screen:\n{}", screen.text()))
}

#[track_caller]
fn expect_near(sim: &mut Sim, label: &str, expected: f64, tolerance: f64) {
    let value = reading(sim, label);
    assert!(
        (value - expected).abs() <= tolerance,
        "{label} shows {value}, expected {expected} ± {tolerance}; screen:\n{}",
        sim.screen().text()
    );
    sim.note(NoteKind::Check, format!("{label} shows {value}"));
}

fn ride_page(sim: &mut Sim, indicator: &str) {
    sim.ride_page(indicator);
}

#[test]
fn stats_page_totals_a_steady_ten_minutes() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    expect_near(&mut sim, "KM", 5.0, 0.05);
    sim.expect_field("RPM", "90");
    sim.checkpoint("main page after ten minutes");

    ride_page(&mut sim, "2/4");
    assert!(
        sim.screen().run("KM/H").is_none(),
        "main page units are gone"
    );
    // Sensors take a few seconds to report after the start.
    expect_near(&mut sim, "DIST KM", 5.0, 0.05);
    expect_near(&mut sim, "AVG KM/H", 30.0, 0.2);
    expect_near(&mut sim, "MAX KM/H", 30.0, 0.3);
    sim.expect_field("AVG W", "200");
    sim.expect_field("MAX W", "200");
    sim.expect_field("NP W", "200");
    sim.expect_field("AVG BPM", "140");
    sim.expect_field("AVG RPM", "90");
    let moving = sim.screen().field("MOVING").unwrap();
    assert!(
        moving.starts_with("09:5") || moving == "10:00",
        "moving {moving}"
    );
    sim.checkpoint("stats page after ten minutes");

    // The summary keeps the totals; its pages wrap, TopLeft is Home.
    sim.stop_ride();
    sim.expect_field("AVG W", "200");
    sim.checkpoint("summary after saving");
    sim.ride_page("2/3");
    sim.expect_field("NP W", "200");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("1/3"));
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn ascent_counts_the_climbs_of_a_hilly_loop() {
    let mut sim = Sim::new().boot();
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ant_sensor(device_type::POWER, POWER);
    // A 4 km loop that rises 50 m, drops 100 m and climbs back: 100 m of ascent.
    sim.ride_route(Route::loop_around(
        Point {
            ele: Some(600.0),
            ..Point::new(-33.45, -70.66)
        },
        4_000.0,
        50.0,
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["SPEED/CAD", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(24.0, 230.0, 0.0, 80.0);
    sim.advance(minutes(5));
    sim.checkpoint("climbing on the main page");
    ride_page(&mut sim, "2/4");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "ASCENT M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    // The final climb brought up the climb page; back to the totals.
    ride_page(&mut sim, "2/4");
    expect_near(&mut sim, "DIST KM", 4.0, 0.05);
    // The 3 m band and smoothing may leave the last metres of the final climb.
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    sim.checkpoint("stats after one hilly lap");
}

#[test]
fn pause_leaves_the_totals_untouched() {
    const TOTALS: [&str; 6] = ["DIST KM", "MOVING", "AVG W", "MAX W", "NP W", "AVG BPM"];
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    ride_page(&mut sim, "2/4");
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
    sim.expect_field("MAX W", "250");
    sim.checkpoint("stats after five paused minutes");

    sim.resume_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    expect_near(&mut sim, "DIST KM", 5.0, 0.08);
    sim.expect_field("AVG W", "250");
    let moving = sim.screen().field("MOVING").unwrap();
    assert!(moving.starts_with("09:5") || moving == "10:00", "{moving}");
    sim.checkpoint("stats after resuming");
    // The main page agrees: the pause is not ride time.
    ride_page(&mut sim, "1/4");
    let active = sim.screen().elapsed().unwrap();
    assert!(active.starts_with("0:10:0"), "{active}");
    sim.checkpoint("main page after resuming");
}

const ZONE_GREY: u16 = ZONES[0];
const ZONE_BLUE: u16 = ZONES[1];
const ZONE_GREEN: u16 = ZONES[2];
const ZONE_YELLOW: u16 = ZONES[3];
const ZONE_ORANGE: u16 = ZONES[4];
const ZONE_RED: u16 = ZONES[5];
const ZONE_PURPLE: u16 = ZONES[6];

fn pixel(sim: &Sim, x: usize, y: usize) -> u16 {
    let screen = sim.screen();
    screen.pixels[y * screen.width + x]
}

/// The main page's power (first) or heart rate (second) zone label, such as
/// "Z4", and its colour.
#[track_caller]
fn zone_chip(sim: &Sim, heart: bool) -> (String, u16) {
    let screen = sim.screen();
    let mut zones: Vec<_> = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.smooth && run.text.len() == 2 && run.text.starts_with('Z'))
        .collect();
    zones.sort_by_key(|run| run.y);
    let run = zones
        .get(usize::from(heart))
        .unwrap_or_else(|| panic!("no zone label; screen:\n{}", screen.text()));
    (run.text.clone(), run.color)
}

#[track_caller]
fn expect_chip(sim: &mut Sim, right: bool, zone: &str, colour: u16) {
    let (text, band) = zone_chip(sim, right);
    assert_eq!((text.as_str(), band), (zone, colour), "zone chip");
    sim.note(NoteKind::Check, format!("{zone} chip in {colour:#06x}"));
}

/// TopLeft until Home: past the last ride page, or out of Settings.
#[track_caller]
fn go_home(sim: &mut Sim) {
    for _ in 0..4 {
        if sim.on_home() {
            return;
        }
        sim.press(Button::TopLeft);
    }
    panic!("not Home; screen:\n{}", sim.screen().text());
}

#[test]
fn zone_chips_take_the_colour_of_each_power_zone() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    // Default profile: FTP 215 W, LTHR 185 bpm.
    for (watts, bpm, power, heart) in [
        (100.0, 120.0, ("Z1", ZONE_GREY), ("Z1", ZONE_GREY)),
        (150.0, 155.0, ("Z2", ZONE_BLUE), ("Z2", ZONE_BLUE)),
        (180.0, 170.0, ("Z3", ZONE_GREEN), ("Z3", ZONE_GREEN)),
        (210.0, 178.0, ("Z4", ZONE_YELLOW), ("Z4", ZONE_YELLOW)),
        (245.0, 188.0, ("Z5", ZONE_ORANGE), ("Z5", ZONE_ORANGE)),
        (300.0, 190.0, ("Z6", ZONE_RED), ("Z5", ZONE_ORANGE)),
        (450.0, 192.0, ("Z7", ZONE_PURPLE), ("Z5", ZONE_ORANGE)),
    ] {
        sim.ride_at(30.0, watts, bpm, 90.0);
        sim.advance(seconds(5));
        sim.expect_field("W", &format!("{watts}"));
        expect_chip(&mut sim, false, power.0, power.1);
        expect_chip(&mut sim, true, heart.0, heart.1);
        sim.checkpoint(&format!("{watts} W in {}", power.0));
    }
    // A lost reading leaves a dim Z-.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("W", "--");
    sim.expect_text("Z-");
    sim.checkpoint("no power, no zone colour");
}

#[test]
fn raising_ftp_in_settings_moves_the_same_watts_down_a_zone() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 0.0, 90.0);
    sim.advance(seconds(10));
    expect_chip(&mut sim, false, "Z5", ZONE_ORANGE);
    sim.checkpoint("250 W at FTP 215");
    sim.stop_ride();
    go_home(&mut sim);

    sim.home("SETTINGS");
    sim.expect_field("FTP WATTS", "215");
    sim.expect_field("LTHR BPM", "185");
    sim.expect_text("Z4 195-226 W");
    sim.checkpoint("settings with the default profile");
    // BottomRight edits the selected field: it fills and the touch pads appear.
    sim.press(Button::BottomRight);
    sim.expect_text("+5");
    sim.checkpoint("editing FTP");
    sim.press(Button::TopLeft);
    sim.expect_no_text("+5");
    go_home(&mut sim);

    sim.edit_profile(250, 185);
    sim.expect_text("Z4 226-263 W");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    sim.checkpoint("settings with FTP 250");
    go_home(&mut sim);

    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("W", "250");
    expect_chip(&mut sim, false, "Z4", ZONE_YELLOW);
    sim.checkpoint("250 W at FTP 250");
}

#[test]
fn training_profile_survives_power_loss() {
    let mut sim = rider();
    let (_, before) = sim.command("PROFILE");
    assert_eq!(
        before, "ftp=215 lthr=185 stored=missing persistence=saved",
        "a fresh device has the defaults"
    );
    // Edited by touch this time: tap a field, then the minus/plus pads.
    sim.edit_profile_by_touch(280, 170);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    sim.checkpoint("profile saved");

    sim.power_loss();
    let (status, after) = sim.command("PROFILE");
    assert_eq!(status, "OK");
    assert_eq!(after, "ftp=280 lthr=170 stored=current persistence=saved");
    sim.home("SETTINGS");
    sim.expect_field("FTP WATTS", "280");
    sim.expect_field("LTHR BPM", "170");
    sim.checkpoint("profile after power loss");

    // The terminal edits the same profile; an edit that never settled before
    // power was cut is not saved.
    let (status, reply) = sim.command("PROFILE LTHR 175");
    assert_eq!(status, "OK");
    assert_eq!(reply, "ftp=280 lthr=175 stored=current persistence=pending");
    assert_eq!(sim.command("PROFILE FTP 9000").0, "INVALID");
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=170 stored=current persistence=saved"
    );
    sim.command("PROFILE LTHR 175");
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 stored=current persistence=saved"
    );
    // Shell preferences saved later keep the profile.
    assert_eq!(sim.command("BRIGHTNESS 70").0, "OK");
    assert_eq!(sim.command("SAVE").0, "OK");
    sim.power_loss();
    assert!(sim.command("SETTINGS").1.contains("brightness=70"));
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 stored=current persistence=saved"
    );
}

#[test]
fn stats_page_splits_time_between_two_power_zones() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    // Five minutes of endurance (Z2), then five of VO2 (Z5); heart rate in Z1.
    sim.ride_at(30.0, 150.0, 130.0, 90.0);
    sim.advance(minutes(5));
    sim.ride_at(30.0, 250.0, 130.0, 90.0);
    sim.advance(minutes(5));
    ride_page(&mut sim, "2/4");
    sim.expect_text("PWR");
    // Without a radar the page is centred: the power bar spans x 70..218 at
    // y 286..294, with 2 px of black between zones; the heart bar sits below.
    let bar: Vec<u16> = (70..218).map(|x| pixel(&sim, x, 290)).collect();
    let bar: Vec<u16> = bar.into_iter().filter(|&c| c != 0).collect();
    let blue = bar.iter().filter(|&&c| c == ZONE_BLUE).count();
    let orange = bar.iter().filter(|&&c| c == ZONE_ORANGE).count();
    assert_eq!(blue + orange, bar.len(), "only Z2 and Z5: {bar:x?}");
    // The gap between the zones comes out of the first.
    assert!(
        (blue + 2).abs_diff(orange) <= 4,
        "{blue} blue, {orange} orange"
    );
    assert_eq!(bar[0], ZONE_BLUE, "lower zones first");
    assert!((70..218).all(|x| pixel(&sim, x, 306) == ZONE_GREY));
    sim.note(
        NoteKind::Check,
        format!("power bar {blue} px Z2, {orange} px Z5; heart all Z1"),
    );
    sim.checkpoint("time in zone after two power levels");
}

/// The text run that reads exactly `text`, and its colour.
#[track_caller]
fn color_of(sim: &Sim, text: &str) -> u16 {
    sim.screen()
        .run(text)
        .unwrap_or_else(|| panic!("no {text:?}; screen:\n{}", sim.screen().text()))
        .color
}

#[test]
fn manual_laps_compare_each_effort_with_the_last() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(5));
    let toast = sim.lap(1);
    // Sensors take a few seconds to report after the start.
    assert!(
        toast.starts_with("LAP 1 04:5") || toast.starts_with("LAP 1 05:0"),
        "{toast}"
    );
    sim.checkpoint("lap toast after the first lap");
    sim.advance(seconds(2));
    sim.expect_no_text("LAP 1 0");
    assert!(sim.screen().run("REC").is_some(), "{}", sim.screen().text());

    // A harder, faster second lap shows green with a plus.
    sim.ride_at(33.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(4));
    ride_page(&mut sim, "3/4");
    sim.expect_text("LAP 2");
    let lap = sim.screen().field("LAP TIME").unwrap();
    assert!(lap.starts_with("04:0"), "{lap}");
    expect_near(&mut sim, "LAP KM", 2.2, 0.05);
    // Within a watt or a beat: the change of effort spans a second.
    expect_near(&mut sim, "LAP W", 250.0, 1.0);
    expect_near(&mut sim, "LAP KM/H", 33.0, 0.2);
    expect_near(&mut sim, "LAP BPM", 150.0, 1.0);
    sim.expect_text("LAP 1");
    sim.expect_field("LAP 1 W", "200");
    let harder = sim.screen().field("W DIFF").unwrap();
    assert!(harder == "+49" || harder == "+50", "{harder}");
    assert_eq!(color_of(&sim, &harder), GREEN);
    let faster = sim.screen().field("KM/H DIFF").unwrap();
    assert!(
        faster.starts_with("+2.") || faster.starts_with("+3."),
        "{faster}"
    );
    assert_eq!(color_of(&sim, &faster), GREEN);
    sim.checkpoint("lap page, second lap harder");

    // An easy third lap is plain grey with a minus: not an alert.
    let toast = sim.lap(2);
    assert!(
        toast.ends_with(" 250W") || toast.ends_with(" 249W"),
        "{toast}"
    );
    sim.ride_at(24.0, 150.0, 130.0, 85.0);
    sim.advance(minutes(3));
    sim.expect_text("LAP 3");
    sim.expect_text("LAP 2");
    let previous: f64 = sim.screen().field("LAP 2 W").unwrap().parse().unwrap();
    let easier = reading(&sim, "W DIFF");
    assert!((easier - (150.0 - previous)).abs() <= 1.0, "{easier}");
    assert_eq!(
        color_of(&sim, &sim.screen().field("W DIFF").unwrap()),
        SECONDARY
    );
    let slower = sim.screen().field("KM/H DIFF").unwrap();
    assert!(
        slower.starts_with("-8.") || slower.starts_with("-9."),
        "{slower}"
    );
    assert_eq!(color_of(&sim, &slower), SECONDARY);
    sim.checkpoint("lap page, easy third lap");
    sim.stop_ride();
    // The summary counts the three laps, the open one included.
    sim.ride_page("3/3");
    sim.expect_field("LAPS", "3");
}

#[test]
fn auto_lap_closes_every_five_kilometres() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    ride_page(&mut sim, "3/4");
    sim.advance(minutes(9) + seconds(50));
    sim.expect_text("LAP 1");
    expect_near(&mut sim, "LAP KM", 4.9, 0.05);
    // 5 km at 30 km/h is ten minutes, plus the seconds sensors take to report.
    sim.wait_for("LAP 1 10:", seconds(30)).unwrap();
    let toast = sim.screen().find("LAP 1 10:").remove(0).text;
    assert!(toast.ends_with(" 220W"), "{toast}");
    sim.checkpoint("auto lap toast at 5 km");
    sim.expect_text("LAP 2");
    sim.expect_text("LAP 1");
    let previous = sim.screen().field("LAP 1").unwrap();
    assert!(previous.starts_with("10:0"), "{previous}");
    // The wheel reads 29.9 km/h, so each 5 km takes a few seconds over ten minutes.
    sim.advance(minutes(10));
    sim.wait_for("LAP 3", seconds(10)).unwrap();
    let previous = sim.screen().field("LAP 2").unwrap();
    assert!(previous.starts_with("10:0"), "{previous}");
    // A steady ride matches its last lap: no sign, plain grey.
    sim.advance(seconds(30));
    sim.expect_field("W DIFF", "0");
    assert_eq!(color_of(&sim, "0"), SECONDARY);
    expect_near(&mut sim, "KM/H DIFF", 0.0, 0.2);
    sim.checkpoint("lap page after two auto laps");
}

#[test]
fn paused_time_is_not_lap_time() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    ride_page(&mut sim, "3/4");
    sim.advance(minutes(2));
    sim.lap(1);
    sim.advance(minutes(2));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pause_ride();
    sim.show_ride_pages();
    let held = sim.screen().field("LAP TIME").unwrap();
    assert!(held.starts_with("02:0"), "{held}");
    sim.advance(minutes(5));
    sim.expect_field("LAP TIME", &held);
    sim.checkpoint("paused, lap time held");

    sim.resume_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    let lap = sim.screen().field("LAP TIME").unwrap();
    // Whole seconds: where the frame falls within a second can show 02:59.
    assert!(lap.starts_with("03:0") || lap == "02:59", "{lap}");
    sim.checkpoint("lap time after resuming");
}

#[test]
fn a_second_lap_press_within_three_seconds_is_ignored() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    ride_page(&mut sim, "3/4");
    sim.advance(minutes(1));
    sim.lap(1);
    // A double press, as a glove or a pothole makes.
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    sim.expect_text("LAP 2");
    sim.expect_no_text("LAP 3");
    sim.checkpoint("double press makes one lap");
    sim.advance(seconds(3));
    sim.lap(2);
    sim.expect_text("LAP 3");
}

#[test]
fn ending_a_ride_needs_a_pause_first() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    // No button labels on the ride screen; the status bar names the state.
    for label in ["PAUSE", "LAP", "STOP", "START"] {
        assert!(sim.screen().run(label).is_none(), "{label} label shown");
    }
    // Recording, TopLeft laps however often it is pressed.
    for _ in 0..3 {
        sim.press(Button::TopLeft);
        sim.expect_no_text("END RIDE?");
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");

    // Paused, the whole screen says so; TopLeft asks, BACK keeps riding.
    sim.pause_ride();
    sim.expect_text("RIDE TIME");
    sim.checkpoint("paused");
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    assert_eq!(sim.vana_page(), "end_ride");
    sim.checkpoint("end ride?");
    // The question waits; nothing ends the ride but an answer.
    sim.advance(seconds(10));
    sim.expect_text("END RIDE?");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=paused"), "{status}");
    sim.press(Button::TopLeft);
    sim.expect_text("PAUSED");
    sim.expect_no_text("END RIDE?");
    // Asked again and answered with ACT: saved, then the summary.
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE SAVED", seconds(5)).unwrap();
    assert_eq!(sim.vana_page(), "summary");
    sim.checkpoint("ride saved summary");
    assert_eq!(sim.rides()[0].kinds().last(), Some(&Kind::Finish));
    sim.done();
}

#[test]
fn ending_a_ride_right_after_a_lap_shows_the_prompt() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    sim.lap(1);
    // The lap toast is still up; pause and end anyway.
    sim.press(Button::BottomRight);
    assert_eq!(sim.ride_state(), "paused");
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.expect_no_text("LAP 1 ");
    sim.checkpoint("end ride asked over a lap toast");
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE SAVED", seconds(5)).unwrap();

    // The helpers cope with a toast too: lap, then stop at once.
    sim.done();
    sim.start_ride();
    sim.advance(seconds(10));
    sim.lap(1);
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 2);
}

/// Flat for 1 km at 600 m, 1.5 km at 6% to 690 m, 1.5 km down, then flat.
fn hill_rider() -> Sim {
    let mut sim = Sim::new().boot();
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::profile(
        Point::new(-33.45, -70.66),
        &[
            (0.0, 600.0),
            (1000.0, 600.0),
            (2500.0, 690.0),
            (4000.0, 600.0),
            (6000.0, 600.0),
        ],
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["SPEED/CAD", "POWER METER"]);
    sim
}

/// Ride on, a second at a time, until `metres` along the route.
fn ride_to(sim: &mut Sim, metres: f64) {
    while sim.world().distance_m < metres {
        sim.advance(seconds(1));
    }
}

/// Ride on until `needle` shows, returning where along the route it appeared.
#[track_caller]
fn ride_until_shown(sim: &mut Sim, needle: &str, before_m: f64) -> f64 {
    while !sim.screen().contains(needle) {
        assert!(
            sim.world().distance_m < before_m,
            "no {needle} by {before_m} m; screen:\n{}",
            sim.screen().text()
        );
        sim.advance(seconds(1));
    }
    let at = sim.world().distance_m;
    sim.note(NoteKind::Check, format!("{needle} at {at:.0} m"));
    at
}

/// Ride on until the page dots read `page`, returning where along the route.
#[track_caller]
fn ride_until_page(sim: &mut Sim, page: &str, before_m: f64) -> f64 {
    while sim.screen().page().as_deref() != Some(page) {
        assert!(
            sim.world().distance_m < before_m,
            "no page {page} by {before_m} m; screen:\n{}",
            sim.screen().text()
        );
        sim.advance(seconds(1));
    }
    let at = sim.world().distance_m;
    sim.note(NoteKind::Check, format!("page {page} at {at:.0} m"));
    at
}

/// A percentage reading such as "6.0%".
#[track_caller]
fn expect_percent(sim: &mut Sim, label: &str, expected: f64, tolerance: f64) {
    let screen = sim.screen();
    let value = screen
        .field(label)
        .unwrap_or_else(|| panic!("no {label}; screen:\n{}", screen.text()));
    let number: f64 = value
        .trim_end_matches('%')
        .parse()
        .unwrap_or_else(|_| panic!("{label} shows {value:?}"));
    assert!(
        (number - expected).abs() <= tolerance,
        "{label} shows {value}, expected {expected}% ± {tolerance}; screen:\n{}",
        screen.text()
    );
    sim.note(NoteKind::Check, format!("{label} shows {value}"));
}

#[test]
fn climb_page_measures_a_six_percent_climb_over_the_top() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "4/4");
    ride_to(&mut sim, 800.0);
    sim.expect_text("NO CLIMB");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    sim.expect_field("ASCENT M", "0");
    expect_percent(&mut sim, "GRADE", 0.0, 0.5);
    sim.checkpoint("climb page on the flat");

    // Up at 14 km/h. 20 m of gain confirms the climb about 330 m in.
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1200.0);
    sim.expect_text("NO CLIMB");
    let confirmed = ride_until_shown(&mut sim, "CLIMBING", 1600.0);
    assert!(
        (1300.0..1450.0).contains(&confirmed),
        "climb confirmed at {confirmed:.0} m"
    );

    ride_to(&mut sim, 1750.0);
    expect_percent(&mut sim, "GRADE", 6.0, 0.5);
    expect_near(&mut sim, "GAIN M", 45.0, 4.0);
    expect_near(&mut sim, "LENGTH KM", 0.75, 0.05);
    expect_percent(&mut sim, "AVG GRADE", 6.0, 0.4);
    // 3.9 m/s at 6% is 840 m/h.
    expect_near(&mut sim, "VAM M/H", 840.0, 60.0);
    expect_near(&mut sim, "ALTITUDE M", 645.0, 3.0);
    let time = sim.screen().field("CLIMB TIME").unwrap();
    assert!(time.starts_with("03:"), "{time}");
    sim.checkpoint("climb page mid-climb");

    ride_to(&mut sim, 2510.0);
    sim.expect_text("CLIMBING");
    expect_near(&mut sim, "GAIN M", 90.0, 4.0);
    expect_near(&mut sim, "LENGTH KM", 1.5, 0.05);
    sim.checkpoint("climb page at the top");

    // Over the top at 40 km/h the climb ends within about 200 m.
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let ended = ride_until_shown(&mut sim, "NO CLIMB", 2800.0);
    assert!(ended > 2500.0, "climb ended at {ended:.0} m");
    ride_to(&mut sim, 4300.0);
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(20));
    sim.expect_text("NO CLIMB");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    expect_near(&mut sim, "ASCENT M", 90.0, 4.0);
    expect_near(&mut sim, "LAST GAIN", 90.0, 4.0);
    expect_near(&mut sim, "LAST KM", 1.5, 0.08);
    expect_percent(&mut sim, "LAST AVG", 6.0, 0.4);
    let time = sim.screen().field("LAST TIME").unwrap();
    // 1.5 km at 14 km/h is 6:26.
    assert!(
        time.starts_with("06:") || time.starts_with("07:0"),
        "{time}"
    );
    sim.checkpoint("climb page after the descent");
}

#[test]
fn a_climb_brings_up_the_climb_page_and_hands_back_the_main_page() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    let shown = ride_until_page(&mut sim, "4/4", 1600.0);
    assert!((1300.0..1450.0).contains(&shown), "at {shown:.0} m");
    sim.expect_text("CLIMBING");
    sim.checkpoint("climb page brought up by the climb");

    ride_to(&mut sim, 2500.0);
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let back = ride_until_page(&mut sim, "1/4", 2800.0);
    assert!(back > 2500.0, "back at {back:.0} m");
    expect_near(&mut sim, "KM/H", 40.0, 0.3);
    sim.checkpoint("main page again over the top");
}

#[test]
fn a_silent_barometer_mid_climb_is_a_gap_not_a_flat_road() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_until_shown(&mut sim, "CLIMBING", 1600.0);
    ride_to(&mut sim, 1600.0);

    // 400 m of climbing without pressure readings: the climb page stays.
    sim.set_barometer(false);
    while sim.world().distance_m < 2000.0 {
        sim.advance(seconds(1));
        assert_eq!(sim.screen().page().as_deref(), Some("4/4"));
        sim.expect_text("CLIMBING");
    }
    sim.checkpoint("climbing through a barometer gap");
    sim.set_barometer(true);
    ride_to(&mut sim, 2400.0);
    sim.expect_text("CLIMBING");
    expect_near(&mut sim, "GAIN M", 84.0, 5.0);
    sim.checkpoint("climbing after the barometer returns");
}

fn link(sim: &Sim, kind: u8) -> Option<LinkState> {
    shell_simulator::ant::link(&sim.device.ant, kind, sim.device.now)
}

/// Let time pass until the channel of this kind is connected.
#[track_caller]
fn wait_connected(sim: &mut Sim, kind: u8, timeout_ms: u64) {
    let mut waited = 0;
    while link(sim, kind) != Some(LinkState::Connected) {
        assert!(
            waited < timeout_ms,
            "{kind} not connected after {waited} ms: {:?}",
            link(sim, kind)
        );
        sim.advance(100);
        waited += 100;
    }
    sim.note(NoteKind::Check, format!("ANT type {kind} connected"));
}

/// Connect requests the companion received since it last started.
fn connects(sim: &Sim, kind: u8) -> Vec<u16> {
    sim.device
        .ant
        .log
        .iter()
        .filter_map(|(_, request)| match request {
            Request::Connect { identity } if identity.device_type == kind => {
                Some(identity.device_number)
            }
            _ => None,
        })
        .collect()
}

/// Open Ready from Home and read its sensors line and the names under it.
#[track_caller]
fn expect_ready_sensors(sim: &mut Sim, lines: &[&str]) {
    sim.home("RIDE");
    sim.expect_text("READY");
    for line in lines {
        sim.expect_text(line);
    }
}

#[test]
fn paired_sensors_reconnect_after_power_loss_without_visiting_sensors() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.advance(seconds(1));
    sim.power_loss();
    sim.checkpoint("home after reboot");
    wait_connected(&mut sim, device_type::HEART_RATE, seconds(5));
    wait_connected(&mut sim, device_type::POWER, seconds(5));
    // Saved identities were asked for directly: no discovery ran.
    assert!(
        sim.device
            .ant
            .log
            .iter()
            .all(|(_, request)| !matches!(request, Request::Scan { .. })),
        "{:?}",
        sim.device.ant.log
    );
    assert_eq!(connects(&sim, device_type::HEART_RATE), [HEART]);
    assert_eq!(connects(&sim, device_type::POWER), [POWER]);

    sim.home("SENSORS");
    sim.press(Button::BottomLeft);
    sim.expect_text("LIVE DATA");
    sim.checkpoint("sensors after reboot");
    go_home(&mut sim);

    expect_ready_sensors(&mut sim, &["2 SENSORS LIVE", "HR", "PWR"]);
    sim.checkpoint("ready after reboot");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.advance(seconds(5));
    sim.expect_field("W", "205");
    sim.expect_field("BPM", "138");
    sim.checkpoint("riding after reboot");
}

#[test]
fn sensor_that_wakes_late_connects_after_boot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    sim.advance(seconds(1));
    // The strap is not worn yet when the device powers on.
    sim.set_present(device_type::HEART_RATE, HEART, false);
    sim.power_loss();
    sim.advance(seconds(25));
    assert_ne!(
        link(&sim, device_type::HEART_RATE),
        Some(LinkState::Connected)
    );
    sim.home("SENSORS");
    sim.press(Button::BottomLeft);
    let screen = sim.screen();
    assert!(
        screen.contains("SEARCHING")
            || screen.contains("NOT FOUND YET")
            || screen.contains("WAITING FOR RADIO"),
        "{}",
        screen.text()
    );
    sim.checkpoint("heart strap still asleep");
    go_home(&mut sim);
    sim.advance(seconds(60));
    // Searches back off instead of hammering the radio.
    let searches = connects(&sim, device_type::HEART_RATE).len();
    assert!((3..=8).contains(&searches), "{searches} searches");

    sim.set_present(device_type::HEART_RATE, HEART, true);
    wait_connected(&mut sim, device_type::HEART_RATE, seconds(45));
    sim.start_ride();
    sim.ride_at(20.0, 0.0, 131.0, 0.0);
    sim.advance(seconds(5));
    sim.expect_field("BPM", "131");
    sim.checkpoint("late strap reading");
}

#[test]
fn disconnect_is_remembered_across_reboot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.home("SENSORS");
    sim.disconnect_sensor("HEART RATE");
    // The tile goes at once; its pages are ignored from now on.
    assert!(
        sim.device
            .sdk
            .saved_sensors()
            .of_type(device_type::HEART_RATE)
            .is_none()
    );
    sim.expect_no_text("LIVE / ");
    sim.checkpoint("heart rate disconnected");
    // The channel closes once the cancelled discovery has released the radio.
    let mut waited = 0;
    while link(&sim, device_type::HEART_RATE) != Some(LinkState::Disconnected) {
        assert!(waited < seconds(15), "{:?}", sim.device.ant.log);
        sim.advance(100);
        waited += 100;
    }
    // Disconnect cancelled the search. A new one offers the strap, still
    // nearby, for explicit selection again.
    sim.press(Button::BottomRight);
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    let mut offered = false;
    for _ in 0..12 {
        let screen = sim.screen();
        if screen.run("HEART RATE").is_some() && screen.contains("READY TO CONNECT") {
            offered = true;
            break;
        }
        sim.press(Button::BottomLeft);
    }
    assert!(offered, "{}", sim.screen().text());
    sim.checkpoint("heart rate offered as nearby");
    go_home(&mut sim);

    sim.power_loss();
    wait_connected(&mut sim, device_type::POWER, seconds(5));
    // Less than the dimming timeout, so the next press is not a wake press.
    sim.advance(seconds(20));
    assert!(connects(&sim, device_type::HEART_RATE).is_empty());
    assert_eq!(link(&sim, device_type::HEART_RATE), None);
    expect_ready_sensors(&mut sim, &["1 SENSOR LIVE", "PWR"]);
    sim.expect_no_text("HR");
    sim.checkpoint("ready without the removed sensor");
}

#[test]
fn same_type_replacement_is_saved_and_survives_reboot() {
    let mut sim = rider();
    const SECOND_HEART: u16 = 5151;
    sim.ant_sensor(device_type::HEART_RATE, SECOND_HEART);
    sim.home("SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("HEART RATE");
    let first = sim
        .device
        .sdk
        .saved_sensors()
        .of_type(device_type::HEART_RATE);
    let first = first.unwrap().1.device_number;
    sim.replace_sensor("HEART RATE");
    sim.checkpoint("heart rate replaced");
    let second = if first == HEART { SECOND_HEART } else { HEART };
    assert_eq!(
        sim.device
            .sdk
            .saved_sensors()
            .of_type(device_type::HEART_RATE)
            .map(|(_, peer)| peer.device_number),
        Some(second)
    );
    go_home(&mut sim);

    sim.power_loss();
    wait_connected(&mut sim, device_type::HEART_RATE, seconds(5));
    assert_eq!(connects(&sim, device_type::HEART_RATE), [second]);
    expect_ready_sensors(&mut sim, &["1 SENSOR LIVE", "HR"]);
    sim.checkpoint("replacement after reboot");
}

#[test]
fn saved_sensors_keep_the_training_profile() {
    let mut sim = rider();
    sim.edit_profile(250, 175);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(1));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=250 lthr=175 stored=current persistence=saved"
    );
    wait_connected(&mut sim, device_type::POWER, seconds(5));
}

fn recording(sim: &mut Sim) -> bool {
    sim.command("RIDE STATUS").1.contains("state=recording")
}

/// The ride clock in seconds.
fn elapsed_secs(sim: &Sim) -> u64 {
    let clock = sim.screen().elapsed().expect("ride clock");
    clock
        .split(':')
        .map(|part| part.parse::<u64>().unwrap())
        .fold(0, |secs, part| secs * 60 + part)
}

#[test]
fn traffic_light_stop_auto_pauses_and_excludes_the_wait() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    // Rolling already, so the start does not read as a stop.
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(30));
    assert_eq!(sim.screen().elapsed().as_deref(), Some("0:00:30"));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    let latency = sim.wait_for("AUTO PAUSED", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-pause took {latency} ms");
    println!("auto-pause latency {latency} ms");
    // The paused screen names an automatic pause in amber.
    let paused = sim.screen().run("AUTO PAUSED").expect("AUTO PAUSED");
    assert_eq!(paused.color, AMBER);
    sim.expect_text("RIDE ON TO RESUME");
    sim.checkpoint("auto paused at the light");
    sim.advance(seconds(60));
    sim.expect_text("AUTO PAUSED");
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO RESUME", seconds(10)).unwrap();
    assert!(recording(&mut sim));
    sim.checkpoint("auto resumed when the light turns green");
    sim.advance(seconds(20));
    sim.expect_no_text("AUTO");
    // About 30 + 7 + 20 s riding; the 60 s wait is not in there.
    let secs = elapsed_secs(&sim);
    assert!((50..=75).contains(&secs), "active {secs} s");
    sim.stop_ride();
    let kinds = sim.rides()[0].kinds();
    assert!(kinds.contains(&Kind::Pause) && kinds.contains(&Kind::Resume));
}

#[test]
fn riding_off_starts_the_ride_by_itself() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.home("RIDE");
    sim.expect_text("READY");
    sim.advance(seconds(5));
    assert!(sim.rides().is_empty());
    sim.checkpoint("waiting at standstill");
    sim.ride_at(20.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO START", seconds(15)).unwrap();
    assert!(recording(&mut sim));
    sim.checkpoint("auto started");
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
    assert!(sim.screen().run("PAUSED").is_some());
    sim.expect_no_text("AUTO");
    sim.expect_text("FOR 01:00");
    assert!(!recording(&mut sim));
    sim.checkpoint("manual pause while riding");
}

#[test]
fn a_speed_sensor_dropout_does_not_pause_the_ride() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    sim.set_present(device_type::SPEED_CADENCE, SPEED, false);
    sim.advance(minutes(2));
    assert!(recording(&mut sim));
    sim.expect_no_text("AUTO PAUSED");
    sim.checkpoint("speed sensor lost, still recording");
    sim.set_present(device_type::SPEED_CADENCE, SPEED, true);
    sim.advance(seconds(10));
    assert!(recording(&mut sim));
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
    sim.expect_field("KM/H", "--");
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
    sim.expect_no_text("AUTO PAUSED");
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
        sim.start_ride();
        sim.advance(minutes(10));
        // Doppler is exact; positions jitter, so their speed wobbles a little.
        let tolerance = if doppler { 0.15 } else { 1.5 };
        expect_near(&mut sim, "KM/H", 25.0, tolerance);
        expect_near(&mut sim, "KM", 4.17, 0.08);
        sim.expect_no_text("AUTO PAUSE OFF");
        if doppler {
            sim.checkpoint("GPS only: speed and distance");
        }
        ride_page(&mut sim, "2/4");
        expect_near(&mut sim, "DIST KM", 4.17, 0.08);
        expect_near(&mut sim, "AVG KM/H", 25.0, 0.5);
        if doppler {
            sim.checkpoint("GPS only: stats after ten minutes");
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
        let latency = sim.wait_for("AUTO PAUSED", seconds(20)).unwrap();
        println!("GPS doppler={doppler} auto-pause latency {latency} ms");
        assert!(latency <= seconds(8), "auto-pause took {latency} ms");
        if doppler {
            sim.checkpoint("GPS only: auto paused at the light");
        }
        let km = paused_km(&sim);
        // Two minutes at the light: jitter neither resumes nor adds distance.
        sim.advance(minutes(2));
        sim.expect_text("AUTO PAUSED");
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
        // Stopping right after the resume: PAUSED and RIDE SAVED still show.
        sim.stop_ride();
    }
}

/// The ride distance the paused screen shows, as in "0.42 KM".
#[track_caller]
fn paused_km(sim: &Sim) -> String {
    let text = sim.screen().text();
    text.lines()
        .find_map(|line| line.strip_suffix(" KM"))
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
    sim.expect_field("KM/H", "--");
    assert!(recording(&mut sim));
    sim.expect_no_text("AUTO PAUSED");
    sim.checkpoint("GPS only: in the tunnel");
    let km = reading(&sim, "KM");
    sim.advance(seconds(20));
    assert_eq!(reading(&sim, "KM"), km, "distance without a speed");
    sim.set_gps_fix(true);
    sim.advance(seconds(20));
    // 80 of the 120 s measured at 30 km/h: 0.67 km. The track shows the gap.
    expect_near(&mut sim, "KM", 0.67, 0.03);
    expect_near(&mut sim, "KM/H", 30.0, 0.2);
    sim.checkpoint("GPS only: speed back after the tunnel");
    assert!(recording(&mut sim));
    sim.stop_ride();
    let ride = &sim.rides()[0];
    let blind = ride.samples().filter(|s| s.speed_mm_s.is_none()).count();
    assert!((38..=45).contains(&blind), "{blind} samples without speed");
}

#[test]
fn gps_only_rider_rolling_off_starts_the_ride() {
    let mut sim = gps_rider(true);
    sim.home("RIDE");
    sim.wait_for("GPS LOCKED", seconds(5)).unwrap();
    // Walking the bike out is not a start.
    sim.ride_at(5.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(30));
    assert!(sim.rides().is_empty());
    sim.ride_at(20.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO START", seconds(15)).unwrap();
    assert!(recording(&mut sim));
    sim.checkpoint("GPS only: auto started");
}

/// Advance in firmware passes until the panel lights; returns the uptime then.
fn wait_lit(sim: &mut Sim) -> u64 {
    while !sim.device.lit() {
        assert!(sim.device.now < seconds(15), "panel never lit");
        sim.advance(10);
    }
    sim.device.now
}

/// The milestones VANA reports for `mise run startup-timing -- --sim`, which
/// reads them from `STARTUP_TIMING_OUT`. Simulated timings are modelled.
#[test]
fn startup_timing() {
    let mut sim = Sim::new();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    let lit = wait_lit(&mut sim);
    sim.checkpoint("power-on: brand flash");
    sim.wait_usable();
    let home = sim.device.now;
    sim.checkpoint("power-on: Home ready");
    sim.expect_text("READY TO RIDE");
    assert!(home - lit <= 500, "brand flash {} ms", home - lit);
    assert!(home <= seconds(5), "Home at {home} ms uptime");
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 180.0, 130.0, 85.0);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.resume_ride();
    let (status, timing) = sim.command("RIDE TIMING");
    assert_eq!(status, "OK", "{timing}");
    for key in [
        "first_frame_ms",
        "home_ms",
        "storage_ms",
        "gnss_fix_ms",
        "ride_press_ms",
        "recording_ms",
    ] {
        assert!(
            !timing.contains(&format!("{key}=-")),
            "{key} missing: {timing}"
        );
    }
    if let Ok(path) = std::env::var("STARTUP_TIMING_OUT") {
        std::fs::write(path, &timing).expect("timing output is writable");
    }
    sim.stop_ride();
}

#[test]
fn turning_off_from_home_asks_first_and_the_top_left_hold_turns_it_back_on() {
    let mut sim = Sim::new().boot();
    sim.press(Button::TopLeft);
    sim.expect_text("TURN OFF");
    sim.checkpoint("turn off sheet");
    // BACK keeps it on.
    sim.press(Button::TopLeft);
    sim.expect_text("READY TO RIDE");
    sim.press(Button::TopLeft);
    sim.report(Button::BottomRight);
    sim.advance(80);
    sim.expect_text("TURNING OFF");
    sim.checkpoint("turning off");
    sim.advance(seconds(1));
    assert!(sim.is_off(), "device still on");
    sim.checkpoint("off");
    // Presses other than the power-on hold do nothing while off.
    sim.press(Button::BottomRight);
    sim.press(Button::TopLeft);
    assert!(sim.is_off());
    sim.advance(minutes(1));
    let usable = sim.turn_on();
    assert!(usable <= seconds(5), "Home {usable} ms after turning on");
    sim.checkpoint("Home after turning on");

    // Separate top-left taps never add up to a hold.
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    for _ in 0..shell_simulator::sim::POWER_ON_REPORTS * 2 {
        sim.press(Button::TopLeft);
    }
    assert!(sim.is_off(), "taps turned the device on");

    // Holding the top-left button, as the live window does, also turns it on.
    sim.hold(Button::TopLeft, seconds(2));
    assert!(!sim.is_off(), "a top-left hold did not turn the device on");
    sim.wait_usable();
}

/// Power on closes the VANA ring round the wordmark while Home gets ready;
/// turning off unwinds it to black before the device powers down.
#[test]
fn the_brand_ring_closes_on_power_on_and_unwinds_on_power_off() {
    use vana::screens::chrome::ACCENT;
    let accent = |sim: &Sim| {
        let screen = sim.screen();
        screen.pixels.iter().filter(|&&p| p == ACCENT).count()
    };
    let mut sim = Sim::new();
    let lit = wait_lit(&mut sim);
    sim.expect_text("VANA");
    sim.checkpoint("reveal: the first lit frame");
    let first = accent(&sim);
    sim.advance(200);
    let half = accent(&sim);
    sim.checkpoint("reveal: the ring half closed");
    assert!(half > first + 300, "the ring closes: {first} then {half}");
    let usable = sim.wait_usable();
    assert!(
        usable + 200 <= 500,
        "Home {} ms after the first frame",
        usable + 200
    );
    let _ = lit;
    sim.expect_text("READY TO RIDE");

    sim.press(Button::TopLeft);
    sim.report(Button::BottomRight);
    sim.advance(80);
    sim.expect_text("TURNING OFF");
    let start = accent(&sim);
    sim.advance(200);
    sim.checkpoint("farewell: the ring unwinding");
    let unwinding = accent(&sim);
    assert!(
        unwinding + 300 < start,
        "the ring unwinds: {start} then {unwinding}"
    );
    assert!(
        !sim.is_off(),
        "the farewell plays before the device turns off"
    );
    sim.advance(seconds(1));
    assert!(sim.is_off());
}

/// Charging shows as a green bolt and a climbing fill; off on USB power,
/// the charge shows on a ring until the screen goes dark.
#[test]
fn charging_while_off_shows_the_charge_then_goes_dark() {
    use vana::app::STANDBY_LIT_MS;
    let mut sim = Sim::new().boot();
    sim.set_battery(64, false);
    sim.advance(seconds(2));
    sim.set_battery(64, true);
    sim.advance(seconds(1));
    sim.expect_text("CHARGING 64%");
    sim.checkpoint("plugged in: charging toast and bolt");
    sim.advance(seconds(4));
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "VANA stays on while charging");
    assert!(sim.device.standby_lit(), "charging standby is dark");
    sim.expect_text("64%");
    sim.expect_text("CHARGING");
    sim.checkpoint("off on USB: the charge");
    sim.set_battery(100, true);
    sim.advance(seconds(1));
    sim.expect_text("CHARGED");
    sim.checkpoint("off on USB: charged");
    sim.advance(STANDBY_LIT_MS);
    assert!(!sim.device.standby_lit(), "standby stays lit");
    assert!(sim.screen().pixels.iter().all(|&p| p == 0));
    sim.checkpoint("off on USB: dark");
    sim.turn_on();
    sim.expect_text("READY TO RIDE");
}

/// Falling charge is announced once at 20%, then urgently at 10% and 5%,
/// on whatever page shows, ride pages included.
#[test]
fn a_falling_battery_warns_once_per_level_during_a_ride() {
    let mut sim = rider();
    sim.set_battery(40, false);
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.set_battery(20, false);
    sim.wait_for("LOW BATTERY 20%", seconds(2)).unwrap();
    sim.checkpoint("low battery during a ride");
    sim.advance(seconds(6));
    sim.expect_no_text("LOW BATTERY");
    sim.set_battery(19, false);
    sim.advance(seconds(6));
    sim.expect_no_text("BATTERY");
    sim.set_battery(10, false);
    sim.wait_for("CHARGE SOON 10%", seconds(2)).unwrap();
    sim.checkpoint("critical battery during a ride");
    sim.stop_ride();
}

#[test]
fn a_ride_must_end_before_the_device_can_turn_off() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 140.0, 0.0);
    sim.advance(seconds(30));
    // TopLeft marks a lap while recording; it never leaves the ride.
    sim.press(Button::TopLeft);
    sim.expect_no_text("TURN OFF");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    // Paused, TopLeft only asks to end the ride; BACK answers no.
    sim.pause_ride();
    sim.press(Button::TopLeft);
    sim.expect_no_text("TURN OFF");
    sim.expect_text("END RIDE?");
    sim.press(Button::TopLeft);
    sim.resume_ride();
    sim.stop_ride();
    sim.press(Button::TopLeft);
    sim.expect_text("READY TO RIDE");
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(2));
    assert!(sim.is_off());
    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Finish));
    sim.turn_on();
    let (_, status) = sim.command("RIDE STATUS");
    assert!(!status.contains("state=recovered"), "{status}");
}

#[test]
fn an_idle_device_warns_then_turns_itself_off_unless_on_usb() {
    use vana::app::{AUTO_OFF_MS, AUTO_OFF_WARNING_MS};
    let mut sim = Sim::new().boot();
    sim.set_battery(60, true);
    sim.advance(AUTO_OFF_MS + minutes(5));
    assert!(!sim.is_off(), "turned off on USB power");
    sim.expect_text("READY TO RIDE");

    sim.set_battery(60, false);
    sim.press(Button::BottomLeft);
    sim.advance(AUTO_OFF_MS - AUTO_OFF_WARNING_MS + seconds(2));
    sim.expect_text("PRESS ANY BUTTON");
    sim.checkpoint("idle off warning");
    // A press keeps it on; the first press may only wake a dimmed screen.
    sim.press(Button::BottomLeft);
    sim.expect_no_text("PRESS ANY BUTTON");
    sim.advance(AUTO_OFF_MS + seconds(5));
    assert!(sim.is_off(), "idle device stayed on");
}

#[test]
fn a_ble_heart_strap_on_home_keeps_vana_on() {
    use vana::app::AUTO_OFF_MS;
    let mut sim = Sim::new().boot();
    sim.select_ble_heart_strap();
    sim.ble_heart_strap(true);
    sim.advance(AUTO_OFF_MS + minutes(5));
    assert!(!sim.is_off(), "turned off with a notifying BLE strap");
    sim.expect_no_text("PRESS ANY BUTTON");
    // Once the strap goes away, the idle period runs out.
    sim.ble_heart_strap(false);
    sim.advance(AUTO_OFF_MS + minutes(1));
    assert!(sim.is_off(), "idle device stayed on after the strap left");
}

#[test]
fn turning_off_right_after_a_profile_edit_keeps_the_edit() {
    let mut sim = rider();
    let (_, reply) = sim.command("PROFILE FTP 225");
    assert!(reply.contains("persistence=pending"), "{reply}");
    // Off before the edit settles.
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    sim.turn_on();
    let (_, after) = sim.command("PROFILE");
    assert!(after.starts_with("ftp=225 "), "{after}");
    assert!(after.contains("persistence=saved"), "{after}");
}

#[test]
fn each_unsaved_profile_edit_is_reported_before_turning_off() {
    let mut sim = rider();
    sim.command("PROFILE FTP 225");
    sim.fail_storage(0, 0);
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 225");
    sim.expect_text("CHANGES NOT SAVED");
    // The background retry saves it.
    sim.advance(seconds(10));
    assert!(sim.command("PROFILE").1.contains("persistence=saved"));

    // A new edit whose turn-off save fails is reported again, not dropped.
    sim.command("PROFILE FTP 250");
    sim.fail_storage(0, 0);
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 250");
    sim.expect_text("CHANGES NOT SAVED");
    // Turning off again now saves it if it can.
    sim.advance(seconds(5));
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    sim.turn_on();
    assert!(sim.command("PROFILE").1.starts_with("ftp=250 "));
}

fn bulk_rider() -> Sim {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    equip(sim.boot())
}

#[test]
fn unformatted_bulk_media_records_internally_until_initialized() {
    let mut sim = bulk_rider();
    sim.pair(&["HEART RATE"]);
    sim.home("RIDE");
    sim.expect_text("ROOM FOR 4 H");
    sim.expect_text("ON INTERNAL MEMORY");
    sim.checkpoint("unformatted MMC: rides go to internal flash");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(
        status.contains("medium=internal") && status.contains("fallback=1"),
        "{status}"
    );
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    assert!(bulk.contains("state=needs_init"), "{bulk}");
    assert_eq!(sim.device.bulk.writes(), 0, "never formatted implicitly");

    sim.init_bulk();
    // The volume table and ten log metadata sectors (superblocks and ring),
    // not the 3 GB region.
    assert_eq!(sim.device.bulk.writes(), 12);
    sim.press(Button::TopLeft);
    sim.home("RIDE");
    sim.advance(seconds(1));
    sim.expect_text("ROOM FOR 2330 H");
    sim.expect_no_text("ON INTERNAL MEMORY");
    sim.checkpoint("formatted MMC: 2,330 hours left in the RIDES volume");
}

#[test]
fn bulk_media_keeps_a_long_ride_through_power_loss() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    // Longer than the whole 4.5 h internal journal.
    for _ in 0..5 {
        sim.advance(minutes(60));
    }
    assert_eq!(sim.screen().elapsed().as_deref(), Some("5:00:00"));
    sim.checkpoint("five hours recorded on MMC");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(
        status.contains("medium=bulk") && status.contains("dropped=0"),
        "{status}"
    );

    let reads = sim.device.bulk.reads();
    sim.power_loss();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert!(status.contains("state=recovered"), "{status}");
    // Boot read metadata and the records after the last checkpoint only.
    let boot_reads: u32 = status
        .split_whitespace()
        .find_map(|field| field.strip_prefix("boot_reads="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(boot_reads <= 77, "{boot_reads} boot reads");
    assert!(sim.device.bulk.reads() - reads < 200);
    sim.checkpoint("booted after power loss");

    let rides = sim.bulk_rides();
    assert_eq!(rides.len(), 1);
    let samples = rides[0].samples().count();
    assert!(samples >= 5 * 3600 - 4, "{samples} samples");
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Recovered));
    assert!(sim.rides().is_empty(), "the internal journal is untouched");

    sim.home("RIDE");
    sim.expect_text("LAST RIDE RECOVERED");
    sim.checkpoint("recovered MMC ride named on Ready");
}

#[test]
fn bulk_failure_mid_ride_continues_on_internal_flash() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.advance(minutes(10));
    let (_, status) = sim.command("RIDE BULK STATUS");
    let before: usize = status
        .split_whitespace()
        .find_map(|field| field.strip_prefix("samples="))
        .unwrap()
        .parse()
        .unwrap();

    // The next MMC write tears and the controller stays failed.
    sim.fail_bulk(0, 100);
    sim.advance(seconds(10));
    sim.wait_for("RECORDING INT", seconds(5)).unwrap();
    sim.checkpoint("MMC failed: the ride continues on internal flash");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(
        status.contains("medium=internal")
            && status.contains("state=recording")
            && status.contains("moved=1"),
        "{status}"
    );
    sim.advance(minutes(5));
    // Ride time and totals carry on across the move.
    let active = sim.screen().elapsed().unwrap();
    assert!(active.starts_with("0:15:"), "{active}");
    sim.stop_ride();
    sim.checkpoint("continued ride saved internally");

    let internal = sim.rides();
    assert_eq!(internal.len(), 1);
    assert_eq!(internal[0].kinds().last(), Some(&Kind::Finish));
    let continued = internal[0].samples().count();
    assert!(continued >= 5 * 60 + 5, "{continued} samples");

    // After reboot the MMC part is recovered and exported, with no torn data.
    sim.power_loss();
    let rides = sim.bulk_rides();
    assert_eq!(rides.len(), 1);
    assert_eq!(rides[0].samples().count(), before);
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Recovered));
}

#[test]
fn missing_bulk_media_is_shown_and_rides_record_internally() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Missing);
    let mut sim = equip(sim.boot());
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.wait_for("RECORDING INT", seconds(5)).unwrap();
    sim.checkpoint("MMC missing: recording on internal flash");
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 1);
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert!(status.contains("state=unavailable"), "{status}");
}

#[test]
fn an_internal_write_failure_after_a_move_is_never_hidden_by_the_fallback() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.advance(minutes(1));
    // The MMC fails: the ride moves to internal flash, named in amber.
    sim.fail_bulk(0, 0);
    sim.wait_for("RECORDING INT", seconds(10)).unwrap();
    sim.checkpoint("MMC failed: recording on internal flash");
    // Then internal flash fails too, and the red fault replaces the fallback.
    sim.fail_storage(0, 0);
    // Pausing asks for a write on internal flash.
    sim.press(Button::BottomRight);
    let mut shown = None;
    for _ in 0..20 {
        sim.advance(seconds(1));
        let screen = sim.screen();
        shown = ["STORAGE ERROR", "WRITE FAILED"]
            .into_iter()
            .find(|fault| screen.contains(fault));
        if shown.is_some() {
            break;
        }
    }
    let fault = shown.unwrap_or_else(|| panic!("no fault shown:\n{}", sim.screen().text()));
    sim.expect_no_text("RECORDING INT");
    sim.expect_no_text("PAUSED INT");
    sim.checkpoint("internal flash failed too: the fault shows");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(
        status_field(&status, "medium"),
        "internal",
        "{fault}: {status}"
    );
}

fn status_field(status: &str, name: &str) -> String {
    status
        .split_whitespace()
        .find_map(|field| field.strip_prefix(name)?.strip_prefix('='))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn bulk_failure_on_resume_continues_the_ride_on_internal_flash() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.advance(minutes(5));
    sim.pause_ride();
    // Nothing is written while paused; the RESUME record is the first to fail.
    sim.fail_bulk(0, 0);
    sim.advance(seconds(30));
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING INT", seconds(3)).unwrap();
    sim.checkpoint("MMC failed on resume: the ride continues on internal flash");
    sim.advance(minutes(2));
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
    assert_eq!(status_field(&status, "state"), "recording", "{status}");
    assert_eq!(status_field(&status, "moved"), "1", "{status}");
    let samples: u32 = status_field(&status, "samples").parse().unwrap();
    assert!(samples >= 100, "the continuation records: {status}");
    let active = sim.screen().elapsed().unwrap();
    assert!(active.starts_with("0:07:"), "{active}");
}

#[test]
fn bulk_failure_while_pausing_continues_paused_on_internal_flash() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.advance(minutes(5));
    sim.fail_bulk(0, 0);
    sim.press(Button::BottomRight);
    sim.wait_for("PAUSED INT", seconds(3)).unwrap();
    sim.checkpoint("MMC failed while pausing: the ride stays paused on internal flash");
    sim.advance(minutes(1));
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
    assert_eq!(status_field(&status, "state"), "paused", "{status}");
    sim.resume_ride();
    sim.wait_for("RECORDING INT", seconds(5)).unwrap();
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 1);
}

#[test]
fn a_second_ride_cannot_start_on_the_other_medium() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.advance(seconds(10));
    for command in ["RIDE INTERNAL START", "RIDE START", "RIDE BULK START"] {
        let (result, _) = sim.command(command);
        assert_eq!(result, "STATE", "{command}");
    }
    let (_, internal) = sim.command("RIDE INTERNAL STATUS");
    assert_eq!(status_field(&internal, "state"), "ready", "{internal}");
    sim.stop_ride();
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    assert_eq!(status_field(&bulk, "state"), "saved", "{bulk}");
}

#[test]
fn a_bulk_ride_that_fills_the_log_shows_saved_then_the_next_ride_goes_internal() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    // Ten records: room for one short ride and nothing more after it.
    sim.device.bulk.set_sectors(2 + 16 + 10);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.home("RIDE");
    sim.expect_text("UNDER 1 MIN OF ROOM");
    sim.press(Button::BottomRight);
    // The countdown replaces RECORDING: there is room for 32 seconds.
    sim.wait_for("REC 0M LEFT", seconds(2)).unwrap();
    sim.advance(seconds(20));
    sim.stop_ride();
    sim.checkpoint("the MMC ride is saved though the MMC has no room left");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "bulk", "{status}");
    assert_eq!(status_field(&status, "state"), "saved", "{status}");
    assert_eq!(status_field(&status, "free_s"), "0", "{status}");
    assert_eq!(status_field(&status, "fallback"), "1", "{status}");
    sim.expect_no_text("INT");
    let active = sim.screen().elapsed().unwrap();
    assert!(
        ("0:00:20"..="0:00:23").contains(&active.as_str()),
        "{active}"
    );

    // The next ride goes to internal flash, and the screen says so.
    sim.done();
    sim.home("RIDE");
    sim.expect_text("ON INTERNAL MEMORY");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING INT", seconds(3)).unwrap();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
    sim.stop_ride();

    // After a reboot nothing is shown from the MMC ride: rides go internal.
    sim.power_loss();
    sim.home("RIDE");
    sim.expect_text("ROOM FOR 4 H");
    sim.expect_text("ON INTERNAL MEMORY");
    sim.checkpoint("an MMC without room: rides record internally");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "fallback"), "1", "{status}");
}

#[test]
fn bulk_rides_without_superblocks_are_kept_out_of_export_and_clear() {
    let mut sim = bulk_rider();
    // Never formatted: nothing to export, and clear does not format it.
    assert_eq!(sim.command("EXPORT BULK INFO").0, "STATE");
    assert_eq!(sim.command("RIDE BULK CLEAR CONFIRM 0").0, "STATE");
    assert_eq!(sim.device.bulk.writes(), 0);

    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
    // Both superblock copies of the RIDES log are lost; the ride's records remain.
    sim.device.bulk.corrupt(2048);
    sim.device.bulk.corrupt(2049);
    sim.power_loss();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert_eq!(status_field(&status, "state"), "corrupt", "{status}");
    let writes = sim.device.bulk.writes();
    // Not offered as an empty journal to export or reclaim.
    assert_ne!(sim.command("EXPORT BULK INFO").0, "OK");
    assert_ne!(sim.command("RIDE BULK CLEAR CONFIRM 0").0, "ACCEPTED");
    sim.advance(seconds(5));
    assert_eq!(
        sim.device.bulk.writes(),
        writes,
        "the records stay for diagnosis"
    );
    sim.home("RIDE");
    sim.expect_text("ON INTERNAL MEMORY");
    sim.checkpoint("MMC metadata lost: kept, and rides record internally");
}

#[test]
fn a_continuation_that_cannot_start_does_not_count_the_bulk_ride_twice() {
    use device_api::storage::{OwnedFlash, Region};
    let mut sim = Sim::new();
    // Internal flash has no room for another ride.
    sim.device
        .media
        .program(Region::Data, 0, &old_ride_leaving(0))
        .unwrap();
    sim.fit_bulk(Presence::Ready);
    // The MMC holds START and ten sample batches, then fills mid-ride.
    sim.device.bulk.set_sectors(2 + 16 + 12);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.home("RIDE");
    sim.expect_text("READY");
    sim.press(Button::BottomRight);
    sim.wait_for("REC 0M LEFT", seconds(2)).unwrap();
    sim.advance(seconds(60));
    sim.expect_text("STORAGE FULL");
    sim.checkpoint("MMC and internal flash both full: the ride ends");
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    let recorded: u64 = status_field(&bulk, "active_ms").parse().unwrap();
    assert!((30_000..60_000).contains(&recorded), "{bulk}");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "moved"), "0", "{status}");
    // The screen shows the bulk ride's time once, not twice.
    let shown = sim.screen().elapsed().unwrap();
    let seconds_shown = shown.split(':').fold(0u64, |total, part| {
        total * 60 + part.parse::<u64>().unwrap()
    });
    assert_eq!(seconds_shown, recorded / 1000, "{shown}");
}

#[test]
fn an_auto_pause_the_mmc_fails_to_record_still_resumes_by_itself() {
    // Whichever write the MMC fails on around the stop (the last sample
    // batch, the pause itself or the checkpoint after it), the ride ends up
    // auto-paused on internal flash and rolls on.
    for after in 0..3 {
        let mut sim = bulk_rider();
        sim.init_bulk();
        sim.pair(&["SPEED/CAD"]);
        sim.ride_at(28.0, 0.0, 0.0, 0.0);
        sim.start_ride();
        sim.advance(seconds(30));
        sim.fail_bulk(after, 0);
        sim.ride_at(0.0, 0.0, 0.0, 0.0);
        sim.wait_for("AUTO PAUSED INT", seconds(15))
            .unwrap_or_else(|error| panic!("failing after {after}: {error}"));
        if after == 0 {
            sim.checkpoint("MMC failed at the stop: auto-paused on internal flash");
        }
        sim.advance(seconds(20));
        sim.ride_at(28.0, 0.0, 0.0, 0.0);
        sim.wait_for("RECORDING INT", seconds(10))
            .unwrap_or_else(|error| panic!("failing after {after}: {error}"));
        let (_, status) = sim.command("RIDE STATUS");
        assert_eq!(status_field(&status, "medium"), "internal", "{status}");
        assert_eq!(status_field(&status, "moved"), "1", "{status}");
    }
}

#[test]
fn a_bulk_ride_that_fails_with_internal_flash_full_stays_on_screen() {
    use device_api::storage::{OwnedFlash, Region};
    let mut sim = Sim::new();
    // Internal flash has no room for the continuation.
    sim.device
        .media
        .program(Region::Data, 0, &old_ride_leaving(0))
        .unwrap();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(30));
    sim.fail_bulk(0, 0);
    sim.advance(seconds(10));
    sim.expect_text("STORAGE ERROR");
    sim.checkpoint("MMC failed and internal flash is full: the failed ride stays shown");
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    assert_eq!(status_field(&bulk, "state"), "error", "{bulk}");
    let recorded: u64 = status_field(&bulk, "active_ms").parse().unwrap();
    assert!(recorded >= 30_000, "{bulk}");
    let shown = sim.screen().elapsed().unwrap();
    let seconds_shown = shown.split(':').fold(0u64, |total, part| {
        total * 60 + part.parse::<u64>().unwrap()
    });
    assert_eq!(seconds_shown, recorded / 1000, "{shown}");
}

#[test]
fn clearing_the_internal_journal_drops_a_moved_rides_carried_time() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
    sim.advance(minutes(1));
    sim.fail_bulk(0, 0);
    sim.wait_for("RECORDING INT", seconds(10)).unwrap();
    sim.advance(seconds(5));
    sim.stop_ride();
    let (_, internal) = sim.command("RIDE INTERNAL STATUS");
    assert_eq!(status_field(&internal, "state"), "saved", "{internal}");
    let upper = status_field(&internal, "slot");
    assert_eq!(
        sim.command(&format!("RIDE CLEAR CONFIRM {upper}")).0,
        "ACCEPTED"
    );
    sim.advance(seconds(10));
    let (_, internal) = sim.command("RIDE INTERNAL STATUS");
    assert_eq!(status_field(&internal, "slot"), "0", "{internal}");
    sim.checkpoint("internal journal cleared after a moved ride: no carried time");
    assert_eq!(sim.screen().elapsed().unwrap(), "0:00:00");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "moved"), "0", "{status}");
}

#[test]
fn a_failed_first_mmc_start_after_an_internal_ride_shows_the_storage_error() {
    let mut sim = bulk_rider();
    sim.pair(&["HEART RATE"]);
    // An internal ride is saved before the MMC is initialized.
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    // The summary shows the saved ride; Home stays put for the next one.
    sim.done();
    sim.advance(seconds(2));
    sim.command("RIDE STATUS");
    sim.init_bulk();
    // The MMC's first write, the START record, fails.
    sim.fail_bulk(0, 0);
    let (result, _) = sim.command("RIDE START");
    assert_eq!(result, "ACCEPTED");
    sim.advance(seconds(2));
    // The failed MMC ride is not shown as saved; the next ride records
    // internally, and Ready says where.
    sim.home("RIDE");
    sim.expect_no_text("RIDE SAVED");
    sim.expect_text("ON INTERNAL MEMORY");
    sim.checkpoint("the MMC fails its first START: Ready falls back to internal");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "bulk", "{status}");
    assert_eq!(status_field(&status, "state"), "error", "{status}");
}

#[test]
fn home_shows_ride_sensors_and_the_last_ride_at_a_glance() {
    let mut sim = rider();
    // Battery shows as an icon; the percentage appears once it runs low.
    sim.set_battery(14, false);
    sim.advance(seconds(2));
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.expect_text("NONE PAIRED");
    sim.expect_no_text("NEXT");
    sim.checkpoint("home before pairing");

    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.wait_for("3/3 LIVE", seconds(5)).unwrap();
    sim.expect_text("GPS READY");
    sim.expect_text("14%");
    sim.checkpoint("home with three live sensors");

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.stop_ride();
    for _ in 0..3 {
        if sim.on_home() {
            break;
        }
        sim.press(Button::TopLeft);
    }
    sim.wait_gone("RIDE SAVED", seconds(3)).unwrap();
    sim.expect_text("RIDES");
    let last = sim.screen().find(" KM").pop().expect("last ride distance");
    assert!(
        last.text.starts_with("4.9") || last.text.starts_with("5.0"),
        "{}",
        last.text
    );
    sim.checkpoint("home after a ride");
}

#[test]
fn tapping_home_and_sensor_tiles_pairs_a_sensor() {
    let mut sim = rider();
    // The SENSORS tile, then the first nearby sensor once the scan is done.
    sim.tap(120, 210);
    sim.expect_text("MY SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_no_text("NEXT");
    sim.checkpoint("sensors menu");
    sim.tap(36, 172);
    sim.wait_for("LIVE DATA", seconds(15)).unwrap();
    sim.checkpoint("sensor connected by tap");
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.expect_text("1/1 LIVE");
    sim.checkpoint("home after tapping a sensor");
    // The RIDE tile opens Ready.
    sim.tap(120, 80);
    sim.expect_text("READY");
    sim.expect_text("1 SENSOR LIVE");
}

#[test]
fn status_bar_and_toasts_follow_the_ride_state() {
    let mut sim = rider();
    sim.set_battery(22, false);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 150.0, 88.0);
    sim.expect_text("RECORDING");
    sim.checkpoint("recording toast");
    sim.advance(seconds(5));
    sim.expect_no_text("RECORDING");
    let rec = sim.screen().run("REC").expect("REC in the status bar");
    assert_eq!((rec.y < 24, rec.color), (true, RED));
    sim.checkpoint("recording, status bar only");
    sim.advance(minutes(1));
    sim.lap(1);
    sim.checkpoint("lap toast");
    sim.advance(seconds(3));

    sim.pause_ride();
    sim.advance(seconds(3));
    // The paused screen, then the ride pages with PAUSED in the status bar.
    let paused = sim.screen().run("PAUSED").expect("PAUSED");
    assert_eq!((paused.y > 24, paused.color), (true, AMBER));
    sim.checkpoint("paused screen");
    sim.show_ride_pages();
    let paused = sim
        .screen()
        .run("PAUSED")
        .expect("PAUSED in the status bar");
    assert_eq!((paused.y < 24, paused.color), (true, AMBER));
    sim.checkpoint("paused, ride pages");

    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.checkpoint("end ride?");
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE SAVED", seconds(5)).unwrap();
    sim.checkpoint("ride saved summary");
    assert!(sim.screen().run("REC").is_none());
}

#[test]
fn swipes_page_through_the_ride_and_taps_never_control_it() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(2));
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));

    sim.swipe_next();
    assert_eq!(sim.screen().page().as_deref(), Some("2/4"));
    sim.expect_text("AVG W");
    sim.checkpoint("swiped to stats");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("4/4"), "wraps");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("3/4"));
    sim.expect_text("LAP TIME");
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("KM/H");

    // Taps where the buttons are, and on readings, change nothing.
    for (x, y) in [(20, 300), (220, 300), (20, 20), (120, 80), (120, 150)] {
        sim.tap(x, y);
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    sim.expect_no_text("END RIDE?");
    sim.checkpoint("still recording after taps");
}

/// The whole ride lifecycle as a rider meets it: Ready, the start, a lap,
/// pausing and paging, a change of mind at "End ride?", then saving, the
/// summary pages and Done back Home.
#[test]
fn the_ride_lifecycle_walks_from_ready_to_the_summary() {
    let mut sim = rider();
    sim.set_battery(22, false);
    sim.home("RIDE");
    assert_eq!(sim.vana_page(), "ready");
    // Nothing paired: Ready says so in words, and still lets the ride start.
    sim.expect_text("ALMOST READY");
    sim.expect_text("NO SENSORS");
    sim.expect_text("TAP HERE TO PAIR ONE");
    sim.expect_text("CHARGE AFTER THE RIDE");
    sim.checkpoint("ready, almost: no sensors and a low battery");
    sim.press(Button::TopLeft);
    assert!(sim.on_home());

    sim.set_battery(80, false);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.home("RIDE");
    sim.expect_text("READY");
    sim.expect_no_text("ALMOST");
    sim.checkpoint("ready: GPS, three sensors, battery and room");

    // ACT starts at once: the ride pages, a RECORDING toast and a chime.
    let chimes = sim.device.speaker.played.len();
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    assert_eq!(sim.vana_page(), "ride");
    // The second note follows the first's guard.
    sim.advance(seconds(1));
    assert_eq!(
        sim.device.speaker.played.len(),
        chimes + 2,
        "start cue: two notes"
    );
    sim.checkpoint("recording");
    sim.ride_at(30.0, 220.0, 140.0, 90.0);
    sim.advance(minutes(6));
    sim.lap(1);
    sim.checkpoint("lap");
    sim.advance(minutes(1));

    // Pause fills the screen and counts the pause.
    sim.pause_ride();
    sim.advance(seconds(12));
    sim.expect_text("PAUSED");
    sim.expect_text("FOR 00:12");
    sim.checkpoint("paused screen");
    // MOVE shows the ride pages under PAUSED, and past the last page the
    // paused screen comes back.
    sim.show_ride_pages();
    sim.checkpoint("paused, ride pages");
    for _ in 0..8 {
        if sim.screen().contains("RIDE TIME") {
            break;
        }
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("RIDE TIME");

    // BACK asks; BACK again keeps the ride.
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.checkpoint("end ride?");
    sim.press(Button::TopLeft);
    assert_eq!(sim.ride_state(), "paused");
    // Resuming chimes too.
    let chimes = sim.device.speaker.played.len();
    sim.resume_ride();
    sim.wait_for("RESUMED", seconds(2)).unwrap();
    assert_eq!(sim.device.speaker.played.len(), chimes + 1, "resume chime");
    sim.advance(minutes(1));

    // Now end it: the summary shows the ride over three pages.
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "summary");
    sim.expect_text("RIDE SAVED");
    sim.expect_field("AVG W", "220");
    sim.checkpoint("summary: ride saved");
    sim.swipe_next();
    sim.expect_field("NP W", "220");
    sim.checkpoint("summary: effort and zones");
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    sim.expect_field("LAPS", "2");
    sim.checkpoint("summary: best power, climbs and laps");
    sim.done();
    sim.checkpoint("home after the ride");
}

/// Rolling off from Ready starts the ride, and a stop at a light shows the
/// whole-screen automatic pause until the rider rolls on.
#[test]
fn rolling_off_from_ready_starts_and_stopping_auto_pauses() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.home("RIDE");
    sim.expect_text("READY");
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_for("REC", seconds(10)).unwrap();
    assert_eq!(sim.vana_page(), "ride");
    sim.checkpoint("auto start from ready");
    sim.advance(minutes(1));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(15)).unwrap();
    sim.expect_text("RIDE ON TO RESUME");
    sim.checkpoint("auto paused screen");
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_gone("AUTO PAUSED", seconds(15)).unwrap();
    sim.expect_no_text("RIDE TIME");
    assert!(recording(&mut sim));
    sim.checkpoint("auto resumed onto the ride pages");
}

/// A ride started from the host while the summary shows takes the rider
/// to its controls, so the buttons can pause and end it.
#[test]
fn a_ride_started_remotely_from_the_summary_shows_its_controls() {
    let mut sim = rider();
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "summary");
    sim.ride_page("3/3");
    sim.command("RIDE STATUS");
    assert_eq!(sim.command("RIDE START").0, "ACCEPTED");
    sim.advance(seconds(2));
    assert_eq!(sim.vana_page(), "ride");
    sim.pause_ride();
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 2);
    // The new summary opens at its first page.
    assert_eq!(sim.screen().page().as_deref(), Some("1/3"));
}

/// Swipe Settings to the section that shows `label`.
#[track_caller]
fn show_setting(sim: &mut Sim, label: &str) {
    for _ in 0..6 {
        if sim.screen().run(label).is_some() {
            return;
        }
        sim.swipe_next();
    }
    panic!("no {label} section; screen:\n{}", sim.screen().text());
}

/// The patterns played since the `from`th.
fn played_since(sim: &Sim, from: usize) -> Vec<u8> {
    let played = &sim.device.speaker.played;
    played[from..].iter().map(|(_, id)| *id).collect()
}

#[test]
fn settings_group_display_ride_sound_and_device_and_keep_every_choice() {
    let mut sim = rider();
    sim.home("SETTINGS");
    sim.expect_text("PROFILE");
    sim.expect_field("FTP WATTS", "215");
    sim.checkpoint("settings open on the profile");
    // MOVE walks the rows, past the last profile row into Display.
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("DISPLAY");
    sim.expect_field("BRIGHTNESS", "50%");
    sim.expect_field("DIM AFTER", "30 S");
    sim.checkpoint("display section");
    // ACT steps a choice; the backlight follows at once.
    sim.press(Button::BottomRight);
    sim.expect_field("BRIGHTNESS", "75%");
    assert_eq!(sim.device.shell.settings.brightness, 75);
    sim.set_option("DIM AFTER", "1 MIN");
    assert_eq!(sim.device.shell.settings.dim_timeout_secs, 60);
    sim.checkpoint("brighter, dims after a minute");

    sim.swipe_next();
    sim.expect_text("RIDE");
    sim.expect_field("AUTO PAUSE", "ON");
    sim.expect_field("AUTO LAP", "EVERY 5 KM");
    sim.set_option("AUTO PAUSE", "OFF");
    sim.set_option("AUTO LAP", "EVERY 1 KM");
    sim.checkpoint("ride section");
    sim.swipe_next();
    sim.set_option("SOUNDS", "OFF");
    sim.expect_text("RADAR STAYS ON");
    sim.checkpoint("sounds off");
    sim.swipe_next();
    sim.expect_text("DEVICE");
    sim.expect_field("SENSORS", "NONE SAVED");
    let firmware = sim.screen().field("FIRMWARE").unwrap();
    assert!(firmware.starts_with("VANA "), "{firmware}");
    sim.checkpoint("device section");
    // Swiping on from the last section wraps to the first.
    sim.swipe_next();
    sim.expect_text("PROFILE");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    sim.advance(seconds(3));
    // The phone row opens the phone companion's page, a placeholder for now.
    sim.swipe_previous();
    sim.expect_text("DEVICE");
    let phone = sim.screen().run("PHONE").unwrap();
    sim.tap(phone.x as u16 + 4, phone.y as u16 + 4);
    sim.expect_text("SEND RIDES TO YOUR PHONE");
    sim.checkpoint("phone placeholder");
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());

    sim.power_loss();
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.contains("stored=current"), "{profile}");
    assert_eq!(sim.device.shell.settings.brightness, 75);
    sim.home("SETTINGS");
    show_setting(&mut sim, "BRIGHTNESS");
    sim.expect_field("BRIGHTNESS", "75%");
    sim.expect_field("DIM AFTER", "1 MIN");
    show_setting(&mut sim, "AUTO PAUSE");
    sim.expect_field("AUTO PAUSE", "OFF");
    sim.expect_field("AUTO LAP", "EVERY 1 KM");
    show_setting(&mut sim, "SOUNDS");
    sim.expect_field("SOUNDS", "OFF");
    sim.checkpoint("choices kept after power loss");
}

#[test]
fn ride_cues_rise_to_go_fall_to_pause_and_mark_laps() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    let from = sim.device.speaker.played.len();
    sim.start_ride();
    sim.advance(seconds(2));
    // Middle then high: go.
    assert_eq!(played_since(&sim, from), [0, 10], "start");
    let from = sim.device.speaker.played.len();
    sim.lap(1);
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [0], "one note marks a lap");
    let from = sim.device.speaker.played.len();
    sim.pause_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [10, 0], "high then middle: stop");
    let from = sim.device.speaker.played.len();
    sim.resume_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [10], "resume");
    sim.pause_ride();
    sim.advance(seconds(2));
    let from = sim.device.speaker.played.len();
    sim.stop_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [21, 0, 10], "a rising run: saved");
    sim.checkpoint("ride saved with its cue");
}

#[test]
fn without_auto_pause_or_sounds_a_stop_keeps_recording_quietly_and_laps_follow_the_choice() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.home("SETTINGS");
    show_setting(&mut sim, "AUTO PAUSE");
    sim.set_option("AUTO PAUSE", "OFF");
    sim.set_option("AUTO LAP", "EVERY 1 KM");
    sim.swipe_next();
    sim.set_option("SOUNDS", "OFF");
    go_home(&mut sim);

    sim.ride_at(30.0, 0.0, 0.0, 0.0);
    let from = sim.device.speaker.played.len();
    sim.start_ride();
    // 1 km at 30 km/h is two minutes.
    sim.advance(minutes(2) + seconds(20));
    ride_page(&mut sim, "3/4");
    sim.expect_text("LAP 2");
    sim.checkpoint("auto lap at 1 km");
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(30));
    assert!(recording(&mut sim), "a stop does not pause");
    sim.expect_no_text("AUTO PAUSED");
    sim.checkpoint("stopped, still recording");
    assert_eq!(played_since(&sim, from), Vec::<u8>::new(), "muted");
}

#[test]
fn an_unsaved_brightness_change_is_reported_before_turning_off() {
    let mut sim = rider();
    sim.home("SETTINGS");
    show_setting(&mut sim, "BRIGHTNESS");
    sim.fail_storage(0, 0);
    sim.set_option("BRIGHTNESS", "75%");
    sim.expect_text("SAVING");
    go_home(&mut sim);
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost the brightness");
    sim.expect_text("CHANGES NOT SAVED");
    sim.checkpoint("brightness not saved, still on");
    // The background retry stores it, and it survives a power loss.
    sim.advance(seconds(10));
    sim.power_loss();
    assert_eq!(sim.device.shell.settings.brightness, 75);
}

/// Ride history: Home's RIDES card lists the saved rides, newest first, and
/// opens one read back from its journal on the summary's pages; ACT there
/// offers it to the phone.
#[test]
fn saved_rides_list_newest_first_and_read_back_onto_the_summary() {
    let mut sim = rider();
    // Before any ride, the list says how rides get there.
    sim.home("RIDES");
    assert_eq!(sim.vana_page(), "history");
    sim.expect_text("NO RIDES YET");
    sim.checkpoint("no rides yet");
    sim.press(Button::TopLeft);
    assert!(sim.on_home());

    sim.pair(&["POWER METER", "SPEED/CAD", "HEART RATE"]);
    sim.ride_at(30.0, 150.0, 130.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(3));
    sim.stop_ride();
    sim.done();
    sim.ride_at(25.0, 240.0, 150.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(5));
    sim.stop_ride();
    sim.done();
    let last = sim.screen().text();

    sim.home("RIDES");
    sim.expect_text("1/2");
    sim.checkpoint("two saved rides");
    // The newest is focused; ACT reads it back.
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "past_ride");
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("SAVED RIDE");
    sim.expect_field("AVG W", "240");
    sim.expect_field("AVG BPM", "150");
    sim.checkpoint("saved ride read back");
    sim.swipe_next();
    sim.expect_field("NP W", "240");
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    // Laps and climbs are not kept with a ride.
    sim.expect_field("LAPS", "--");
    sim.checkpoint("saved ride best power");

    // ACT offers this ride to the phone; BACK comes back to it.
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "phone");
    sim.expect_text("SEND THIS RIDE TO PHONE");
    sim.checkpoint("send a saved ride to the phone");
    sim.press(Button::TopLeft);
    assert_eq!(sim.vana_page(), "past_ride");

    // The older ride, from the list.
    sim.press(Button::TopLeft);
    assert_eq!(sim.vana_page(), "history");
    sim.press(Button::BottomLeft);
    sim.expect_text("2/2");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_field("AVG W", "150");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    assert!(sim.on_home());
    // Home still names the last ride, not the one read back.
    assert_eq!(sim.screen().text(), last);

    // A new ride takes over: its own totals, from zero.
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    sim.expect_field("AVG W", "240");
    sim.done();
}

/// A saved MMC ride on a card that later fails says it cannot be read,
/// rather than reading forever; the internal rides around it still read.
#[test]
fn a_saved_ride_on_a_failed_card_says_so_and_rides_list_newest_first() {
    let mut sim = bulk_rider();
    sim.pair(&["POWER METER"]);
    sim.init_bulk();
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    // An MMC ride, then one whose start fails and continues internally.
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
    sim.done();
    // The card fails the next ride's START; rides then record internally.
    sim.fail_bulk(0, 0);
    sim.command("RIDE STATUS");
    assert_eq!(sim.command("RIDE START").0, "ACCEPTED");
    sim.advance(seconds(2));
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert!(status.contains("state=error"), "{status}");
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    sim.done();

    sim.home("RIDES");
    sim.expect_text("1/2");
    // Newest first: the internal continuation, then the MMC ride.
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("0:01:00");
    sim.press(Button::TopLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE UNREADABLE", seconds(5)).unwrap();
    sim.checkpoint("a saved ride on a failed card");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    assert!(sim.on_home());
}

#[test]
fn riding_through_sunset_dims_the_backlight_smoothly_and_settings_can_hold_day() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    // Santiago on 14 October 2026: the sun sets at 22:51 UTC (19:51 local)
    // and the sky is dim (4 degrees down) by about 23:12.
    sim.set_gps_clock(22 * 3600 + 40 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    assert_eq!(sim.device.shell.effective(), 50, "day");
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.checkpoint("riding before sunset");
    sim.advance(minutes(30));
    assert_eq!(sim.device.shell.effective(), 50, "still light at 23:10");
    // Sample the backlight through the change: it only steps down, a few
    // percent at a time, and settles at the night level.
    let mut levels = vec![sim.device.shell.effective()];
    for _ in 0..(4 * 60 * 10) {
        sim.advance(100);
        levels.push(sim.device.shell.effective());
    }
    assert!(
        levels.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] <= 4),
        "{levels:?}"
    );
    assert_eq!(levels.last(), Some(&20), "night level");
    let ramp = levels
        .iter()
        .filter(|level| (21..50).contains(*level))
        .count();
    assert!(
        (5..=10).contains(&ramp),
        "a ramp of about a second: {ramp} steps"
    );
    assert!(recording(&mut sim));
    sim.checkpoint("after dark, the backlight is down");
    sim.stop_ride();
    go_home(&mut sim);

    // Settings says why, and DAY holds the full backlight.
    sim.home("SETTINGS");
    show_setting(&mut sim, "DAY / NIGHT");
    sim.expect_field("DAY / NIGHT", "AUTO");
    sim.expect_text("NIGHT NOW");
    sim.expect_text("NIGHT 20%");
    sim.checkpoint("settings in auto, at night");
    sim.set_option("DAY / NIGHT", "DAY");
    sim.advance(seconds(2));
    assert_eq!(sim.device.shell.effective(), 50);
    assert_eq!(sim.device.shell.settings.brightness, 50, "never stored");
    sim.checkpoint("held at day");
}

/// Losing the fix before dusk stops the receiver's clock; night still comes
/// on time from the last fix's time carried on.
#[test]
fn night_comes_on_time_after_the_gps_fix_is_lost() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.set_gps_clock(22 * 3600 + 40 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    assert_eq!(sim.device.shell.effective(), 50, "day");
    // A ride keeps the screen awake.
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.set_gps_fix(false);
    sim.advance(minutes(30));
    assert_eq!(sim.device.shell.effective(), 50, "still light at 23:10");
    sim.advance(minutes(8));
    assert_eq!(sim.device.shell.effective(), 20, "dark by 23:18");
    sim.checkpoint("night without a fix");
}
