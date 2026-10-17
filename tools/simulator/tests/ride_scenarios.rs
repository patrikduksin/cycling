//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
#![cfg(feature = "cycling")]
use device_api::ble_transport::Link;
use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::log::{Entry, Kind, SLOT_SIZE, SLOTS, Sample, Source, commit_word, encode};

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

    // The rider is told the interrupted ride was kept, then rides on.
    sim.home("TRAIN");
    sim.wait_for("ACTIVE TIME", seconds(5)).unwrap();
    sim.expect_text("RECOVERED");
    sim.checkpoint("recovered ride named on the ride screen");
    sim.press(device_api::input::Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(seconds(20));
    sim.stop_ride();
    let rides = sim.rides();
    assert_eq!(rides.len(), 2);
    assert_ne!(rides[0].id, rides[1].id);
    assert_eq!(rides[1].kinds().last(), Some(&Kind::Finish));
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
    sim.home("TRAIN");
    sim.expect_text("STORAGE 0:10 LEFT");
    sim.checkpoint("connection check shows ten minutes of storage");
    sim.wait_for("ACTIVE TIME", seconds(5)).unwrap();
    sim.expect_text("STORAGE 10M LEFT");
    sim.press(device_api::input::Button::BottomRight);
    sim.wait_for("REC 9M LEFT", seconds(2)).unwrap();
    sim.ride_at(28.0, 0.0, 140.0, 0.0);
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
    sim.home("TRAIN");
    sim.wait_for("ACTIVE TIME", seconds(5)).unwrap();
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
    sim.expect_text("1/3");
    expect_near(&mut sim, "DISTANCE", 5.0, 0.05);
    sim.expect_field("CADENCE", "90");
    sim.checkpoint("main page after ten minutes");

    ride_page(&mut sim, "2/3");
    sim.expect_no_text("WATTS Z");
    // Sensors take a few seconds to report after the start.
    expect_near(&mut sim, "DISTANCE KM", 5.0, 0.05);
    expect_near(&mut sim, "AVG KM/H", 30.0, 0.2);
    expect_near(&mut sim, "MAX KM/H", 30.0, 0.3);
    sim.expect_field("AVG WATTS", "200");
    sim.expect_field("MAX WATTS", "200");
    sim.expect_field("NP WATTS", "200");
    expect_near(&mut sim, "WORK KJ", 120.0, 2.0);
    sim.expect_field("AVG BPM", "140");
    sim.expect_field("MAX BPM", "140");
    sim.expect_field("AVG RPM", "90");
    let moving = sim.screen().field("MOVING TIME").unwrap();
    assert!(
        moving.starts_with("00:09:5") || moving == "00:10:00",
        "moving {moving}"
    );
    sim.checkpoint("stats page after ten minutes");

    // Stopping keeps the totals on screen; pages still wrap, TopLeft is Home.
    sim.stop_ride();
    sim.expect_field("AVG WATTS", "200");
    sim.checkpoint("stats page after saving");
    sim.press(Button::BottomLeft);
    sim.expect_text("3/3");
    sim.press(Button::BottomLeft);
    sim.expect_text("1/3");
    sim.press(Button::TopLeft);
    sim.expect_text("READY TO RIDE");
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
    ride_page(&mut sim, "2/3");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "ASCENT M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    expect_near(&mut sim, "DISTANCE KM", 4.0, 0.05);
    // The 3 m band and smoothing may leave the last metres of the final climb.
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    sim.checkpoint("stats after one hilly lap");
}

#[test]
fn pause_leaves_the_totals_untouched() {
    const TOTALS: [&str; 6] = [
        "DISTANCE KM",
        "MOVING TIME",
        "AVG WATTS",
        "MAX WATTS",
        "WORK KJ",
        "MAX BPM",
    ];
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    ride_page(&mut sim, "2/3");
    expect_near(&mut sim, "DISTANCE KM", 2.5, 0.05);

    // Paused but still rolling hard: nothing the wheel or pedals do counts.
    sim.pause_ride();
    let screen = sim.screen();
    let before: Vec<_> = TOTALS.iter().map(|label| screen.field(label)).collect();
    sim.checkpoint("stats when paused");
    sim.ride_at(30.0, 400.0, 170.0, 100.0);
    sim.advance(minutes(5));
    let screen = sim.screen();
    for (label, before) in TOTALS.iter().zip(&before) {
        assert_eq!(&screen.field(label), before, "{label} changed while paused");
    }
    sim.expect_field("MAX WATTS", "250");
    sim.checkpoint("stats after five paused minutes");

    sim.resume_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    expect_near(&mut sim, "DISTANCE KM", 5.0, 0.08);
    sim.expect_field("AVG WATTS", "250");
    let moving = sim.screen().field("MOVING TIME").unwrap();
    assert!(
        moving.starts_with("00:09:5") || moving == "00:10:00",
        "{moving}"
    );
    sim.checkpoint("stats after resuming");
    // The main page agrees: the pause is not ride time.
    ride_page(&mut sim, "1/3");
    let active = sim.screen().field("ACTIVE TIME").unwrap();
    assert!(active.starts_with("00:10:0"), "{active}");
    sim.checkpoint("main page after resuming");
}

/// The text run that reads exactly `text`, and its colour.
#[track_caller]
fn color_of(sim: &Sim, text: &str) -> u16 {
    sim.screen()
        .run(text)
        .unwrap_or_else(|| panic!("no {text:?}; screen:\n{}", sim.screen().text()))
        .color
}

const GREEN: u16 = 0x07e0;
const RED: u16 = 0xf800;

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
    sim.expect_text("RECORDING");

    // A harder, faster second lap shows green with a plus.
    sim.ride_at(33.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(4));
    ride_page(&mut sim, "3/3");
    sim.expect_text("LAP 2");
    let lap = sim.screen().field("LAP TIME").unwrap();
    assert!(lap.starts_with("04:0"), "{lap}");
    expect_near(&mut sim, "LAP KM", 2.2, 0.05);
    // Within a watt or a beat: the change of effort spans a second.
    expect_near(&mut sim, "LAP AVG W", 250.0, 1.0);
    expect_near(&mut sim, "LAP KM/H", 33.0, 0.2);
    expect_near(&mut sim, "LAP AVG BPM", 150.0, 1.0);
    sim.expect_text("PREV LAP 1");
    sim.expect_field("PREV AVG W", "200");
    let harder = sim.screen().field("W VS PREV").unwrap();
    assert!(harder == "+49" || harder == "+50", "{harder}");
    assert_eq!(color_of(&sim, &harder), GREEN);
    let faster = sim.screen().field("KM/H VS PREV").unwrap();
    assert!(
        faster.starts_with("+2.") || faster.starts_with("+3."),
        "{faster}"
    );
    assert_eq!(color_of(&sim, &faster), GREEN);
    sim.checkpoint("lap page, second lap harder");

    // An easy third lap is red with a minus.
    let toast = sim.lap(2);
    assert!(
        toast.ends_with(" 250W") || toast.ends_with(" 249W"),
        "{toast}"
    );
    sim.ride_at(24.0, 150.0, 130.0, 85.0);
    sim.advance(minutes(3));
    sim.expect_text("LAP 3");
    sim.expect_text("PREV LAP 2");
    let previous: f64 = sim.screen().field("PREV AVG W").unwrap().parse().unwrap();
    let easier = reading(&sim, "W VS PREV");
    assert!((easier - (150.0 - previous)).abs() <= 1.0, "{easier}");
    assert_eq!(
        color_of(&sim, &sim.screen().field("W VS PREV").unwrap()),
        RED
    );
    let slower = sim.screen().field("KM/H VS PREV").unwrap();
    assert!(
        slower.starts_with("-8.") || slower.starts_with("-9."),
        "{slower}"
    );
    assert_eq!(color_of(&sim, &slower), RED);
    sim.checkpoint("lap page, easy third lap");
    sim.stop_ride();
    // Laps stay on screen after saving, like the totals.
    sim.expect_text("LAP 3");
}

#[test]
fn auto_lap_closes_every_five_kilometres() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    ride_page(&mut sim, "3/3");
    sim.advance(minutes(9) + seconds(50));
    sim.expect_text("LAP 1");
    expect_near(&mut sim, "LAP KM", 4.9, 0.05);
    // 5 km at 30 km/h is ten minutes, plus the seconds sensors take to report.
    sim.wait_for("LAP 1 10:", seconds(30)).unwrap();
    let toast = sim.screen().find("LAP 1 10:").remove(0).text;
    assert!(toast.ends_with(" 220W"), "{toast}");
    sim.checkpoint("auto lap toast at 5 km");
    sim.expect_text("LAP 2");
    sim.expect_text("PREV LAP 1");
    let previous = sim.screen().field("PREV LAP 1").unwrap();
    assert!(previous.starts_with("10:0"), "{previous}");
    // The wheel reads 29.9 km/h, so each 5 km takes a few seconds over ten minutes.
    sim.advance(minutes(10));
    sim.wait_for("LAP 3", seconds(10)).unwrap();
    let previous = sim.screen().field("PREV LAP 2").unwrap();
    assert!(previous.starts_with("10:0"), "{previous}");
    // A steady ride matches its last lap: no sign, no colour.
    sim.advance(seconds(30));
    sim.expect_field("W VS PREV", "0");
    assert_eq!(color_of(&sim, "0"), 0xffff);
    expect_near(&mut sim, "KM/H VS PREV", 0.0, 0.2);
    sim.checkpoint("lap page after two auto laps");
}

#[test]
fn paused_time_is_not_lap_time() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    ride_page(&mut sim, "3/3");
    sim.advance(minutes(2));
    sim.lap(1);
    sim.advance(minutes(2));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pause_ride();
    let held = sim.screen().field("LAP TIME").unwrap();
    assert!(held.starts_with("02:0"), "{held}");
    sim.advance(minutes(5));
    sim.expect_field("LAP TIME", &held);
    sim.checkpoint("paused, lap time held");

    sim.resume_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    let lap = sim.screen().field("LAP TIME").unwrap();
    assert!(lap.starts_with("03:0"), "{lap}");
    sim.checkpoint("lap time after resuming");
}

#[test]
fn a_second_lap_press_within_three_seconds_is_ignored() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    ride_page(&mut sim, "3/3");
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
    // No button labels on the ride screen; the status row names the state.
    for label in ["PAUSE", "LAP", "STOP", "START"] {
        assert!(sim.screen().run(label).is_none(), "{label} label shown");
    }
    // Recording, TopLeft laps however often it is pressed.
    for _ in 0..3 {
        sim.press(Button::TopLeft);
        sim.expect_no_text("PRESS AGAIN TO SAVE");
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");

    sim.pause_ride();
    sim.checkpoint("paused");
    sim.press(Button::TopLeft);
    sim.expect_text("PRESS AGAIN TO SAVE");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=paused"), "{status}");
    sim.checkpoint("paused, press again to save");
    // The confirmation lapses after three seconds, within a frame.
    sim.advance(seconds(3));
    sim.wait_gone("PRESS AGAIN TO SAVE", 600).unwrap();
    sim.expect_text("PAUSED");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    sim.checkpoint("saved after pause and end ride");
    assert_eq!(sim.rides()[0].kinds().last(), Some(&Kind::Finish));
    sim.press(Button::TopLeft);
    sim.expect_text("READY TO RIDE");
}

#[test]
fn ending_a_ride_right_after_a_lap_shows_the_prompt() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(1));
    sim.lap(1);
    // The lap toast still covers the status row; pause and end anyway.
    sim.press(Button::BottomRight);
    assert_eq!(sim.ride_state(), "paused");
    sim.press(Button::TopLeft);
    sim.expect_text("PRESS AGAIN TO SAVE");
    sim.expect_no_text("LAP 1 ");
    sim.checkpoint("end ride armed over a lap toast");
    sim.press(Button::TopLeft);
    sim.wait_for("SAVED", seconds(5)).unwrap();

    // The helpers cope with a toast too: lap, then stop at once.
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(seconds(10));
    sim.lap(1);
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 2);
}
