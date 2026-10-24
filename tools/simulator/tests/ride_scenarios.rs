//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
#![cfg(feature = "cycling")]
use device_api::ant::{LinkState, Request};
use device_api::ble_transport::Link;
use device_api::gps::FixState;
use device_api::input::Button;
use device_api::positioning::Positioning;
use firmware_services::gps::unix_seconds;
use shell_simulator::ant::{BatteryReport, ZeroReply, device_type};
use shell_simulator::bulk::Presence;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::fit::Refusal;
use vana::ride::log::{
    Entry, Kind, SLOT_SIZE, SLOTS, Sample, Slot, Source, commit_word, decode, encode,
};
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
    sim.skip_setup();
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
fn radar_names_new_vehicles_and_calls_all_clear_when_they_pass() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    sim.vehicle(Vehicle {
        range_m: 140.0,
        closing_kmh: 30.0,
        fast: false,
        side: Side::Behind,
    });
    sim.wait_for("1 VEHICLE", seconds(3)).unwrap();
    sim.checkpoint("radar: one vehicle behind");
    sim.vehicle(Vehicle {
        range_m: 140.0,
        closing_kmh: 30.0,
        fast: false,
        side: Side::Behind,
    });
    sim.wait_for("2 VEHICLES", seconds(3)).unwrap();
    let played = sim.device.speaker.played.len();
    sim.wait_for("ALL CLEAR", seconds(30)).unwrap();
    sim.checkpoint("radar: all clear after they passed");
    sim.advance(seconds(1));
    let clear: Vec<_> = sim.device.speaker.played[played..].to_vec();
    // The all-clear cue runs low to high: 2.5 kHz, then 4 kHz.
    let ids: Vec<_> = clear.iter().map(|&(_, id)| id).collect();
    assert!(
        ids.windows(2).any(|pair| pair == [21, 10]),
        "the all-clear cue sounds: {ids:?}"
    );
}

/// The panel's top-left pixel, which no screen paints over.
fn corner(sim: &Sim) -> u16 {
    sim.screen().pixels[0]
}

/// Whether any pixel of the panel is exactly `color`.
fn shows(sim: &Sim, color: u16) -> bool {
    sim.screen().pixels.contains(&color)
}

/// The simulator's text reader matches light text on black, so it cannot read
/// these screens; the checks are on the colours the light look must produce.
#[test]
fn the_light_theme_turns_every_screen_white_and_keeps_its_colours() {
    use vana::screens::chrome::{POWER, SPEED, WHITE};
    use vana::screens::look::light;
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    assert_eq!(corner(&sim), 0, "dark by default");
    sim.home("SETTINGS");
    sim.swipe_next();
    sim.swipe_next();
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    sim.expect_field("THEME", "DARK");
    sim.press(Button::BottomRight);
    assert_eq!(corner(&sim), WHITE, "Settings turns white at once");
    assert!(shows(&sim, light(WHITE)), "dark text on the white page");
    sim.checkpoint("settings in the light theme");
    sim.press(Button::TopLeft);
    assert_eq!(corner(&sim), WHITE);
    assert!(shows(&sim, light(GREEN)), "the RIDE card's green");
    sim.checkpoint("home in the light theme");
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    // MOVE goes on from SETTINGS past RIDES and wraps to RIDE.
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.advance(seconds(20));
    assert_eq!(corner(&sim), WHITE);
    assert!(shows(&sim, light(SPEED)) && shows(&sim, light(POWER)));
    sim.checkpoint("ride page in the light theme");
    sim.swipe_next();
    sim.advance(seconds(1));
    assert_eq!(corner(&sim), WHITE);
    sim.checkpoint("second ride page in the light theme");
}

#[test]
fn heart_rate_band_and_eat_and_drink_alerts_sound_and_show() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.home("SETTINGS");
    sim.set_option("HR ABOVE", "170 BPM");
    sim.set_option("EAT AND DRINK", "EVERY 20 MIN");
    sim.checkpoint("alert limits in Settings");
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "{}", sim.screen().text());
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
fn main_page_power_is_a_three_second_average() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.expect_field("W", "200");
    // A sprint: the reading climbs over three seconds instead of jumping.
    sim.ride_at(30.0, 500.0, 140.0, 90.0);
    let mut seen = Vec::new();
    for _ in 0..8 {
        sim.advance(500);
        seen.push(reading(&sim, "W") as u32);
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
fn stats_page_totals_a_steady_ten_minutes() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    assert_eq!(sim.screen().page().as_deref(), Some("1/6"));
    expect_near(&mut sim, "KM", 5.0, 0.05);
    sim.expect_field("RPM", "90");
    sim.checkpoint("main page after ten minutes");

    ride_page(&mut sim, "2/6");
    assert!(
        sim.screen().run("KM/H").is_none(),
        "main page units are gone"
    );
    // Sensors take a few seconds to report after the start.
    expect_near(&mut sim, "DIST KM", 5.0, 0.05);
    expect_near(&mut sim, "AVG KM/H", 30.0, 0.2);
    expect_near(&mut sim, "MAX KM/H", 30.0, 0.3);
    sim.expect_field("AVG W", "200");
    sim.expect_field("NP W", "200");
    // Against the default FTP of 215 W: IF 200/215, TSS 10 min × 0.93² / 60 × 100.
    sim.expect_field("IF", "0.93");
    sim.expect_field("TSS", "14");
    sim.expect_field("AVG BPM", "140");
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
    sim.ride_page("2/4");
    sim.expect_field("NP W", "200");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn the_summary_shows_calories_and_the_heart_rate_recovery_after_a_minute() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 170.0, 90.0);
    sim.advance(minutes(10));
    sim.stop_ride();
    // 200 W for ten minutes is 120 kJ, about 120 kcal.
    // A saved ride with altitude has four pages; the effort page is second.
    sim.press(Button::BottomLeft);
    sim.expect_field("WORK KJ", "120");
    sim.expect_field("KCAL", "120");
    sim.expect_text("HR RECOVERY");
    sim.expect_text("MEASURING");
    sim.checkpoint("summary measuring heart rate recovery");

    // The rider stops pedalling and the strap reads lower a minute on.
    sim.ride_at(0.0, 0.0, 142.0, 0.0);
    sim.advance(seconds(45));
    sim.expect_text("MEASURING");
    sim.advance(seconds(30));
    sim.expect_text("-28 BPM");
    sim.expect_no_text("MEASURING");
    sim.checkpoint("summary with heart rate recovery");
}

#[test]
fn a_ble_strap_gets_a_recovery_after_the_ride_is_saved() {
    let mut sim = rider();
    sim.ble_heart_strap(true);
    sim.select_ble_heart_strap();
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 160.0, 90.0);
    sim.wait_for_field("BPM", "160", seconds(30));
    sim.advance(minutes(2));
    sim.stop_ride();
    // A saved ride with altitude has four pages; the effort page is second.
    sim.press(Button::BottomLeft);
    sim.expect_text("MEASURING");
    sim.ride_at(0.0, 0.0, 130.0, 0.0);
    sim.advance(seconds(70));
    sim.expect_text("-30 BPM");
    sim.checkpoint("summary with ble heart rate recovery");
}

#[test]
fn sitting_out_a_long_pause_before_ending_measures_nothing() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 170.0, 90.0);
    sim.advance(minutes(2));
    sim.pause_ride();
    sim.ride_at(0.0, 0.0, 100.0, 0.0);
    sim.advance(minutes(3));
    sim.stop_ride();
    // A saved ride with altitude has four pages; the effort page is second.
    sim.press(Button::BottomLeft);
    sim.expect_text("KCAL");
    sim.advance(seconds(70));
    sim.expect_no_text("HR RECOVERY");
}

#[test]
fn a_ride_without_a_heart_strap_shows_no_recovery() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(2));
    sim.stop_ride();
    // A saved ride with altitude has four pages; the effort page is second.
    sim.press(Button::BottomLeft);
    sim.expect_text("KCAL");
    sim.advance(seconds(90));
    sim.expect_no_text("HR RECOVERY");
    sim.expect_no_text("MEASURING");
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
    ride_page(&mut sim, "2/6");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "ASCENT M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    // The final climb brought up the climb page; back to the totals.
    ride_page(&mut sim, "2/6");
    expect_near(&mut sim, "DIST KM", 4.0, 0.05);
    // The 3 m band and smoothing may leave the last metres of the final climb.
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    sim.checkpoint("stats after one hilly lap");

    // The exports carry the barometric elevation: a 600 m start, up to 650 m,
    // down to 550 m, and the session's ascent and descent.
    sim.stop_ride();
    let live = sim.screen().field("ASCENT M").unwrap();
    println!("ascent live summary: {live} m");
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    sim.checkpoint("summary overview after the hilly lap");
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("DESCENT M");
    expect_near(&mut sim, "MIN M", 550.0, 5.0);
    expect_near(&mut sim, "MAX M", 650.0, 5.0);
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    expect_near(&mut sim, "DESCENT M", 100.0, 5.0);
    sim.checkpoint("summary elevation profile of the hilly lap");
    let rides = sim.rides();
    let ride = &rides[0];
    let metres: Vec<f64> = ride
        .samples()
        .filter_map(|s| s.altitude)
        .map(|a| f64::from(a.dm()) / 10.0)
        .collect();
    assert!(metres.len() > 500, "{} samples with altitude", metres.len());
    let (low, high) = metres
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), m| (l.min(*m), h.max(*m)));
    assert!(
        low > 540.0 && high < 660.0 && high - low > 95.0,
        "{low} {high}"
    );
    let gpx = ride.gpx();
    assert!(gpx.matches("<ele>").count() > 500, "GPX elevation");
    let fit = ride.fit(Some(1_789_000_000_000)).expect("a FIT file");
    let records = fit_messages(&fit, 20);
    let altitudes: Vec<f64> = records
        .iter()
        .filter_map(|r| r.get(&2))
        .map(|&v| v as f64 / 5.0 - 500.0)
        .collect();
    assert_eq!(altitudes.len(), metres.len(), "FIT records with altitude");
    let session = &fit_messages(&fit, 18)[0];
    let (ascent, descent) = (session[&22], session[&23]);
    assert!((90..=110).contains(&ascent), "FIT ascent {ascent} m");
    assert!((90..=110).contains(&descent), "FIT descent {descent} m");

    // Read back from the journal, the ride shows the same ascent and profile.
    sim.done();
    sim.home("RIDES");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    let saved = sim.screen().field("ASCENT M").unwrap();
    println!("ascent saved summary: {saved} m");
    expect_near(&mut sim, "ASCENT M", 100.0, 5.0);
    sim.checkpoint("saved hilly lap read back");
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("DESCENT M");
    expect_near(&mut sim, "MIN M", 550.0, 5.0);
    expect_near(&mut sim, "MAX M", 650.0, 5.0);
    expect_near(&mut sim, "DESCENT M", 100.0, 5.0);
    sim.checkpoint("saved hilly lap elevation profile");
    sim.swipe_next();
    sim.expect_text("BEST POWER W");
}

/// Valid fields by number of every data message with this global number,
/// from a FIT file of the firmware's own encoder (little endian, no
/// compressed timestamps).
fn fit_messages(bytes: &[u8], global: u16) -> Vec<std::collections::BTreeMap<u8, u64>> {
    use std::collections::BTreeMap;
    /// Global number and (field, size, base type) of each defined field.
    type Definition = (u16, Vec<(u8, usize, u8)>);
    let end = 14 + u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut definitions: BTreeMap<u8, Definition> = BTreeMap::new();
    let (mut at, mut found) = (14, Vec::new());
    while at < end {
        let header = bytes[at];
        let local = header & 0xf;
        at += 1;
        if header & 0x40 != 0 {
            let number = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]);
            let count = usize::from(bytes[at + 4]);
            at += 5;
            let fields = (0..count)
                .map(|i| {
                    let f = &bytes[at + 3 * i..];
                    (f[0], usize::from(f[1]), f[2])
                })
                .collect();
            at += 3 * count;
            definitions.insert(local, (number, fields));
            continue;
        }
        let (number, fields) = &definitions[&local];
        let mut values = BTreeMap::new();
        for &(field, size, base) in fields {
            let raw = &bytes[at..at + size];
            at += size;
            if base == 0x07 {
                continue;
            }
            let mut wide = [0u8; 8];
            wide[..size].copy_from_slice(raw);
            let value = u64::from_le_bytes(wide);
            let invalid = match base {
                0x84 => 0xffff,
                0x86 => 0xffff_ffff,
                _ => u64::MAX,
            };
            if value != invalid {
                values.insert(field, value);
            }
        }
        if *number == global {
            found.push(values);
        }
    }
    found
}

#[test]
fn pause_leaves_the_totals_untouched() {
    const TOTALS: [&str; 7] = ["DIST KM", "MOVING", "AVG W", "NP W", "IF", "TSS", "AVG BPM"];
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(5));
    ride_page(&mut sim, "2/6");
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
    expect_near(&mut sim, "DIST KM", 5.0, 0.08);
    sim.expect_field("AVG W", "250");
    let moving = sim.screen().field("MOVING").unwrap();
    assert!(moving.starts_with("09:5") || moving == "10:00", "{moving}");
    sim.checkpoint("stats after resuming");
    // The main page agrees: the pause is not ride time.
    ride_page(&mut sim, "1/6");
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

/// The KCAL on the summary's effort page, once it shows a number.
fn summary_kcal(sim: &mut Sim) -> u32 {
    sim.press(Button::BottomLeft);
    sim.screen()
        .field("KCAL")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no KCAL number; screen:\n{}", sim.screen().text()))
}

#[test]
fn a_heart_rate_only_ride_shows_calories_once_the_body_is_set() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 0.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.stop_ride();
    sim.press(Button::BottomLeft);
    sim.expect_field("KCAL", "--");
    sim.checkpoint("heart rate only, body not set: no calories");
    go_home(&mut sim);

    sim.home("SETTINGS");
    sim.swipe_next();
    sim.expect_text("BODY");
    sim.expect_field("WEIGHT KG", "NOT SET");
    sim.expect_field("AGE YEARS", "NOT SET");
    sim.expect_field("SEX", "NOT SET");
    sim.checkpoint("settings: body not set");
    // The first press of an unset number lands on its start, then a kilo
    // or a year at a time.
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.expect_field("WEIGHT KG", "70");
    for _ in 0..5 {
        sim.press(Button::BottomRight);
    }
    sim.expect_text("+1");
    sim.checkpoint("editing weight");
    sim.press(Button::TopLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.expect_field("AGE YEARS", "30");
    sim.press(Button::TopLeft);
    sim.expect_field("AGE YEARS", "30 YEARS");
    sim.set_option("SEX", "MALE");
    sim.expect_field("WEIGHT KG", "75 KG");
    sim.checkpoint("settings: body set");
    sim.swipe_previous();
    sim.expect_text("PROFILE");
    sim.expect_text("2.9 W/KG");
    sim.checkpoint("settings: FTP with watts per kilogram");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);

    sim.start_ride();
    sim.ride_at(30.0, 0.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.stop_ride();
    // Keytel, male 30 y 75 kg at 140 bpm for ten minutes: 129.5 kcal.
    let kcal = summary_kcal(&mut sim);
    assert!((129..=130).contains(&kcal), "{kcal} kcal");
    sim.checkpoint("heart rate only, body set: calories on the summary");
    go_home(&mut sim);

    sim.power_loss();
    assert!(sim.command("PROFILE").1.contains("stored=current"));
    sim.home("SETTINGS");
    sim.expect_text("2.9 W/KG");
    sim.swipe_next();
    sim.expect_field("WEIGHT KG", "75 KG");
    sim.expect_field("AGE YEARS", "30 YEARS");
    sim.expect_field("SEX", "MALE");
}

#[test]
fn training_profile_survives_power_loss() {
    let mut sim = rider();
    let (_, before) = sim.command("PROFILE");
    assert_eq!(
        before, "ftp=215 lthr=185 stored=current persistence=saved",
        "a fresh device has the defaults, saved when setup was skipped"
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
    ride_page(&mut sim, "2/6");
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
    ride_page(&mut sim, "3/6");
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
    sim.ride_page("4/4");
    sim.expect_field("LAPS", "3");
}

#[test]
fn auto_lap_closes_every_five_kilometres() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    ride_page(&mut sim, "3/6");
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

/// A 2 km loop ridden at 30 km/h with AUTO LAP set to AT START; `jitter_m`
/// is the receiver's error per axis. Ends riding on the main page.
fn at_start_rider(jitter_m: f64) -> Sim {
    auto_lap_rider(jitter_m, "AT START")
}

fn auto_lap_rider(jitter_m: f64, choice: &str) -> Sim {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_route(Route::loop_around(Point::new(-33.45, -70.66), 2_000.0, 0.0));
    if jitter_m > 0.0 {
        sim.set_gps_quality(true, 1.8, jitter_m);
    }
    sim.home("SETTINGS");
    show_setting(&mut sim, "AUTO LAP");
    sim.set_option("AUTO LAP", choice);
    sim.checkpoint("settings with auto lap at the start");
    // The RIDE section's heading looks like the Home tile to `on_home`.
    sim.swipe_next();
    go_home(&mut sim);
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
    ride_page(&mut sim, "3/6");
    // Two kilometres at 30 km/h is four minutes; the lap closes about 35 m past
    // the start, a few seconds on.
    sim.advance(minutes(3) + seconds(30));
    sim.expect_text("LAP 1");
    sim.expect_no_text("LAP 2");
    sim.wait_for("LAP 1 04:", seconds(40)).unwrap();
    let toast = sim.screen().find("LAP 1 04:").remove(0).text;
    assert!(toast.ends_with(" 220W"), "{toast}");
    sim.checkpoint("lap toast at the start, first loop");
    sim.expect_text("LAP 2");
    expect_near(&mut sim, "LAP KM", 0.0, 0.1);

    sim.wait_for("LAP 2 04:", seconds(300)).unwrap();
    sim.checkpoint("lap toast at the start, second loop");
    sim.advance(seconds(10));
    sim.expect_text("LAP 3");
    let previous = sim.screen().field("LAP 2").unwrap();
    assert!(
        previous.starts_with("04:0") || previous.starts_with("04:1"),
        "{previous}"
    );
    sim.checkpoint("lap page after two loops");

    sim.wait_for("LAP 3 04:", seconds(300)).unwrap();
    sim.checkpoint("lap toast at the start, third loop");
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(position_laps(&mut sim), 3);

    let fit = sim.rides()[0]
        .fit(Some(1_789_000_000_000))
        .expect("a FIT file");
    let laps = fit_messages(&fit, 19);
    let triggers: Vec<u64> = laps.iter().map(|lap| lap[&24]).collect();
    // position_start for each loop, then the ride's end.
    assert_eq!(triggers, [3, 3, 3, 7]);
    let distances: Vec<u64> = laps.iter().map(|lap| lap[&9]).collect();
    for centimetres in &distances[..3] {
        assert!((199_000..=207_000).contains(centimetres), "{distances:?}");
    }
    sim.ride_page("4/4");
    sim.expect_field("LAPS", "4");
}

#[test]
fn auto_lap_at_start_counts_each_loop_once_through_gps_jitter() {
    let mut sim = at_start_rider(12.0);
    ride_page(&mut sim, "3/6");
    sim.advance(minutes(12) + seconds(30));
    sim.expect_text("LAP 4");
    sim.checkpoint("lap page after three loops, noisy GPS");
    sim.stop_ride();
    assert_eq!(position_laps(&mut sim), 3);
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
    ride_page(&mut sim, "3/6");
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
    ride_page(&mut sim, "3/6");
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
    ride_page(&mut sim, "4/6");
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
    assert_eq!(sim.screen().page().as_deref(), Some("1/6"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    let shown = ride_until_page(&mut sim, "4/6", 1600.0);
    assert!((1300.0..1450.0).contains(&shown), "at {shown:.0} m");
    sim.expect_text("CLIMBING");
    sim.checkpoint("climb page brought up by the climb");

    ride_to(&mut sim, 2500.0);
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let back = ride_until_page(&mut sim, "1/6", 2800.0);
    assert!(back > 2500.0, "back at {back:.0} m");
    expect_near(&mut sim, "KM/H", 40.0, 0.3);
    sim.checkpoint("main page again over the top");
}

/// Turn the named ride pages off in Settings, then go back Home.
fn hide_ride_pages(sim: &mut Sim, pages: &[&str]) {
    sim.home("SETTINGS");
    for _ in 0..5 {
        sim.swipe_next();
    }
    sim.expect_text("RIDE PAGES");
    for label in ["STATS", "LAP", "CLIMB", "MAP", "BACK TO START"] {
        sim.expect_field(label, "ON");
        if pages.contains(&label) {
            sim.press(Button::BottomRight);
            sim.expect_field(label, "OFF");
        }
        if label == "CLIMB" && pages.contains(&label) {
            sim.expect_text("NO CLIMB POP-UP");
        }
        if matches!(label, "CLIMB" | "MAP" | "BACK TO START") {
            sim.checkpoint(&format!("{label} row with {} off", pages.join(" and ")));
        }
        sim.press(Button::BottomLeft);
    }
    go_home(sim);
}

#[test]
fn turned_off_ride_pages_leave_the_dots_and_the_page_walk() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["STATS", "LAP"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(5));
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    sim.checkpoint("main page of four");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("2/4"));
    sim.expect_text("CLIMB");
    sim.checkpoint("climb page second of four");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("3/4"));
    sim.checkpoint("map page third of four");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("4/4"));
    sim.checkpoint("back to start page last of four");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("3/4"));
    sim.swipe_next();
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"), "wraps to main");
}

#[test]
fn a_map_page_turned_off_leaves_the_walk_from_climb_to_back() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["MAP"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(5));
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    assert_eq!(sim.screen().page().as_deref(), Some("4/5"));
    sim.expect_text("CLIMB");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("5/5"));
    sim.checkpoint("back to start page right after climb");
}

#[test]
fn a_climb_does_not_bring_up_a_climb_page_turned_off() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["CLIMB"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    while sim.world().distance_m < 2500.0 {
        sim.advance(seconds(1));
        assert_eq!(
            sim.screen().page().as_deref(),
            Some("1/5"),
            "at {:.0} m",
            sim.world().distance_m
        );
    }
    sim.expect_no_text("CLIMBING");
    sim.checkpoint("main page kept through the climb");
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
        assert_eq!(sim.screen().page().as_deref(), Some("4/6"));
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
fn a_manually_paused_rider_who_rides_on_hears_a_reminder_every_half_minute() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.advance(seconds(1));
    let from = sim.device.speaker.played.len();
    sim.advance(seconds(60));
    assert_eq!(
        played_since(&sim, from),
        [0, 0, 10, 0, 0, 10],
        "two reminders in a minute"
    );
    assert!(sim.screen().run("PAUSED").is_some());
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(10));
    let from = sim.device.speaker.played.len();
    sim.advance(seconds(60));
    assert_eq!(
        played_since(&sim, from),
        Vec::<u8>::new(),
        "quiet once stopped"
    );
    assert!(!recording(&mut sim));
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
        ride_page(&mut sim, "2/6");
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
    // A fresh device opens first-run setup where Home would be, as soon.
    sim.checkpoint("power-on: setup ready");
    sim.expect_text("WELCOME");
    assert!(home - lit <= 500, "brand flash {} ms", home - lit);
    assert!(home <= seconds(5), "Home at {home} ms uptime");
    sim.skip_setup();
    sim.expect_text("READY TO RIDE");
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
    sim.skip_setup();
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

/// Ready names the time left once the charge has fallen long enough to know
/// the rate; then a ride that drains faster warns once, before the low-battery
/// levels, that the battery may not last an hour more.
#[test]
fn the_battery_estimate_shows_on_ready_and_warns_during_a_fast_drain() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.set_battery(90, false);
    sim.home("RIDE");
    assert_eq!(sim.vana_page(), "ready");
    sim.expect_text("BATTERY 90%");
    sim.expect_no_text("LEFT");
    // 10 %/h on Ready: a point every 6 minutes; the estimate waits for 40.
    for percent in (82..90).rev() {
        sim.advance(minutes(6));
        sim.set_battery(percent, false);
    }
    sim.advance(seconds(2));
    sim.expect_text("BATTERY 82%");
    sim.expect_text("ABOUT 8 H LEFT");
    sim.checkpoint("ready: battery with time left");

    // A ride drains at 40 %/h, a point every 90 seconds.
    // The screen dimmed while Ready waited; the first press only wakes it.
    sim.press(Button::BottomRight);
    if sim.wait_for("RECORDING", seconds(1)).is_err() {
        sim.press(Button::BottomRight);
        sim.wait_for("RECORDING", seconds(2)).unwrap();
    }
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    let mut percent = 82;
    let mut shown = false;
    while percent > 12 && !shown {
        // The warning comes on its own tick, so wait for it between points.
        shown = sim.wait_for(" LEFT", seconds(90)).is_ok();
        if !shown {
            percent -= 1;
            sim.set_battery(percent, false);
        }
    }
    assert!(shown, "warned before the battery ran out, at {percent}%");
    sim.checkpoint("ride: the battery may not last another hour");
    // Once: not again while the ride goes on.
    sim.advance(seconds(10));
    sim.expect_no_text(" LEFT");
    sim.set_battery(percent - 1, false);
    sim.advance(seconds(150));
    sim.expect_no_text(" LEFT");
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
    sim.expect_text("CHANGES NOT");
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
    sim.expect_text("CHANGES NOT");
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
    // The ride is dated by the receiver's clock, so it is this week's ride.
    sim.wait_for("WEEK 5 KM", seconds(10)).unwrap();
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
    assert_eq!(sim.screen().page().as_deref(), Some("1/6"));

    sim.swipe_next();
    assert_eq!(sim.screen().page().as_deref(), Some("2/6"));
    sim.expect_text("AVG W");
    sim.checkpoint("swiped to stats");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("1/6"));
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("6/6"), "wraps");
    sim.expect_text("BACK TO START");
    sim.swipe_previous();
    sim.expect_text("MAP");
    sim.swipe_previous();
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("3/6"));
    sim.expect_text("LAP TIME");
    sim.swipe_next();
    sim.swipe_next();
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

// Structured training: workouts, the player, uploads and both FTP tests,
// ridden with simulated power.

/// A rider with power and speed paired, riding 30 km/h at an easy 120 W.
fn training_rider() -> Sim {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    sim
}

/// The device's beeps since `from` (uptime), as pattern ids.
fn beeps_since(sim: &Sim, from: u64) -> Vec<u8> {
    sim.device
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
    sim.press(Button::TopLeft);
    sim.advance(seconds(3));
}

#[test]
fn a_structured_workout_runs_its_steps_with_targets_beeps_and_laps() {
    let mut sim = training_rider();
    sim.home("TRAIN");
    sim.expect_text("WORKOUTS");
    sim.expect_text("FREE RIDE");
    sim.checkpoint("workout picker");
    go_home(&mut sim);
    sim.choose_workout("VO2 MAX 5X3");
    sim.checkpoint("workout ready to start");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    // The workout page leads the ride pages.
    assert_eq!(sim.screen().page().as_deref(), Some("1/7"));
    sim.expect_text("1/12");

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
    sim.expect_text("WORK 1/5 3:00");
    sim.checkpoint("next step announced");
    assert_eq!(sim.workout()["laps"], "1", "the step closed a lap");

    // On target: 106-120 % of 215 W.
    follow(&mut sim, 20, on_target);
    assert_eq!(sim.workout()["target"], "228-258");
    assert_eq!(sim.screen().run("ON TARGET").unwrap().color, GREEN);
    sim.checkpoint("interval on target");
    // The main page draws the same target under its power gauge.
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("2/7"));
    sim.checkpoint("main page with the target band");
    sim.ride_page("1/7");
    ride_step(&mut sim, on_target);
    ride_step(&mut sim, on_target);

    // The second interval ridden too easy reads UNDER, in amber.
    assert_eq!(sim.workout()["step"], "4/12");
    follow(&mut sim, 20, |band| band.unwrap().0 * 0.8);
    assert_eq!(sim.screen().run("UNDER").unwrap().color, AMBER);
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
    let y = vana::screens::ride::EXTEND_ROWS.start as u16 + 12;
    sim.tap(120, y);
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
fn a_long_workout_list_pages_with_held_move_and_a_thirty_thirty_runs() {
    let mut sim = training_rider();
    sim.home("TRAIN");
    sim.expect_text("1/17");
    // Held MOVE turns a page of four cards; a tap of MOVE moves one.
    sim.hold(Button::BottomLeft, 500);
    sim.expect_text("6/17");
    sim.checkpoint("workout picker paged");
    sim.hold(Button::BottomLeft, 500);
    sim.hold(Button::BottomLeft, 500);
    sim.expect_text("16/17");
    sim.press(Button::BottomLeft);
    sim.expect_text("17/17");
    sim.expect_text("FTP RAMP TEST");
    sim.checkpoint("workout picker last card");
    // Past the last card MOVE wraps to the first.
    sim.press(Button::BottomLeft);
    sim.expect_text("1/17");
    go_home(&mut sim);
    sim.choose_workout("VO2 MAX 30-30S");
    sim.checkpoint("thirty thirty ready");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
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
fn twenty_minute_ftp_test_offers_and_saves_the_new_ftp() {
    let mut sim = training_rider();
    sim.choose_workout("FTP TEST 20 MIN");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    // Skip to the twenty minutes with MARK.
    follow(&mut sim, 30, on_target);
    sim.world().power_w = 260.0;
    for _ in 0..10 {
        next_step(&mut sim);
    }
    assert_eq!(sim.workout()["step"], "11/12");
    sim.expect_text("ALL OUT");
    follow(&mut sim, 5 * 60, |_| 260.0);
    sim.expect_text("AVG 260 W");
    sim.checkpoint("twenty minutes all out");
    let before = sim.now();
    ride_step(&mut sim, |_| 260.0);
    // 95 % of 260 W.
    sim.wait_for("NEW FTP", seconds(2)).unwrap();
    sim.expect_text("247");
    sim.expect_text("NOW 215 W");
    sim.expect_text("SAVE?");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "the ride goes on under the sheet"
    );
    sim.checkpoint("new FTP offered");
    sim.press(Button::BottomRight);
    sim.expect_text("FTP 247 W SAVED");
    sim.checkpoint("new FTP saved");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "ACT on the sheet does not pause"
    );
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=247 "), "{profile}");
    // The cool down now targets the new FTP: 40-55 % of 247 W.
    assert_eq!(sim.workout()["target"], "99-136");
    sim.stop_ride();
}

#[test]
fn ramp_test_ends_at_failure_and_keeping_the_old_ftp_changes_nothing() {
    let mut sim = training_rider();
    sim.choose_workout("FTP RAMP TEST");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    follow(&mut sim, 20, on_target);
    next_step(&mut sim);
    assert_eq!(sim.workout()["step"], "2/3");
    // The rider holds every minute up to 300 W, then fades.
    let effort = |band: Option<(f64, f64)>| {
        let low = band.map_or(100.0, |(low, _)| low);
        if low <= 300.0 { low + 5.0 } else { 250.0 }
    };
    follow(&mut sim, 5 * 60 + 30, effort);
    sim.expect_text("HOLD 200 W");
    sim.checkpoint("ramp at 200 W");
    let before = sim.now();
    ride_step(&mut sim, effort);
    // 75 % of the best minute, 305 W.
    sim.wait_for("NEW FTP", seconds(2)).unwrap();
    sim.expect_text("229");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    sim.checkpoint("ramp result offered");
    assert_eq!(sim.workout()["step"], "3/3", "cool down after the ramp");
    sim.press(Button::TopLeft);
    sim.expect_text("FTP KEPT 215 W");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "BACK on the sheet neither laps nor stops"
    );
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=215 "), "{profile}");
    sim.stop_ride();
}

#[test]
fn an_uploaded_workout_appears_in_the_picker_and_rides() {
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
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    // A file damaged on the way is listed as skipped and never offered.
    let mut damaged = bytes.to_vec();
    damaged[10] ^= 1;
    sim.put_file("workouts/broken.vw", &damaged);
    let (_, list) = sim.command("WORKOUT LIST");
    assert!(list.contains("files=ok uploads=0 skipped=1"), "{list}");
    // The host puts the workout on FILES; opening the picker lists it.
    sim.put_file("workouts/over-unders.vw", &bytes);
    sim.home("TRAIN");
    for _ in 0..17 {
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("OVER UNDERS");
    sim.expect_text("8 STEPS");
    sim.checkpoint("uploaded workout in the picker");
    let (_, list) = sim.command("WORKOUT LIST");
    assert!(
        list.contains("uploads=1 skipped=1 upload0=workouts/over-unders.vw:OVER_UNDERS"),
        "{list}"
    );
    go_home(&mut sim);
    sim.choose_workout("OVER UNDERS");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
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
        "the next ride is free again"
    );
    // A workout whose file goes away after it was chosen rides free.
    sim.choose_workout("OVER UNDERS");
    let (status, out) = sim.command("FILES DELETE workouts/over-unders.vw");
    assert_eq!(status, "OK", "{out}");
    sim.press(Button::BottomRight);
    sim.wait_for("WORKOUT MISSING", seconds(2)).unwrap();
    assert_eq!(sim.workout()["workout"], "none");
    sim.stop_ride();
    sim.done();
    // WORKOUT DELETE removes an upload's file.
    sim.put_file("workouts/over-unders.vw", &bytes);
    let (_, list) = sim.command("WORKOUT LIST");
    assert!(list.contains("uploads=1"), "{list}");
    let (status, out) = sim.command("WORKOUT DELETE 0");
    assert_eq!(
        (status, out.as_str()),
        ("OK", "deleted=workouts/over-unders.vw")
    );
    assert_eq!(sim.file("workouts/over-unders.vw"), None);
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
    // The second before 220 W was set rides at the lead-in power; where it
    // falls in the sampling second moves the average by a watt.
    let average = sim.screen().field("AVG W");
    assert!(
        matches!(average.as_deref(), Some("219" | "220")),
        "AVG W {average:?}"
    );
    sim.checkpoint("summary: ride saved");
    sim.swipe_next();
    // The first second of a ride may still read the power before it.
    let watts: u32 = sim.screen().field("NP W").unwrap().parse().unwrap();
    assert!(watts.abs_diff(220) <= 1, "NP W {watts}");
    sim.checkpoint("summary: effort and zones");
    sim.press(Button::BottomLeft);
    sim.expect_text("DESCENT M");
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    sim.expect_field("LAPS", "2");
    sim.checkpoint("summary: best power, climbs and laps");
    sim.done();
    sim.checkpoint("home after the ride");
}

/// Two rides: the first sets every best, the second beats them and offers a
/// higher FTP as the summary's last page. BESTS lists them under Settings.
#[test]
fn a_second_ride_sets_personal_bests_and_offers_an_ftp() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    // A card with no volumes has no place for bests, and BESTS says so.
    sim.home("SETTINGS");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_field("BESTS", "NOT KEPT");
    sim.press(Button::BottomRight);
    sim.expect_text("BESTS NOT KEPT");
    sim.checkpoint("BESTS where nothing can be kept");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    sim.init_bulk();
    sim.init_volumes();
    // The host formats the FILES store with its first file; VANA never does.
    sim.put_file("notes.txt", b"x");
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(22));
    let chimes = sim.device.speaker.played.len();
    sim.stop_ride();
    sim.expect_text("7 NEW BESTS");
    sim.advance(seconds(1));
    assert!(sim.device.speaker.played.len() >= chimes + 3, "best chime");
    sim.checkpoint("first ride: seven new bests");
    sim.expect_text("BEST");
    // 95% of 200 W is under the FTP in force: no offer, three pages.
    for _ in 0..4 {
        if sim.screen().contains("RIDE SAVED") {
            break;
        }
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("RIDE SAVED");
    sim.expect_no_text("FTP");
    sim.done();
    sim.expect_no_text("NEW BESTS");

    sim.ride_at(32.0, 260.0, 150.0, 92.0);
    sim.start_ride();
    sim.advance(minutes(25));
    sim.stop_ride();
    sim.expect_text("NEW BESTS");
    sim.checkpoint("second ride: new bests toast on the summary");
    sim.expect_text("BEST");
    sim.checkpoint("summary: time and distance marked BEST");
    // Backwards from the first page never lands on the FTP offer.
    sim.swipe_previous();
    sim.expect_text("BEST POWER W");
    sim.expect_no_text("FTP 24");
    sim.expect_text("BEST");
    sim.checkpoint("summary: best power marked BEST");
    sim.press(Button::BottomLeft);
    sim.expect_text("FTP 24");
    sim.expect_text("NOW 215 W");
    sim.checkpoint("summary: FTP offer");
    sim.press(Button::BottomRight);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.expect_text("SAVED");
    assert!(sim.command("PROFILE").1.contains("ftp=247"));
    sim.checkpoint("home: FTP saved");

    // The bests are in a file, so they outlive a power loss.
    sim.power_loss();
    sim.home("SETTINGS");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_field("BESTS", "7 SET");
    sim.checkpoint("settings: BESTS row");
    sim.press(Button::BottomRight);
    sim.expect_text("LONGEST RIDE");
    sim.expect_text("MOST ASCENT");
    sim.expect_text("260 W");
    sim.checkpoint("the BESTS view");
    // With a weight set, each power best also reads in watts per kilogram.
    sim.expect_no_text("W/KG");
    sim.press(Button::TopLeft);
    sim.swipe_next();
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.expect_field("WEIGHT KG", "70");
    sim.press(Button::TopLeft);
    sim.swipe_previous();
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.expect_text("LONGEST RIDE");
    sim.expect_text("3.7 W/KG");
    sim.checkpoint("the BESTS view with watts per kilogram");
    // ACT asks first; BACK keeps them.
    sim.press(Button::BottomRight);
    sim.expect_text("CLEAR BESTS?");
    sim.checkpoint("clear the bests?");
    sim.press(Button::TopLeft);
    sim.expect_text("LONGEST RIDE");
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    sim.expect_no_text("260 W");
    sim.expect_text("--");
    sim.press(Button::TopLeft);
    sim.expect_field("BESTS", "NONE YET");

    // A file that does not decode is left alone until the rider clears it.
    sim.put_file("bests.dat", b"not a bests file");
    sim.press(Button::BottomRight);
    sim.expect_text("BESTS FILE DAMAGED");
    sim.checkpoint("BESTS file damaged");
    sim.press(Button::BottomRight);
    sim.expect_text("CLEAR BESTS?");
    sim.press(Button::BottomRight);
    sim.advance(seconds(1));
    sim.expect_text("--");
    sim.expect_no_text("DAMAGED");
    assert_eq!(
        sim.file("bests.dat").map(|bytes| bytes.len()),
        Some(42),
        "cleared in place"
    );
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
    for _ in 0..vana::screens::settings::SECTIONS.len() + 1 {
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
    // MOVE walks the rows, past the profile and body rows into Display.
    for _ in 0..6 {
        sim.press(Button::BottomLeft);
    }
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
    sim.expect_text("CLOCK");
    sim.expect_field("TIME ZONE", "CHILE");
    sim.expect_text("UTC-3 NOW");
    sim.set_option("TIME ZONE", "UTC-4");
    sim.checkpoint("clock section");
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
    // Every alert is off but the radar count, until the rider sets one.
    sim.expect_text("ALERTS");
    sim.expect_field("RADAR COUNT", "ON");
    sim.expect_field("EAT AND DRINK", "OFF");
    sim.expect_field("LOW CADENCE", "OFF");
    sim.set_option("LOW CADENCE", "70 RPM");
    sim.checkpoint("alerts section");
    sim.swipe_next();
    sim.expect_text("LIMITS");
    for label in ["HR ABOVE", "HR BELOW", "POWER ABOVE", "POWER BELOW"] {
        sim.expect_field(label, "OFF");
    }
    sim.set_option("POWER ABOVE", "300 W");
    sim.checkpoint("limits section");
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
    // The phone row opens Connect phone.
    sim.swipe_previous();
    sim.expect_text("DEVICE");
    let phone = sim.screen().run("PHONE").unwrap();
    sim.tap(phone.x as u16 + 4, phone.y as u16 + 4);
    sim.advance(seconds(1));
    sim.expect_text("JOIN VANA WI-FI");
    sim.checkpoint("connect phone from settings");
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
    show_setting(&mut sim, "TIME ZONE");
    sim.expect_field("TIME ZONE", "UTC-4");
    show_setting(&mut sim, "AUTO PAUSE");
    sim.expect_field("AUTO PAUSE", "OFF");
    sim.expect_field("AUTO LAP", "EVERY 1 KM");
    show_setting(&mut sim, "SOUNDS");
    sim.expect_field("SOUNDS", "OFF");
    show_setting(&mut sim, "LOW CADENCE");
    sim.expect_field("LOW CADENCE", "70 RPM");
    show_setting(&mut sim, "POWER ABOVE");
    sim.expect_field("POWER ABOVE", "300 W");
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
    ride_page(&mut sim, "3/6");
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
    sim.expect_text("CHANGES NOT");
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
    sim.expect_text("DESCENT M");
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    sim.expect_field("LAPS", "1");
    sim.expect_field("CLIMBS", "--");
    sim.checkpoint("saved ride best power");

    // ACT offers this ride to the phone; BACK comes back to it.
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "phone");
    sim.advance(seconds(1));
    sim.expect_text("JOIN VANA WI-FI");
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
fn laps_are_kept_with_the_ride_in_its_fit_file_and_saved_summary() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD", "HEART RATE"]);
    sim.home("SETTINGS");
    show_setting(&mut sim, "AUTO LAP");
    sim.set_option("AUTO LAP", "EVERY 1 KM");
    // Off the RIDE section, whose title reads as the Home card.
    sim.swipe_next();
    go_home(&mut sim);

    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    sim.expect_text("LAP 1");
    sim.checkpoint("a lap press");
    sim.advance(minutes(2) + seconds(30));
    sim.stop_ride();
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_field("LAPS", "3");
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

    sim.home("RIDES");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("BEST POWER W");
    sim.expect_field("LAPS", "3");
    sim.checkpoint("saved ride keeps its laps");
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

/// Receiver UTC in ms at simulated time `t_ms`, for a receiver left on its
/// default clock: 10:00 UTC on 14 October 2026 at t=0.
fn receiver_utc_ms(t_ms: u64) -> u64 {
    unix_seconds(*b"141026", *b"100000").unwrap() as u64 * 1000 + t_ms
}

/// A rider at speed, so the ride is never auto-paused.
fn cyclist(sim: &mut Sim) {
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 150.0, 130.0, 80.0);
}

/// Press START on the ready screen; returns the simulated time of the press.
fn press_start(sim: &mut Sim) -> u64 {
    sim.home("RIDE");
    sim.expect_text("READY");
    let pressed = sim.now();
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    pressed
}

/// The ride's start, as the FIT export counts it back from the first dated sample.
fn fit_start_ms(ride: &shell_simulator::rides::Ride) -> u64 {
    ride.fit_plan()
        .expect("the ride is dated")
        .offset_ms
        .expect("the plan has a start")
}

#[test]
fn a_ride_without_wifi_is_dated_by_the_gps_receiver() {
    let mut sim = rider();
    cyclist(&mut sim);
    let pressed = press_start(&mut sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let utc: Vec<_> = ride.samples().map(|s| s.utc_ms).collect();
    assert!(utc.iter().all(Option::is_some), "every sample is dated");
    assert!(
        utc.windows(2).all(|w| w[0] < w[1]),
        "time only moves forward"
    );
    let start = fit_start_ms(&ride);
    let expected = receiver_utc_ms(pressed);
    assert!(
        start.abs_diff(expected) < 1_000,
        "the FIT starts at {start}, the rider pressed START at {expected}"
    );
    go_home(&mut sim);
    sim.home("RIDES");
    sim.expect_text("OCT 14");
    sim.checkpoint("the ride is listed by its date");
}

#[test]
fn a_fix_found_after_the_start_dates_the_ride_from_its_start() {
    let mut sim = rider();
    sim.set_gps_fix(false);
    cyclist(&mut sim);
    let pressed = press_start(&mut sim);
    sim.advance(minutes(3));
    sim.set_gps_fix(true);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let samples: Vec<_> = ride.samples().collect();
    let first_dated = samples.iter().position(|s| s.utc_ms.is_some()).unwrap();
    assert!(first_dated > 150, "undated for the first 3 minutes");
    assert!(
        samples[first_dated..].iter().all(|s| s.utc_ms.is_some()),
        "dated from the first fix on"
    );
    assert!(samples[..first_dated].iter().all(|s| s.utc_ms.is_none()));
    let start = fit_start_ms(&ride);
    let expected = receiver_utc_ms(pressed);
    assert!(
        start.abs_diff(expected) < 1_000,
        "the FIT starts at {start}, not at the fix: the rider pressed START at {expected}"
    );
}

#[test]
fn a_ride_through_utc_midnight_keeps_going_forward() {
    let mut sim = rider();
    sim.set_gps_clock(23 * 3600 + 59 * 60, (14, 10, 26));
    cyclist(&mut sim);
    sim.advance(seconds(5));
    press_start(&mut sim);
    sim.advance(minutes(3));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let utc: Vec<_> = ride.samples().map(|s| s.utc_ms.unwrap()).collect();
    assert!(
        utc.windows(2).all(|w| w[0] < w[1]),
        "UTC steps back at midnight"
    );
    let day = |ms: u64| ms / 86_400_000;
    let (before, after) = (day(utc[0]), day(*utc.last().unwrap()));
    assert_eq!(after, before + 1, "the ride rolls into the next UTC day");
    assert_eq!(
        after,
        unix_seconds(*b"151026", *b"000000").unwrap() as u64 / 86_400
    );
}

#[test]
fn a_receiver_date_outside_the_plausible_window_leaves_the_ride_undated() {
    let mut sim = rider();
    // A GPS week rollover reports 2006 for today.
    sim.set_gps_clock(10 * 3600, (14, 10, 6));
    sim.advance(seconds(5));
    let gps = sim.device.position.snapshot(sim.now()).unwrap().gps;
    assert_eq!(gps.state, FixState::Fresh);
    assert_eq!(gps.utc_ms, None, "the receiver never takes the date");
    cyclist(&mut sim);
    sim.advance(seconds(5));
    press_start(&mut sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    assert!(ride.samples().all(|s| s.utc_ms.is_none()));
    assert_eq!(ride.fit_plan().err(), Some(Refusal::NoClock));
    go_home(&mut sim);
    sim.home("RIDES");
    sim.expect_text("RIDE 1");
    sim.expect_no_text("OCT");
}

/// At 16:00 UTC on `date`, the status bar reads `local` and the rides list
/// `listed` and `local`.
fn clock_reads(date: (u8, u8, u8), local: &str, listed: &str) -> Sim {
    let mut sim = rider();
    sim.set_gps_clock(16 * 3600, date);
    sim.advance(seconds(5));
    sim.expect_text(local);
    sim.checkpoint(&format!("home at 16:00 UTC, {listed}"));
    cyclist(&mut sim);
    press_start(&mut sim);
    sim.advance(seconds(30));
    sim.stop_ride();
    go_home(&mut sim);
    sim.home("RIDES");
    sim.expect_text(&format!("{listed}  {local}"));
    sim.checkpoint(&format!("rides list, {listed}"));
    sim
}

/// Santiago keeps UTC-4 in winter. A rider in Magallanes or Aysén picks
/// UTC-3, which holds all year.
#[test]
fn the_clock_reads_santiago_winter_time_in_june() {
    let mut sim = clock_reads((21, 6, 26), "12:00", "JUN 21");
    go_home(&mut sim);
    sim.home("SETTINGS");
    show_setting(&mut sim, "TIME ZONE");
    sim.expect_field("TIME ZONE", "CHILE");
    sim.expect_text("UTC-4 NOW");
    sim.checkpoint("time zone chile in winter");
    sim.set_option("TIME ZONE", "UTC-3");
    sim.expect_text("ALL YEAR");
    // Each swipe costs about a second, so the minute may have turned.
    assert!(
        sim.screen().contains("1:00") || sim.screen().contains("1:01"),
        "{}",
        sim.screen().text()
    );
    sim.checkpoint("time zone fixed at utc-3");
    go_home(&mut sim);
    sim.home("RIDES");
    sim.expect_text("JUN 21  1:00");
}

/// Santiago keeps UTC-3 in summer.
#[test]
fn the_clock_reads_santiago_summer_time_in_december() {
    clock_reads((21, 12, 26), "1:00", "DEC 21");
}

/// Numbers glide between quarter-second ANT+ samples: a step in power is
/// drawn through values in between, never past the new reading, and lands on
/// it exactly.
#[test]
fn readings_glide_between_samples_and_settle_on_the_reading() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 200.0, 142.0, 88.0);
    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("W", "200");
    sim.ride_at(28.0, 300.0, 142.0, 88.0);
    let mut drawn = Vec::new();
    let mut captured = false;
    // Long enough for the 3 s power to take in only the new reading.
    for _ in 0..200 {
        sim.advance(20);
        let watts: u32 = sim.screen().field("W").unwrap().parse().unwrap();
        if drawn.last() != Some(&watts) {
            drawn.push(watts);
        }
        if !captured && (240..=260).contains(&watts) {
            sim.checkpoint("power gliding to a new reading");
            captured = true;
        }
    }
    assert!(
        drawn.windows(2).all(|w| w[0] <= w[1]) && drawn.iter().all(|w| (200..=300).contains(w)),
        "{drawn:?}"
    );
    assert!(
        drawn.iter().filter(|&&w| w > 200 && w < 300).count() >= 3,
        "glides through values between samples: {drawn:?}"
    );
    assert_eq!(drawn.last(), Some(&300), "{drawn:?}");
    sim.checkpoint("power settled on the reading");
}

/// A new rider's device: nothing saved, so it opens first-run setup.
fn new_rider() -> Sim {
    let mut sim = equip(Sim::new());
    sim.wait_usable();
    sim
}

#[test]
fn first_start_sets_up_power_heart_and_sensors_then_rides() {
    let mut sim = new_rider();
    sim.expect_text("WELCOME");
    sim.checkpoint("welcome");
    sim.press(Button::BottomRight);
    sim.expect_text("POWER");
    sim.expect_text("215 W");
    sim.checkpoint("power: set or don't know");
    // SET FTP opens the Settings editor; finishing it goes on to heart.
    sim.press(Button::BottomRight);
    sim.expect_field("FTP WATTS", "215");
    for _ in 0..7 {
        sim.press(Button::BottomRight);
    }
    sim.expect_field("FTP WATTS", "250");
    sim.checkpoint("editing FTP from setup");
    sim.press(Button::TopLeft);
    sim.expect_text("HEART");
    sim.expect_text("185 BPM");
    // I DON'T KNOW keeps the default, by touch this time.
    let unknown = sim.screen().run("I DON'T KNOW").unwrap();
    sim.tap(unknown.x as u16 + 4, unknown.y as u16 + 4);
    sim.expect_text("PAIR SENSORS");
    sim.checkpoint("sensors");
    // PAIR SENSORS hands off to the sensor menu; leaving it finishes setup.
    sim.press(Button::BottomRight);
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("HEART RATE");
    for _ in 0..3 {
        if sim.screen().contains("READY") && sim.screen().contains("HOME") {
            break;
        }
        sim.press(Button::TopLeft);
    }
    sim.expect_text("RIDE");
    sim.expect_text("HOME");
    sim.checkpoint("ready");
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "ready");
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=250 lthr=185 stored=current persistence=saved"
    );
    // Setup is done for good: the next start opens Home.
    sim.power_loss();
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    sim.expect_text("READY TO RIDE");
}

#[test]
fn setup_resumes_where_it_was_left_and_never_blocks_a_ride() {
    let mut sim = new_rider();
    sim.press(Button::BottomRight);
    sim.expect_text("POWER");
    // BACK leaves setup for Home, where a ride starts as always.
    sim.press(Button::TopLeft);
    assert!(sim.on_home());
    sim.checkpoint("home before setup is finished");
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    go_home(&mut sim);
    sim.advance(seconds(3));
    // The next start resumes setup on the page it was left at.
    sim.power_loss();
    sim.expect_text("POWER");
    sim.checkpoint("setup resumed at power");
    // SKIP SETUP is on Welcome only; finishing from here also works.
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.expect_text("HEART");
}

/// Setup's ways into Settings and the sensor menu return to setup only when
/// taken from setup.
#[test]
fn setup_hand_offs_return_to_setup_only_from_setup() {
    let mut sim = new_rider();
    sim.press(Button::BottomRight);
    sim.expect_text("POWER");
    // SET FTP, then leave the editor by swiping away and going Home.
    sim.press(Button::BottomRight);
    sim.expect_field("FTP WATTS", "215");
    sim.swipe_next();
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    // An ordinary edit later stays in Settings.
    sim.home("SETTINGS");
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.press(Button::TopLeft);
    assert_eq!(sim.vana_page(), "settings");
    sim.press(Button::TopLeft);
    // Sensors from Home goes back Home even with setup at its last step.
    sim.power_loss();
    sim.expect_text("POWER");
    for _ in 0..2 {
        sim.press(Button::BottomLeft);
        sim.press(Button::BottomRight);
    }
    sim.expect_text("PAIR SENSORS");
    sim.press(Button::BottomRight);
    sim.press(Button::TopLeft);
    sim.expect_text("READY");
    sim.press(Button::TopLeft);
    assert!(sim.on_home());
    sim.home("SENSORS");
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
}

/// Run a ride command and read its completion, as a host does.
fn ride_command(sim: &mut Sim, command: &str) {
    let (status, out) = sim.command(command);
    assert_eq!(status, "ACCEPTED", "{command}: {out}");
    sim.advance(500);
    let (_, out) = sim.command("RIDE STATUS");
    assert!(out.contains("result=OK"), "{command}: {out}");
}

#[test]
fn ride_history_lists_every_saved_ride_of_both_journals() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["HEART RATE"]);
    // 22 short rides on each journal: far more than the four each journal's
    // catalog keeps.
    for ride in 0..22u32 {
        for medium in ["INTERNAL", "BULK"] {
            sim.ride_at(20.0, 150.0, 100.0 + f64::from(ride), 80.0);
            ride_command(&mut sim, &format!("RIDE {medium} START"));
            sim.advance(seconds(12));
            ride_command(&mut sim, &format!("RIDE {medium} PAUSE"));
            ride_command(&mut sim, &format!("RIDE {medium} FINISH"));
        }
    }
    sim.advance(seconds(5));
    // Opening the list starts the index; until it has read both journals
    // the catalogs' newest rides show.
    sim.home("RIDES");
    sim.advance(seconds(20));
    sim.expect_text("1/44");
    sim.checkpoint("every saved ride of both journals");
    // Held MOVE pages through the list; MOVE then steps to the oldest.
    sim.hold(Button::BottomLeft, 3000);
    let shown = sim.screen().text();
    assert!(
        !shown.contains("1/44") && !shown.contains("2/44"),
        "{shown}"
    );
    sim.checkpoint("paged through the rides");
    for _ in 0..44 {
        if sim.screen().text().contains("44/44") {
            break;
        }
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("44/44");
    // The oldest ride, outside every catalog, reads back. Dated by the
    // receiver, the rides list by start time, so the oldest is INTERNAL's first
    // ride; its first sample still reads the 72 bpm resting rate, hence 98.
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "past_ride");
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("SAVED RIDE");
    sim.expect_field("AVG BPM", "98");
    sim.checkpoint("the oldest ride read back");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    assert!(sim.on_home());

    // A new ride is listed once saved.
    ride_command(&mut sim, "RIDE INTERNAL START");
    sim.advance(seconds(12));
    ride_command(&mut sim, "RIDE INTERNAL PAUSE");
    ride_command(&mut sim, "RIDE INTERNAL FINISH");
    sim.advance(seconds(5));
    sim.home("RIDES");
    sim.expect_text("1/45");
}

/// Rides saved on given days, as internal flash holds them after being
/// ridden: `(start UTC seconds, minutes, km/h)`, one sample every three seconds.
fn journal_of_rides(rides: &[(u64, u64, f64)]) -> Vec<u8> {
    let powered: Vec<_> = rides.iter().map(|&(s, m, k)| (s, m, k, None)).collect();
    journal_of_powered_rides(&powered)
}

/// As `journal_of_rides`, with a steady power in watts for rides that had a meter.
fn journal_of_powered_rides(rides: &[(u64, u64, f64, Option<u16>)]) -> Vec<u8> {
    let mut slots = Vec::new();
    let mut push = |entry: &Entry| {
        let mut slot = encode(entry);
        slot.0[SLOT_SIZE - 4..].copy_from_slice(&commit_word().0);
        slots.extend_from_slice(&slot.0);
    };
    for (index, &(start_s, minutes, kmh, watts)) in rides.iter().enumerate() {
        let id = index as u32 + 1;
        let speed = (kmh / 0.0036) as u32;
        let samples: Vec<Sample> = (1..=minutes * 20)
            .map(|n| Sample {
                active_ms: n * 3_000,
                utc_ms: Some(start_s * 1000 + n * 3_000),
                speed_mm_s: Some(speed),
                power_watts: watts,
                ..Sample::default()
            })
            .collect();
        push(&Entry::event(Kind::Start, Source::Live, id, 0, 0));
        let mut sequence = 1;
        for batch in samples.chunks(4) {
            push(&Entry::batch(Source::Live, id, sequence, batch).unwrap());
            sequence += 1;
        }
        let active_ms = minutes * 60_000;
        push(&Entry::event(
            Kind::Finish,
            Source::Live,
            id,
            sequence,
            active_ms,
        ));
    }
    slots
}

/// The rides list opens on this week at a glance, and Home's RIDES card
/// names the week's distance. Rides are those of Chile's UTC-3 clock:
/// today is Wednesday 21 October 2026, noon.
#[test]
fn rides_list_opens_on_this_week_and_home_names_its_distance() {
    use device_api::storage::{OwnedFlash, Region};
    const DAY: u64 = 86_400;
    // Monday 2026-10-19 00:00 UTC.
    let monday = 20_745 * DAY;
    let rides = [
        // Last week: Tuesday and Saturday.
        (monday - 6 * DAY + 12 * 3600, 50, 22.0),
        (monday - 2 * DAY + 13 * 3600, 95, 27.0),
        // This week: Monday, Tuesday evening and Wednesday morning.
        (monday + 10 * 3600 + 30 * 60, 45, 24.0),
        (monday + DAY + 22 * 3600 + 30 * 60, 70, 26.0),
        (monday + 2 * DAY + 11 * 3600, 35, 28.0),
    ];
    let mut sim = Sim::new();
    sim.device
        .media
        .program(Region::Data, 0, &journal_of_rides(&rides))
        .unwrap();
    let mut sim = equip(sim.boot());
    // Wednesday 15:00 UTC, noon in Chile.
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim.advance(seconds(20));
    // Home reads the journal in the background and names the week.
    sim.expect_text("WEEK 65 KM");
    sim.checkpoint("home names the week's distance");

    sim.home("RIDES");
    sim.advance(seconds(5));
    assert_eq!(sim.vana_page(), "history");
    sim.expect_text("THIS WEEK");
    sim.expect_text("3 RIDES");
    sim.expect_text("LAST WEEK 61 KM");
    sim.expect_text("1/5");
    sim.checkpoint("the rides list opens on this week");
    // The header gives way to the headline from the fourth ride on.
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("3/5");
    sim.expect_text("THIS WEEK");
    sim.press(Button::BottomLeft);
    sim.expect_text("4/5");
    sim.expect_no_text("THIS WEEK");
    sim.checkpoint("scrolled past the header");
    // ACT opens the focused ride.
    sim.press(Button::BottomRight);
    assert_eq!(sim.vana_page(), "past_ride");
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("SAVED RIDE");
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    assert!(sim.on_home());
}

/// The week header adds up the training stress of the rides that had power, at
/// the FTP now (215 W): a steady hour at FTP is 100, and a ride without a
/// meter adds nothing and is not shown as zero.
#[test]
fn the_week_header_shows_training_stress_from_rides_with_power() {
    use device_api::storage::{OwnedFlash, Region};
    const DAY: u64 = 86_400;
    let monday = 20_745 * DAY;
    let rides = [
        (monday - 6 * DAY + 12 * 3600, 50, 22.0, Some(190)),
        (monday - 2 * DAY + 13 * 3600, 95, 27.0, None),
        (monday + 10 * 3600 + 30 * 60, 60, 24.0, Some(215)),
        (monday + DAY + 22 * 3600 + 30 * 60, 70, 26.0, None),
        (monday + 2 * DAY + 11 * 3600, 30, 28.0, Some(300)),
    ];
    let mut sim = Sim::new();
    sim.device
        .media
        .program(Region::Data, 0, &journal_of_powered_rides(&rides))
        .unwrap();
    let mut sim = equip(sim.boot());
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim.advance(seconds(20));
    sim.home("RIDES");
    sim.advance(seconds(5));
    assert_eq!(sim.vana_page(), "history");
    sim.expect_text("THIS WEEK");
    sim.expect_text("LAST WEEK");
    // An hour at 215 W is 100; half an hour at 300 W is 30 min x (300/215)^2.
    sim.expect_text("TSS 197");
    sim.checkpoint("the week header shows training stress");
}

/// The QR code on the panel, read back as a phone camera would.
fn scanned(sim: &Sim) -> String {
    let screen = sim.screen();
    let luma = |x: usize, y: usize| {
        let pixel = screen.pixels[y * screen.width + x];
        // RGB565 green channel is enough to tell dark from light.
        (((pixel >> 5) & 0x3f) << 2) as u8
    };
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(screen.width, screen.height, luma);
    let grids = image.detect_grids();
    assert_eq!(grids.len(), 1, "one code on screen:\n{}", screen.text());
    grids[0].decode().expect("the code decodes").1
}

/// After a ride, Connect phone shows a code to join the device's Wi-Fi,
/// then, once a phone joins, a code that opens the rides page.
#[test]
fn connect_phone_shows_scannable_join_and_page_codes() {
    let mut sim = rider();
    sim.start_ride();
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.press(Button::BottomRight);
    sim.home("SETTINGS");
    show_setting(&mut sim, "PHONE");
    let row = sim.screen().run("PHONE").expect("phone row");
    sim.tap(row.x as u16 + 4, row.y as u16 + 4);
    sim.advance(seconds(1));
    assert_eq!(sim.vana_page(), "phone");
    sim.expect_text("JOIN VANA WI-FI");
    let config = sim
        .device
        .access_point
        .config
        .expect("access point started");
    let join = scanned(&sim);
    assert_eq!(
        join,
        format!(
            "WIFI:T:WPA;S:{};P:{};;",
            config.ssid.text(),
            config.password.text()
        )
    );
    assert!(sim.device.phone_server.listening);
    sim.checkpoint("connect phone: join wi-fi");

    sim.device.access_point.clients = 1;
    sim.advance(seconds(1));
    sim.expect_text("OPEN YOUR RIDES");
    let page = scanned(&sim);
    assert!(page.starts_with("http://192.168.4.1/?t="), "{page}");
    sim.checkpoint("connect phone: open rides");

    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    assert_eq!(sim.vana_page(), "home");
    assert!(
        sim.device.access_point.config.is_none(),
        "access point stopped"
    );
    assert!(!sim.device.phone_server.listening);
}

/// A GPX ride out and back: 800 m north, 1.2 km east and 700 m north, then
/// home the same way, with a wrong turn 150 m south and back on the way.
fn out_and_back_gpx() -> String {
    gpx_through(
        "out and back",
        &[
            (0.0, 0.0),
            (0.0, 800.0),
            (1200.0, 800.0),
            (1200.0, 1500.0),
            // Home: back down and west, a wrong turn south, then back.
            (1200.0, 800.0),
            (700.0, 800.0),
            (700.0, 650.0),
            (700.0, 800.0),
            (0.0, 800.0),
            (0.0, 0.0),
        ],
    )
}

/// A GPX loop of about 5 km: north, round a bend east, down the far side
/// and back west to the start, then up the first leg again.
fn bend_loop_gpx() -> String {
    gpx_through(
        "bend loop",
        &[
            (0.0, 0.0),
            (0.0, 900.0),
            (500.0, 1300.0),
            (1300.0, 1250.0),
            (1700.0, 600.0),
            (1300.0, -100.0),
            (600.0, -250.0),
            (0.0, 0.0),
            (0.0, 900.0),
        ],
    )
}

/// A GPX route through points given in metres east and north of its start,
/// a point every 10 m or so.
fn gpx_through(name: &str, legs: &[(f64, f64)]) -> String {
    let start = Point::new(-33.45, -70.66);
    let metres = |(east, north): (f64, f64)| {
        let lat = start.lat + north / 111_195.0;
        let lon = start.lon + east / (111_195.0 * start.lat.to_radians().cos());
        (Point::new(lat, lon), None)
    };
    let mut points = vec![metres(legs[0])];
    for pair in legs.windows(2) {
        let length = ((pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1) / 10.0).ceil();
        for step in 1..=length as usize {
            let t = step as f64 / length;
            points.push(metres((
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            )));
        }
    }
    shell_simulator::route::to_gpx(name, &points)
}

/// The map page draws the ride's own track: just the rider at first, the
/// whole loop as it grows with the rider heading up, and north up (and
/// saying so) once the ride stops.
#[test]
fn map_page_draws_the_ride_track_and_goes_north_up_when_still() {
    let mut sim = rider();
    sim.ride_route(Route::from_gpx(&bend_loop_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    // MOVE reaches MAP after the climb page; BACK TO START follows it.
    for _ in 0..4 {
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("MAP");
    ride_to(&mut sim, 60.0);
    sim.checkpoint("the map early in the ride");

    // Round the bend: the track is rotated so the road ahead is up.
    ride_to(&mut sim, 1_900.0);
    sim.expect_no_text("N UP");
    sim.checkpoint("the map mid ride, heading up");

    // Further round the loop the zoom has stepped out to keep it all in.
    ride_to(&mut sim, 3_600.0);
    sim.checkpoint("the map on the far side of the loop");

    // Standing still the map turns north up and says so.
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(20)).unwrap();
    sim.press(Button::BottomLeft);
    sim.expect_text("MAP");
    // The heading is forgotten after 15 s without moving.
    sim.advance(seconds(20));
    sim.expect_text("N UP");
    sim.checkpoint("the map standing still, north up");

    // The next page is BACK TO START.
    sim.press(Button::BottomLeft);
    sim.expect_text("BACK TO START");
    sim.stop_ride();
}

/// Following the track back, the way home is green on the map.
#[test]
fn map_page_shows_the_way_home_in_green_while_following_the_track_back() {
    let mut sim = rider();
    sim.ride_route(Route::from_gpx(&out_and_back_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    // Out to the turnaround, then a swipe back twice: BACK TO START, MAP.
    ride_to(&mut sim, 2_700.0);
    sim.swipe_previous();
    sim.swipe_previous();
    sim.expect_text("MAP");
    sim.expect_no_text("FOLLOWING");
    sim.checkpoint("the map at the turnaround");

    // Riding back the way it came follows the track by itself.
    ride_until_shown(&mut sim, "FOLLOWING", 3_300.0);
    ride_to(&mut sim, 3_400.0);
    sim.expect_text("FOLLOWING");
    sim.checkpoint("the map following the track back");
    sim.stop_ride();
}

/// Back to start: the last ride page points the way home, straight or along
/// the ride's own track, and following it back nudges off a wrong turn.
#[test]
fn back_to_start_points_home_and_follows_the_track_back() {
    let mut sim = rider();
    sim.ride_route(Route::from_gpx(&out_and_back_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    // Out to the turnaround, 2.7 km on: a swipe back from the first page.
    ride_to(&mut sim, 2_700.0);
    sim.swipe_previous();
    sim.expect_text("BACK TO START");
    sim.expect_text("DIRECT");
    let direct = sim.screen().field("DIRECT KM").unwrap();
    assert!(
        ["1.91", "1.92", "1.93"].contains(&direct.as_str()),
        "{direct}"
    );
    let track = sim.screen().field("TRACK KM").unwrap();
    assert!(
        ["2.68", "2.69", "2.70", "2.71"].contains(&track.as_str()),
        "{track}"
    );
    sim.checkpoint("the way back from the turnaround");

    // Riding back the way it came follows the track by itself.
    let at = ride_until_shown(&mut sim, "FOLLOWING TRACK", 3_300.0);
    assert!((3_000.0..3_200.0).contains(&at), "{at}");
    sim.expect_text("ON TRACK");
    sim.expect_text("STOP FOLLOWING");
    ride_to(&mut sim, 3_400.0);
    sim.checkpoint("following the track back");

    // The wrong turn: off the track past 50 m, a nudge on any page.
    sim.press(Button::BottomLeft);
    ride_to(&mut sim, 3_965.0);
    sim.wait_for("OFF TRACK", seconds(10)).unwrap();
    sim.checkpoint("off track nudge");
    sim.swipe_previous();
    sim.expect_text("OFF TRACK");
    sim.expect_text("TRACK KM");
    ride_to(&mut sim, 4_180.0);
    sim.wait_for("BACK ON TRACK", seconds(10)).unwrap();

    // Home along the track.
    ride_to(&mut sim, 5_680.0);
    sim.wait_for("BACK AT START", seconds(20)).unwrap();
    sim.expect_text("AT START");
    sim.checkpoint("back at the start");
    // A tap on the pad stops following, or starts it sooner.
    sim.tap(120, 255);
    sim.expect_text("FOLLOW TRACK BACK");
    sim.stop_ride();
}

/// With BACK TO START off its pad cannot stop following, so riding back over
/// the track never starts it and no OFF TRACK nudge comes.
#[test]
fn back_to_start_off_never_follows_or_nudges() {
    let mut sim = rider();
    sim.ride_route(Route::from_gpx(&out_and_back_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    hide_ride_pages(&mut sim, &["BACK TO START"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(2));
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    while sim.world().distance_m < 5_680.0 {
        sim.advance(seconds(1));
        let text = sim.screen().text();
        assert!(
            !text.contains("FOLLOWING TRACK") && !text.contains("OFF TRACK"),
            "at {:.0} m:\n{text}",
            sim.world().distance_m
        );
    }
    sim.checkpoint("back at the start with no nudges");
    sim.stop_ride();
}

/// Riding into dusk in Santiago on 14 October 2026 (sunset 19:55 local,
/// 22:55 UTC): the back-to-start page says when the light ends, a toast warns
/// half an hour ahead and again at sunset, each once, and once dark it says so.
#[test]
fn riding_into_dusk_warns_before_sunset_and_again_at_sunset() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.set_gps_clock(22 * 3600 + 10 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.expect_no_text("SUNSET IN");
    // The way home says when the light ends: 22:55 UTC is 19:55 here.
    sim.swipe_previous();
    sim.expect_text("BACK TO START");
    let caption = sim.screen().text();
    assert!(
        caption.contains("SUNSET 19:54") || caption.contains("SUNSET 19:55"),
        "{caption}"
    );
    sim.checkpoint("back to start with the sunset time");
    sim.swipe_next();
    let latency = sim.wait_for("SUNSET IN 30 MIN", minutes(20)).unwrap();
    // About 22:25 UTC: 15 minutes in, less the two spent already.
    assert!(
        (minutes(11)..=minutes(14)).contains(&latency),
        "warned after {latency} ms"
    );
    sim.advance(seconds(1));
    let ids = played_since(&sim, 0);
    assert!(ids.windows(2).any(|pair| pair == [10, 10]), "{ids:?}");
    sim.checkpoint("toast: sunset in 30 minutes");
    // Once: nothing more until the sun sets.
    sim.advance(seconds(30));
    sim.expect_no_text("SUNSET IN");
    sim.wait_for("LIGHTS ON", minutes(40)).unwrap();
    sim.checkpoint("toast: sunset, lights on");
    sim.advance(minutes(5));
    sim.expect_no_text("LIGHTS ON");
    sim.swipe_previous();
    sim.expect_text("BACK TO START");
    sim.expect_text("DARK");
    sim.checkpoint("back to start after sunset");
}

/// A ride that starts in the dark has no sunset to warn of, and turning the
/// SUNSET setting off silences the warning.
#[test]
fn no_sunset_alert_when_off_or_when_the_ride_starts_after_dark() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.home("SETTINGS");
    sim.set_option("SUNSET", "OFF");
    sim.checkpoint("sunset alert off in Settings");
    sim.press(Button::TopLeft);
    sim.set_gps_clock(22 * 3600 + 10 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(60));
    sim.expect_no_text("SUNSET IN");
    sim.expect_no_text("LIGHTS ON");

    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.set_gps_clock(23 * 3600 + 30 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(10));
    sim.expect_no_text("SUNSET IN");
    sim.expect_no_text("LIGHTS ON");
}

/// The light caption on BACK TO START draws in the light theme too, before
/// and after sunset (the light theme cannot be read as text, so it is checked
/// by colour and by the screenshots).
#[test]
fn the_sunset_caption_shows_in_the_light_theme() {
    use vana::screens::chrome::{SECONDARY, WHITE};
    use vana::screens::look::light;
    for (utc, colour, name) in [
        (22 * 3600 + 10 * 60, SECONDARY, "light theme before sunset"),
        (23 * 3600 + 30 * 60, WHITE, "light theme after sunset"),
    ] {
        let mut sim = rider();
        sim.pair(&["SPEED/CAD"]);
        sim.home("SETTINGS");
        sim.swipe_next();
        sim.swipe_next();
        for _ in 0..3 {
            sim.press(Button::BottomLeft);
        }
        sim.expect_field("THEME", "DARK");
        sim.press(Button::BottomRight);
        sim.press(Button::TopLeft);
        sim.set_gps_clock(utc, (14, 10, 26));
        sim.advance(seconds(15));
        sim.ride_at(25.0, 0.0, 0.0, 0.0);
        sim.press(Button::BottomLeft);
        sim.press(Button::BottomLeft);
        sim.press(Button::BottomRight);
        sim.press(Button::BottomRight);
        sim.advance(minutes(2));
        sim.swipe_previous();
        sim.advance(seconds(1));
        assert_eq!(corner(&sim), WHITE);
        assert!(shows(&sim, light(colour)), "{name}");
        sim.checkpoint(name);
    }
}

/// How far VANA thinks the ride went against how far the rider really went,
/// both in km, since `start_m` on the world's odometer.
fn distances(sim: &mut Sim, start_m: f64) -> (f64, f64) {
    let travelled = (sim.world().distance_m - start_m) / 1000.0;
    (reading(sim, "KM"), travelled)
}

/// Open Settings on its RIDE section, where the wheel rows are.
fn ride_settings(sim: &mut Sim) {
    sim.home("SETTINGS");
    for _ in 0..4 {
        sim.swipe_next();
    }
}

/// Back Home from Settings. Its RIDE section's big title looks like Home's
/// RIDE card, so `on_home` cannot tell.
fn leave_settings(sim: &mut Sim) {
    sim.press(Button::TopLeft);
    assert_eq!(sim.vana_page(), "home");
}

/// `RIDE WHEEL` as fields.
fn wheel_fields(sim: &mut Sim) -> String {
    let (status, fields) = sim.command("RIDE WHEEL");
    assert_eq!(status, "OK", "{fields}");
    fields
}

#[test]
fn a_wheel_three_percent_off_is_learned_from_gps_and_used_from_the_next_ride() {
    let mut sim = rider();
    // The bike's wheel is 2200 mm round; VANA assumes 2136 mm, 3 % short.
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);

    ride_settings(&mut sim);
    sim.expect_text("2136 MM");
    sim.expect_text("AUTO");
    sim.checkpoint("settings: wheel 2136 mm, auto, before any ride");
    leave_settings(&mut sim);

    // Ride 1 measures with the wrong size and learns the right one, but keeps
    // the wrong one until the ride ends, so its distance never jumps, and
    // writes nothing to the record until then.
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(4));
    let early_speed = reading(&sim, "KM/H");
    assert!(
        (early_speed - 28.0 * 2136.0 / 2200.0).abs() < 0.2,
        "{early_speed}"
    );
    sim.checkpoint("ride 1: a wheel 3 % off reads slow");
    assert!(
        wheel_fields(&mut sim).contains("windows=1"),
        "{}",
        wheel_fields(&mut sim)
    );
    sim.advance(minutes(16));
    let late_speed = reading(&sim, "KM/H");
    assert!(
        (late_speed - early_speed).abs() < 0.2,
        "a size learned mid-ride must not change speed: {early_speed} then {late_speed}"
    );
    let fields = wheel_fields(&mut sim);
    assert!(
        fields.contains("wheel_mm=2136 stored_mm=2136 mode=auto"),
        "{fields}"
    );
    assert!(
        fields.contains("estimate_mm=22") && fields.contains("pending_mm=22"),
        "{fields}"
    );
    let (shown_before, true_before) = distances(&mut sim, start);
    let error_before = (shown_before - true_before) / true_before * 100.0;
    assert!((error_before + 2.9).abs() < 0.2, "{error_before:.2} %");
    sim.checkpoint("ride 1: still on the old size after learning");
    sim.stop_ride();
    sim.wait_for("WHEEL SET 22", seconds(5)).unwrap();
    sim.checkpoint("ride 1 over: the rider is told once");
    // The learned size depends on where the ride's GPS windows fall, so a
    // millimetre of timing drift may move it.
    let fields = wheel_fields(&mut sim);
    let learned: u16 = fields
        .split("stored_mm=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|mm| mm.parse().ok())
        .unwrap();
    assert!((2199..=2201).contains(&learned), "{fields}");
    assert!(
        fields.contains(&format!("wheel_mm={learned} stored_mm={learned} mode=auto")),
        "{fields}"
    );
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");
    go_home(&mut sim);

    ride_settings(&mut sim);
    sim.expect_text(&format!("{learned} MM"));
    sim.expect_text("AUTO");
    sim.checkpoint("settings: wheel learned, auto");
    leave_settings(&mut sim);

    // Ride 2 measures with the learned size.
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let speed = reading(&sim, "KM/H");
    assert!((speed - 28.0).abs() < 0.2, "{speed}");
    let (shown_after, true_after) = distances(&mut sim, start);
    let error_after = (shown_after - true_after) / true_after * 100.0;
    assert!(error_after.abs() < 0.3, "{error_after:.2} %");
    sim.checkpoint("ride 2: the learned wheel reads right");
    sim.stop_ride();
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");
    println!("| ride | wheel in force | VANA km | true km | distance error |");
    println!("| --- | --- | --- | --- | --- |");
    println!(
        "| 1, before | 2136 mm | {shown_before:.2} | {true_before:.2} | {error_before:+.2} % |"
    );
    println!(
        "| 2, after | {learned} mm | {shown_after:.2} | {true_after:.2} | {error_after:+.2} % |"
    );
}

#[test]
fn a_hilly_winding_loop_is_learned_just_as_well() {
    let mut sim = rider();
    // A 4 km loop bends 7.5 degrees per fix interval; it climbs 60 m.
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        4_000.0,
        60.0,
    ));
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let (shown_before, true_before) = distances(&mut sim, start);
    let error_before = (shown_before - true_before) / true_before * 100.0;
    sim.stop_ride();
    sim.wait_for("WHEEL SET 22", seconds(5)).unwrap();
    let fields = wheel_fields(&mut sim);
    let learned: f64 = fields
        .split("stored_mm=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|mm| mm.parse().ok())
        .unwrap();
    assert!((learned - 2200.0).abs() / 2200.0 < 0.005, "{fields}");
    go_home(&mut sim);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let (shown_after, true_after) = distances(&mut sim, start);
    let error_after = (shown_after - true_after) / true_after * 100.0;
    assert!(error_after.abs() < 0.5, "{error_after:.2} %");
    sim.stop_ride();
    println!(
        "| hilly, winding 4 km loop, ride 1 before | 2136 mm | {shown_before:.2} | {true_before:.2} | {error_before:+.2} % |"
    );
    println!(
        "| same loop, ride 2 after (learned {learned} mm) | {learned} mm | {shown_after:.2} | {true_after:.2} | {error_after:+.2} % |"
    );
}

#[test]
fn a_manual_wheel_size_is_kept_across_rides_and_power_loss() {
    let mut sim = rider();
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);

    ride_settings(&mut sim);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.expect_text("+5");
    // Steps land on multiples of five, so 2105 or 2155 are reachable.
    for _ in 0..4 {
        sim.press(Button::BottomRight);
    }
    sim.expect_text("2155");
    sim.expect_text("MANUAL");
    sim.checkpoint("editing the wheel size");
    sim.press(Button::TopLeft);
    sim.press(Button::BottomLeft);
    sim.expect_field("WHEEL MODE", "MANUAL");
    sim.checkpoint("settings: wheel kept as set");
    sim.press(Button::BottomRight);
    sim.expect_field("WHEEL MODE", "AUTO");
    // An edit that ends where it began changes nothing, and leaves AUTO.
    sim.tap(120, 190);
    sim.press(Button::BottomRight);
    sim.press(Button::BottomLeft);
    sim.press(Button::TopLeft);
    sim.expect_text("AUTO");
    sim.expect_field("WHEEL MODE", "AUTO");
    sim.tap(120, 242);
    sim.expect_field("WHEEL MODE", "MANUAL");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    leave_settings(&mut sim);

    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(15));
    let speed = reading(&sim, "KM/H");
    assert!((speed - 28.0 * 2155.0 / 2200.0).abs() < 0.2, "{speed}");
    assert!(
        wheel_fields(&mut sim).contains("windows=0"),
        "no learning while manual"
    );
    sim.stop_ride();
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");

    sim.power_loss();
    ride_settings(&mut sim);
    sim.expect_text("2155 MM");
    sim.expect_text("MANUAL");
}

fn battery(sim: &mut Sim, kind: u8, number: u16, report: BatteryReport) {
    sim.ant_sensor(kind, number).battery = Some(report);
}

fn run_color(sim: &Sim, text: &str) -> u16 {
    sim.screen()
        .run(text)
        .unwrap_or_else(|| panic!("no {text:?}; screen:\n{}", sim.screen().text()))
        .color
}

#[test]
fn a_low_battery_turns_sensors_amber_on_home_and_ready() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(2, 2_900),
    );
    battery(
        &mut sim,
        device_type::HEART_RATE,
        HEART,
        BatteryReport::new(4, 2_450),
    );
    sim.advance(seconds(12));
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert_eq!(run_color(&sim, "HR LOW"), AMBER);
    sim.expect_text("BATTERY");
    assert_eq!(run_color(&sim, "2/2 LIVE"), AMBER);
    sim.checkpoint("home with a low heart rate battery");

    sim.home("RIDE");
    sim.expect_text("READY");
    sim.expect_text("HR LOW BATTERY");
    sim.expect_text("CHANGE IT SOON");
    sim.checkpoint("ready with a low heart rate battery");
    sim.press(Button::TopLeft);

    sim.home("SENSORS");
    sim.open_sensor_detail("HEART RATE");
    sim.expect_text("LOW BATTERY  2.4 V");
    assert_eq!(run_color(&sim, "LOW BATTERY  2.4 V"), AMBER);
    sim.checkpoint("heart rate detail with a low battery");
    sim.press(Button::TopLeft);
    sim.open_sensor_detail("POWER METER");
    sim.expect_text("GOOD BATTERY  2.9 V");
    assert_eq!(run_color(&sim, "GOOD BATTERY  2.9 V"), SECONDARY);
    sim.checkpoint("power meter detail with a good battery");
}

#[test]
fn a_critical_battery_is_named_and_a_fresh_one_clears_the_warning() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(5, 2_100),
    );
    sim.advance(seconds(12));
    assert_eq!(run_color(&sim, "PWR CRITICAL"), RED);
    sim.checkpoint("home with a critical power meter battery");
    sim.home("RIDE");
    sim.expect_text("PWR CRITICAL BATTERY");
    sim.expect_text("CHANGE IT BEFORE RIDING");
    sim.checkpoint("ready with a critical power meter battery");
    sim.press(Button::TopLeft);

    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(1, 3_000),
    );
    sim.advance(seconds(12));
    sim.home("RIDE");
    sim.expect_text("1 SENSOR LIVE");
    sim.expect_no_text("CRITICAL BATTERY");
}

#[test]
fn a_sensor_that_is_not_live_is_named_before_any_battery_warning() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    battery(
        &mut sim,
        device_type::HEART_RATE,
        HEART,
        BatteryReport::new(4, 2_450),
    );
    sim.advance(seconds(12));
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(20));
    sim.expect_text("1/2 LIVE");
    sim.expect_no_text("LOW BATTERY");
    sim.checkpoint("home does not name the battery while a sensor is missing");
    sim.home("RIDE");
    sim.expect_text("SENSORS WAKING UP");
    sim.expect_text("PWR");
    sim.expect_no_text("LOW BATTERY");
    sim.checkpoint("ready names the missing sensor ahead of a low battery");
}

#[test]
fn reserved_battery_values_and_battery_pages_never_disturb_readings() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    for status in [0, 6, 7] {
        battery(
            &mut sim,
            device_type::HEART_RATE,
            HEART,
            BatteryReport::new(status, 2_450),
        );
        battery(
            &mut sim,
            device_type::POWER,
            POWER,
            BatteryReport::new(status, 2_450),
        );
        sim.advance(seconds(12));
        sim.home("RIDE");
        sim.expect_text("2 SENSORS LIVE");
        sim.expect_no_text("LOW BATTERY");
        sim.press(Button::TopLeft);
    }
    battery(
        &mut sim,
        device_type::HEART_RATE,
        HEART,
        BatteryReport::new(4, 2_450),
    );
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(4, 2_450),
    );
    sim.home("RIDE");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(minutes(1));
    sim.expect_field("W", "205");
    sim.expect_field("BPM", "138");
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(
        ride.samples()
            .skip(5)
            .all(|s| s.power_watts == Some(205) && s.heart_bpm == Some(138)),
        "battery pages leaked into the recording"
    );
}

fn connected_strap(sim: &mut Sim, level: Option<u8>) {
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(level);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
}

#[test]
fn a_low_ble_strap_battery_turns_home_and_ready_amber() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(12));
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert_eq!(run_color(&sim, "HR LOW"), AMBER);
    assert_eq!(run_color(&sim, "1/1 LIVE"), AMBER);
    sim.checkpoint("home with a low ble heart strap battery");

    sim.home("RIDE");
    sim.expect_text("HR LOW BATTERY");
    sim.expect_text("CHANGE IT SOON");
    sim.checkpoint("ready with a low ble heart strap battery");
}

#[test]
fn a_critical_ble_strap_battery_is_red_and_the_next_read_clears_it() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(3));
    assert_eq!(run_color(&sim, "HR CRITICAL"), RED);
    sim.home("RIDE");
    sim.expect_text("HR CRITICAL BATTERY");
    sim.expect_text("CHANGE IT BEFORE RIDING");
    sim.checkpoint("ready with a critical ble heart strap battery");
    sim.press(Button::TopLeft);

    // A fresh cell shows on the strap's next minute re-read.
    sim.ble_heart_strap_battery(Some(100));
    sim.advance(seconds(65));
    sim.expect_no_text("CRITICAL");
    assert_ne!(run_color(&sim, "1/1 LIVE"), AMBER);
    sim.checkpoint("home after the strap's battery was replaced");
}

#[test]
fn a_healthy_hostile_or_missing_ble_battery_level_warns_of_nothing() {
    for level in [Some(80), Some(101), Some(255), None] {
        let mut sim = rider();
        connected_strap(&mut sim, level);
        sim.home("RIDE");
        sim.expect_text("1 SENSOR LIVE");
        sim.expect_no_text("LOW BATTERY");
        sim.expect_no_text("CRITICAL BATTERY");

/// Open the power meter's detail page. Opening Sensors starts a search, and
/// the radio takes no message until its window closes, so `settled` waits that
/// out; otherwise the request is made while the radio is still held.
fn calibrate_power_meter(sim: &mut Sim, reply: ZeroReply, settled: bool) {
    sim.ant_sensor(device_type::POWER, POWER).zero = reply;
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(3));
    sim.home("SENSORS");
    sim.open_sensor_detail("POWER METER");
    sim.expect_text("CALIBRATE");
    sim.checkpoint("power meter detail opens on back");
    sim.press(Button::BottomLeft);
    sim.checkpoint("power meter detail with calibrate focused");
    if settled {
        sim.advance(seconds(16));
    }
}

#[test]
fn a_ble_strap_that_is_still_connecting_is_waited_for_not_blamed_for_its_battery() {
    let mut sim = rider();
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(Some(12));
    sim.select_ble_heart_strap();
    sim.advance(seconds(3));
    sim.home("RIDE");
    sim.expect_text("SENSORS WAKING UP");
    sim.expect_no_text("LOW BATTERY");
}

#[test]
fn a_live_ant_heart_strap_stands_for_the_role_and_a_silent_one_does_not() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    connected_strap(&mut sim, Some(12));
    sim.expect_text("1/1 LIVE");
    sim.expect_no_text("LOW BATTERY");

    sim.set_present(device_type::HEART_RATE, HEART, false);
    sim.advance(seconds(20));
    sim.expect_text("1/2 LIVE");
    sim.checkpoint("home with a silent ant strap and a live ble strap");
    sim.home("RIDE");
    sim.expect_text("SENSORS WAKING UP");
    sim.press(Button::TopLeft);

    sim.set_present(device_type::HEART_RATE, HEART, true);
    sim.advance(seconds(40));
    sim.expect_text("1/1 LIVE");
}

#[test]
fn a_failed_ble_link_is_left_off_home_and_ready() {
    let mut sim = rider();
    sim.ble_heart_strap(false);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
    sim.device.ble.snapshot.link = Link::Failed;
    sim.advance(seconds(1));
    sim.home("RIDE");
    sim.expect_text("NO SENSORS");

fn calibrating_a_power_meter_shows_the_offset_it_reports() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(-37), false);
    sim.press(Button::BottomRight);
    sim.wait_for("WAITING FOR RADIO", seconds(1)).unwrap();
    sim.checkpoint("power meter waiting for the radio");
    sim.wait_for("CALIBRATING", seconds(20)).unwrap();
    sim.checkpoint("power meter calibrating");
    sim.wait_for("CALIBRATED", seconds(5)).unwrap();
    sim.expect_text("OFFSET -37");
    assert_eq!(run_color(&sim, "CALIBRATED  OFFSET -37"), GREEN);
    sim.checkpoint("power meter calibrated with its offset");
    sim.expect_text("DISCONNECT");
    sim.wait_gone("CALIBRATED", seconds(12)).unwrap();
}

#[test]
fn a_power_meter_that_refuses_to_zero_ends_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Failure, true);
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATION FAILED", seconds(5)).unwrap();
    assert_eq!(run_color(&sim, "CALIBRATION FAILED"), AMBER);
    sim.expect_text("CRANKS STILL, TRY AGAIN");
    sim.checkpoint("power meter calibration failed");
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATING", seconds(1)).unwrap();
    sim.wait_for("CALIBRATION FAILED", seconds(5)).unwrap();
}

#[test]
fn a_silent_power_meter_times_out_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Silence, true);
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATING", seconds(1)).unwrap();
    sim.advance(seconds(8));
    sim.expect_text("CALIBRATING");
    sim.checkpoint("power meter still waiting for its answer");
    sim.wait_for("CALIBRATION FAILED", seconds(4)).unwrap();
    sim.checkpoint("power meter calibration timed out");
}

#[test]
fn calibrating_a_power_meter_never_disturbs_a_later_ride() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(512), true);
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATED", seconds(5)).unwrap();
    sim.press(Button::TopLeft);
    sim.press(Button::TopLeft);
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.home("RIDE");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(minutes(1));
    sim.expect_field("W", "205");
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(
        ride.samples().skip(5).all(|s| s.power_watts == Some(205)),
        "calibration pages leaked into the recording"
    );
}
