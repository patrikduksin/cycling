//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
use device_api::ant::{LinkState, Request};
use device_api::ble_transport::Link;
use device_api::gps::FixState;
use device_api::input::Button;
use device_api::positioning::Positioning;
use firmware_services::gps::unix_seconds;
use shell_simulator::ant::{BatteryReport, ZeroReply, device_type};
use shell_simulator::bulk::Presence;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Screen, Sim, minutes, seconds};
use shell_simulator::trace::timeline::{Kind as Traced, session_clock};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::fit::Refusal;
use vana::ride::log::{
    Entry, Kind, LapStart, SLOT_SIZE, SLOTS, Sample, Slot, commit_word, decode, encode,
};
use vana::screens::chrome::{AMBER, GREEN, RED, SECONDARY, WHITE, ZONES};

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;
const SPEED_ONLY: u16 = 778;
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
        .sim()
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
            Entry::event(Kind::Start, 1, 0, 0)
        } else if index == used - 1 {
            Entry::event(Kind::Finish, 1, sequence, index as u64 * 4_000)
        } else {
            let first = (index as u64 - 1) * 4 + 1;
            let batch = [
                sample(first),
                sample(first + 1),
                sample(first + 2),
                sample(first + 3),
            ];
            Entry::batch(1, sequence, &batch).unwrap()
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
fn approaching_vehicle_is_drawn_and_sounds_one_alarm() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(5));
    // Starting chimed once; no alert yet.
    assert_eq!(alerts(&sim), 0);
    assert!(
        !sim.device.sim_mut().speaker.played.is_empty(),
        "the start chime"
    );

    sim.vehicle(Vehicle {
        range_m: 120.0,
        closing_kmh: 40.0,
        fast: true,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
    assert_eq!(alerts(&sim), 1, "the alarm sounds as the vehicle appears");
    sim.advance(seconds(4));
    assert_eq!(alerts(&sim), 1, "closing in ticks, not another alarm");
    sim.checkpoint("vehicle close, alarm sounded once");
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
fn radar_beeps_faster_and_higher_as_a_vehicle_closes_then_calls_all_clear() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    let before = sim.device.sim_mut().speaker.played.len();
    sim.vehicle(Vehicle {
        range_m: 140.0,
        closing_kmh: 30.0,
        fast: false,
        side: Side::Behind,
    });
    for _ in 0..80 {
        if sim.wait_for("ALL CLEAR", 500).is_ok() {
            break;
        }
        sim.expect_no_text("VEHICLE");
    }
    sim.expect_text("ALL CLEAR");
    sim.checkpoint("radar: all clear after the vehicle passed");
    sim.advance(seconds(1));
    let heard = sim.device.sim_mut().speaker.played[before..].to_vec();

    // The alarm, then low, middle and high ticks, then ALL CLEAR (low, high).
    let mut ids: Vec<_> = heard.iter().map(|&(_, id)| id).collect();
    ids.dedup();
    assert_eq!(ids, [22, 21, 0, 10, 21, 10], "{heard:?}");
    let gap = |id: u8| -> u64 {
        let at: Vec<_> = heard.iter().filter(|h| h.1 == id).map(|h| h.0).collect();
        at.windows(2).map(|pair| pair[1] - pair[0]).min().unwrap()
    };
    assert!(
        gap(21) > gap(0),
        "low ticks are slower than middle: {heard:?}"
    );
    assert!(
        gap(0) > gap(10),
        "middle ticks are slower than high: {heard:?}"
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
    use vana::screens::chrome::{POWER, SPEED};
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
fn a_speed_and_cadence_sensor_alone_drives_cadence_and_its_low_alert() {
    let mut sim = rider();
    // The power meter is in range but not saved: only the SPD/C crank speaks.
    sim.pair(&["SPEED/CAD"]);
    sim.home("SETTINGS");
    sim.set_option("LOW CADENCE", "60 RPM");
    sim.press(Button::TopLeft);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.ride_at(28.0, 0.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.expect_field("RPM", "85");
    sim.expect_no_text("CADENCE LOW");
    sim.checkpoint("cadence from the speed and cadence sensor");
    sim.ride_at(28.0, 0.0, 0.0, 55.0);
    sim.wait_for("CADENCE LOW 55", seconds(30)).unwrap();
    sim.checkpoint("alert: cadence below the limit");
    sim.ride_at(28.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(8));
    sim.expect_field("RPM", "0");
}

#[test]
fn ble_heart_strap_drives_heart_rate_and_drops_out_honestly() {
    ble_heart_strap(&mut rider());
}

fn ble_heart_strap(sim: &mut Sim) {
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
    assert_eq!(sim.device.sim_mut().ble.snapshot.link, Link::Connected);
    sim.advance(seconds(4));
    sim.expect_field("BPM", "--");
    assert_ne!(sim.device.sim_mut().ble.snapshot.link, Link::Connected);
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
    assert_eq!(alerts(&sim), 1, "one alarm for the whole group");
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
    assert_eq!(alerts(&sim), 2, "the next vehicle sounds the alarm again");
}

#[test]
fn pause_excludes_time_and_two_hour_ride_keeps_every_sample() {
    two_hour_ride_with_a_pause(&mut rider());
}

fn two_hour_ride_with_a_pause(sim: &mut Sim) {
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
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    expect_near(&mut sim, "KM", 5.0, 0.05);
    sim.expect_field("RPM", "90");
    sim.checkpoint("main page after ten minutes");

    ride_page(&mut sim, "2/5");
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
fn ble_select_and_forget_wait_for_a_pending_ble_operation() {
    let mut sim = rider();
    sim.select_ble_heart_strap();
    let (_, settings) = sim.command("SETTINGS");
    let (_, ble) = sim.command("BLE");
    sim.hold_ble_operation();
    let (status, out) = sim.command("BLE SELECT CSC 4f54484552 -");
    assert_eq!(status, "UNAVAILABLE", "BLE SELECT while pending: {out}");
    let (status, out) = sim.command("BLE FORGET");
    assert_eq!(status, "UNAVAILABLE", "BLE FORGET while pending: {out}");
    assert_eq!(
        sim.command("SETTINGS").1,
        settings,
        "a refused choice changed the settings"
    );
    assert!(ble.contains("saved=true profile=1 "), "{ble}");
    let (_, refused) = sim.command("BLE");
    assert!(refused.contains("saved=true profile=1 "), "{refused}");
    sim.finish_ble_operation();
    let (status, out) = sim.command("BLE FORGET");
    assert_eq!(
        status, "ACCEPTED",
        "BLE FORGET once the operation completed: {out}"
    );
    let (_, forgotten) = sim.command("BLE");
    assert!(forgotten.contains("saved=false profile=0 "), "{forgotten}");
}

#[test]
fn wifi_config_and_forget_wait_for_a_pending_wifi_operation() {
    let mut sim = rider();
    let (status, out) = sim.command("WIFI CONFIG WPA2 54657374 70617373776f7264");
    assert_eq!(status, "ACCEPTED", "WIFI CONFIG: {out}");
    sim.hold_wifi_operation();
    let (_, wifi) = sim.command("WIFI");
    let (status, out) = sim.command("WIFI CONFIG WPA3 4f74686572 70617373776f7264");
    assert_eq!(status, "UNAVAILABLE", "WIFI CONFIG while pending: {out}");
    let (status, out) = sim.command("WIFI FORGET");
    assert_eq!(status, "UNAVAILABLE", "WIFI FORGET while pending: {out}");
    assert_eq!(
        sim.command("WIFI").1,
        wifi,
        "a refused change reached Wi-Fi"
    );
    sim.finish_wifi_operation();
    let (status, out) = sim.command("WIFI FORGET");
    assert_eq!(
        status, "ACCEPTED",
        "WIFI FORGET once the operation completed: {out}"
    );
    assert!(sim.command("WIFI").1.contains("saved=false"));
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
    ride_page(&mut sim, "2/5");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "ASCENT M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    // The final climb brought up the climb page; back to the totals.
    ride_page(&mut sim, "2/5");
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
    ride_page(&mut sim, "2/5");
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
    ride_page(&mut sim, "1/5");
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
    ride_page(&mut sim, "2/5");
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
    manual_laps(&mut rider());
}

fn manual_laps(sim: &mut Sim) {
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
    ride_page(sim, "3/5");
    sim.expect_text("LAP 2");
    let lap = sim.screen().field("LAP TIME").unwrap();
    assert!(lap.starts_with("04:0"), "{lap}");
    expect_near(sim, "LAP KM", 2.2, 0.05);
    // Within a watt or a beat: the change of effort spans a second.
    expect_near(sim, "LAP W", 250.0, 1.0);
    expect_near(sim, "LAP KM/H", 33.0, 0.2);
    expect_near(sim, "LAP BPM", 150.0, 1.0);
    sim.expect_text("LAP 1");
    sim.expect_field("LAP 1 W", "200");
    let harder = sim.screen().field("W DIFF").unwrap();
    assert!(harder == "+49" || harder == "+50", "{harder}");
    assert_eq!(color_of(sim, &harder), GREEN);
    let faster = sim.screen().field("KM/H DIFF").unwrap();
    assert!(
        faster.starts_with("+2.") || faster.starts_with("+3."),
        "{faster}"
    );
    assert_eq!(color_of(sim, &faster), GREEN);
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
    let easier = reading(sim, "W DIFF");
    assert!((easier - (150.0 - previous)).abs() <= 1.0, "{easier}");
    assert_eq!(
        color_of(sim, &sim.screen().field("W DIFF").unwrap()),
        SECONDARY
    );
    let slower = sim.screen().field("KM/H DIFF").unwrap();
    assert!(
        slower.starts_with("-8.") || slower.starts_with("-9."),
        "{slower}"
    );
    assert_eq!(color_of(sim, &slower), SECONDARY);
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
    ride_page(&mut sim, "3/5");
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
    ride_page(&mut sim, "3/5");
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
    ride_page(&mut sim, "3/5");
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
    ride_page(&mut sim, "3/5");
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
    ride_page(&mut sim, "3/5");
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

/// The climb page's grade: its largest percentage, which needs no caption.
#[track_caller]
fn expect_grade(sim: &mut Sim, expected: f64, tolerance: f64) {
    let screen = sim.screen();
    let grade = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.text.ends_with('%'))
        .max_by_key(|run| run.scale)
        .unwrap_or_else(|| panic!("no grade; screen:\n{}", screen.text()));
    let number: f64 = grade.text.trim_end_matches('%').parse().unwrap();
    assert!(
        (number - expected).abs() <= tolerance,
        "grade {}, expected {expected}% ± {tolerance}",
        grade.text
    );
    sim.note(NoteKind::Check, format!("grade shows {}", grade.text));
}

#[test]
fn climb_page_measures_a_six_percent_climb_over_the_top() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "4/5");
    ride_to(&mut sim, 800.0);
    sim.expect_text("NO CLIMB");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    sim.expect_field("ASCENT M", "0");
    expect_grade(&mut sim, 0.0, 0.5);
    sim.expect_text("180");
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
    expect_grade(&mut sim, 6.0, 0.5);
    expect_near(&mut sim, "GAIN M", 45.0, 4.0);
    expect_near(&mut sim, "SO FAR KM", 0.75, 0.05);
    sim.expect_text("260");
    sim.checkpoint("climb page mid-climb");

    ride_to(&mut sim, 2510.0);
    sim.expect_text("CLIMBING");
    expect_near(&mut sim, "GAIN M", 90.0, 4.0);
    expect_near(&mut sim, "SO FAR KM", 1.5, 0.05);
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
    sim.checkpoint("climb page after the descent");
}

#[test]
fn in_saver_the_climb_page_comes_up_already_climbing() {
    let mut sim = hill_rider();
    sim.open_mode_row();
    sim.press(Button::BottomRight);
    sim.expect_field("MODE", "SAVER");
    go_home(&mut sim);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 500.0);
    sim.pause_ride();
    sim.advance(WALL_AND_RIDE_SECONDS_APART_MS);
    sim.resume_ride();
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    while sim.screen().page().as_deref() != Some("4/5") {
        assert!(
            sim.world().distance_m < 1600.0,
            "no climb page; screen:\n{}",
            sim.screen().text()
        );
        sim.advance(50);
    }
    sim.expect_text("CLIMBING");
    sim.checkpoint("saver: climb page brought up by the climb");
}

#[test]
fn a_climb_brings_up_the_climb_page_and_hands_back_the_main_page() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    let shown = ride_until_page(&mut sim, "4/5", 1600.0);
    assert!((1300.0..1450.0).contains(&shown), "at {shown:.0} m");
    sim.expect_text("CLIMBING");
    sim.checkpoint("climb page brought up by the climb");

    ride_to(&mut sim, 2500.0);
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let back = ride_until_page(&mut sim, "1/5", 2800.0);
    assert!(back > 2500.0, "back at {back:.0} m");
    expect_near(&mut sim, "KM/H", 40.0, 0.3);
    sim.checkpoint("main page again over the top");
}

/// Turn the named ride pages off in Settings, then go back Home.
fn hide_ride_pages(sim: &mut Sim, pages: &[&str]) {
    sim.home("SETTINGS");
    for _ in 0..6 {
        sim.swipe_next();
    }
    sim.expect_text("RIDE PAGES");
    for label in ["STATS", "LAP", "CLIMB", "MAP"] {
        sim.expect_field(label, "ON");
        if pages.contains(&label) {
            sim.press(Button::BottomRight);
            sim.expect_field(label, "OFF");
        }
        if label == "CLIMB" && pages.contains(&label) {
            sim.expect_text("NO CLIMB POP-UP");
        }
        if matches!(label, "CLIMB" | "MAP") {
            sim.checkpoint(&format!("{label} row with {} off", pages.join(" and ")));
        }
        sim.press(Button::BottomLeft);
    }
    go_home(sim);
}

#[test]
fn chosen_main_page_readings_show_on_the_ride_and_survive_a_reboot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(5));
    sim.expect_field("RPM", "90");
    sim.expect_text("KM");
    sim.checkpoint("main page with the default readings");
    sim.stop_ride();
    sim.press(Button::BottomLeft);
    go_home(&mut sim);

    sim.home("SETTINGS");
    for _ in 0..5 {
        sim.swipe_next();
    }
    sim.expect_text("MAIN PAGE");
    sim.expect_field("LOWER LEFT", "CADENCE");
    sim.expect_field("LOWER RIGHT", "DISTANCE");
    sim.checkpoint("settings: main page readings at their defaults");
    sim.set_option("LOWER LEFT", "KCAL");
    sim.set_option("LOWER RIGHT", "NP");
    sim.checkpoint("settings: KCAL and NP chosen");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.expect_field("KCAL", "120");
    sim.expect_field("NP", "200");
    sim.expect_no_text("RPM");
    sim.checkpoint("main page showing KCAL and NP");
    sim.stop_ride();
    sim.press(Button::BottomLeft);
    go_home(&mut sim);

    sim.power_loss();
    assert!(sim.command("PROFILE").1.contains("stored=current"));
    sim.home("SETTINGS");
    for _ in 0..5 {
        sim.swipe_next();
    }
    sim.expect_field("LOWER LEFT", "KCAL");
    sim.expect_field("LOWER RIGHT", "NP");
    go_home(&mut sim);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(2));
    sim.expect_text("KCAL");
    sim.expect_text("NP");
    sim.expect_no_text("RPM");
    sim.checkpoint("main page after a reboot still showing KCAL and NP");
}

#[test]
fn turned_off_ride_pages_leave_the_dots_and_the_page_walk() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["STATS", "LAP"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(5));
    assert_eq!(sim.screen().page().as_deref(), Some("1/3"));
    sim.checkpoint("main page of three");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("2/3"));
    sim.expect_text("CLIMB");
    sim.checkpoint("climb page second of three");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("3/3"));
    sim.checkpoint("map page last of three");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("2/3"));
    sim.swipe_next();
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("1/3"), "wraps to main");
}

#[test]
fn a_map_page_turned_off_leaves_the_walk_from_climb_to_main() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["MAP"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(5));
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    assert_eq!(sim.screen().page().as_deref(), Some("4/4"));
    sim.expect_text("CLIMB");
    sim.checkpoint("climb page last of four");
    sim.press(Button::BottomLeft);
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"), "wraps to main");
}

#[test]
fn a_climb_does_not_bring_up_a_climb_page_turned_off() {
    let mut sim = hill_rider();
    hide_ride_pages(&mut sim, &["CLIMB"]);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    assert_eq!(sim.screen().page().as_deref(), Some("1/4"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    while sim.world().distance_m < 2500.0 {
        sim.advance(seconds(1));
        assert_eq!(
            sim.screen().page().as_deref(),
            Some("1/4"),
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
        assert_eq!(sim.screen().page().as_deref(), Some("4/5"));
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
    shell_simulator::ant::link(&sim.device.sim().ant, kind, sim.device.now)
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
        .sim()
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
            .sim_mut()
            .ant
            .log
            .iter()
            .all(|(_, request)| !matches!(request, Request::Scan { .. })),
        "{:?}",
        sim.device.sim_mut().ant.log
    );
    assert_eq!(connects(&sim, device_type::HEART_RATE), [HEART]);
    assert_eq!(connects(&sim, device_type::POWER), [POWER]);

    sim.home("SENSORS");
    sim.expect_text("LIVE");
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
    assert!(
        sim.device
            .sdk
            .saved_sensors()
            .of_type(device_type::HEART_RATE)
            .is_none()
    );
    sim.expect_no_text("HEART RATE");
    sim.checkpoint("heart rate disconnected");
    // The channel closes once the cancelled discovery has released the radio.
    let mut waited = 0;
    while link(&sim, device_type::HEART_RATE) != Some(LinkState::Disconnected) {
        assert!(waited < seconds(15), "{:?}", sim.device.sim_mut().ant.log);
        sim.advance(100);
        waited += 100;
    }
    sim.nearby_page();
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_text("HEART RATE");
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
    traffic_light_stop(&mut rider());
}

fn traffic_light_stop(sim: &mut Sim) {
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
    assert!(recording(sim));
    sim.checkpoint("auto resumed when the light turns green");
    sim.advance(seconds(20));
    sim.expect_no_text("AUTO");
    // About 30 + 7 + 20 s riding; the 60 s wait is not in there.
    let secs = elapsed_secs(sim);
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
    let from = sim.device.sim_mut().speaker.played.len();
    sim.advance(seconds(60));
    assert_eq!(
        played_since(&sim, from),
        [0, 0, 10, 0, 0, 10],
        "two reminders in a minute"
    );
    assert!(sim.screen().run("PAUSED").is_some());
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
    sim.expect_no_text("AUTO PAUSED");
    sim.checkpoint("speed sensor lost, still recording");
    let km = reading(&sim, "KM");
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(20)).unwrap();
    sim.advance(seconds(30));
    sim.expect_text("AUTO PAUSED");
    sim.ride_at(36.0, 0.0, 0.0, 0.0);
    let latency = sim.wait_for("AUTO RESUME", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-resume took {latency} ms");
    sim.advance(minutes(1));
    assert!(recording(&mut sim));
    expect_near(&mut sim, "KM/H", 36.0, 0.2);
    let ridden = reading(&sim, "KM") - km;
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
    sim.wait_for("AUTO PAUSED", seconds(15)).unwrap();
    sim.advance(seconds(30));
    sim.expect_text("AUTO PAUSED");
    sim.ride_at(36.0, 0.0, 0.0, 0.0);
    let latency = sim.wait_for("AUTO RESUME", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-resume took {latency} ms");
    sim.advance(minutes(1));
    assert!(recording(&mut sim));
    expect_near(&mut sim, "KM/H", 36.0, 0.2);
    expect_near(&mut sim, "KM", 0.6, 0.05);
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
        ride_page(&mut sim, "2/5");
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

/// The milestones VANA reports for `cycling startup-timing --sim`, which
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
    sim.advance(seconds(3));
    sim.expect_text("READY TO RIDE");
}

#[test]
fn turned_off_on_usb_power_the_device_starts_again_in_charging_standby() {
    let mut sim = hashed().boot();
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    let ridden = sim.device.boot;
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    let pressed = sim.now();
    sim.advance(seconds(2));
    assert_eq!(sim.now(), pressed + 2_000, "the world went on for 2 s");
    assert!(sim.is_off(), "turned off");
    let standby = sim.device.boot;
    assert_ne!(standby, ridden, "the companion started the device again");
    assert!(sim.device.standby_lit(), "the charge shows");
    sim.expect_text("CHARGING");
    sim.advance(seconds(3));
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    let frames = sim.frame_hashes();
    assert!(!frames.is_empty() && replayed.frame_hashes() == frames);
    sim.turn_on();
    sim.advance(seconds(3));
    sim.expect_text("READY TO RIDE");
    assert_eq!(sim.device.boot, standby, "the wake keeps the session");
}

#[test]
fn a_ride_after_a_long_charging_standby_is_in_its_file_whole() {
    use firmware_services::trace::{Event, decode};
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "in charging standby");
    sim.advance(minutes(40));
    sim.turn_on();
    sim.advance(seconds(3));
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    assert!(sim.device.sync_trace(), "the wake opened the file");
    let pulled = sim.store_file(&sim.device.trace_file().unwrap()).unwrap();
    let (_, events) = decode(&pulled).unwrap();
    let lost = events
        .filter(|event| matches!(event, Event::Lost { .. }))
        .count();
    assert_eq!(lost, 0, "the standby fit the spool");
    println!("{}", trace_size(&pulled, sim.device.now));
    let mut capture = sim.capture();
    capture.trace = pulled;
    let mut replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
    assert!(
        replayed.frame_hashes() == sim.frame_hashes(),
        "the same frames"
    );
    let samples = |sim: &mut Sim| {
        sim.rides()
            .iter()
            .map(|ride| ride.samples().count())
            .collect::<Vec<_>>()
    };
    assert_eq!(samples(&mut replayed), samples(&mut sim));
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
    assert!(sim.on_home(), "the Clear ends on the first page");
    sim.checkpoint("internal journal cleared after a moved ride: no carried time");
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
    sim.tap(120, 210);
    sim.expect_text("NEARBY");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_no_text("NEXT");
    sim.checkpoint("sensors menu");
    sim.tap(120, 118);
    sim.expect_text("HEART RATE ADDED");
    sim.expect_text("POWER METER");
    sim.checkpoint("sensor connected by tap");
    sim.press(Button::TopLeft);
    sim.wait_for("LIVE", seconds(15)).unwrap();
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
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));

    sim.swipe_next();
    assert_eq!(sim.screen().page().as_deref(), Some("2/5"));
    sim.expect_text("AVG W");
    sim.checkpoint("swiped to stats");
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("5/5"), "wraps");
    expect_map(&sim);
    sim.swipe_previous();
    sim.swipe_previous();
    assert_eq!(sim.screen().page().as_deref(), Some("3/5"));
    sim.expect_text("LAP TIME");
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
    assert_eq!(sim.screen().page().as_deref(), Some("1/6"));
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
    assert_eq!(sim.screen().page().as_deref(), Some("2/6"));
    sim.checkpoint("main page with the target band");
    sim.ride_page("1/6");
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
    sim.expect_text("HOLD 121 W");
    sim.expect_text("LAP TO END");
    assert_eq!(target(&mut sim), Some((115.0, 133.0)), "95-110 % of 121 W");
    sim.checkpoint("ramp early at 121 W");
    follow(&mut sim, 8 * 60 - 87 + 30, effort);
    sim.expect_text("HOLD 212 W");
    sim.checkpoint("ramp at 212 W");
    let before = sim.now();
    ride_step(&mut sim, effort);
    // 75 % of the best minute, 295 W.
    sim.wait_for("NEW FTP", seconds(2)).unwrap();
    sim.expect_text("221");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    sim.checkpoint("ramp result offered");
    assert_eq!(sim.workout()["done"], "1", "the workout ends with the ramp");
    sim.press(Button::TopLeft);
    sim.expect_text("FTP KEPT 215 W");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "BACK on the sheet neither laps nor stops"
    );
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=215 "), "{profile}");
    assert_eq!(
        sim.screen().page().as_deref(),
        Some("2/6"),
        "the main ride page, not the workout page"
    );
    sim.checkpoint("main ride page after keeping the old FTP");
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
    let chimes = sim.device.sim_mut().speaker.played.len();
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    assert_eq!(sim.vana_page(), "ride");
    // The second note follows the first's guard.
    sim.advance(seconds(1));
    assert_eq!(
        sim.device.sim_mut().speaker.played.len(),
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
    let chimes = sim.device.sim_mut().speaker.played.len();
    sim.resume_ride();
    sim.wait_for("RESUMED", seconds(2)).unwrap();
    assert_eq!(
        sim.device.sim_mut().speaker.played.len(),
        chimes + 1,
        "resume chime"
    );
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
    let chimes = sim.device.sim_mut().speaker.played.len();
    sim.stop_ride();
    sim.expect_text("7 NEW BESTS");
    sim.advance(seconds(1));
    assert!(
        sim.device.sim_mut().speaker.played.len() >= chimes + 3,
        "best chime"
    );
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

/// Saved rides, one per `(days ago, watts)` (0 W: no power meter), each
/// `minutes` long starting 12:00 UTC that day, as the journal holds them after
/// weeks of riding. Samples are 3 s apart, the longest step the recorder counts.
fn training_history(plan: &[(u64, u16)], minutes: u64) -> Vec<u8> {
    let samples = minutes * 60 * 1_000 / HISTORY_STEP_MS;
    let mut bytes = Vec::new();
    let mut push = |entry: Entry| {
        let mut slot = encode(&entry);
        slot.0[SLOT_SIZE - 4..].copy_from_slice(&commit_word().0);
        bytes.extend_from_slice(&slot.0);
    };
    for (ride, &(days_ago, watts)) in plan.iter().enumerate() {
        let ride_id = ride as u32 + 1;
        let started = history_start(days_ago);
        let sample = |n: u64| Sample {
            active_ms: n * HISTORY_STEP_MS,
            utc_ms: Some(started + n * HISTORY_STEP_MS),
            power_watts: (watts != 0).then_some(watts),
            ..Sample::default()
        };
        push(Entry::event(Kind::Start, ride_id, 0, 0));
        let mut sequence = 1;
        for batch in 0..samples / 4 {
            let samples: Vec<_> = (1..=4).map(|i| sample(batch * 4 + i)).collect();
            push(Entry::batch(ride_id, sequence, &samples).unwrap());
            sequence += 1;
        }
        push(Entry::event(
            Kind::Finish,
            ride_id,
            sequence,
            samples * HISTORY_STEP_MS,
        ));
    }
    bytes
}

const HISTORY_STEP_MS: u64 = 3_000;

/// When a ride of `training_history` started, `days_ago` days back.
fn history_start(days_ago: u64) -> u64 {
    receiver_utc_ms(0) - receiver_utc_ms(0) % 86_400_000 + 12 * 3_600_000 - days_ago * 86_400_000
}

/// Six weeks of rides with power and one ride without: a build that turns hard
/// in the fifth week and eases off in the last days. The trend dips and then
/// recovers, and the ride without power is named as left out.
#[test]
fn form_page_reads_fitness_fatigue_and_form_from_six_weeks_of_rides() {
    use device_api::storage::{OwnedFlash, Region};
    let plan: Vec<(u64, u16)> = vec![
        (41, 230),
        (39, 250),
        (37, 220),
        (34, 260),
        (32, 240),
        (30, 250),
        (27, 260),
        (25, 270),
        (23, 250),
        (20, 270),
        (18, 280),
        (16, 260),
        (15, 290),
        (14, 280),
        (13, 290),
        (12, 310),
        (11, 330),
        (3, 0),
    ];
    let mut sim = Sim::new();
    sim.device
        .media
        .program(Region::Data, 0, &training_history(&plan, 30))
        .unwrap();
    let mut sim = equip(sim.boot());
    sim.home("SETTINGS");
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    sim.wait_for_field("FORM", "+6 FRESH", seconds(30));
    sim.checkpoint("settings: FORM row");
    sim.press(Button::BottomRight);
    sim.wait_for("FATIGUE", seconds(60)).unwrap();
    // Worked outside the firmware from the same rides: each ride's stress at
    // the default 215 W FTP, the seven-day seed, then CTL/ATL a day at a time.
    // Fitness 19, fatigue 13, form +6; form a week apart: -10 -13 -13 -32 -16.
    // The first week of the 120-day window is empty, so both averages start
    // at zero.
    sim.expect_field("FITNESS", "19");
    sim.expect_field("FATIGUE", "13");
    sim.expect_text("+6");
    sim.expect_text("FRESH");
    sim.expect_text("RIDES WITHOUT POWER");
    sim.checkpoint("form page: fitness, fatigue, form and six weeks");
    sim.press(Button::TopLeft);
    sim.expect_field("FORM", "+6 FRESH");
}

/// Rides of `HISTORY_RIDES` over 120 days, through two FTP changes: the first
/// third at 160 W, the next at 190 W and the last at the 215 W set now.
fn ftp_history() -> Vec<(u64, u16, u16)> {
    (0..HISTORY_RIDES)
        .map(|ride| {
            let days_ago = 119 - ride * 119 / (HISTORY_RIDES - 1);
            let watts = 250 + (ride % 5) as u16 * 10;
            let ftp = match ride * 3 / HISTORY_RIDES {
                0 => 160,
                1 => 190,
                _ => 215,
            };
            (days_ago, watts, ftp)
        })
        .collect()
}

const HISTORY_RIDES: u64 = 200;
/// Power time the ride index keeps for a three-minute `training_history` ride:
/// 22 units of 8 s, as the first sample opens the ride and adds no time.
const HISTORY_POWER_MS: u64 = 22 * 8_000;

/// Fitness, fatigue and form worked outside the firmware's facts: each ride's
/// stress at the FTP it was ridden at, by the day it started.
fn reference_form(plan: &[(u64, u16, u16)]) -> vana::ride::fitness::Form {
    use vana::ride::fitness::{DAY_MAX, DAYS, fold};
    let mut days = [0u32; DAYS];
    for &(days_ago, watts, ftp) in plan {
        let stress = vana::ride::load::stress_score(watts, HISTORY_POWER_MS, ftp).unwrap();
        days[DAYS - 1 - days_ago as usize] += stress;
    }
    fold(&days.map(|day| day.min(DAY_MAX) as u16))
}

fn files_entries(sim: &mut Sim) -> usize {
    use firmware_services::{files, volume};
    let mut sector = [0u8; 512];
    let mut store = volume::open(&mut sim.device.bulk, &volume::FILES, &mut sector).unwrap();
    files::mount(&mut store, &mut sector).unwrap().len()
}

/// 200 rides over FORM's 120 days, through two FTP changes, each saved with
/// its facts. FORM counts every ride at the FTP it was ridden at, as a fold
/// worked outside the firmware does, and the facts take one FILES entry.
#[test]
fn form_counts_each_of_200_rides_at_the_ftp_it_was_ridden_at() {
    use device_api::storage::{OwnedFlash, Region};
    let plan = ftp_history();
    let rides: Vec<(u64, u16)> = plan.iter().map(|&(days, watts, _)| (days, watts)).collect();
    let mut sim = Sim::new();
    sim.device
        .media
        .program(Region::Data, 0, &training_history(&rides, 3))
        .unwrap();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    for (ride, &(days_ago, _, ftp)) in plan.iter().enumerate() {
        let record = vana::kept::facts::record::Record {
            journal: b'i',
            ride_id: ride as u32 + 1,
            started_utc_ms: history_start(days_ago) + HISTORY_STEP_MS,
            facts: vana::kept::facts::record::Facts {
                profile: vana::ride::zones::TrainingProfile::new(ftp, 172),
                ..Default::default()
            },
        };
        vana::kept::facts::record::save(&mut sim.device.bulk, &record).unwrap();
    }
    let entries = files_entries(&mut sim);
    let mut sim = equip(sim.boot());
    sim.home("SETTINGS");
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    sim.press(Button::BottomRight);
    sim.wait_for("FATIGUE", seconds(120)).unwrap();
    let reference = reference_form(&plan);
    sim.checkpoint("form page: 200 rides through two FTP changes");
    let screen = sim.screen();
    let fitness = screen.field("FITNESS");
    let fatigue = screen.field("FATIGUE");
    sim.press(Button::TopLeft);
    let form = sim.screen().field("FORM");
    let shown = (
        fitness,
        fatigue,
        form.and_then(|row| row.split(' ').next().map(String::from)),
    );
    println!(
        "reference fitness {} fatigue {} form {:+}; shown {shown:?}; FILES entries {entries}",
        reference.fitness, reference.fatigue, reference.form
    );
    assert_eq!(
        shown,
        (
            Some(reference.fitness.to_string()),
            Some(reference.fatigue.to_string()),
            Some(format!("{:+}", reference.form)),
        ),
        "FORM counts each ride at the FTP it was ridden at"
    );
    assert_eq!(entries, 1, "the facts of 200 rides take one FILES entry");
}

/// A rider with no ride that had power gets a plain answer, not zeros.
#[test]
fn form_page_without_rides_with_power_says_so() {
    let mut sim = rider();
    sim.home("SETTINGS");
    for _ in 0..3 {
        sim.press(Button::BottomLeft);
    }
    sim.press(Button::BottomRight);
    sim.wait_for("NO POWER RIDES", seconds(60)).unwrap();
    sim.expect_text("NEEDS A POWER METER");
    sim.expect_no_text("LEFT OUT");
    sim.checkpoint("form page with no ride that had power");
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
    let played = &sim.device.sim().speaker.played;
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
    for _ in 0..7 {
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("DISPLAY");
    sim.expect_field("BRIGHTNESS", "AUTO");
    sim.expect_field("DIM AFTER", "30 S");
    sim.checkpoint("display section");
    sim.press(Button::BottomRight);
    sim.expect_field("BRIGHTNESS", "25%");
    sim.press(Button::BottomRight);
    sim.press(Button::BottomRight);
    sim.expect_field("BRIGHTNESS", "75%");
    assert_eq!(sim.device.shell.settings.brightness, 75);
    assert!(!sim.device.shell.auto());
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
    // Every alert is off but the radar all clear, until the rider sets one.
    sim.expect_text("ALERTS");
    sim.expect_field("RADAR ALL CLEAR", "ON");
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
    let from = sim.device.sim_mut().speaker.played.len();
    sim.start_ride();
    sim.advance(seconds(2));
    // Middle then high: go.
    assert_eq!(played_since(&sim, from), [0, 10], "start");
    let from = sim.device.sim_mut().speaker.played.len();
    sim.lap(1);
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [0], "one note marks a lap");
    let from = sim.device.sim_mut().speaker.played.len();
    sim.pause_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [10, 0], "high then middle: stop");
    let from = sim.device.sim_mut().speaker.played.len();
    sim.resume_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [10], "resume");
    sim.pause_ride();
    sim.advance(seconds(2));
    let from = sim.device.sim_mut().speaker.played.len();
    sim.stop_ride();
    sim.advance(seconds(2));
    assert_eq!(played_since(&sim, from), [21, 0, 10], "a rising run: saved");
    sim.checkpoint("ride saved with its cue");
}

fn traced(sim: &Sim, kind: Traced) -> Vec<String> {
    shell_simulator::trace::timeline::read(&sim.trace())
        .expect("the session's own trace decodes")
        .entries
        .into_iter()
        .filter(|entry| entry.kind == kind && !entry.repeat)
        .map(|entry| entry.text)
        .collect()
}

fn marks(sim: &Sim) -> Vec<String> {
    traced(sim, Traced::Mark)
}

fn marks_on_a_ride(sim: &mut Sim) {
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    let first = sim.screen().page().expect("page dots");
    assert!(first.starts_with("1/"), "{first}");
    sim.hold(Button::BottomLeft, 1000);
    let second = sim.screen().page().expect("page dots");
    assert!(second.starts_with("2/"), "one page on, not three: {second}");
    sim.advance(seconds(3));
    assert!(marks(sim).is_empty(), "a hold of a second marks nothing");
    sim.expect_no_text("MARK");

    let page = sim.seen().page.expect("a page").1;
    let pressed = sim.device.clock_ms();
    let from = sim.device.sim().speaker.played.len();
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(
        sim.screen().page(),
        Some(second.clone()),
        "the page put back"
    );
    assert_eq!(sim.seen().page.expect("a page").1, page);
    assert_eq!(
        marks(sim),
        [format!("1 pressed {} on {page}", session_clock(pressed))]
    );
    sim.checkpoint("marked");
    sim.advance(seconds(3));
    assert_eq!(played_since(sim, from), [0, 0, 0], "middle, middle, middle");
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 2");
    assert_eq!(sim.screen().page(), Some(second), "the page put back");
    assert_eq!(marks(sim).len(), 2);
    sim.advance(seconds(30));
    sim.stop_ride();
}

#[test]
fn holding_move_marks_the_moment_and_leaves_the_ride_as_it_was() {
    let mut sim = rider();
    marks_on_a_ride(&mut sim);
    let toasts = traced(&sim, Traced::Toast);
    assert!(
        !toasts.iter().any(|toast| toast.starts_with("LAP")),
        "a mark takes no lap: {toasts:?}"
    );
    let states = traced(&sim, Traced::Ride);
    let paused = states.iter().filter(|state| *state == "paused").count();
    assert_eq!(paused, 1, "only the pause that ends the ride: {states:?}");
}

#[test]
fn holding_move_on_a_ride_page_steps_one_page_however_long_it_is_held() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    let first = sim.screen().page().expect("page dots");
    assert!(first.starts_with("1/"), "{first}");
    sim.hold(Button::BottomLeft, 1400);
    let held = sim.screen().page().expect("page dots");
    assert!(held.starts_with("2/"), "one page on, not four: {held}");
    sim.checkpoint("held-move-one-page");
    sim.advance(seconds(1));
    sim.press(Button::BottomLeft);
    let next = sim.screen().page().expect("page dots");
    assert!(next.starts_with("3/"), "a new press still pages: {next}");
}

#[test]
fn holding_move_on_home_marks_and_keeps_the_focus() {
    let mut sim = rider();
    sim.advance(seconds(3));
    assert!(sim.screen().white("RIDE"), "{}", sim.screen().text());
    let pressed = sim.device.clock_ms();
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    assert!(sim.screen().white("RIDE"), "{}", sim.screen().text());
    assert_eq!(
        marks(&sim),
        [format!("1 pressed {} on home", session_clock(pressed))]
    );
}

/// A pause the rider paged away from, then a resume: the paused screen
/// stays hidden in the view until the next pause brings it back.
fn paused_and_paged_away(sim: &mut Sim) {
    sim.pause_ride();
    sim.show_ride_pages();
    sim.resume_ride();
    sim.advance(seconds(5));
}

#[test]
fn pausing_during_a_mark_keeps_the_paused_screen() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    paused_and_paged_away(&mut sim);
    sim.hold_while(Button::BottomLeft, 1600, |sim, held| {
        if held == 500 {
            sim.report(Button::BottomRight);
        }
    });
    sim.expect_text("MARK 1");
    assert_eq!(sim.ride_state(), "paused");
    assert_eq!(sim.seen().page.expect("a page").1, "ride/paused");
    assert_eq!(marks(&sim).len(), 1);
}

/// How long after `setup` its ride first meets `reached`, stepping a tenth
/// of a second.
fn ms_until(setup: fn() -> Sim, reached: fn(&mut Sim) -> bool) -> u64 {
    let mut sim = setup();
    let mut waited = 0;
    while !reached(&mut sim) {
        assert!(waited < seconds(120), "never reached");
        sim.advance(100);
        waited += 100;
    }
    waited
}

fn stopping_at_a_light() -> Sim {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    paused_and_paged_away(&mut sim);
    sim.advance(seconds(30));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim
}

#[test]
fn an_auto_pause_during_a_mark_keeps_the_paused_screen() {
    let auto_paused = |sim: &mut Sim| {
        sim.seen()
            .ride
            .is_some_and(|(_, state)| state == "auto_paused")
    };
    let pauses = ms_until(stopping_at_a_light, auto_paused);
    let mut sim = stopping_at_a_light();
    sim.advance(pauses - 700);
    assert!(!auto_paused(&mut sim));
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    assert!(auto_paused(&mut sim));
    assert_eq!(sim.seen().page.expect("a page").1, "ride/paused");
}

fn climbing() -> Sim {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1200.0);
    sim
}

#[test]
fn the_climb_page_coming_up_during_a_mark_stays() {
    let on_climb = |sim: &mut Sim| {
        sim.screen()
            .page()
            .is_some_and(|dots| dots.starts_with("4/"))
    };
    let shown = ms_until(climbing, on_climb);
    let mut sim = climbing();
    sim.advance(shown - 700);
    assert!(!on_climb(&mut sim), "{:?}", sim.screen().page());
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    let pages = traced(&sim, Traced::Page);
    let climb = pages.iter().position(|page| page == "ride/climb");
    assert_eq!(
        &pages[climb.expect("the climb page came up")..],
        ["ride/climb"],
        "nothing took the climb page away: {pages:?}"
    );
    ride_to(&mut sim, 4500.0);
    assert_eq!(
        sim.seen().page.expect("a page").1,
        "ride/main",
        "past the climb, the page from before the hold"
    );
}

fn seen_page(sim: &Sim) -> String {
    sim.seen().page.expect("a page").1
}

/// On the last ride page, the page after the climb page, when the climb
/// page comes up by itself.
fn on_the_climb_page() -> Sim {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    sim.advance(seconds(5));
    while sim
        .screen()
        .page()
        .and_then(|dots| dots.split_once('/').map(|(on, of)| on != of))
        .expect("page dots")
    {
        sim.press(Button::BottomLeft);
    }
    assert_eq!(seen_page(&sim), "ride/map");
    while seen_page(&sim) != "ride/climb" {
        assert!(sim.world().distance_m < 2500.0, "no climb page");
        sim.advance(100);
    }
    sim
}

#[test]
fn a_mark_on_the_climb_page_that_came_up_goes_back_after_the_top() {
    let mut sim = on_the_climb_page();
    sim.advance(seconds(5));
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(seen_page(&sim), "ride/climb", "the page put back");
    ride_to(&mut sim, 4500.0);
    assert_eq!(
        seen_page(&sim),
        "ride/map",
        "past the climb, the page from before it came up"
    );
}

fn near_the_top() -> Sim {
    let mut sim = on_the_climb_page();
    ride_to(&mut sim, 2450.0);
    sim
}

#[test]
fn a_climb_that_ends_during_a_mark_leaves_its_page() {
    let over = ms_until(near_the_top, |sim| seen_page(sim) != "ride/climb");
    let mut sim = near_the_top();
    sim.advance(over - 700);
    assert_eq!(seen_page(&sim), "ride/climb");
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_text("MARK 1");
    sim.advance(seconds(10));
    assert_eq!(
        seen_page(&sim),
        "ride/map",
        "the rider's page, not the page of a climb that is over"
    );
}

#[test]
fn input_lost_during_a_hold_marks_nothing() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    sim.hold_while(Button::BottomLeft, 1600, |sim, held| {
        if held == 500 {
            let at = sim.device.clock_ms();
            sim.device.input.push(at, device_api::input::Input::Cancel);
        }
    });
    sim.advance(seconds(1));
    sim.expect_no_text("MARK");
    assert!(marks(&sim).is_empty(), "{:?}", marks(&sim));
}

#[test]
fn a_move_that_closes_the_power_sheet_marks_nothing_however_long_it_is_held() {
    let mut sim = rider();
    sim.advance(seconds(3));
    sim.press(Button::BottomLeft);
    assert!(!sim.screen().white("RIDE"), "{}", sim.screen().text());
    sim.advance(seconds(5));
    sim.press(Button::TopLeft);
    sim.expect_text("TURN OFF");
    sim.hold(Button::BottomLeft, 1600);
    sim.expect_no_text("TURN OFF");
    sim.advance(seconds(1));
    sim.expect_no_text("MARK");
    assert!(marks(&sim).is_empty(), "{:?}", marks(&sim));
    assert!(
        !sim.screen().white("RIDE"),
        "the focus stays where the tap left it: {}",
        sim.screen().text()
    );
}

#[test]
fn a_ride_with_marks_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), marks_on_a_ride, false);
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
    let from = sim.device.sim_mut().speaker.played.len();
    sim.start_ride();
    // 1 km at 30 km/h is two minutes.
    sim.advance(minutes(2) + seconds(20));
    ride_page(&mut sim, "3/5");
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
    sim.set_option("BRIGHTNESS", "25%");
    sim.advance(seconds(5));
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

/// A lap pressed a moment before the pause that ends the ride starts a lap
/// no later sample would carry; the saved ride and its FIT file keep it.
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
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    let live = sim.screen().field("LAPS").unwrap();
    sim.checkpoint("the summary counts the last lap");
    sim.done();

    let ride = sim.rides().pop().unwrap();
    let marks: Vec<_> = ride.samples().filter_map(|s| s.lap).collect();
    let fit = fit_lap_triggers(&ride.fit(Some(1_789_000_000_000)).unwrap());
    sim.home("RIDES");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("BEST POWER W");
    let saved = sim.screen().field("LAPS").unwrap();
    sim.checkpoint("the saved ride keeps the last lap");
    sim.note(
        NoteKind::Check,
        format!(
            "laps: summary {live}, saved ride {saved}, FIT {}",
            fit.len()
        ),
    );
    assert_eq!(
        (live.as_str(), saved.as_str(), fit.as_slice()),
        ("2", "2", [0, 7].as_slice()),
        "summary, saved ride, FIT lap triggers (manual, session end)"
    );
    assert_eq!(
        marks,
        [LapStart::Ride, LapStart::Manual],
        "journal lap marks"
    );
}

/// A lap pressed a moment before AUTO LAP AT START closes one at the start:
/// two laps within a second, and the saved ride and its FIT file keep both.
#[test]
fn a_lap_pressed_just_before_the_start_keeps_both_laps() {
    let pass_ms = {
        let mut sim = at_start_rider(0.0);
        sim.wait_for("LAP 1 04:", minutes(5)).unwrap();
        sim.now()
    };
    let mut sim = at_start_rider(0.0);
    sim.advance(pass_ms - 300 - sim.now());
    sim.report(Button::TopLeft);
    sim.advance(10);
    sim.expect_text("LAP 1 ");
    let second = sim.wait_for("LAP 2 ", seconds(1)).unwrap();
    sim.checkpoint("a lap pressed, then the start closes one");
    sim.advance(seconds(30));
    sim.pause_ride();
    sim.stop_ride();
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    let live = sim.screen().field("LAPS").unwrap();
    sim.done();

    let ride = sim.rides().pop().unwrap();
    let marks: Vec<_> = ride
        .samples()
        .enumerate()
        .filter_map(|(index, s)| s.lap.map(|lap| (index, lap)))
        .collect();
    let fit = fit_lap_triggers(&ride.fit(Some(1_789_000_000_000)).unwrap());
    sim.home("RIDES");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.swipe_next();
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("BEST POWER W");
    let saved = sim.screen().field("LAPS").unwrap();
    sim.checkpoint("the saved ride keeps both laps");
    sim.note(
        NoteKind::Check,
        format!(
            "second lap toast {second} ms after the first; laps: summary {live}, saved ride {saved}, \
             FIT {fit:?}, journal {marks:?}"
        ),
    );
    assert_eq!(
        (live.as_str(), saved.as_str(), fit.as_slice()),
        ("3", "3", [0, 3, 7].as_slice()),
        "summary, saved ride, FIT lap triggers (manual, position start, session end)"
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
fn riding_through_sunset_turns_the_day_night_decision_and_leaves_the_backlight_alone() {
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
    sim.advance(minutes(40));
    assert_eq!(
        sim.device.shell.effective(),
        50,
        "no light reading, no change"
    );
    assert!(recording(&mut sim));
    sim.checkpoint("riding after dark");
    sim.stop_ride();
    go_home(&mut sim);

    sim.home("SETTINGS");
    show_setting(&mut sim, "DAY / NIGHT");
    sim.expect_field("DAY / NIGHT", "AUTO");
    sim.expect_text("NIGHT NOW");
    sim.checkpoint("settings in auto, at night");
    sim.set_option("DAY / NIGHT", "DAY");
    sim.expect_text("DAY");
    assert_eq!(sim.device.shell.effective(), 50);
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
    sim.home("SETTINGS");
    show_setting(&mut sim, "DAY / NIGHT");
    sim.expect_text("DAY NOW");
    go_home(&mut sim);
    // A ride keeps the screen awake.
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.set_gps_fix(false);
    sim.advance(minutes(38));
    sim.stop_ride();
    go_home(&mut sim);
    sim.home("SETTINGS");
    show_setting(&mut sim, "DAY / NIGHT");
    sim.expect_text("NIGHT NOW");
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
    let gps = sim.device.sim().position.snapshot(sim.now()).unwrap().gps;
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

/// Numbers never glide: a rider glancing down mid-glide could not read one.
/// A step in heart rate shows as the new reading on the first frame that
/// changes the number, while the heart-rate gauge under it glides through
/// columns in between and lands on the reading.
#[test]
fn numbers_jump_to_each_reading_while_the_gauges_glide() {
    // The heart-rate gauge's rows on the main page.
    const HEART_GAUGE: std::ops::Range<usize> = 237..242;
    let lit = |screen: &Screen| {
        HEART_GAUGE
            .flat_map(|y| screen.pixels[y * screen.width..(y + 1) * screen.width].iter())
            .filter(|pixel| ZONES.contains(pixel))
            .count()
    };
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 200.0, 130.0, 88.0);
    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("BPM", "130");
    let before = lit(&sim.screen());
    sim.ride_at(28.0, 200.0, 170.0, 88.0);
    let mut numbers = vec![String::from("130")];
    let mut gauge = vec![before];
    let mut captured = false;
    for _ in 0..100 {
        sim.advance(10);
        let screen = sim.screen();
        let bpm = screen.field("BPM").unwrap();
        if numbers.last() != Some(&bpm) {
            numbers.push(bpm);
        }
        let lit = lit(&screen);
        if gauge.last() != Some(&lit) {
            gauge.push(lit);
        }
        if !captured && gauge.len() == 3 {
            sim.checkpoint("the gauge gliding under the new number");
            captured = true;
        }
    }
    assert_eq!(numbers, ["130", "170"], "the number jumps");
    assert!(
        gauge.windows(2).all(|w| w[0] < w[1]) && gauge.len() >= 4,
        "the gauge glides: {gauge:?}"
    );
    sim.checkpoint("the gauge settled on the reading");
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
    // The list follows the finger: dragged up and held still, the header
    // folds into the pinned RIDES band and later rides come up.
    sim.drag((120, 280), (120, 80), 400, 200);
    sim.advance(seconds(1));
    sim.expect_text("RIDES");
    let dragged = sim.screen().text();
    assert!(!dragged.contains("THIS WEEK"), "{dragged}");
    sim.checkpoint("dragged up the list");
    // A flick coasts on well past where the finger stopped, then rests with
    // the focus on a card in view.
    sim.drag((120, 280), (120, 180), 50, 0);
    sim.advance(seconds(3));
    let coasted = sim.screen().text();
    assert_ne!(coasted, dragged);
    sim.checkpoint("flicked and coasted");
    // Pulled back down hard, it lands on the top: the header and 1/44.
    for _ in 0..4 {
        sim.drag((120, 60), (120, 300), 60, 0);
    }
    sim.advance(seconds(3));
    sim.expect_text("THIS WEEK");
    sim.checkpoint("flicked back to the top");
    // Held MOVE pages through the list; MOVE then steps to the oldest.
    sim.hold(Button::BottomLeft, 3000);
    let shown = sim.screen().text();
    assert!(
        !shown.lines().any(|line| line == "1/44" || line == "2/44"),
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
        push(&Entry::event(Kind::Start, id, 0, 0));
        let mut sequence = 1;
        for batch in samples.chunks(4) {
            push(&Entry::batch(id, sequence, batch).unwrap());
            sequence += 1;
        }
        let active_ms = minutes * 60_000;
        push(&Entry::event(Kind::Finish, id, sequence, active_ms));
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

/// The text of a run on screen that starts with `prefix`, such as "TSS 40".
fn shown_with(sim: &Sim, prefix: &str) -> String {
    let screen = sim.screen();
    screen
        .text()
        .lines()
        .flat_map(|line| line.split("  "))
        .map(str::trim)
        .find(|run| run.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix}; screen:\n{}", screen.text()))
        .to_string()
}

/// A saved ride keeps the FTP it was ridden at, its heart-rate recovery and
/// its climbs on FILES. Raising the FTP afterwards leaves the week's training
/// stress as it was, and after a reboot the ride read back shows the recovery
/// and the climbs.
#[test]
fn a_saved_ride_keeps_its_ftp_heart_rate_recovery_and_climbs() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    sim.init_volumes();
    sim.put_file("notes.txt", b"x");
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ride_route(Route::profile(
        Point::new(-33.45, -70.66),
        &[
            (0.0, 600.0),
            (1000.0, 600.0),
            (2500.0, 690.0),
            (4000.0, 600.0),
            (12000.0, 600.0),
        ],
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim.edit_profile(250, 185);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);
    sim.start_ride();
    sim.ride_at(25.0, 250.0, 170.0, 90.0);
    sim.advance(minutes(24));
    sim.stop_ride();
    sim.press(Button::BottomLeft);
    sim.ride_at(0.0, 0.0, 142.0, 0.0);
    sim.advance(seconds(75));
    sim.expect_text("-28 BPM");
    // The screen dimmed during the minute; the first press only wakes it.
    sim.press(Button::BottomRight);
    sim.done();

    let record = b"bi"
        .iter()
        .find_map(|&journal| facts_of(&mut sim, journal, 1))
        .expect("the saved ride's facts in the table");
    println!("{record:?}");
    let facts = record.facts;
    assert_eq!(facts.profile.map(|p| p.ftp_watts), Some(250));
    assert_eq!(facts.recovery_bpm, Some(28));
    let climbing = facts.climbing.expect("climbs kept");
    assert_eq!(climbing.count, 1, "{climbing:?}");
    assert!((80..=95).contains(&climbing.gain_m), "{climbing:?}");

    sim.power_loss();
    sim.set_gps_clock(16 * 3600, (21, 10, 26));
    sim.advance(seconds(20));
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("THIS WEEK");
    // 24 minutes at FTP is 40.
    let ridden = shown_with(&sim, "TSS");
    assert_eq!(ridden, "TSS 40");
    sim.checkpoint("week stress at the FTP the ride was ridden at");
    sim.press(Button::TopLeft);

    sim.edit_profile(300, 185);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);
    sim.home("RIDES");
    sim.advance(seconds(1));
    assert_eq!(
        shown_with(&sim, "TSS"),
        ridden,
        "a new FTP restates nothing"
    );
    sim.checkpoint("week stress unchanged after raising FTP to 300");

    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("SAVED RIDE");
    sim.press(Button::BottomLeft);
    sim.expect_text("HR RECOVERY");
    sim.expect_text("-28 BPM");
    sim.checkpoint("saved ride keeps its heart rate recovery");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    sim.expect_field("CLIMBS", "1");
    sim.checkpoint("saved ride keeps its climbs");
}

/// Without a card the ride saves as before: the week's stress follows the
/// FTP now, and the saved ride shows no recovery and no climbs.
#[test]
fn without_files_a_saved_ride_reads_as_it_did_before() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim.edit_profile(250, 185);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);
    sim.start_ride();
    sim.ride_at(25.0, 250.0, 170.0, 90.0);
    sim.advance(minutes(24));
    sim.stop_ride();
    sim.ride_at(0.0, 0.0, 142.0, 0.0);
    sim.advance(seconds(75));
    sim.press(Button::BottomRight);
    sim.done();
    sim.edit_profile(300, 185);
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);
    sim.home("RIDES");
    sim.advance(seconds(5));
    // 40 at 250 W is 40 x (250/300)^2 at 300 W.
    assert_eq!(shown_with(&sim, "TSS"), "TSS 28");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.press(Button::BottomLeft);
    sim.expect_no_text("HR RECOVERY");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("BEST POWER W");
    sim.expect_field("CLIMBS", "--");
}

/// A card with a formatted FILES store, the sensors paired and the clock set.
fn files_rider() -> Sim {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.init_volumes();
    sim.put_file("notes.txt", b"x");
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim
}

/// The facts the table on FILES keeps for ride `ride_id` of `journal`.
fn facts_of(sim: &mut Sim, journal: u8, ride_id: u32) -> Option<vana::kept::facts::record::Record> {
    let bytes = sim.file(vana::kept::facts::record::TABLE)?;
    vana::kept::facts::record::decode_table(&bytes)
        .find(|record| (record.journal, record.ride_id) == (journal, ride_id))
}

fn fill_files(sim: &mut Sim, free: usize) {
    use firmware_services::{files, volume};
    let mut sector = [0u8; 512];
    let mut media = sim.device.host_bulk();
    let mut store = volume::open(&mut media, &volume::FILES, &mut sector).unwrap();
    let used = files::mount(&mut store, &mut sector).unwrap().len();
    for n in used..files::CAPACITY - free {
        let name = files::Name::new(&format!("fill/{n:03}.txt")).unwrap();
        files::write(&mut store, &name, b"x", &mut sector).unwrap();
    }
    sim.note(
        NoteKind::Device,
        format!("host filled FILES to {free} free entries"),
    );
}

fn app_files(sim: &mut Sim) -> Vec<String> {
    let mut names = sim.file_names().expect("FILES lists");
    names.retain(|name| !name.starts_with("fill/"));
    names
}

#[test]
fn with_files_nearly_full_facts_give_way_so_a_discarded_ride_still_hides() {
    let mut sim = files_rider();
    fill_files(&mut sim, 2);
    sim.ride_at(25.0, 200.0, 165.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(3));
    sim.stop_ride();
    sim.expect_text("2 NEW BESTS");
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.advance(seconds(75));
    sim.checkpoint("ride saved with FILES nearly full");
    assert_eq!(app_files(&mut sim), ["notes.txt", "bests.dat"]);
    assert!(facts_of(&mut sim, b'b', 1).is_none(), "the facts gave way");
    sim.press(Button::BottomRight);
    sim.done();

    sim.ride_at(30.0, 220.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.discard_ride();
    sim.checkpoint("the second ride discarded");
    assert_eq!(
        app_files(&mut sim),
        ["notes.txt", "bests.dat", vana::kept::deleted::TABLE]
    );

    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim.advance(seconds(61));
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1 RIDE");
    sim.expect_text("1/1");
    sim.checkpoint("only the saved ride is listed");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.expect_text("SAVED RIDE");
    sim.press(Button::BottomLeft);
    sim.expect_no_text("HR RECOVERY");
    sim.checkpoint("the saved ride has no recovery: its facts gave way");
}

/// A ride the host ends saves before the summary starts measuring; the
/// recovery still reaches its facts a minute later.
#[test]
fn a_ride_the_host_ends_keeps_its_heart_rate_recovery() {
    let mut sim = files_rider();
    sim.start_ride();
    sim.ride_at(25.0, 200.0, 165.0, 90.0);
    sim.advance(minutes(5));
    ride_command(&mut sim, "RIDE BULK PAUSE");
    ride_command(&mut sim, "RIDE BULK FINISH");
    sim.wait_for("RIDE SAVED", seconds(5)).unwrap();
    let saved = facts_of(&mut sim, b'b', 1).expect("facts written on save");
    assert_eq!(saved.facts.recovery_bpm, None, "still measuring");
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.advance(seconds(75));
    let measured = facts_of(&mut sim, b'b', 1).unwrap();
    assert_eq!(measured.facts.recovery_bpm, Some(25));
}

/// Clearing a journal deletes its facts with its rides, so the ride that
/// takes id 1 again reads only its own.
#[test]
fn a_cleared_journal_forgets_its_facts_and_the_next_ride_one_keeps_its_own() {
    let mut sim = files_rider();
    sim.start_ride();
    sim.ride_at(25.0, 200.0, 165.0, 90.0);
    sim.advance(minutes(3));
    sim.stop_ride();
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.advance(seconds(75));
    assert_eq!(
        facts_of(&mut sim, b'b', 1).and_then(|r| r.facts.recovery_bpm),
        Some(25)
    );
    sim.press(Button::BottomRight);
    sim.done();

    let (_, status) = sim.command("RIDE BULK STATUS");
    let upper = status_field(&status, "slot");
    let (accepted, out) = sim.command(&format!("RIDE BULK CLEAR CONFIRM {upper}"));
    assert_eq!(accepted, "ACCEPTED", "{out}");
    sim.advance(seconds(10));
    assert!(
        facts_of(&mut sim, b'b', 1).is_none(),
        "forgotten with its ride"
    );

    sim.start_ride();
    sim.ride_at(25.0, 200.0, 0.0, 90.0);
    sim.advance(minutes(3));
    sim.stop_ride();
    sim.done();
    let again = facts_of(&mut sim, b'b', 1).expect("the new ride 1");
    assert_eq!(again.facts.recovery_bpm, None);
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1/1");
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.press(Button::BottomLeft);
    sim.expect_no_text("HR RECOVERY");
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
        .sim_mut()
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
    assert!(sim.device.sim_mut().phone_server.listening);
    sim.checkpoint("connect phone: join wi-fi");

    sim.device.sim_mut().access_point.clients = 1;
    sim.advance(seconds(1));
    sim.expect_text("OPEN YOUR RIDES");
    let page = scanned(&sim);
    assert!(page.starts_with("http://192.168.4.1/?t="), "{page}");
    sim.checkpoint("connect phone: open rides");

    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    assert_eq!(sim.vana_page(), "home");
    assert!(
        sim.device.sim_mut().access_point.config.is_none(),
        "access point stopped"
    );
    assert!(!sim.device.sim_mut().phone_server.listening);
}

/// A course loaded from the phone page is kept on the memory card beside
/// the files already there, read back after a restart, and cleared from the
/// page.
#[test]
fn a_course_loaded_from_the_phone_is_kept_across_a_restart() {
    use device_api::server::Method;
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.put_file("workouts/keep.vw", b"kept as it was");
    sim.connect_phone();
    let gpx = bend_loop_gpx();
    let (status, body) = sim.phone_request(Method::Put, "/api/course", gpx.as_bytes());
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"BEND LOOP""#), "{body}");
    let stored = sim.file("course.dat").expect("course.dat on FILES");
    assert_eq!(&stored[..5], b"VCRS\x02");
    let turns = sim.file("course-turns.dat").expect("its turns beside it");
    assert_eq!(&turns[..5], b"VTRN\x02");
    assert_eq!(
        &turns[8..12],
        &stored[stored.len() - 4..],
        "named by the course's CRC"
    );
    assert_eq!(
        sim.file("workouts/keep.vw").as_deref(),
        Some(&b"kept as it was"[..])
    );

    sim.power_loss();
    sim.connect_phone();
    let (_, body) = sim.phone_request(Method::Get, "/api/course", b"");
    assert!(
        body.contains(r#""loaded":true"#) && body.contains("BEND LOOP"),
        "{body}"
    );
    let (status, _) = sim.phone_request(Method::Delete, "/api/course", b"");
    assert_eq!(status, 204);
    assert!(sim.file("course.dat").is_none());
    assert!(sim.file("course-turns.dat").is_none());
    let (_, body) = sim.phone_request(Method::Get, "/api/course", b"");
    assert!(body.contains(r#""loaded":false"#), "{body}");
}

/// A workout added from the phone page is stored beside the files already
/// on the card, found in the TRAIN picker, ridden, and deleted from the page.
/// A second phone upload of the same name replaces it; a damaged one changes
/// nothing.
#[test]
fn a_workout_added_from_the_phone_is_found_in_the_picker_and_ridden() {
    use device_api::server::Method;
    use vana::workout::codec::encode;
    use vana::workout::model::{Block, Intensity, Length, Step, Target, Workout};
    const STEPS: &[Step] = &[
        Step::new(
            Intensity::Warmup,
            Length::Time(600),
            Target::Ftp { low: 50, high: 65 },
        ),
        Step::new(
            Intensity::Work,
            Length::Time(600),
            Target::Ftp { low: 88, high: 94 },
        ),
        Step::new(
            Intensity::Recovery,
            Length::Time(300),
            Target::Ftp { low: 45, high: 55 },
        ),
        Step::new(
            Intensity::Cooldown,
            Length::Time(600),
            Target::Ftp { low: 40, high: 55 },
        ),
    ];
    const BLOCKS: &[Block] = &[Block {
        first: 1,
        len: 2,
        count: 3,
    }];
    let bytes = encode(&Workout::new("FROM THE PHONE", STEPS, BLOCKS, None));
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.put_file("workouts/keep.vw", b"not a workout, left as it was");
    sim.connect_phone();
    let (status, body) = sim.phone_request(Method::Get, "/api/workouts", b"");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""uploads":[]"#) && body.contains(r#""skipped":1"#),
        "{body}"
    );

    // A damaged upload changes nothing.
    let mut damaged = bytes.to_vec();
    damaged[9] ^= 1;
    let (status, body) = sim.phone_request(Method::Put, "/api/workouts", &damaged);
    assert_eq!(status, 422, "{body}");
    let (status, body) = sim.phone_request(Method::Put, "/api/workouts", &bytes);
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""name":"FROM THE PHONE","minutes":65,"steps":8"#),
        "{body}"
    );
    let stored = sim
        .file(&format!(
            "workouts/from-the-phone-{:08x}.vw",
            firmware_services::checksum::crc32(b"FROM THE PHONE")
        ))
        .expect("the workout file on FILES");
    assert_eq!(stored, &bytes[..]);
    assert_eq!(
        sim.file("workouts/keep.vw").as_deref(),
        Some(&b"not a workout, left as it was"[..])
    );
    go_home(&mut sim);
    sim.home("TRAIN");
    for _ in 0..17 {
        sim.press(Button::BottomLeft);
    }
    sim.expect_text("FROM THE PHONE");
    sim.expect_text("8 STEPS");
    sim.checkpoint("workout from the phone in the picker");
    go_home(&mut sim);

    // The picker lists it; choosing it and starting a ride plays it.
    sim.choose_workout("FROM THE PHONE");
    sim.checkpoint("phone workout chosen");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    assert_eq!(sim.workout()["workout"], "FROM_THE_PHONE");
    assert_eq!(sim.workout()["step"], "1/8");
    sim.checkpoint("phone workout riding");
    sim.stop_ride();
    sim.done();

    // Back on the page, the workout is listed and deleted.
    sim.connect_phone();
    let (_, body) = sim.phone_request(Method::Get, "/api/workouts", b"");
    let file = body
        .split(r#""file":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no workout listed: {body}"))
        .to_string();
    let (status, _) = sim.phone_request(Method::Delete, &format!("/api/workouts?file={file}"), b"");
    assert_eq!(status, 204);
    assert_eq!(sim.file(&format!("workouts/{file}")), None);
    assert!(sim.file("workouts/keep.vw").is_some());
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
    gpx_spaced(name, legs, 10.0)
}

fn gpx_spaced(name: &str, legs: &[(f64, f64)], spacing: f64) -> String {
    let start = Point::new(-33.45, -70.66);
    let metres = |(east, north): (f64, f64)| {
        let lat = start.lat + north / 111_195.0;
        let lon = start.lon + east / (111_195.0 * start.lat.to_radians().cos());
        (Point::new(lat, lon), None)
    };
    let mut points = vec![metres(legs[0])];
    for pair in legs.windows(2) {
        let length = ((pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1) / spacing).ceil();
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

#[test]
fn map_page_draws_the_ride_track_heading_up_and_keeps_it_when_still() {
    let mut sim = rider();
    sim.ride_route(Route::from_gpx(&bend_loop_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.ride_page("5/5");
    expect_map(&sim);
    ride_to(&mut sim, 60.0);
    sim.expect_text("100 M");
    sim.checkpoint("the map early in the ride");

    // Round the bend: the track is rotated so the road ahead is up.
    ride_to(&mut sim, 1_900.0);
    assert!(
        ridden_track_pixels(&sim) > 100,
        "the track behind the rider"
    );
    sim.checkpoint("the map mid ride, heading up");

    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(20)).unwrap();
    sim.press(Button::BottomLeft);
    let moving = ridden_track_pixels(&sim);
    sim.advance(seconds(20));
    expect_map(&sim);
    sim.expect_no_text("N UP");
    assert_eq!(ridden_track_pixels(&sim), moving, "the map stays as it was");
    sim.checkpoint("the map standing still, heading held");
    sim.stop_ride();
}

fn ridden_track_pixels(sim: &Sim) -> usize {
    map_pixels(sim, vana::screens::chrome::rgb(0x1f, 0x4a, 0x78))
}

#[test]
fn a_restart_keeps_the_track_and_the_start_for_the_next_ride() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_route(Route::loop_around(Point::new(-33.45, -70.66), 3_000.0, 0.0));
    sim.home("SETTINGS");
    show_setting(&mut sim, "AUTO LAP");
    sim.set_option("AUTO LAP", "AT START");
    sim.swipe_next();
    go_home(&mut sim);
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.start_ride();
    ride_to(&mut sim, 4_500.0);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("5/5");
    let before = ridden_track_pixels(&sim);
    sim.checkpoint("the map before the power cut");

    sim.power_loss();
    sim.home("RIDE");
    sim.expect_text("LAST RIDE RECOVERED");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.advance(seconds(20));
    sim.ride_page("5/5");
    let after = ridden_track_pixels(&sim);
    sim.note(
        NoteKind::Check,
        format!("map ink {before} px before, {after} px after"),
    );
    assert!(after * 10 >= before * 9, "{after} px against {before} px");
    sim.checkpoint("the map after the restart, the whole track");

    // On round the loop: the ride's start is where the lap closes.
    sim.ride_page("3/5");
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    let lap = ride_until_shown(&mut sim, "LAP 1 0", 6_300.0);
    assert!((6_000.0..6_200.0).contains(&lap), "{lap}");
    sim.checkpoint("lap toast at the ride's start after the restart");
    sim.stop_ride();
    let rides = sim.rides();
    assert_eq!(rides.len(), 2);
    let laps = rides[1]
        .samples()
        .filter(|s| s.lap == Some(vana::ride::log::LapStart::Position))
        .count();
    assert_eq!(laps, 1);
}

#[test]
fn a_five_hour_track_on_the_card_reads_back_after_a_restart() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 0.0, 0.0, 85.0);
    for _ in 0..5 {
        sim.advance(minutes(60));
    }
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("5/5");
    let before = ridden_track_pixels(&sim);
    sim.checkpoint("the map after five hours");

    sim.power_loss();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert!(status.contains("state=recovered"), "{status}");
    sim.home("RIDE");
    sim.press(Button::BottomRight);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.ride_page("5/5");
    let mut waited = 0;
    while sim.screen().contains("NO GPS") {
        assert!(waited < 120, "no track after {waited} s");
        sim.advance(seconds(1));
        waited += 1;
    }
    let after = ridden_track_pixels(&sim);
    sim.note(
        NoteKind::Check,
        format!("track read back in {waited} s: map ink {before} px before, {after} px after"),
    );
    assert!(after * 10 >= before * 9, "{after} px against {before} px");
    sim.checkpoint("the map after the restart, five hours read back");
}

#[test]
fn a_ride_that_moves_to_internal_memory_keeps_the_track_and_the_start() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_route(Route::loop_around(Point::new(-33.45, -70.66), 3_000.0, 0.0));
    sim.home("SETTINGS");
    show_setting(&mut sim, "AUTO LAP");
    sim.set_option("AUTO LAP", "AT START");
    sim.swipe_next();
    go_home(&mut sim);
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.start_ride();
    ride_to(&mut sim, 4_500.0);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("5/5");
    let before = ridden_track_pixels(&sim);
    sim.checkpoint("the map on the card before it fails");

    // Riding on, the next write to the card fails.
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.advance(seconds(10));
    sim.fail_bulk(0, 0);
    sim.wait_for("RECORDING INT", seconds(15)).unwrap();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "moved"), "1", "{status}");
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("5/5");
    let after = ridden_track_pixels(&sim);
    sim.note(
        NoteKind::Check,
        format!("map ink {before} px before the move, {after} px after"),
    );
    sim.checkpoint("the map after the move, the whole track");
    assert!(after * 10 >= before * 9, "{after} px against {before} px");

    // On round the loop: the ride's start is where the lap closes.
    sim.ride_page("3/5");
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    let lap = ride_until_shown(&mut sim, "LAP 2 0", 6_300.0);
    assert!((6_000.0..6_200.0).contains(&lap), "{lap}");
    sim.checkpoint("lap toast at the ride's start after the move");
    sim.advance(seconds(5));
    sim.stop_ride();
    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    let laps = rides[0]
        .samples()
        .filter(|s| s.lap == Some(vana::ride::log::LapStart::Position))
        .count();
    assert_eq!(laps, 1);
}

/// The legs of the course the rider loads: the bend loop once round, back
/// to its start.
const COURSE_LOOP: [(f64, f64); 8] = [
    (0.0, 0.0),
    (0.0, 900.0),
    (500.0, 1300.0),
    (1300.0, 1250.0),
    (1700.0, 600.0),
    (1300.0, -100.0),
    (600.0, -250.0),
    (0.0, 0.0),
];

/// The ride: the course, with a wrong turn 400 m along its third leg, 175 m
/// north and back, and on past the end for a little.
fn course_ride_gpx() -> String {
    let mut legs = COURSE_LOOP[..3].to_vec();
    legs.extend([(900.0, 1275.0), (905.0, 1450.0), (920.0, 1274.0)]);
    legs.extend_from_slice(&COURSE_LOOP[3..]);
    legs.push((0.0, 300.0));
    gpx_through("loop with a wrong turn", &legs)
}

fn map_pixels(sim: &Sim, color: u16) -> usize {
    let screen = sim.screen();
    (22..290)
        .flat_map(|y| screen.pixels[y * screen.width..][..screen.width].iter())
        .filter(|&&pixel| pixel == color)
        .count()
}

fn course_pixels(sim: &Sim) -> usize {
    map_pixels(sim, vana::screens::chrome::ACCENT)
}

#[track_caller]
fn expect_map(sim: &Sim) {
    assert_eq!(sim.screen().page().as_deref(), Some("5/5"));
    let ground = map_pixels(sim, vana::screens::chrome::rgb(0x10, 0x10, 0x12));
    assert!(ground > 30_000, "{ground} px of the map's ground");
}

fn map_badge(sim: &Sim) -> Vec<String> {
    sim.screen()
        .frame()
        .read()
        .into_iter()
        .filter(|run| (30..90).contains(&run.y) && run.x < 160)
        .map(|run| run.text)
        .collect()
}

const TURN_SOON: [u8; 2] = [21, 0];
const TURN_NOW: [u8; 1] = [10];
const OFF_COURSE: [u8; 2] = [10, 21];
const ON_COURSE: [u8; 2] = [21, 10];
const ARRIVED: [u8; 2] = [0, 10];
const ARRIVAL_CUE: &str = "arrived";

fn cues_since(sim: &Sim, from: usize) -> Vec<(String, Vec<u8>)> {
    timed_cues_since(sim, from)
        .into_iter()
        .map(|(_, cue, notes)| (cue, notes))
        .collect()
}

fn timed_cues_since(sim: &Sim, from: usize) -> Vec<(u64, String, Vec<u8>)> {
    use firmware_services::trace::{Event, decode};
    let trace = sim.trace();
    let (_, events) = decode(&trace).unwrap();
    let sounds: Vec<(String, u8, u8)> = events
        .filter_map(|event| match event {
            Event::Sound {
                pattern, cue, note, ..
            } => Some((cue.to_string(), pattern, note)),
            _ => None,
        })
        .collect();
    let played = &sim.device.sim().speaker.played;
    assert_eq!(
        sounds.len(),
        played.len(),
        "the trace has every note the speaker played"
    );
    let mut cues: Vec<(u64, String, Vec<u8>)> = Vec::new();
    for ((cue, pattern, note), &(at, _)) in sounds.into_iter().zip(played).skip(from) {
        match cues.last_mut() {
            Some(last) if note > 1 => last.2.push(pattern),
            _ => cues.push((at, cue, vec![pattern])),
        }
    }
    cues
}

fn with_a_course(gpx: &str) -> Sim {
    use device_api::server::Method;
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.format_files();
    sim.connect_phone();
    let (status, body) = sim.phone_request(Method::Put, "/api/course", gpx.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    sim
}

#[track_caller]
fn ride_until_cue_checking(sim: &mut Sim, cue: &str, before_m: f64, each: impl Fn(&Sim)) -> f64 {
    let from = sim.device.sim().speaker.played.len();
    let mut heard = from;
    loop {
        assert!(
            sim.world().distance_m < before_m,
            "no {cue} cue by {before_m} m; cues {:?}; screen:\n{}",
            cues_since(sim, from),
            sim.screen().text()
        );
        sim.advance(seconds(1));
        each(sim);
        let played = sim.device.sim().speaker.played.len();
        if played > heard {
            heard = played;
            if cues_since(sim, from).iter().any(|(name, _)| name == cue) {
                break;
            }
        }
    }
    let at = sim.world().distance_m;
    sim.note(NoteKind::Check, format!("{cue} cue at {at:.0} m"));
    at
}

#[track_caller]
fn ride_until_cue(sim: &mut Sim, cue: &str, before_m: f64) -> f64 {
    ride_until_cue_checking(sim, cue, before_m, |_| {})
}

#[track_caller]
fn ride_until_only(sim: &mut Sim, cue: &str, notes: &[u8], before_m: f64) -> f64 {
    let from = sim.device.sim().speaker.played.len();
    let at = ride_until_cue(sim, cue, before_m);
    sim.advance(seconds(1));
    assert_eq!(played_since(sim, from), notes, "{cue} at {at:.0} m");
    at
}

#[test]
fn a_course_from_the_phone_is_followed_on_the_map_off_and_back_on_course() {
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let mut sim = with_a_course(&course);

    sim.ride_route(Route::from_gpx(&course_ride_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    // Riding off along the course follows it.
    let joined = ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    assert!(joined < 120.0, "{joined}");
    sim.ride_page("5/5");
    expect_map(&sim);
    ride_to(&mut sim, 1_400.0);
    assert!(!map_badge(&sim).is_empty(), "the badge shows the next turn");
    let blue = course_pixels(&sim);
    assert!(blue > 100, "the course ahead is drawn: {blue} pixels");
    sim.checkpoint("the map with the course ahead in blue");

    let off = ride_until_only(&mut sim, "off_course", &OFF_COURSE, 2_150.0);
    assert!((1_990.0..2_090.0).contains(&off), "{off}");
    sim.checkpoint("off the course on the map");
    let again = ride_until_only(&mut sim, "off_course", &OFF_COURSE, 2_250.0);
    assert!(
        (100.0..112.0).contains(&(again - off)),
        "{off} then {again}"
    );
    let on = ride_until_only(&mut sim, "on_course", &ON_COURSE, 2_400.0);
    assert!((2_200.0..2_340.0).contains(&on), "{on}");
    sim.checkpoint("back on the course on the map");

    let end = ride_until_cue(&mut sim, ARRIVAL_CUE, 5_800.0);
    assert!((5_500.0..5_700.0).contains(&end), "{end}");
    sim.advance(seconds(1));
    assert_eq!(cues_since(&sim, 0).last().unwrap().1, ARRIVED);
    assert_eq!(map_badge(&sim), ["ARRIVED"]);
    sim.checkpoint("the end of the course");
    // On up the course's first leg again: the course is done for this ride.
    while sim.world().distance_m < end + 250.0 {
        sim.advance(seconds(1));
        sim.expect_no_text("FOLLOWING COURSE");
    }
    sim.stop_ride();
}

/// A course with a right-angle turn after 600 m, a gentle bend 45 degrees
/// left over 100 m, 400 m on, a hairpin left of 10 m radius, and 400 m back
/// down beside the way up.
fn turns_course_gpx() -> String {
    let mut legs = vec![(0.0, 0.0), (0.0, 600.0), (500.0, 600.0)];
    let mut at = (500.0f64, 600.0f64);
    for heading in [75.0f64, 60.0, 45.0] {
        let (sin, cos) = heading.to_radians().sin_cos();
        at = (at.0 + 35.0 * sin, at.1 + 35.0 * cos);
        legs.push(at);
    }
    let (sin, cos) = 45f64.to_radians().sin_cos();
    let top = (at.0 + 400.0 * sin, at.1 + 400.0 * cos);
    legs.push(top);
    // Round to the left: the centre is 10 m off the way, to its left.
    let centre = (top.0 - 10.0 * cos, top.1 + 10.0 * sin);
    for step in 1..=12 {
        let a = (135.0 - 15.0 * f64::from(step)).to_radians();
        legs.push((centre.0 + 10.0 * a.cos(), centre.1 - 10.0 * a.sin()));
    }
    let bottom = *legs.last().unwrap();
    legs.push((bottom.0 - 400.0 * sin, bottom.1 - 400.0 * cos));
    gpx_through("turns", &legs)
}

#[test]
fn a_followed_course_sounds_its_turns_and_not_a_gentle_bend() {
    let gpx = turns_course_gpx();
    let mut sim = with_a_course(&gpx);

    sim.ride_route(Route::from_gpx(&gpx).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    sim.ride_page("5/5");
    expect_map(&sim);

    let soon = ride_until_only(&mut sim, "turn_soon", &TURN_SOON, 500.0);
    assert!((290.0..310.0).contains(&soon), "{soon}");
    sim.checkpoint("the right turn's banner opens");
    let now = ride_until_only(&mut sim, "turn_now", &TURN_NOW, 620.0);
    assert!((560.0..580.0).contains(&now), "{now}");
    sim.checkpoint("30 m before the right turn");

    // Round the gentle bend and on: silence until the hairpin is near.
    ride_to(&mut sim, 700.0);
    let from = sim.device.sim_mut().speaker.played.len();
    while sim.world().distance_m < 1_280.0 {
        sim.advance(seconds(1));
    }
    assert_eq!(
        played_since(&sim, from),
        Vec::<u8>::new(),
        "no cue on the bend"
    );

    let soon = ride_until_only(&mut sim, "turn_soon", &TURN_SOON, 1_500.0);
    assert!((1_290.0..1_320.0).contains(&soon), "{soon}");
    sim.checkpoint("the hairpin's banner opens");
    let now = ride_until_only(&mut sim, "turn_now", &TURN_NOW, 1_620.0);
    assert!((1_565.0..1_585.0).contains(&now), "{now}");
    sim.checkpoint("30 m before the hairpin");
    let end = ride_until_cue(&mut sim, ARRIVAL_CUE, 2_100.0);
    assert!(end > 1_950.0, "{end}");
    sim.stop_ride();
}

#[test]
fn a_course_is_still_followed_after_the_ride_moves_to_internal_memory() {
    use device_api::server::Method;
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.format_files();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));

    sim.ride_route(Route::from_gpx(&course_ride_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    ride_to(&mut sim, 1_000.0);

    sim.fail_bulk(0, 0);
    sim.wait_for("RECORDING INT", seconds(15)).unwrap();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "moved"), "1", "{status}");
    sim.checkpoint("the ride moved to internal memory mid-course");

    let no_second_join = |sim: &Sim| {
        let text = sim.screen().text();
        assert!(!text.contains("FOLLOWING COURSE"), "joined again:\n{text}");
    };
    let off = ride_until_cue_checking(&mut sim, "off_course", 2_150.0, no_second_join);
    assert!((1_990.0..2_090.0).contains(&off), "{off}");
    sim.checkpoint("off the course after the move");
    let on = ride_until_cue_checking(&mut sim, "on_course", 2_400.0, no_second_join);
    assert!((2_200.0..2_340.0).contains(&on), "{on}");
    let end = ride_until_cue_checking(&mut sim, ARRIVAL_CUE, 5_800.0, no_second_join);
    assert!((5_500.0..5_700.0).contains(&end), "{end}");
    sim.checkpoint("the end of the course after the move");
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 1);
}

fn following_the_loop_on_the_map() -> Sim {
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let mut sim = with_a_course(&course);
    sim.ride_route(Route::from_gpx(&course_ride_gpx()).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    sim.ride_page("5/5");
    expect_map(&sim);
    ride_to(&mut sim, 600.0);
    assert!(!map_badge(&sim).is_empty());
    sim
}

#[test]
fn a_long_press_on_the_map_stops_following_the_course() {
    let mut sim = following_the_loop_on_the_map();
    sim.tap(120, 160);
    sim.advance(seconds(1));
    assert!(!map_badge(&sim).is_empty(), "a tap does not stop following");
    sim.long_press(120, 160, 1_200);
    assert_eq!(map_badge(&sim), Vec::<String>::new(), "no turn to show");
    sim.checkpoint("following stopped from the map");
    let from = sim.device.sim().speaker.played.len();
    while sim.world().distance_m < 2_400.0 {
        sim.advance(seconds(1));
        let text = sim.screen().text();
        assert!(
            !text.contains("FOLLOWING"),
            "at {:.0} m:\n{text}",
            sim.world().distance_m
        );
    }
    assert_eq!(
        played_since(&sim, from),
        Vec::<u8>::new(),
        "no cue off the course"
    );
    sim.stop_ride();
}

#[test]
fn a_long_press_on_the_badge_stops_following_too() {
    let mut sim = following_the_loop_on_the_map();
    sim.tap(70, 60);
    sim.advance(seconds(1));
    assert!(!map_badge(&sim).is_empty(), "a tap does not stop following");
    sim.long_press(70, 60, 1_200);
    assert_eq!(map_badge(&sim), Vec::<String>::new(), "no turn to show");
}

fn riding_too_far_beside_a_zigzag_to_follow() -> Sim {
    let zigzag: Vec<(f64, f64)> = (0..=14)
        .map(|leg| (f64::from(leg % 2) * 15.0, 150.0 * f64::from(leg)))
        .collect();
    let course = gpx_spaced("zigzag", &zigzag, 1_000.0);
    let mut sim = with_a_course(&course);
    let beside = gpx_through("beside it", &[(50.0, 0.0), (50.0, 2_100.0)]);
    sim.ride_route(Route::from_gpx(&beside).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.ride_page("5/5");
    sim
}

#[test]
fn before_following_the_map_comes_back_on_the_leg_ridden_beside_meanwhile() {
    let mut sim = riding_too_far_beside_a_zigzag_to_follow();
    ride_to(&mut sim, 375.0);
    sim.ride_page("1/5");
    ride_to(&mut sim, 1_000.0);
    sim.ride_page("5/5");
    let back = course_pixels(&sim);
    sim.checkpoint("back on the map, on the leg beside the rider");
    assert!(back > 1_000, "{back} px of course on return");
}

struct WatchedAndAway {
    watched: Sim,
    away: Sim,
}

impl WatchedAndAway {
    fn new(ride: impl Fn() -> Sim) -> Self {
        let mut pair = Self {
            watched: ride(),
            away: ride(),
        };
        pair.both(|sim| sim.ride_page("5/5"));
        pair
    }

    fn both(&mut self, act: impl Fn(&mut Sim)) {
        act(&mut self.watched);
        act(&mut self.away);
        let (watched, away) = (self.watched.now(), self.away.now());
        self.watched.advance(away.saturating_sub(watched));
        self.away.advance(watched.saturating_sub(away));
    }

    fn leave(&mut self) {
        self.away.ride_page("1/5");
        self.both(|_| {});
    }

    #[track_caller]
    fn come_back(&mut self, what: &str) {
        self.away.ride_page("5/5");
        self.both(|_| {});
        self.away.checkpoint(what);
        self.same_map(what);
    }

    #[track_caller]
    fn ride_to(&mut self, metres: f64) {
        while self.watched.world().distance_m < metres {
            self.both(|sim| sim.advance(seconds(1)));
            if self.away.screen().page().as_deref() == Some("5/5") {
                let at = self.watched.world().distance_m;
                self.same_map(&format!("at {at:.0} m"));
            }
        }
    }

    #[track_caller]
    fn same_map(&self, what: &str) {
        let (watched, away) = (course_pixels(&self.watched), course_pixels(&self.away));
        assert!(watched > 0, "{what}: no course drawn");
        assert!(
            away.abs_diff(watched) * 10 <= watched,
            "{what}: {away} px of course back from away, {watched} px watched"
        );
    }
}

#[test]
fn the_map_comes_back_on_the_leg_it_would_show_had_it_been_watched() {
    let course = gpx_through(
        "out and back",
        &[(0.0, 0.0), (0.0, 800.0), (30.0, 800.0), (30.0, 0.0)],
    );
    let ride = gpx_through(
        "beside, then along",
        &[
            (-45.0, -100.0),
            (-45.0, 400.0),
            (-45.0, 200.0),
            (0.0, 200.0),
            (0.0, 800.0),
            (30.0, 800.0),
            (30.0, -100.0),
        ],
    );
    let mut rides = WatchedAndAway::new(|| {
        let mut sim = with_a_course(&course);
        sim.ride_route(Route::from_gpx(&ride).unwrap());
        sim.pair(&["POWER METER", "SPEED/CAD"]);
        sim.ride_at(25.0, 180.0, 0.0, 85.0);
        sim.start_ride();
        sim
    });

    rides.leave();
    rides.ride_to(150.0);
    rides.come_back("beside the way out, from fixes with no heading yet");

    rides.ride_to(450.0);
    rides.leave();
    rides.ride_to(650.0);
    rides.come_back("after the U-turn, riding south beside the way back");

    rides.ride_to(690.0);
    rides.leave();
    rides.both(|sim| sim.ride_at(0.0, 0.0, 0.0, 0.0));
    rides.both(|sim| sim.advance(seconds(20)));
    rides.both(|sim| sim.ride_at(25.0, 180.0, 0.0, 85.0));
    rides.ride_to(800.0);
    rides.come_back("after a stop at the corner, across onto the way out");

    rides.ride_to(900.0);
    rides.leave();
    rides.ride_to(1_100.0);
    rides.come_back("following the way out");
    assert!(!map_badge(&rides.away).is_empty(), "following");

    rides.ride_to(2_100.0);
    rides.leave();
    rides.ride_to(2_250.0);
    rides.come_back("past the course's end");
    assert_eq!(map_badge(&rides.away), ["ARRIVED"]);
}

#[test]
fn after_a_long_press_the_map_comes_back_on_the_leg_it_would_show_had_it_been_watched() {
    let mut rides = WatchedAndAway::new(following_the_loop_on_the_map);
    rides.both(|sim| sim.long_press(120, 160, 1_200));
    assert_eq!(
        map_badge(&rides.away),
        Vec::<String>::new(),
        "following stopped"
    );
    rides.leave();
    rides.ride_to(1_500.0);
    rides.come_back("riding on along the course without following it");
    rides.ride_to(1_600.0);
}

fn following_a_short_course_with_a_radar() -> Sim {
    let course = gpx_through("short", &[(0.0, 0.0), (0.0, 800.0)]);
    let mut sim = with_a_course(&course);
    let ride = gpx_through("past its end", &[(0.0, 0.0), (0.0, 1_200.0)]);
    sim.ride_route(Route::from_gpx(&ride).unwrap());
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["POWER METER", "SPEED/CAD", "RADAR"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    ride_to(&mut sim, 600.0);
    sim
}

fn arrival_chime_ms() -> u64 {
    let mut sim = following_a_short_course_with_a_radar();
    ride_until_cue(&mut sim, ARRIVAL_CUE, 1_000.0);
    let second = sim.now();
    let mut fine = following_a_short_course_with_a_radar();
    fine.advance(second - seconds(1) - fine.now());
    let heard = fine.device.sim().speaker.played.len();
    while fine.device.sim().speaker.played.len() == heard {
        assert!(fine.now() < second, "no note by {second} ms");
        fine.advance(10);
    }
    fine.now()
}

const CHIME_WITHIN_MS: i64 = 10_000;

fn cues_after_acting_before_chime(
    chime_ms: u64,
    lead_ms: u64,
    for_ms: u64,
    act: impl Fn(&mut Sim),
) -> Vec<(i64, String, Vec<u8>)> {
    let mut sim = following_a_short_course_with_a_radar();
    let from = sim.device.sim().speaker.played.len();
    sim.advance(chime_ms - lead_ms - sim.now());
    act(&mut sim);
    sim.advance(for_ms);
    timed_cues_since(&sim, from)
        .into_iter()
        .map(|(at, name, notes)| (at as i64 - chime_ms as i64, name, notes))
        .collect()
}

fn whole_chime_at(heard: &[(i64, String, Vec<u8>)]) -> Option<i64> {
    heard
        .iter()
        .find(|(_, name, notes)| name == ARRIVAL_CUE && notes[..] == ARRIVED)
        .map(|&(at, ..)| at)
}

fn first_at(heard: &[(i64, String, Vec<u8>)], cue: &str) -> Option<i64> {
    heard
        .iter()
        .find(|(_, name, _)| name == cue)
        .map(|&(at, ..)| at)
}

fn closing_car() -> Vehicle {
    Vehicle {
        range_m: 60.0,
        closing_kmh: 30.0,
        fast: true,
        side: Side::Behind,
    }
}

#[test]
fn a_radar_alarm_that_cuts_the_arrival_chime_short_lets_it_sound_whole_after() {
    let chime_ms = arrival_chime_ms();
    let heard = cues_after_acting_before_chime(chime_ms, 100, seconds(15), |sim| {
        sim.vehicle(closing_car())
    });
    let first: Vec<_> = heard
        .iter()
        .take(2)
        .map(|(_, name, notes)| (name, notes))
        .collect();
    assert_eq!(
        first,
        [
            (&ARRIVAL_CUE.to_string(), &vec![0]),
            (&"radar_approach".to_string(), &vec![22])
        ],
        "{heard:?}"
    );
    let whole = whole_chime_at(&heard);
    assert!(whole.is_some_and(|at| at < CHIME_WITHIN_MS), "{heard:?}");
}

#[test]
fn a_radar_alarm_at_arrival_still_gives_the_whole_chime_within_10_s() {
    let chime_ms = arrival_chime_ms();
    let heard = cues_after_acting_before_chime(chime_ms, seconds(3), seconds(15), |sim| {
        sim.vehicle(closing_car())
    });
    let whole = whole_chime_at(&heard).expect("the whole chime");
    assert!((0..CHIME_WITHIN_MS).contains(&whole), "{heard:?}");
    assert!(
        heard
            .iter()
            .any(|(at, name, _)| name.starts_with("radar_") && (0..whole).contains(at)),
        "the chime waited behind the alarm: {heard:?}"
    );
    assert!(first_at(&heard, "radar_passing").is_some(), "{heard:?}");
}

#[test]
fn an_alarm_sustained_past_10_s_after_arrival_drops_the_arrival_chime() {
    let chime_ms = arrival_chime_ms();
    let heard = cues_after_acting_before_chime(chime_ms, 0, seconds(30), |sim| {
        for range_m in [12.0, 22.0, 32.0] {
            sim.vehicle(Vehicle {
                range_m,
                closing_kmh: 6.0,
                fast: false,
                side: Side::Behind,
            });
        }
    });
    let radar_after = |ms: i64| {
        heard
            .iter()
            .any(|(at, name, _)| name.starts_with("radar_") && *at >= ms)
    };
    assert!(radar_after(CHIME_WITHIN_MS), "sustained: {heard:?}");
    assert!(first_at(&heard, "radar_clear").is_some(), "{heard:?}");
    assert_eq!(whole_chime_at(&heard), None, "{heard:?}");
    assert!(
        heard
            .iter()
            .all(|(at, name, _)| name != ARRIVAL_CUE || *at < CHIME_WITHIN_MS),
        "{heard:?}"
    );
}

#[track_caller]
fn pressed_as_the_rider_arrives_the_chime_still_sounds(button: Button, cue: &str) {
    let chime_ms = arrival_chime_ms();
    let by_lead: Vec<_> = (0..=60)
        .step_by(10)
        .map(|lead_ms| {
            let press = |sim: &mut Sim| sim.report(button);
            let heard = cues_after_acting_before_chime(chime_ms, lead_ms, seconds(15), press);
            (lead_ms, heard)
        })
        .collect();
    for (lead_ms, heard) in &by_lead {
        let whole = whole_chime_at(heard);
        assert!(
            first_at(heard, cue).is_some() && whole.is_some_and(|at| at < CHIME_WITHIN_MS),
            "{lead_ms} ms before: {heard:?}"
        );
    }
    let chime_first =
        |heard: &[(i64, String, Vec<u8>)]| first_at(heard, ARRIVAL_CUE) < first_at(heard, cue);
    assert!(
        by_lead.iter().any(|(_, heard)| chime_first(heard))
            && by_lead.iter().any(|(_, heard)| !chime_first(heard)),
        "the presses straddle the pass that arrives: {by_lead:?}"
    );
}

#[test]
fn a_lap_in_the_pass_that_arrives_and_the_arrival_chime_both_sound() {
    pressed_as_the_rider_arrives_the_chime_still_sounds(Button::TopLeft, "lap");
}

#[test]
fn pausing_in_the_pass_that_arrives_keeps_the_arrival_chime() {
    pressed_as_the_rider_arrives_the_chime_still_sounds(Button::BottomRight, "pause");
}

/// Santiago on 14 October 2026: sunset 19:55 local, 22:55 UTC.
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

/// A straight road `km` north of the start and back, for rides whose way home
/// is the way they came.
fn straight_out_and_back_gpx(km: f64) -> String {
    gpx_through(
        "straight out and back",
        &[(0.0, 0.0), (0.0, km * 1000.0), (0.0, 0.0)],
    )
}

/// Advance `minutes` a second at a time, so no two-second toast can pass
/// unseen, and fail if `needle` shows.
#[track_caller]
fn expect_quiet(sim: &mut Sim, minutes: u64, needle: &str) {
    for _ in 0..minutes * 60 {
        sim.advance(seconds(1));
        sim.expect_no_text(needle);
    }
}

/// Santiago on 14 October 2026 (UTC-3, sunset 19:55 local, 22:55 UTC), riding
/// out at 25 km/h from 90 minutes before sunset. The way home takes as long as
/// the ride so far (the straight line times 1.3 is capped at the track ridden,
/// which on a straight road is the line), so after t minutes the margin is
/// 90 - 2t: it drops under 15 minutes at 37.5, and the last turn is then
/// 22:17:30 UTC, 19:17 here.
#[test]
fn riding_out_toward_dusk_says_when_to_turn_back() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_route(Route::from_gpx(&straight_out_and_back_gpx(40.0)).unwrap());
    sim.set_gps_clock(21 * 3600 + 25 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(20));
    sim.expect_no_text("TURN BACK");
    let latency = sim.wait_for("TURN BACK BY", minutes(30)).unwrap();
    // 37.5 minutes after the start, less the 20 spent already.
    assert!(
        (minutes(17)..=minutes(18)).contains(&latency),
        "told after {latency} ms"
    );
    sim.expect_text("TURN BACK BY 19:17");
    sim.advance(seconds(1));
    let ids = played_since(&sim, 0);
    assert!(ids.windows(2).any(|pair| pair == [10, 10]), "{ids:?}");
    sim.checkpoint("toast: turn back by 19:17");
    // The toast ends on the first frame after its two seconds.
    sim.wait_gone("TURN BACK", seconds(2)).unwrap();
    // Once: the margin holds while riding on, and the toast does not return,
    // not at the sunset warning, nor at sunset, nor in the dark after it.
    expect_quiet(&mut sim, 3, "TURN BACK");
    sim.wait_for("SUNSET IN 30 MIN", minutes(30)).unwrap();
    expect_quiet(&mut sim, 5, "TURN BACK");
    sim.wait_for("LIGHTS ON", minutes(40)).unwrap();
    expect_quiet(&mut sim, 10, "TURN BACK");
    sim.stop_ride();
}

/// Riding home already says nothing, even when slow riding home shrinks the
/// margin under 15 minutes: out 12.5 km at 25 km/h, 60 minutes from sunset,
/// then back at 10 km/h. Without the guard it would speak after about 15
/// minutes. A ride in the dark and one with SUNSET off say nothing either.
#[test]
fn no_turn_back_when_riding_home_in_the_dark_or_with_sunset_off() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_route(Route::from_gpx(&straight_out_and_back_gpx(12.5)).unwrap());
    sim.set_gps_clock(21 * 3600 + 25 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(29));
    sim.expect_no_text("TURN BACK");
    sim.ride_at(10.0, 0.0, 0.0, 0.0);
    expect_quiet(&mut sim, 28, "TURN BACK");
    sim.checkpoint("riding home slowly, no turn back toast");
    sim.stop_ride();

    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.set_gps_clock(23 * 3600 + 30 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    expect_quiet(&mut sim, 30, "TURN BACK");

    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.home("SETTINGS");
    sim.set_option("SUNSET", "OFF");
    sim.press(Button::TopLeft);
    sim.set_gps_clock(21 * 3600 + 25 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    expect_quiet(&mut sim, 50, "TURN BACK");
}

/// A square loop of 8 km sides, ridden counter-clockwise from its corner.
fn square_loop_gpx() -> String {
    gpx_through(
        "square loop",
        &[
            (0.0, 0.0),
            (0.0, 8_000.0),
            (8_000.0, 8_000.0),
            (8_000.0, 0.0),
            (0.0, 0.0),
        ],
    )
}

/// Round a loop at 25 km/h from two hours before sunset. The farthest the
/// rider gets from the start is 11 km, 35 minutes of riding home with the
/// winding of roads, so the margin never drops under 15 minutes. On the last
/// leg the whole breadcrumb is 25 km, but home is 7 km away: the way home is
/// the straight line, so no turn-back alert is due, nor is turning back the
/// shorter way there.
#[test]
fn riding_round_a_loop_never_says_turn_back_on_the_last_leg() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_route(Route::from_gpx(&square_loop_gpx()).unwrap());
    sim.set_gps_clock(20 * 3600 + 55 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    expect_quiet(&mut sim, 62, "TURN BACK");
    assert!(
        sim.world().distance_m > 25_000.0,
        "{:.0} m",
        sim.world().distance_m
    );
    sim.checkpoint("on the last leg of the loop, no turn back toast");
    sim.stop_ride();
}

/// 25 minutes at 3 km/h is long enough for the margin to fall under 15
/// minutes.
#[test]
fn no_turn_back_when_stopping_on_the_way_home() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_route(Route::from_gpx(&straight_out_and_back_gpx(8.0)).unwrap());
    sim.set_gps_clock(21 * 3600 + 40 * 60, (14, 10, 26));
    sim.advance(seconds(15));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    expect_quiet(&mut sim, 30, "TURN BACK");
    sim.ride_at(3.0, 0.0, 0.0, 0.0);
    expect_quiet(&mut sim, 25, "TURN BACK");
    sim.checkpoint("crawling home, no turn back toast");
    sim.stop_ride();
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
    sim.wait_for("WHEEL SET ", seconds(5)).unwrap();
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
    sim.expect_text("2.4");
    assert_eq!(run_color(&sim, "2.4"), AMBER);
    sim.checkpoint("heart rate detail with a low battery");
    sim.press(Button::TopLeft);
    sim.open_sensor_detail("POWER METER");
    sim.expect_text("2.9");
    assert_eq!(run_color(&sim, "2.9"), WHITE);
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
    sim.device.sim_mut().ble.snapshot.link = Link::Failed;
    sim.advance(seconds(1));
    sim.home("RIDE");
    sim.expect_text("NO SENSORS");
}

/// Open the power meter's detail page and wait out the moment ACT ignores
/// after it opens, so the next ACT, on the calibrate cue, zeroes it.
fn calibrate_power_meter(sim: &mut Sim, reply: ZeroReply) {
    sim.ant_sensor(device_type::POWER, POWER).zero = reply;
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(3));
    sim.home("SENSORS");
    sim.open_sensor_detail("POWER METER");
    sim.checkpoint("power meter detail");
    sim.advance(seconds(1));
}

#[test]
fn calibrating_a_power_meter_shows_the_offset_it_reports() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(-37));
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATING", seconds(20)).unwrap();
    sim.checkpoint("power meter calibrating");
    sim.wait_for("CALIBRATED", seconds(5)).unwrap();
    sim.expect_text("OFFSET -37");
    assert_eq!(run_color(&sim, "CALIBRATED OFFSET -37"), GREEN);
    sim.checkpoint("power meter calibrated with its offset");
    sim.expect_text("BATTERY");
    sim.wait_gone("CALIBRATED", seconds(12)).unwrap();
}

#[test]
fn a_power_meter_that_refuses_to_zero_ends_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Failure);
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
    calibrate_power_meter(&mut sim, ZeroReply::Silence);
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
    calibrate_power_meter(&mut sim, ZeroReply::Offset(512));
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

fn hashed() -> Sim {
    let mut sim = Sim::new();
    sim.hash_frames();
    sim
}

#[track_caller]
fn assert_replays_exactly(
    recorded: Sim,
    scenario: fn(&mut Sim),
    bulk: bool,
) -> shell_simulator::replay::Capture {
    assert_replays_exactly_after(recorded, scenario, bulk, |_| {})
}

#[track_caller]
fn assert_replays_exactly_after(
    mut recorded: Sim,
    scenario: fn(&mut Sim),
    bulk: bool,
    alter: fn(&mut shell_simulator::replay::Capture),
) -> shell_simulator::replay::Capture {
    scenario(&mut recorded);
    let mut capture = recorded.capture();
    alter(&mut capture);
    println!("{}", trace_size(&capture.trace, recorded.device.now));
    let mut replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    if let Some(difference) =
        shell_simulator::replay::first_difference(&capture.trace, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
    let (frames, replayed_frames) = (recorded.frame_hashes(), replayed.frame_hashes());
    assert!(frames.len() > 50, "{} frames", frames.len());
    let first = frames
        .iter()
        .zip(&replayed_frames)
        .position(|(a, b)| a != b);
    assert_eq!(
        (first, replayed_frames.len()),
        (None, frames.len()),
        "the first frame that differs, and how many the replay showed"
    );
    assert!(
        replayed.ride_media() == recorded.ride_media(),
        "the internal journal's bytes differ"
    );
    let samples = |sim: &mut Sim, bulk: bool| {
        let rides = if bulk { sim.bulk_rides() } else { sim.rides() };
        rides
            .iter()
            .map(|ride| (ride.id, ride.kinds(), ride.samples().copied().collect()))
            .collect::<Vec<(u32, Vec<Kind>, Vec<Sample>)>>()
    };
    let rides = samples(&mut recorded, bulk);
    assert!(
        rides.iter().any(|ride| ride.2.len() > 30),
        "a ride was saved"
    );
    assert_eq!(samples(&mut replayed, bulk), rides);
    capture
}

fn trace_size(bytes: &[u8], uptime_ms: u64) -> String {
    use firmware_services::trace::{Encoder, MAX_BLOCK, decode};
    let (_, events) = decode(bytes).unwrap();
    let mut encoder = Encoder::new(vec![0u8; MAX_BLOCK]);
    let mut kinds = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for event in events {
        let before = encoder.len();
        let size = match encoder.push(&event) {
            Ok(()) => encoder.len() - before,
            Err(_) => {
                encoder.restart();
                let before = encoder.len();
                encoder.push(&event).unwrap();
                encoder.len() - before
            }
        };
        let debug = format!("{event:?}");
        let kind = debug.split([' ', '(', '{']).next().unwrap_or_default();
        let entry = kinds.entry(kind.into()).or_default();
        entry.0 += 1;
        entry.1 += size;
    }
    let minutes = uptime_ms as f64 / 60_000.0;
    let mut out = format!(
        "trace: {} bytes over {minutes:.1} min of uptime, {:.0} bytes/min;",
        bytes.len(),
        bytes.len() as f64 / minutes
    );
    for (kind, (count, size)) in kinds {
        out += &format!(" {kind} {count}x {:.0} B/min,", size as f64 / minutes);
    }
    out
}

#[test]
fn a_ride_with_laps_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), manual_laps, false);
}

#[test]
fn a_trace_damaged_mid_way_replays_to_where_it_breaks_and_says_so() {
    use shell_simulator::replay::{Kind, first_difference};
    let mut sim = equip(Sim::new().boot());
    manual_laps(&mut sim);
    let mut capture = sim.capture();
    let middle = capture.trace.len() / 2;
    capture.trace[middle] ^= 0x40;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.count(Kind::Truncated), 1, "{report}");
    assert!(
        replayed.device.now < sim.device.now,
        "the replay stops where the trace breaks"
    );
    let difference = first_difference(&capture.trace, &replayed.trace()).unwrap();
    assert!(difference.contains("damaged"), "{difference}");
}

#[test]
fn an_auto_paused_ride_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), traffic_light_stop, false);
}

#[test]
fn a_ride_with_a_ble_strap_selected_by_command_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), ble_heart_strap, false);
}

#[test]
fn a_command_runs_after_its_pass_as_on_the_c606() {
    use firmware_services::trace::{Event, decode};
    let capture = assert_replays_exactly(equip(hashed().boot()), ble_heart_strap, false);
    let (_, events) = decode(&capture.trace).unwrap();
    let (mut passes, mut commands) = (Vec::new(), 0);
    for event in events {
        match event {
            Event::Pass { now_ms } => passes.push(now_ms),
            Event::Command(_) => commands += 1,
            _ => {}
        }
    }
    assert!(commands > 0, "the scenario sends a command");
    let again = passes.windows(2).find(|pair| pair[1] <= pair[0]);
    assert_eq!(again, None, "a pass at or before the one before it");
}

#[test]
fn a_two_hour_ride_with_a_pause_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), two_hour_ride_with_a_pause, false);
}

const WIFI_UTC_MS: u64 = 1_793_521_800_000;

fn a_ride_dated_by_wifi(sim: &mut Sim) {
    sim.sync_network_time(WIFI_UTC_MS);
    let synced = sim.now();
    cyclist(sim);
    let pressed = press_start(sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let start = fit_start_ms(&sim.rides().pop().unwrap());
    let expected = WIFI_UTC_MS + pressed - synced;
    assert!(
        start.abs_diff(expected) < 1_000,
        "the ride starts at {start}, Wi-Fi time at the press is {expected}"
    );
}

#[test]
fn a_ride_dated_by_wifi_replays_exactly_whatever_the_process_clock_holds() {
    use firmware_services::network_time::{Timestamp, update};
    update(
        Timestamp {
            unix_seconds: (WIFI_UTC_MS / 1000 + 365 * 86_400) as u32,
            millis: 0,
            stratum: 2,
        },
        0,
    );
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_dated_by_wifi, false);
    let (_, events) = firmware_services::trace::decode(&capture.trace).unwrap();
    let synced = events
        .filter_map(|event| match event {
            firmware_services::trace::Event::NetworkTime(time) => time.unix_seconds,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(synced.len(), 1, "one synchronization: {synced:?}");
    assert!(synced[0].abs_diff(WIFI_UTC_MS / 1000) < 60, "{synced:?}");
}

fn a_ride_on_new_bulk_media(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(minutes(5));
    sim.pause_ride();
    sim.advance(minutes(1));
    sim.resume_ride();
    sim.advance(minutes(5));
    sim.stop_ride();
}

#[test]
fn a_ride_on_bulk_media_replays_exactly() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    assert_replays_exactly(equip(sim.boot()), a_ride_on_new_bulk_media, true);
}

#[test]
fn a_trace_header_names_the_settings_memory_and_media_the_device_started_from() {
    use firmware_services::trace::origin::{
        Config, FileEntry, Found, Journal, Media, Memory, RideLog,
    };
    let field = |out: &str, key: &str| -> String {
        out.split_whitespace()
            .find_map(|word| word.strip_prefix(key))
            .unwrap_or_else(|| panic!("{key} in {out}"))
            .to_string()
    };
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.install_terrain(b"CHILE-V1\0\0\0\0\0\0\0\0", &[7u8; 2048]);
    sim.put_file("workouts/tempo.workout", b"tempo");
    let mut sim = equip(sim.boot());
    a_ride_on_new_bulk_media(&mut sim);
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    let (_, internal) = sim.command("RIDE INTERNAL STATUS");
    sim.device.restart().unwrap();
    let mut sim = sim.boot();
    let settings = sim.device.shell.settings;
    a_ride_on_new_bulk_media_again(&mut sim);
    let (_, after) = sim.command("RIDE BULK STATUS");
    assert!(
        field(&after, "slot=") != field(&bulk, "slot="),
        "this session rode on the card"
    );

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture.trace).unwrap();
    let config = Config::decode(header.config).unwrap();
    assert_eq!(config.memory, Memory::ALL);
    assert!(!config.redacted);
    let (stored, _) = firmware_shell::preferences::decode(Some(config.settings));
    assert_eq!(stored, settings, "the settings at power-on");

    let media = Media::decode(header.media).unwrap();
    let Some(RideLog::Log { store, head }) = media.rides else {
        panic!("the card's ride log: {:?}", media.rides);
    };
    assert_eq!(format!("{store:016x}"), field(&bulk, "store="));
    assert_eq!(
        head.to_string(),
        field(&bulk, "slot="),
        "the card's head at power-on"
    );
    let Some(Journal::Scanned { next, last }) = media.journal else {
        panic!("the internal journal: {:?}", media.journal);
    };
    assert_eq!(next.to_string(), field(&internal, "slot="));
    assert_eq!(last.is_some(), next > 0, "its last slot's CRC");
    let Found::Read(terrain) = media.terrain else {
        panic!("the pack: {:?}", media.terrain);
    };
    assert!(terrain.ready);
    assert_eq!(terrain.length, 2048);
    assert_eq!(&terrain.tag[..8], b"CHILE-V1");
    let Found::Read(files) = media.files else {
        panic!("FILES: {:?}", media.files);
    };
    let names: Vec<FileEntry<'_>> = files.entries().collect();
    assert!(
        names
            .iter()
            .any(|file| file.name == "workouts/tempo.workout" && file.length == 5),
        "{names:?}"
    );
}

#[test]
fn a_replay_gives_vana_only_the_working_memory_the_recording_had() {
    use firmware_services::trace::{self, origin::Config};
    let mut sim = equip(Sim::new().boot());
    sim.advance(seconds(5));
    let mut capture = sim.capture();
    assert!(sim.device.sdk.detach_terrain().is_some());
    let (header, _) = trace::decode(&capture.trace).unwrap();
    let mut config = Config::decode(header.config).unwrap();
    config.memory.terrain = false;
    let mut encoded = vec![0u8; 1024];
    let len = config.encode(&mut encoded).unwrap();
    let rewritten = trace::Header {
        config: &encoded[..len],
        ..header
    };
    let mut block = vec![0u8; trace::MAX_BLOCK];
    let block_len = trace::encode_header(&rewritten, &mut block).unwrap();
    let old = 6 + usize::from(u16::from_le_bytes([capture.trace[0], capture.trace[1]]));
    capture.trace = [&block[..block_len], &capture.trace[old..]].concat();
    let mut replayed = Sim::replay(&capture);
    assert!(
        replayed.device.sdk.detach_terrain().is_none(),
        "no tile cache, as the recording had none"
    );
}

fn a_ride_on_new_bulk_media_again(sim: &mut Sim) {
    sim.advance(seconds(10));
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(minutes(2));
    sim.stop_ride();
}

#[test]
fn a_session_recorded_into_the_store_replays_exactly_from_the_file_a_host_pulls() {
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    a_ride_on_new_bulk_media(&mut sim);
    assert!(sim.device.sync_trace(), "the session records into STORE");
    let name = sim.device.trace_file().unwrap();
    assert!(name.starts_with("traces/000001-"), "{name}");
    let pulled = sim.store_file(&name).expect("every record reads back");
    let mut capture = sim.capture();
    assert_eq!(pulled.len(), capture.trace.len());
    assert!(pulled == capture.trace, "the file is the trace");
    capture.trace = pulled;

    let mut replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
    let (frames, replayed_frames) = (sim.frame_hashes(), replayed.frame_hashes());
    assert!(frames.len() > 100);
    assert!(frames == replayed_frames, "the same frames from power-on");
    assert!(
        replayed.ride_media() == sim.ride_media(),
        "the same journal"
    );
    let samples = |sim: &mut Sim| {
        sim.bulk_rides()
            .iter()
            .map(|ride| (ride.id, ride.samples().copied().collect::<Vec<Sample>>()))
            .collect::<Vec<_>>()
    };
    let rides = samples(&mut sim);
    assert!(
        rides.iter().any(|ride| ride.1.len() > 30),
        "a ride was saved"
    );
    assert_eq!(samples(&mut replayed), rides);
}

#[test]
fn a_new_store_replays_with_the_entropy_it_was_named_from() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly_after(
        equip(sim.boot()),
        a_ride_on_new_bulk_media,
        true,
        |capture| capture.origin.bulk.reseed_entropy(0xdec0de),
    );
    let (_, events) = firmware_services::trace::decode(&capture.trace).unwrap();
    let entropy = events
        .filter(|event| matches!(event, firmware_services::trace::Event::Entropy(_)))
        .count();
    assert!(entropy > 0, "the store's entropy is recorded");
}

fn paired_then_ridden(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(10));
    sim.start_ride();
    sim.advance(minutes(2));
    sim.ride_at(33.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(2));
    sim.stop_ride();
}

fn sensor_data(sim: &mut Sim) -> Vec<Vec<Sample>> {
    sim.rides()
        .iter()
        .map(|ride| ride.samples().copied().collect())
        .collect()
}

#[test]
fn a_ride_replayed_as_stimulus_on_the_same_build_saves_the_same_ride() {
    use shell_simulator::replay::Mode;
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let mut replayed = Sim::replay_as(&recorded.capture(), Mode::Stimulus, |_| {});
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report:?}");
    let rides = sensor_data(&mut recorded);
    assert!(
        rides.iter().any(|ride| ride.len() > 200),
        "a ride was saved"
    );
    assert_eq!(sensor_data(&mut replayed), rides);
}

fn connected_in_order(trace: &[u8]) -> Vec<u8> {
    use firmware_services::trace::{Event, decode};
    let (_, events) = decode(trace).unwrap();
    events
        .filter_map(|event| match event {
            Event::AntRequest {
                operation: device_api::ant::AntOperation::Connect(peer),
                result: Ok(_),
            } => Some(peer.device_type),
            _ => None,
        })
        .collect()
}

#[test]
fn a_ride_replays_against_a_build_that_connects_the_power_meter_first() {
    use device_api::ant::{AntOperation, Error};
    use shell_simulator::replay::{Kind, Mode};
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let mut power_asked = false;
    let mut replayed = Sim::replay_as(&recorded.capture(), Mode::Stimulus, |sources| {
        sources.adapt_ant(move |operation, _| match operation {
            AntOperation::Connect(peer) if peer.device_type == device_type::POWER => {
                power_asked = true;
                None
            }
            AntOperation::Connect(_) if !power_asked => Some(Err(Error::Busy)),
            _ => None,
        })
    });
    let (power, heart, speed) = (
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::SPEED_CADENCE,
    );
    assert_eq!(connected_in_order(&recorded.trace()), [heart, power, speed]);
    let mut order = connected_in_order(&replayed.trace());
    assert_eq!(order[0], power, "{order:?}");
    order.sort_unstable();
    assert_eq!(order, [power, heart, speed]);
    let report = replayed.device.report().unwrap();
    println!("{report:#?}");
    assert!(report.count(Kind::Refused) >= 1, "{report:?}");
    assert!(report.count(Kind::Modelled) >= 1, "{report:?}");
    assert!(report.count(Kind::NotHeard) > 0, "{report:?}");
    assert_eq!(sensor_data(&mut replayed), sensor_data(&mut recorded));
}

fn count_events(
    trace: &[u8],
    matches: impl Fn(&firmware_services::trace::Event<'_>) -> bool,
) -> usize {
    let (_, events) = firmware_services::trace::decode(trace).unwrap();
    events.filter(|event| matches(event)).count()
}

fn a_course_uploaded_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
}

#[test]
fn a_course_uploaded_from_the_phone_mid_session_replays_exactly() {
    use firmware_services::trace::Event;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let capture = assert_replays_exactly(equip(sim.boot()), a_course_uploaded_from_the_phone, true);
    let read: usize = {
        let (_, events) = firmware_services::trace::decode(&capture.trace).unwrap();
        events
            .filter_map(|event| match event {
                Event::ServerRead { part, .. } => Some(part.len()),
                _ => None,
            })
            .sum()
    };
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    assert_eq!(read, course.len(), "the whole body is in the trace");
    let joined = count_events(&capture.trace, |event| {
        matches!(
            event,
            Event::AccessPoint {
                state: device_api::server::AccessPointState::Up { clients: 1 },
                ..
            }
        )
    });
    assert_eq!(joined, 1, "the phone joined once");
}

#[test]
fn the_largest_phone_upload_reaches_the_trace_file_whole() {
    use device_api::server::Method;
    use firmware_services::trace::{Event, decode};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let end = course.rfind("</gpx>").unwrap();
    let max = firmware_services::http::MAX_BODY as usize;
    let mut body = course[..end].to_string();
    body.push_str(&" ".repeat(max - course.len()));
    body.push_str(&course[end..]);
    assert_eq!(body.len(), max);
    let (status, reply) = sim.phone_request(Method::Put, "/api/course", body.as_bytes());
    assert_eq!(status, 200, "{reply}");
    sim.advance(seconds(6));
    let (peak, capacity) = sim.device.trace_spool_peak().unwrap();
    println!(
        "a {} KiB upload peaks the spool at {peak} of {capacity} B",
        max / 1024
    );
    assert!(sim.device.sync_trace());
    let pulled = sim.store_file(&sim.device.trace_file().unwrap()).unwrap();
    let (_, events) = decode(&pulled).unwrap();
    let (mut read, mut lost) = (0, 0);
    for event in events {
        match event {
            Event::ServerRead { part, .. } => read += part.len(),
            Event::Lost { blocks, .. } => lost += blocks,
            _ => {}
        }
    }
    assert_eq!((read, lost), (max, 0), "the whole body, nothing lost");
    assert!(peak < capacity / 8, "the drain keeps up: {peak} B");
}

fn a_redacted_phone_session_then_a_ride() -> (Sim, [String; 5]) {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    sim.device.redact_secrets();
    let mut sim = equip(sim.boot());
    let (status, out) = sim.command("WIFI CONFIG WPA2 486f6d65 7365637265743132");
    assert_eq!(status, "ACCEPTED", "{out}");
    sim.device.restart().unwrap();
    let mut sim = sim.boot();
    sim.command("WIFI CONFIG WPA3 486f6d65 70617373776f7264");
    sim.init_bulk();
    sim.connect_phone();
    let status = sim.device.sdk.phone_status().expect("a phone session");
    let secrets = [
        status.token.text().to_string(),
        status.password.text().to_string(),
        "7365637265743132".to_string(),
        "70617373776f7264".to_string(),
        "secret12".to_string(),
    ];
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (code, body) = sim.phone_request(
        device_api::server::Method::Put,
        "/api/course",
        course.as_bytes(),
    );
    assert_eq!(code, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    (sim, secrets)
}

#[test]
fn a_redacted_phone_session_replays_on_placeholders_and_holds_no_secret() {
    use firmware_services::trace::origin::{Config, Media};
    use firmware_services::trace::{Event, decode};
    let (mut sim, secrets) = a_redacted_phone_session_then_a_ride();
    let capture = sim.capture();
    let text = String::from_utf8_lossy(&capture.trace);
    for secret in &secrets {
        assert!(!text.contains(secret.as_str()), "the trace holds {secret}");
    }
    let (header, events) = decode(&capture.trace).unwrap();
    let config = Config::decode(header.config).unwrap();
    assert!(config.redacted);
    let (settings, _) = firmware_shell::preferences::decode(Some(config.settings));
    let wifi = settings.wifi.expect("the saved network");
    assert_eq!(wifi.ssid.text(), "Home");
    assert_eq!(wifi.password.text(), "xxxxxxxx", "kept its length");
    assert!(Media::decode(header.media).is_some());
    let sessions: Vec<String> = events
        .filter_map(|event| match event {
            Event::Server {
                request: Some(request),
                ..
            } => Some(request.session.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        sessions.iter().any(|session| session == "****************"),
        "{sessions:?}"
    );

    let mut replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    assert_eq!(
        report.count(shell_simulator::replay::Kind::Redacted),
        2,
        "the phone page draws the redacted credentials: {report}"
    );
    assert!(
        replayed.ride_media() == sim.ride_media(),
        "the same journal"
    );
    assert_eq!(replayed.bulk_rides().len(), sim.bulk_rides().len());
    assert_eq!(
        replayed.file("course"),
        sim.file("course"),
        "the same course"
    );
    let placeholder = replayed.device.sdk.phone_status();
    assert!(placeholder.is_none(), "the session closed for the ride");
}

/// The capture, with the rider shown another page where its last page
/// opened.
fn with_the_last_page_unseen(
    mut capture: shell_simulator::replay::Capture,
) -> shell_simulator::replay::Capture {
    use firmware_services::trace::{Event, decode};
    let trace: &'static [u8] = Box::leak(capture.trace.clone().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let last = events
        .iter()
        .rposition(|event| matches!(event, Event::Page(_)))
        .unwrap();
    events[last] = Event::Page("a page the rider never saw");
    capture.trace = mutations::encode(&header, events);
    capture
}

#[test]
fn a_redacted_phone_session_leaves_a_later_difference_first() {
    let (sim, _) = a_redacted_phone_session_then_a_ride();
    let replayed = Sim::replay(&with_the_last_page_unseen(sim.capture()));
    let report = replayed.device.report().unwrap();
    let first = report.first.as_ref().expect("the altered page diverges");
    assert_eq!(first.kind, shell_simulator::replay::Kind::Seen, "{report}");
    assert!(
        first.detail.contains("a page the rider never saw"),
        "{report}"
    );
    assert!(
        report.count(shell_simulator::replay::Kind::Redacted) > 0,
        "{report}"
    );
}

#[test]
fn a_stimulus_replay_names_no_frame_in_what_the_rider_saw() {
    let (sim, _) = a_redacted_phone_session_then_a_ride();
    let capture = with_the_last_page_unseen(sim.capture());
    let stimulus = shell_simulator::replay::Mode::Stimulus;
    let replayed = Sim::replay_as(&capture, stimulus, |_| {});
    let report = replayed.device.report().unwrap();
    let first = report.first.as_ref().expect("the altered page diverges");
    assert!(
        first.detail.contains("a page the rider never saw") && !first.detail.contains("Panel("),
        "a stimulus replay compares no frames: {report}"
    );
}

fn a_ride_whose_journal_write_tears(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.fail_storage(0, 10);
    sim.advance(seconds(30));
}

#[test]
fn a_storage_failure_mid_ride_replays_exactly() {
    use firmware_services::trace::{Event, Medium, StorageFault};
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_journal_write_tears,
        false,
    );
    let torn = count_events(&capture.trace, |event| {
        matches!(
            event,
            Event::StorageFault(StorageFault {
                medium: Medium::Data,
                landed: 10,
                ..
            })
        )
    });
    assert_eq!(torn, 1, "one torn write, with what landed");
}

fn a_ride_whose_card_fails(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.fail_bulk(0, 100);
    sim.advance(minutes(1));
    sim.stop_ride();
}

#[test]
fn a_bulk_failure_mid_ride_replays_exactly() {
    use firmware_services::trace::{Event, Medium, StorageFault};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_fails, false);
    let torn = count_events(&capture.trace, |event| {
        matches!(
            event,
            Event::StorageFault(StorageFault {
                medium: Medium::Bulk,
                landed: 100,
                ..
            })
        )
    });
    assert_eq!(torn, 1, "the torn write, with what landed");
}

fn a_ride_whose_card_goes_missing(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.fit_bulk(Presence::Missing);
    sim.advance(minutes(1));
    sim.stop_ride();
}

#[test]
fn a_card_that_goes_missing_mid_ride_replays_exactly() {
    use firmware_services::trace::{Event, Medium, StorageError, StorageFault};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_goes_missing, false);
    let missing = count_events(&capture.trace, |event| {
        matches!(
            event,
            Event::StorageFault(StorageFault {
                medium: Medium::Bulk,
                error: StorageError::Unavailable,
                ..
            })
        )
    });
    assert!(missing > 0, "the card's absence is recorded");
}

fn a_ride_whose_panel_and_backlight_refuse(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(30));
    sim.device.display.0.borrow_mut().fail_next = true;
    sim.device.power.0.borrow_mut().fail = true;
    let (status, out) = sim.command("BRIGHTNESS 40");
    assert_eq!(status, "OK", "{out}");
    sim.advance(seconds(5));
    sim.device.power.0.borrow_mut().fail = false;
    sim.device.power.0.borrow_mut().available = device_api::observation::Availability::Failed;
    sim.advance(seconds(5));
    sim.device.power.0.borrow_mut().available = device_api::observation::Availability::Ready;
    sim.advance(seconds(30));
    sim.stop_ride();
}

#[test]
fn a_panel_and_backlight_that_refuse_replay_exactly() {
    use firmware_services::trace::{Event, Output};
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_panel_and_backlight_refuse,
        false,
    );
    for output in [Output::Panel, Output::Backlight] {
        let refused = count_events(
            &capture.trace,
            |event| matches!(event, Event::Refused { output: refused, .. } if *refused == output),
        );
        assert!(refused > 0, "{output:?} refused");
    }
    let changes = count_events(&capture.trace, |event| matches!(event, Event::Backlight(_)));
    assert_eq!(changes, 3, "ready, failed, ready again");
}

fn a_ride_then_turned_off(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    sim.done();
    sim.press(Button::TopLeft);
    sim.press(Button::BottomRight);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "turned off");
    sim.advance(seconds(2));
}

#[test]
fn a_ride_ended_by_turning_off_replays_exactly() {
    use firmware_services::trace::Event;
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_then_turned_off, false);
    let asked = count_events(&capture.trace, |event| {
        matches!(event, Event::PowerRequest { result: Ok(()), .. })
    });
    let off = count_events(&capture.trace, |event| matches!(event, Event::PoweredOff));
    assert_eq!(
        (asked, off),
        (1, 1),
        "one shutdown asked, then the power went"
    );
}

#[test]
fn a_session_turned_off_is_in_its_file_up_to_the_shutdown_request() {
    use firmware_services::trace::{Event, decode};
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.advance(seconds(1));
    let name = sim
        .device
        .trace_file()
        .expect("the session records into STORE");
    a_ride_then_turned_off(&mut sim);
    let passes = |trace: &[u8]| {
        let (_, events) = decode(trace).unwrap();
        let (mut passes, mut asked) = (Vec::new(), None);
        for event in events {
            match event {
                Event::Pass { now_ms } => passes.push(now_ms),
                Event::PowerRequest { .. } | Event::PoweredOff => {
                    asked = asked.or(passes.last().copied())
                }
                _ => {}
            }
        }
        (passes, asked)
    };
    let (recorded, asked) = passes(&sim.capture().trace);
    let asked = asked.expect("VANA asked to turn off");
    let pulled = sim.store_file(&name).expect("every record reads back");
    let (filed, _) = passes(&pulled);
    let before = recorded.iter().position(|&at| at == asked).unwrap();
    assert_eq!(
        filed,
        recorded[..before],
        "every pass before the one that asked"
    );
    let mut capture = sim.capture();
    capture.trace = pulled;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
}

mod mutations {
    use super::*;
    use firmware_services::trace::frozen::{Record, Recorder};
    use firmware_services::trace::{Event, Header, decode};
    use shell_simulator::replay::{Blocks, Capture, first_difference};

    pub struct Recording {
        capture: Capture,
        pub trace: &'static [u8],
        frames: Vec<u64>,
        media: Vec<u8>,
        settings: Vec<u8>,
        card: u64,
        backlight: (u8, u32),
    }

    type Outputs = (Vec<u64>, Vec<u8>, Vec<u8>, u64, (u8, u32));

    /// What a session left behind: its frames, its ride journal, its saved
    /// settings, the sectors it wrote to the card, and the backlight.
    fn outputs(sim: &mut Sim) -> Outputs {
        let power = sim.device.power.0.borrow();
        let backlight = (power.brightness, power.writes);
        drop(power);
        (
            sim.frame_hashes(),
            sim.ride_media(),
            sim.device
                .media
                .contents(device_api::storage::Region::Configuration),
            sim.device.bulk.digest(),
            backlight,
        )
    }

    pub fn record(sim: Sim, scenario: fn(&mut Sim)) -> Recording {
        let mut sim = sim;
        scenario(&mut sim);
        let capture = sim.capture();
        let (frames, media, settings, card, backlight) = outputs(&mut sim);
        Recording {
            trace: Vec::leak(capture.trace.clone()),
            frames,
            media,
            settings,
            card,
            backlight,
            capture,
        }
    }

    pub fn kind(event: &Event<'_>) -> String {
        let debug = format!("{event:?}");
        debug
            .split([' ', '(', '{'])
            .next()
            .unwrap_or_default()
            .into()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mutation {
        Drop,
        Alter,
    }

    fn altered(event: Event<'static>) -> Option<Event<'static>> {
        use device_api::observation::{Error, Observation};
        let flip = |result: Result<(), Error>| match result {
            Ok(()) => Err(Error::Failed),
            Err(_) => Ok(()),
        };
        Some(match event {
            Event::Input(mut edge) => {
                edge.input = match edge.input {
                    device_api::input::Input::Button { button, code } => {
                        device_api::input::Input::Button {
                            button: if button == Button::TopLeft {
                                Button::BottomRight
                            } else {
                                Button::TopLeft
                            },
                            code,
                        }
                    }
                    _ => device_api::input::Input::Cancel,
                };
                Event::Input(edge)
            }
            Event::AntPacket(mut packet) => {
                packet.data[7] = packet.data[7].wrapping_add(40);
                Event::AntPacket(packet)
            }
            Event::AntRequest { operation, result } => Event::AntRequest {
                operation,
                result: match result {
                    Ok(_) => Err(device_api::ant::Error::Busy),
                    Err(_) => Ok(device_api::ant::Admission::Accepted),
                },
            },
            Event::BlePacket(mut packet) => {
                packet.bytes[1] = packet.bytes[1].wrapping_add(40);
                Event::BlePacket(packet)
            }
            Event::Physical(mut snapshot) => {
                if let Observation::Fresh { value, .. } = &mut snapshot.pressure {
                    value.pressure_centi_pa -= 50_000;
                }
                Event::Physical(snapshot)
            }
            Event::Power(Some(mut snapshot)) => {
                if let Observation::Fresh { value, .. } = &mut snapshot.power {
                    *value = if *value == 0 { 7 } else { 0 };
                }
                Event::Power(Some(snapshot))
            }
            Event::Position {
                availability,
                snapshot: Some(mut snapshot),
            } => {
                snapshot.gps.latitude_e7 = snapshot.gps.latitude_e7.map(|lat| lat + 100_000);
                Event::Position {
                    availability,
                    snapshot: Some(snapshot),
                }
            }
            Event::NetworkTime(mut time) => {
                time.unix_seconds = time.unix_seconds.map(|seconds| seconds + 7 * 3_600);
                Event::NetworkTime(time)
            }
            Event::Entropy(value) => Event::Entropy(value ^ 0xffff),
            Event::StorageFault(mut fault) => {
                use firmware_services::trace::StorageError;
                match fault.landed {
                    0..=5 => {
                        fault.error = match fault.error {
                            StorageError::Unsupported => StorageError::Unavailable,
                            _ => StorageError::Unsupported,
                        }
                    }
                    landed => fault.landed = landed - 5,
                }
                Event::StorageFault(fault)
            }
            Event::Refused { output, attempt } => Event::Refused {
                output,
                attempt: attempt.wrapping_add(1),
            },
            Event::SoundRequest { play, result } => Event::SoundRequest {
                play,
                result: flip(result),
            },
            Event::PowerRequest {
                operation,
                delay_ms,
                result,
            } => Event::PowerRequest {
                operation,
                delay_ms,
                result: flip(result),
            },
            Event::AccessPointRequest { start, result } => Event::AccessPointRequest {
                start,
                result: flip(result),
            },
            Event::ServerRead { id, part, more } if part.iter().any(u8::is_ascii_digit) => {
                let mut changed = part.to_vec();
                let digit = changed.iter_mut().find(|byte| byte.is_ascii_digit())?;
                *digit = if *digit == b'9' { b'0' } else { *digit + 1 };
                Event::ServerRead {
                    id,
                    part: Vec::leak(changed),
                    more,
                }
            }
            Event::ServerWrite { id, taken } => Event::ServerWrite {
                id,
                taken: taken.saturating_sub(1),
            },
            Event::ServerFinish(id) => Event::ServerFinish(id + 1),
            Event::Pass { now_ms } => Event::Pass { now_ms: now_ms + 5 },
            Event::AntRadio(mut radio) => {
                radio.scanning = !radio.scanning;
                Event::AntRadio(radio)
            }
            Event::AntDiscoveries(mut found) => {
                found[0] = match found[0] {
                    Some(_) => None,
                    None => found.iter().flatten().next().copied(),
                };
                Event::AntDiscoveries(found)
            }
            Event::AntLink {
                channel,
                snapshot: Some(mut snapshot),
            } => {
                snapshot.stale = !snapshot.stale;
                Event::AntLink {
                    channel,
                    snapshot: Some(snapshot),
                }
            }
            Event::AntSend(Some(mut send)) => {
                send.stage = device_api::ant::SendStage::Uncertain;
                Event::AntSend(Some(send))
            }
            Event::BleLink {
                availability,
                mut snapshot,
            } => {
                snapshot.disconnections += 1;
                Event::BleLink {
                    availability,
                    snapshot,
                }
            }
            Event::BleBattery(mut level) => {
                level.raw = level.raw.wrapping_add(30);
                Event::BleBattery(level)
            }
            Event::Startup(startup) => Event::Startup(match startup {
                device_api::sensors::Startup::Ready => {
                    device_api::sensors::Startup::RunningDegraded
                }
                _ => device_api::sensors::Startup::Ready,
            }),
            Event::SoundState(mut snapshot) => {
                snapshot.state = device_api::sound::State::Failed;
                Event::SoundState(snapshot)
            }
            Event::PowerControl(mut status) => {
                status.ready = !status.ready;
                Event::PowerControl(status)
            }
            Event::Backlight(_) => Event::Backlight(device_api::observation::Availability::Failed),
            Event::AccessPoint { state, reach } => Event::AccessPoint {
                state: match state {
                    device_api::server::AccessPointState::Up { clients } => {
                        device_api::server::AccessPointState::Up {
                            clients: clients + 1,
                        }
                    }
                    _ => device_api::server::AccessPointState::Failed,
                },
                reach,
            },
            Event::Server { request, room } => Event::Server {
                request,
                room: room / 2,
            },
            Event::Page(_) => Event::Page("bests"),
            Event::Panel(digest) => Event::Panel(digest ^ 1),
            Event::Sound {
                pattern,
                cue,
                note,
                of,
            } => Event::Sound {
                pattern: pattern ^ 1,
                cue,
                note,
                of,
            },
            _ => return None,
        })
    }

    pub fn encode(
        header: &Header<'_>,
        events: impl IntoIterator<Item = Event<'static>>,
    ) -> Vec<u8> {
        let mut file = vec![0u8; 4096];
        let len = firmware_services::trace::encode_header(header, &mut file).unwrap();
        file.truncate(len);
        let buffer = vec![0u8; 4096].into_boxed_slice();
        let mut recorder = Recorder::new(buffer, Blocks(file));
        for event in events {
            recorder.record(&event);
        }
        let mut bytes = recorder.sink().0.clone();
        bytes.extend_from_slice(recorder.open_block());
        bytes
    }

    pub fn mutated(trace: &'static [u8], kind: &str, how: Mutation) -> Option<Vec<u8>> {
        let (header, events) = decode(trace).unwrap();
        let mut events: Vec<Event<'static>> = events.collect();
        let of_kind: Vec<usize> = (0..events.len())
            .filter(|&index| self::kind(&events[index]) == kind)
            .collect();
        let at = *of_kind.get(of_kind.len() / 2)?;
        match how {
            Mutation::Drop => {
                events.remove(at);
            }
            Mutation::Alter => {
                let changed = altered(events[at]).filter(|changed| *changed != events[at])?;
                events[at] = changed;
            }
        }
        Some(encode(&header, events))
    }

    pub fn with_too_large(trace: &'static [u8]) -> Vec<u8> {
        let (header, events) = decode(trace).unwrap();
        let mut events: Vec<Event<'static>> = events.collect();
        let second = events
            .iter()
            .position(|event| matches!(event, Event::Pass { .. }))
            .unwrap()
            + 1;
        events.insert(second, Event::TooLarge(Event::Command("").tag()));
        encode(&header, events)
    }

    #[derive(Debug)]
    pub struct Caught {
        pub report: Option<String>,
        pub visible: bool,
        pub frames: bool,
        pub handed: bool,
    }

    impl Caught {
        pub fn any(&self) -> bool {
            self.report.is_some() || self.visible || self.handed
        }

        pub fn how(&self) -> String {
            match (&self.report, self.visible, self.handed) {
                (Some(report), _, _) => format!("report: {report}"),
                (None, true, _) => "visible: frames or journal".into(),
                (None, false, true) => "handed: re-recorded trace".into(),
                (None, false, false) => "NOT CAUGHT".into(),
            }
        }
    }

    pub fn replay(recording: &Recording, trace: Vec<u8>) -> Caught {
        let capture = Capture {
            origin: recording.capture.origin.untapped(),
            trace,
        };
        let mut replayed = Sim::replay(&capture);
        let report = replayed.device.report().unwrap();
        let report = report
            .first
            .map(|first| format!("{:?} at {} ms", first.kind, first.at_ms));
        let outputs = outputs(&mut replayed);
        let frames = shown(&outputs.0) != shown(&recording.frames);
        let visible = outputs
            != (
                recording.frames.clone(),
                recording.media.clone(),
                recording.settings.clone(),
                recording.card,
                recording.backlight,
            );
        let handed = first_difference(recording.trace, &replayed.trace()).is_some();
        Caught {
            report,
            visible,
            frames,
            handed,
        }
    }

    /// What the panel showed, frame by frame, without a frame drawn again
    /// as it was.
    fn shown(frames: &[u64]) -> Vec<u64> {
        let mut shown = frames.to_vec();
        shown.dedup();
        shown
    }

    /// How a mutation must show. Most change what VANA does where the
    /// report or the outputs see it; some heal, and then only the values
    /// handed to VANA show it, proving the trace was their only source.
    #[derive(Clone, Copy, Debug)]
    pub enum Shows {
        Directly,
        Heals(&'static str),
    }

    /// Each row's middle event, mutated as the row says, must show as the
    /// row expects. The unmutated trace must not, or the check proves
    /// nothing.
    #[track_caller]
    pub fn assert_every_mutation_shows(recording: &Recording, rows: &[(&str, Mutation, Shows)]) {
        let clean = replay(recording, recording.trace.to_vec());
        assert!(!clean.any(), "the unmutated trace replays clean: {clean:?}");
        let (mut missed, mut silent) = (Vec::new(), Vec::new());
        for &(kind, how, shows) in rows {
            let trace = mutated(recording.trace, kind, how)
                .unwrap_or_else(|| panic!("no {kind} to {how:?} in the trace"));
            let caught = replay(recording, trace);
            if caught.frames && caught.report.is_none() {
                silent.push((kind, how));
            }
            let (shown, why) = match shows {
                Shows::Directly => (caught.report.is_some() || caught.visible, ""),
                Shows::Heals(why) => (caught.any(), why),
            };
            println!("| {kind} | {how:?} | {} | {why} |", caught.how());
            if !shown {
                missed.push((kind, how, shows));
            }
        }
        assert!(
            missed.is_empty(),
            "mutations that did not show as expected: {missed:?}"
        );
        assert!(
            silent.is_empty(),
            "strict replays that showed other frames and reported nothing: {silent:?}"
        );
    }
}

fn a_short_ride_with_every_sensor(sim: &mut Sim) {
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(Some(80));
    sim.select_ble_heart_strap();
    sim.sync_network_time(WIFI_UTC_MS);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(40));
    sim.press(Button::BottomRight);
    sim.advance(seconds(40));
    sim.stop_ride();
}

type Row = (&'static str, mutations::Mutation, mutations::Shows);

/// Rows that drop and alter each kind and expect both to show directly,
/// with `heals` in place of the rows it names.
fn rows(kinds: &[&'static str], heals: &[Row]) -> Vec<Row> {
    use mutations::{Mutation, Shows};
    let mut rows: Vec<Row> = kinds
        .iter()
        .flat_map(|&kind| [Mutation::Drop, Mutation::Alter].map(|how| (kind, how, Shows::Directly)))
        .collect();
    for &(kind, how, shows) in heals {
        let row = rows
            .iter_mut()
            .find(|row| (row.0, row.1) == (kind, how))
            .unwrap_or_else(|| panic!("{kind} {how:?} heals but is not a row"));
        row.2 = shows;
    }
    rows
}

fn heals(kind: &'static str, how: mutations::Mutation, why: &'static str) -> Row {
    (kind, how, mutations::Shows::Heals(why))
}

#[test]
fn every_sensor_input_mutated_shows_in_its_replay() {
    use mutations::Mutation::{Alter, Drop};
    let recording = mutations::record(equip(hashed().boot()), a_short_ride_with_every_sensor);
    let once = "the next report, within a second, replaces it";
    let mut rows = rows(
        &[
            "Pass",
            "Input",
            "AntRadio",
            "AntDiscoveries",
            "AntLink",
            "AntPacket",
            "AntRequest",
            "BleLink",
            "BlePacket",
            "BleBattery",
            "Position",
            "Physical",
            "Power",
            "NetworkTime",
            "Startup",
            "SoundState",
            "SoundRequest",
            "PowerControl",
            "Backlight",
            "StorageFault",
            "Page",
            "Sound",
            "Panel",
        ],
        &[
            heals(
                "Pass",
                Drop,
                "its events join the pass before; nothing VANA shows moves by 10 ms",
            ),
            heals(
                "Pass",
                Alter,
                "5 ms later; nothing VANA shows moves by 5 ms",
            ),
            heals(
                "AntRadio",
                Alter,
                "no screen this ride shows reads the scan flag",
            ),
            heals(
                "AntDiscoveries",
                Drop,
                "the sensors are already chosen; a scan list changes nothing",
            ),
            heals(
                "AntDiscoveries",
                Alter,
                "the sensors are already chosen; a scan list changes nothing",
            ),
            heals("AntLink", Drop, once),
            heals("AntLink", Alter, once),
            heals("AntPacket", Drop, "the next page's running totals catch up"),
            heals(
                "AntRequest",
                Alter,
                "the channel snapshots that follow still report the sensor connected",
            ),
            heals(
                "BleLink",
                Drop,
                "only the strap's counters change there, and no screen shows them",
            ),
            heals(
                "BleLink",
                Alter,
                "only the strap's counters change there, and no screen shows them",
            ),
            heals("BlePacket", Drop, once),
            heals("BlePacket", Alter, once),
            heals(
                "BleBattery",
                Drop,
                "the strap's battery reaches no screen in this ride",
            ),
            heals(
                "BleBattery",
                Alter,
                "the strap's battery reaches no screen in this ride",
            ),
            heals("Physical", Drop, once),
            heals("Physical", Alter, once),
            heals("Power", Drop, once),
            heals("Power", Alter, once),
            heals(
                "Startup",
                Alter,
                "VANA shows a ready and a degraded companion alike",
            ),
            heals(
                "SoundState",
                Drop,
                "VANA reads only its play answers, never the buzzer's state",
            ),
            heals(
                "SoundState",
                Alter,
                "VANA reads only its play answers, never the buzzer's state",
            ),
            heals(
                "StorageFault",
                Drop,
                "one of the absent card's refusals; the next request is refused again",
            ),
            heals(
                "StorageFault",
                Alter,
                "one of the absent card's refusals; the next request is refused again",
            ),
        ],
    );
    rows.push(heals(
        "Command",
        Drop,
        "the middle command only reads the ride's state",
    ));
    mutations::assert_every_mutation_shows(&recording, &rows);
    let marked = mutations::with_too_large(recording.trace);
    let caught = mutations::replay(&recording, marked);
    println!("| TooLarge | Insert | {} | |", caught.how());
    assert!(caught.report.is_some(), "{caught:?}");
}

fn a_power_meter_calibrated(sim: &mut Sim) {
    calibrate_power_meter(sim, ZeroReply::Offset(-37));
    sim.press(Button::BottomRight);
    sim.wait_for("CALIBRATED", seconds(25)).unwrap();
}

#[test]
fn every_ant_send_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_power_meter_calibrated);
    let rows = rows(
        &["AntSend", "AntRequest"],
        &[
            heals(
                "AntSend",
                mutations::Mutation::Alter,
                "the next status poll reports the send again",
            ),
            heals(
                "AntRequest",
                mutations::Mutation::Alter,
                "the middle request is the power meter's connection, asked for again once refused; SENSORS no longer searches as it opens",
            ),
        ],
    );
    mutations::assert_every_mutation_shows(&recording, &rows);
}

fn a_course_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(2));
}

#[test]
fn every_phone_input_mutated_shows_in_its_replay() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let recording = mutations::record(equip(sim.boot()), a_course_from_the_phone);
    use mutations::Mutation::{Alter, Drop};
    let rows = rows(
        &[
            "AccessPoint",
            "AccessPointRequest",
            "Server",
            "ServerRead",
            "ServerWrite",
            "ServerFinish",
            "Entropy",
        ],
        &[
            heals(
                "AccessPoint",
                Alter,
                "VANA reads only whether a phone joined, not how many",
            ),
            heals(
                "Server",
                Alter,
                "VANA writes what fits, and the response still completes",
            ),
            heals(
                "ServerFinish",
                Drop,
                "what finishing changed then follows the write before it, in the same pass",
            ),
        ],
    );
    mutations::assert_every_mutation_shows(&recording, &rows);
}

fn a_ride_with_faults(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.device.display.0.borrow_mut().fail_next = true;
    sim.device.power.0.borrow_mut().fail = true;
    let (status, out) = sim.command("BRIGHTNESS 40");
    assert_eq!(status, "OK", "{out}");
    sim.advance(seconds(2));
    sim.device.power.0.borrow_mut().fail = false;
    sim.advance(seconds(20));
    sim.fail_storage(0, 10);
    sim.advance(seconds(20));
}

#[test]
fn every_fault_input_mutated_shows_in_its_replay() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let recording = mutations::record(equip(sim.boot()), a_ride_with_faults);
    mutations::assert_every_mutation_shows(
        &recording,
        &rows(&["StorageFault", "Refused", "Backlight"], &[]),
    );
}

#[test]
fn every_card_fault_mutated_shows_in_its_replay() {
    for scenario in [a_ride_whose_card_fails, a_ride_whose_card_goes_missing] {
        let mut sim = hashed();
        sim.fit_bulk(Presence::Ready);
        let recording = mutations::record(equip(sim.boot()), scenario);
        let after = "one of the card's refusals after the ride moved to internal flash";
        let rows = rows(
            &["StorageFault"],
            &[
                heals("StorageFault", mutations::Mutation::Drop, after),
                heals("StorageFault", mutations::Mutation::Alter, after),
            ],
        );
        mutations::assert_every_mutation_shows(&recording, &rows);
    }
}

#[test]
fn every_power_input_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_ride_then_turned_off);
    let mut rows = rows(
        &["PowerRequest", "PowerControl"],
        &[heals(
            "PowerRequest",
            mutations::Mutation::Alter,
            "the status and the power loss recorded after it still carry the shutdown",
        )],
    );
    rows.push((
        "PoweredOff",
        mutations::Mutation::Drop,
        mutations::Shows::Directly,
    ));
    mutations::assert_every_mutation_shows(&recording, &rows);
}

/// A ride whose page turns on a press the harness sent over the terminal,
/// mid-ride, as a host test drives a C606.
fn a_ride_paged_by_the_harness(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(seconds(30));
    let (status, opened) = sim.command("HARNESS OPEN 1 60000");
    assert_eq!(status, "OK", "{opened}");
    let session = opened
        .split_whitespace()
        .find_map(|word| word.strip_prefix("session="))
        .unwrap()
        .to_string();
    for command in ["INPUT BEGIN 1", "INPUT ADD 1 0 BUTTON 1", "INPUT RUN 1"] {
        let (status, out) = sim.command(&format!("HARNESS {session} {command}"));
        assert!(
            matches!(status, "OK" | "ACCEPTED"),
            "{command}: {status} {out}"
        );
    }
    sim.advance(seconds(1));
    expect_page(sim, "ride/stats");
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
}

#[test]
fn a_ride_paged_by_the_harness_replays_exactly() {
    use firmware_services::trace::Event;
    let capture =
        assert_replays_exactly(equip(hashed().boot()), a_ride_paged_by_the_harness, false);
    let pressed = count_events(&capture.trace, |event| matches!(event, Event::Synthetic(_)));
    assert_eq!(pressed, 1, "the harness pressed once");
}

fn a_ride_paged_by_a_press(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(seconds(30));
    sim.press(Button::BottomLeft);
    expect_page(sim, "ride/stats");
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
}

#[track_caller]
fn expect_page(sim: &Sim, page: &str) {
    let seen = sim.seen().page.map(|(_, page)| page);
    assert_eq!(seen.as_deref(), Some(page));
}

#[test]
fn a_ride_paged_by_a_press_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), a_ride_paged_by_a_press, false);
}

struct Drawn {
    at: u64,
    drawn_ms: u64,
    numbers: [Option<String>; 4],
    clock: Option<String>,
    texts: Vec<String>,
}

fn is_ride_clock(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 3
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1..]
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_digit()))
}

const TICK_SLACK_MS: u64 = 60;
const WALL_AND_RIDE_SECONDS_APART_MS: u64 = 370;
const RADAR_REPORTS_PER_S: usize = 4;
const CAR_SECONDS: u64 = 6;

fn tick_phase(frames: &[Drawn]) -> u64 {
    frames
        .windows(2)
        .find(|pair| pair[0].clock != pair[1].clock)
        .map(|pair| pair[1].drawn_ms % 1000)
        .expect("the ride clock turned over")
}

fn off_tick_in(frames: &[Drawn], phase: u64) -> Vec<u64> {
    frames
        .windows(2)
        .filter(|pair| pair[0].texts != pair[1].texts)
        .filter(|pair| (pair[1].drawn_ms + 1000 - phase) % 1000 > TICK_SLACK_MS)
        .map(|pair| pair[1].at)
        .collect()
}

fn frames_drawn(sim: &mut Sim, secs: u64, mut feed: impl FnMut(&mut Sim, u64)) -> Vec<Drawn> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let (mut frames, mut last, mut shown) = (Vec::new(), 0, 0);
    let start = sim.now();
    while sim.now() - start < seconds(secs) {
        if (sim.now() - start).is_multiple_of(250) {
            feed(sim, sim.now() - start);
        }
        sim.advance(10);
        let submissions = sim.device.shell.display_submissions;
        if submissions == shown {
            continue;
        }
        shown = submissions;
        let screen = sim.screen();
        let mut hasher = DefaultHasher::new();
        screen.pixels.hash(&mut hasher);
        let pixels = hasher.finish();
        if pixels == last {
            continue;
        }
        last = pixels;
        let texts: Vec<String> = screen
            .frame()
            .read()
            .into_iter()
            .map(|run| run.text)
            .collect();
        let clock = texts.iter().find(|text| is_ride_clock(text)).cloned();
        frames.push(Drawn {
            at: sim.now() - start,
            drawn_ms: sim.now(),
            numbers: ["KM/H", "W", "BPM", "RPM"].map(|label| screen.field(label)),
            clock,
            texts,
        });
    }
    frames
}

fn wobbling() -> impl FnMut(&mut Sim, u64) {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    move |sim, _| {
        let mut wobble = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2001) as f64 / 1000.0 - 1.0
        };
        sim.ride_at(
            28.0 + wobble(),
            230.0 + 35.0 * wobble(),
            142.0 + 2.0 * wobble(),
            88.0 + 3.0 * wobble(),
        );
    }
}

struct Rides {
    number_changes: Vec<u64>,
    longest_burst: usize,
    car_frames: usize,
    off_tick: Vec<u64>,
}

fn measure(sim: &mut Sim) -> Rides {
    sim.advance(WALL_AND_RIDE_SECONDS_APART_MS);
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(10));
    let noisy = frames_drawn(sim, 10, wobbling());
    let number_changes = noisy
        .windows(2)
        .filter(|pair| pair[0].numbers != pair[1].numbers)
        .map(|pair| pair[1].at)
        .collect();
    let phase = tick_phase(&noisy);
    let mut off_tick = off_tick_in(&noisy, phase);
    for _ in 0..8 {
        sim.press(Button::BottomLeft);
        sim.advance(seconds(1));
        if sim.seen().page.is_some_and(|(_, page)| page == "ride/main") {
            break;
        }
        off_tick.extend(off_tick_in(&frames_drawn(sim, 5, wobbling()), phase));
    }
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(5));
    let step = frames_drawn(sim, 6, |sim, at| {
        if at == 1_000 {
            sim.ride_at(28.0, 330.0, 142.0, 88.0);
        }
    });
    let (mut longest_burst, mut run) = (0, 0);
    for pair in step.windows(2) {
        run = if pair[1].at - pair[0].at <= 50 {
            run + 1
        } else {
            0
        };
        longest_burst = longest_burst.max(run + 1);
    }
    sim.vehicle(Vehicle {
        range_m: 150.0,
        closing_kmh: 25.0,
        fast: false,
        side: Side::Behind,
    });
    let car_frames = frames_drawn(sim, CAR_SECONDS, |_, _| {}).len();
    Rides {
        number_changes,
        longest_burst,
        car_frames,
        off_tick,
    }
}

#[test]
fn saver_mode_is_chosen_in_settings_kept_over_a_restart_and_draws_only_what_arrives() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"]);

    let mut normal = rider();
    normal.ant_sensor(device_type::RADAR, RADAR);
    normal.pair(&["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"]);
    let smooth = measure(&mut normal);
    assert!(
        smooth
            .number_changes
            .windows(2)
            .any(|pair| pair[1] - pair[0] < 300),
        "{:?}",
        smooth.number_changes
    );
    assert!(smooth.longest_burst >= 4, "{}", smooth.longest_burst);

    sim.open_mode_row();
    sim.expect_field("MODE", "NORMAL");
    sim.expect_text("SMOOTH");
    sim.checkpoint("settings: the display mode starts NORMAL");
    sim.press(Button::BottomRight);
    sim.expect_field("MODE", "SAVER");
    sim.expect_text("NO GLIDES");
    sim.checkpoint("settings: the display mode SAVER");
    sim.wait_for("SAVED", seconds(5)).unwrap();
    go_home(&mut sim);

    sim.power_loss();
    assert!(sim.command("PROFILE").1.contains("stored=current"));
    sim.open_mode_row();
    sim.expect_field("MODE", "SAVER");
    sim.checkpoint("settings: SAVER after a restart");
    go_home(&mut sim);

    let saver = measure(&mut sim);
    sim.checkpoint("main page in SAVER");
    assert!(
        saver.number_changes.len() >= 8
            && saver
                .number_changes
                .windows(2)
                .all(|pair| pair[1] - pair[0] >= 900),
        "numbers change once a second at most: {:?}",
        saver.number_changes
    );
    assert!(
        saver.off_tick.is_empty(),
        "text changed between ride-clock seconds at {:?} ms",
        saver.off_tick
    );
    assert!(
        saver.longest_burst <= 2,
        "no glide frames: {}",
        saver.longest_burst
    );
    let radar_and_numbers = CAR_SECONDS as usize * (RADAR_REPORTS_PER_S + 1);
    assert!(
        saver.car_frames <= radar_and_numbers && saver.car_frames * 2 < smooth.car_frames,
        "{} frames in SAVER, {} in NORMAL",
        saver.car_frames,
        smooth.car_frames
    );
}

fn backlight_in(sim: &mut Sim, light: Option<u16>) -> u8 {
    sim.set_ambient_light(light);
    sim.advance(seconds(60));
    sim.device.shell.effective()
}

#[test]
fn auto_brightness_follows_the_light_saver_sits_lower_and_a_percent_ignores_it() {
    let mut sim = rider();
    sim.home("SETTINGS");
    show_setting(&mut sim, "DIM AFTER");
    sim.set_option("DIM AFTER", "NEVER");
    sim.expect_field("BRIGHTNESS", "AUTO");
    assert!(sim.device.shell.auto());
    assert_eq!(
        backlight_in(&mut sim, None),
        50,
        "no reading: the stored 50%"
    );

    let bright = backlight_in(&mut sim, Some(1_500));
    let dim = backlight_in(&mut sim, Some(68));
    assert!((82..=88).contains(&bright), "{bright}");
    assert!((21..=27).contains(&dim), "{dim}");
    sim.checkpoint("settings: brightness AUTO in a dim room");
    assert_eq!(
        backlight_in(&mut sim, None),
        50,
        "reading lost: back to 50%"
    );

    go_home(&mut sim);
    sim.open_mode_row();
    sim.press(Button::BottomRight);
    sim.expect_field("MODE", "SAVER");
    let saver_bright = backlight_in(&mut sim, Some(1_500));
    let saver_dim = backlight_in(&mut sim, Some(68));
    assert!(
        saver_bright < bright && saver_dim < dim,
        "{saver_bright} {saver_dim}"
    );
    assert!(saver_bright <= 60 && saver_dim <= 18);

    show_setting(&mut sim, "BRIGHTNESS");
    sim.set_option("BRIGHTNESS", "50%");
    assert!(!sim.device.shell.auto());
    for light in [Some(40), Some(4_000), None] {
        assert_eq!(backlight_in(&mut sim, light), 50, "{light:?}");
    }
    sim.checkpoint("settings: brightness fixed at 50% ignores the light");
}

mod navigation {
    use super::*;
    use device_api::server::Method;

    fn straight_then(east: f64) -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (0.0, 7_000.0), (east, 7_000.0)]
    }

    fn on_course(course: &[(f64, f64)], route: &[(f64, f64)]) -> Sim {
        on_course_spaced(course, route, 10.0)
    }

    fn on_course_spaced(course: &[(f64, f64)], route: &[(f64, f64)], spacing: f64) -> Sim {
        let mut sim = Sim::new();
        sim.fit_bulk(Presence::Ready);
        let mut sim = equip(sim.boot());
        sim.init_bulk();
        sim.format_files();
        for set in ["PROFILE FTP 254", "PROFILE LTHR 158"] {
            assert_eq!(sim.command(set).0, "OK", "{set}");
        }
        sim.connect_phone();
        let gpx = gpx_spaced("course", course, spacing);
        let (status, body) = sim.phone_request(Method::Put, "/api/course", gpx.as_bytes());
        assert_eq!(status, 200, "{body}");
        sim.press(Button::TopLeft);
        sim.advance(seconds(1));
        sim.ride_route(Route::from_gpx(&gpx_spaced("route", route, spacing)).unwrap());
        sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
        sim
    }

    fn start_following(sim: &mut Sim) {
        sim.start_ride();
        ride_until_shown(sim, "FOLLOWING COURSE", 150.0);
    }

    const CHIP: core::ops::Range<usize> = 72..100;
    const BANNER: core::ops::Range<usize> = 22..104;

    fn sign_text(sim: &Sim, rows: core::ops::Range<usize>) -> Vec<String> {
        sim.screen()
            .frame()
            .read()
            .into_iter()
            .filter(|run| rows.contains(&run.y))
            .map(|run| run.text)
            .collect()
    }

    #[track_caller]
    fn ride_until_sign(
        sim: &mut Sim,
        rows: core::ops::Range<usize>,
        words: &[&str],
        before_m: f64,
    ) -> f64 {
        while sign_text(sim, rows.clone()) != words {
            assert!(
                sim.world().distance_m < before_m,
                "no sign {words:?} by {before_m} m; it reads {:?}",
                sign_text(sim, rows)
            );
            sim.advance(seconds(1));
        }
        let at = sim.world().distance_m;
        sim.note(NoteKind::Check, format!("sign {words:?} at {at:.0} m"));
        at
    }

    fn readings_at(sim: &Sim) -> Vec<(u16, usize, usize)> {
        use vana::screens::chrome::{CADENCE, HEART, POWER, SPEED};
        sim.screen()
            .frame()
            .read()
            .into_iter()
            .filter(|run| run.y >= BANNER.end)
            .filter(|run| [SPEED, POWER, HEART, CADENCE].contains(&run.color) || run.text == "KM")
            .map(|run| (run.color, run.x, run.y))
            .collect()
    }

    #[test]
    fn far_from_the_next_turn_main_shows_a_chip_under_the_time() {
        let course = straight_then(1_000.0);
        let mut sim = on_course(&course, &course);
        sim.ride_at(31.4, 220.0, 145.0, 91.0);
        start_following(&mut sim);
        let from = sim.device.sim().speaker.played.len();
        let at = ride_until_sign(&mut sim, CHIP, &["6.4", "KM"], 1_000.0);
        assert!((500.0..700.0).contains(&at), "{at}");
        assert_eq!(cues_since(&sim, from), [], "the chip is silent");
        sim.checkpoint("ride-straight");
    }

    #[test]
    fn a_turn_banner_covers_the_time_and_moves_no_reading() {
        let course = straight_then(-1_000.0);
        let mut sim = on_course(&course, &course);
        sim.ride_at(24.6, 182.0, 141.0, 84.0);
        start_following(&mut sim);
        ride_to(&mut sim, 6_600.0);
        let at_rest = readings_at(&sim);
        assert_eq!(at_rest.len(), 5, "{at_rest:?}");

        let soon = ride_until_only(&mut sim, "turn_soon", &TURN_SOON, 6_850.0);
        assert!((6_690.0..6_710.0).contains(&soon), "{soon}");
        ride_until_sign(&mut sim, BANNER, &["120", "M"], 6_900.0);
        assert_eq!(readings_at(&sim), at_rest, "the banner moves no reading");
        sim.expect_no_text("0:");
        sim.checkpoint("ride-turns");

        let now = ride_until_only(&mut sim, "turn_now", &TURN_NOW, 7_000.0);
        assert!((6_960.0..6_980.0).contains(&now), "{now}");
        ride_to(&mut sim, 7_100.0);
        assert_eq!(readings_at(&sim), at_rest, "nor does it leaving");
        assert_eq!(
            sign_text(&sim, CHIP),
            ["900", "M"],
            "the chip again, to the end"
        );
    }

    #[test]
    fn a_missed_turn_turns_the_sign_amber_until_the_rider_rejoins() {
        let course = [(0.0, 0.0), (0.0, 2_000.0), (1_000.0, 2_000.0)];
        let route = [
            (0.0, 0.0),
            (0.0, 2_220.0),
            (0.0, 2_000.0),
            (1_000.0, 2_000.0),
        ];
        let mut sim = on_course(&course, &route);
        sim.ride_at(26.1, 205.0, 141.0, 84.0);
        start_following(&mut sim);
        ride_until_only(&mut sim, "turn_soon", &TURN_SOON, 2_000.0);
        ride_until_only(&mut sim, "turn_now", &TURN_NOW, 2_000.0);

        let off = ride_until_only(&mut sim, "off_course", &OFF_COURSE, 2_120.0);
        assert!((2_070.0..2_090.0).contains(&off), "40 m off for 5 s: {off}");
        ride_until_sign(&mut sim, BANNER, &["140", "M", "TURN BACK"], 2_160.0);
        sim.checkpoint("ride-offcourse");

        let again = ride_until_only(&mut sim, "off_course", &OFF_COURSE, 2_240.0);
        assert!(
            (100.0..120.0).contains(&(again - off)),
            "{off} then {again}"
        );
        let on = ride_until_only(&mut sim, "on_course", &ON_COURSE, 2_520.0);
        assert!((2_410.0..2_500.0).contains(&on), "{on}");
        sim.expect_no_text("TURN BACK");
    }

    #[test]
    fn arriving_shows_the_ride_totals_for_a_minute() {
        let course = [(0.0, 0.0), (0.0, 50_000.0), (-6_200.0, 50_000.0)];
        let mut route = course.to_vec();
        route.push((-7_000.0, 50_000.0));
        let mut sim = on_course_spaced(&course, &route, 100.0);
        sim.ride_at(27.1, 168.0, 139.0, 88.0);
        start_following(&mut sim);
        let end = ride_until_cue(&mut sim, ARRIVAL_CUE, 56_400.0);
        assert!((56_100.0..56_300.0).contains(&end), "{end}");
        sim.advance(seconds(1));
        assert_eq!(cues_since(&sim, 0).last().unwrap().1, ARRIVED);
        ride_until_sign(&mut sim, BANNER, &["ARRIVED"], 56_400.0);
        for caption in ["DISTANCE", "RIDE TIME", "AVG KM/H", "AVG W", "AVG BPM"] {
            sim.expect_text(caption);
        }
        sim.checkpoint("arrived");

        sim.advance(seconds(60));
        sim.expect_no_text("ARRIVED");
        sim.expect_no_text("AVG W");
        sim.expect_text("KM/H");
    }

    #[test]
    fn starting_against_the_course_says_turn_around_until_riding_its_way() {
        let course = straight_then(1_000.0);
        let route = [(0.0, 3_000.0), (0.0, 2_600.0), (0.0, 3_400.0)];
        let mut sim = on_course(&course, &route);
        sim.ride_at(24.0, 180.0, 140.0, 85.0);
        sim.start_ride();
        let from = sim.device.sim().speaker.played.len();
        let at = ride_until_sign(&mut sim, BANNER, &["TURN", "AROUND"], 150.0);
        assert!(at < 80.0, "{at}");
        let heard = cues_since(&sim, from);
        assert!(
            heard.iter().all(|(cue, _)| cue == "start"),
            "turn around is silent: {heard:?}"
        );
        sim.checkpoint("start-turn-around");
        sim.tap(120, 60);
        expect_map(&sim);
        assert_eq!(map_badge(&sim), ["TURN", "AROUND"]);
        sim.checkpoint("start-turn-around-map");
        ride_until_shown(&mut sim, "FOLLOWING COURSE", 1_200.0);
        sim.expect_no_text("AROUND");
    }

    #[test]
    fn tapping_the_sign_opens_the_map_even_with_the_map_page_hidden() {
        let course = straight_then(1_000.0);
        let mut sim = on_course(&course, &course);
        hide_ride_pages(&mut sim, &["MAP"]);
        sim.ride_at(30.0, 180.0, 150.0, 85.0);
        start_following(&mut sim);
        assert_eq!(
            sim.screen().page().as_deref(),
            Some("1/5"),
            "a loaded course keeps the map in the cycle"
        );
        ride_until_sign(&mut sim, CHIP, &["6.5", "KM"], 1_000.0);
        sim.tap(120, 200);
        assert_eq!(
            sim.screen().page().as_deref(),
            Some("1/5"),
            "not on the sign"
        );
        sim.tap(60, 86);
        expect_map(&sim);
        sim.checkpoint("the chip tapped opens the map");

        sim.press(Button::BottomLeft);
        assert_eq!(sim.screen().page().as_deref(), Some("1/5"));
        ride_until_cue(&mut sim, "turn_soon", 6_900.0);
        sim.tap(120, 60);
        expect_map(&sim);
        sim.checkpoint("the banner tapped opens the map");
    }
}

mod course_map {
    use super::*;
    use device_api::server::Method;
    use vana::screens::chrome::BLACK;

    const MOCKUP: &str = include_str!("../../../docs/design/course-navigation/map-approach.svg");
    const SANTIAGO: &[u8] =
        include_bytes!("../../../packages/terrain/tests/data/santiago-centro.terrain");
    const PX_PER_M: f64 = 240.0 / 400.0;
    const RIDER: (f64, f64) = (120.0, 223.0);
    const RIDER_BESIDE_RADAR: (usize, usize) = (134, 223);
    const AHEAD_S: f64 = 40.0;
    const ROWS_AHEAD: f64 = 201.0;
    const START_TO_MOCKUP_RIDER_M: f64 = 1_000.0;
    const FIX_CATCHES_UP_S: u64 = 2;
    const SPACING_PAST_COURSE_MIN_STEP_M: f64 = 5.5;

    fn mockup_course() -> Vec<(f64, f64)> {
        let route = MOCKUP
            .split("<polyline points=\"")
            .find(|polyline| polyline.contains("stroke=\"#0a84ff\""))
            .and_then(|polyline| polyline.split('"').next())
            .expect("map-approach draws the route");
        let drawn: Vec<(f64, f64)> = route
            .split(' ')
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
                (
                    (x - RIDER.0) / PX_PER_M,
                    START_TO_MOCKUP_RIDER_M + (RIDER.1 - y) / PX_PER_M,
                )
            })
            .collect();
        let mut course = vec![(0.0, 0.0)];
        let mut stretch = vec![drawn[0]];
        for (k, &p) in drawn.iter().enumerate().skip(1) {
            stretch.push(p);
            if k + 1 == drawn.len() || turn_angle(stretch[stretch.len() - 2], p, drawn[k + 1]) > 0.5
            {
                course.extend(resampled(&stretch));
                stretch = vec![p];
            }
        }
        let &(east, north) = drawn.last().unwrap();
        course.push((east, north + 3_000.0));
        course
    }

    fn turn_angle(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
        let turn = (c.1 - b.1).atan2(c.0 - b.0) - (b.1 - a.1).atan2(b.0 - a.0);
        turn.sin().atan2(turn.cos()).abs()
    }

    fn resampled(stretch: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let lengths: Vec<f64> = stretch
            .windows(2)
            .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
            .collect();
        let total: f64 = lengths.iter().sum();
        let count = (total / SPACING_PAST_COURSE_MIN_STEP_M).floor().max(1.0);
        (1..=count as usize)
            .map(|k| {
                let mut at = total * k as f64 / count;
                for (pair, &length) in stretch.windows(2).zip(&lengths) {
                    if at <= length {
                        let t = at / length;
                        return (
                            pair[0].0 + t * (pair[1].0 - pair[0].0),
                            pair[0].1 + t * (pair[1].1 - pair[0].1),
                        );
                    }
                    at -= length;
                }
                *stretch.last().unwrap()
            })
            .collect()
    }

    fn kmh_at_px_per_m(px_per_m: f64) -> f64 {
        ROWS_AHEAD / px_per_m / AHEAD_S * 3.6
    }

    fn stand_at(sim: &mut Sim, metres: f64, kmh: f64) {
        sim.ride_at(kmh, 210.0, 150.0, 88.0);
        ride_to(sim, metres - 20.0);
        sim.ride_at(4.0, 120.0, 150.0, 60.0);
        ride_to(sim, metres - 0.6);
        sim.ride_at(0.0, 0.0, 150.0, 0.0);
        sim.advance(seconds(FIX_CATCHES_UP_S));
        sim.ride_at(kmh, 210.0, 150.0, 88.0);
    }

    const EVERY_SENSOR: [&str; 3] = ["HEART RATE", "POWER METER", "SPEED/CAD"];

    fn on_the_map(course: &[(f64, f64)], route: &[(f64, f64)]) -> Sim {
        on_the_map_with(course, route, &EVERY_SENSOR)
    }

    fn on_the_map_with(course: &[(f64, f64)], route: &[(f64, f64)], sensors: &[&str]) -> Sim {
        let (course, route) = (gpx_through("course", course), gpx_through("route", route));
        on_the_map_of(&course, &route, Some(SANTIAGO), sensors)
    }

    fn on_the_map_of(course: &str, route: &str, pack: Option<&[u8]>, sensors: &[&str]) -> Sim {
        let mut sim = Sim::new();
        sim.fit_bulk(Presence::Ready);
        let mut sim = equip(sim.boot());
        sim.init_bulk();
        sim.format_files();
        if let Some(pack) = pack {
            sim.install_terrain(b"CHILE-TERRAIN-V1", pack);
        }
        sim.connect_phone();
        let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
        assert_eq!(status, 200, "{body}");
        sim.press(Button::TopLeft);
        sim.advance(seconds(1));
        sim.ride_route(Route::from_gpx(route).unwrap());
        sim.ant_sensor(device_type::RADAR, RADAR);
        sim.pair(sensors);
        sim.ride_at(36.0, 210.0, 150.0, 88.0);
        sim.start_ride();
        ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
        sim.ride_page("5/5");
        sim
    }

    fn badge(sim: &Sim) -> Vec<String> {
        sim.screen()
            .frame()
            .read()
            .into_iter()
            .filter(|run| (30..90).contains(&run.y) && run.x < 160)
            .map(|run| run.text)
            .collect()
    }

    const SANTIAGO_COURSE: &str =
        include_str!("../../../packages/terrain/tests/data/santiago-centro-course.gpx");
    const MINOR_ROAD: u16 = vana::screens::chrome::rgb(0x2a, 0x2a, 0x2d);
    const MAJOR_ROAD: u16 = vana::screens::chrome::rgb(0x44, 0x44, 0x48);

    #[test]
    fn a_course_through_central_santiago_runs_over_its_streets_on_the_map() {
        let mut sim = on_the_map_of(
            SANTIAGO_COURSE,
            SANTIAGO_COURSE,
            Some(SANTIAGO),
            &EVERY_SENSOR,
        );
        ride_to(&mut sim, 1_200.0);
        expect_map(&sim);
        let (minor, major) = (map_pixels(&sim, MINOR_ROAD), map_pixels(&sim, MAJOR_ROAD));
        assert!(
            minor > 500 && major > 500,
            "{minor} minor and {major} major road px"
        );
        assert!(
            course_pixels(&sim) > 1_000,
            "the route is drawn over the streets"
        );
        sim.checkpoint("map-santiago");
    }

    #[test]
    fn without_a_terrain_pack_the_map_draws_the_course_on_bare_ground() {
        let mut sim = on_the_map_of(SANTIAGO_COURSE, SANTIAGO_COURSE, None, &EVERY_SENSOR);
        ride_to(&mut sim, 1_200.0);
        expect_map(&sim);
        assert_eq!(
            map_pixels(&sim, MINOR_ROAD) + map_pixels(&sim, MAJOR_ROAD),
            0
        );
        assert!(course_pixels(&sim) > 1_000);
        sim.checkpoint("map-no-pack");
    }

    #[test]
    fn the_map_rides_the_three_mockups_at_their_scales() {
        let course = mockup_course();
        let route = [(0.0, 0.0), (0.0, START_TO_MOCKUP_RIDER_M + 600.0)];
        let mut sim = on_the_map_with(&course, &route, &["HEART RATE", "POWER METER"]);
        let approach = kmh_at_px_per_m(PX_PER_M);
        stand_at(&mut sim, START_TO_MOCKUP_RIDER_M, approach);
        assert_eq!(badge(&sim), ["240", "M"], "to where the turn starts");
        assert_eq!(scale(&sim.screen()), (60, "100 M".into()));
        sim.checkpoint("map-approach");

        stand_at(
            &mut sim,
            START_TO_MOCKUP_RIDER_M + 210.0,
            kmh_at_px_per_m(0.8),
        );
        assert_eq!(badge(&sim), ["30", "M"]);
        assert_eq!(scale(&sim.screen()), (80, "100 M".into()));
        sim.checkpoint("map-close");

        sim.ride_at(approach, 210.0, 150.0, 88.0);
        let off = ride_until_cue(&mut sim, "off_course", START_TO_MOCKUP_RIDER_M + 395.0);
        assert!(off < START_TO_MOCKUP_RIDER_M + 395.0, "{off}");
        stand_at(&mut sim, START_TO_MOCKUP_RIDER_M + 390.0, approach);
        assert_eq!(badge(&sim), ["140", "M", "TURN BACK"]);
        assert_eq!(scale(&sim.screen()).1, "100 M");
        sim.checkpoint("map-offcourse");
    }

    fn at(screen: &Screen, x: usize, y: usize) -> u16 {
        screen.pixels[y * screen.width + x]
    }

    fn whole_puck_bounds(screen: &Screen, (cx, cy): (usize, usize)) -> Option<[usize; 4]> {
        let ring: Vec<(usize, usize)> = (cy - 20..cy + 20)
            .flat_map(|y| (cx - 20..cx + 20).map(move |x| (x, y)))
            .filter(|&(x, y)| at(screen, x, y) == BLACK)
            .collect();
        let (xs, ys) = (ring.iter().map(|p| p.0), ring.iter().map(|p| p.1));
        let bounds = [xs.clone().min()?, ys.clone().min()?, xs.max()?, ys.max()?];
        let whole = at(screen, cx, cy - 11) == WHITE && at(screen, cx, cy + 10) == WHITE;
        whole.then_some(bounds)
    }

    #[test]
    fn the_puck_holds_one_pixel_on_every_fix_on_course_and_off() {
        let course = [
            (0.0, 0.0),
            (0.0, 1_200.0),
            (500.0, 1_200.0),
            (500.0, 4_000.0),
        ];
        let route = [(0.0, 0.0), (0.0, 2_400.0)];
        let sensors = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];
        for radar in [false, true] {
            let paired = if radar { &sensors[..] } else { &sensors[..3] };
            let centre = if radar {
                RIDER_BESIDE_RADAR
            } else {
                (120, 223)
            };
            let mut sim = on_the_map_with(&course, &route, paired);
            let mut seen = std::collections::BTreeMap::new();
            let mut off = false;
            for (kmh, until_m) in [(40.0, 700.0), (12.0, 1_000.0), (28.0, 2_300.0)] {
                sim.ride_at(kmh, 210.0, 150.0, 88.0);
                while sim.world().distance_m < until_m {
                    sim.advance(seconds(1));
                    off |= badge(&sim).contains(&"TURN BACK".to_string());
                    let at_m = sim.world().distance_m as i64;
                    seen.entry(whole_puck_bounds(&sim.screen(), centre))
                        .or_insert(at_m);
                }
            }
            assert!(off, "radar {radar}: the ride went off course");
            assert_eq!(
                seen.len(),
                1,
                "radar {radar}: the puck at, from metres: {seen:?}"
            );
            assert!(seen.keys().all(Option::is_some), "radar {radar}: {seen:?}");
        }
    }

    #[test]
    fn beside_a_radar_the_rider_and_the_scale_stay_whole() {
        let course = [
            (0.0, 0.0),
            (0.0, 1_000.0),
            (225.0, 1_000.0),
            (225.0, 3_000.0),
        ];
        let sensors = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];
        let mut sim = on_the_map_with(&course, &course, &sensors);
        ride_to(&mut sim, 910.0);
        let screen = sim.screen();
        let puck = whole_puck_bounds(&screen, RIDER_BESIDE_RADAR).expect("the puck, whole");
        assert!(puck[0] >= 30, "its ring starts at x {}, cut", puck[0]);
        assert_eq!(
            scale(&screen),
            (50, "100 M".into()),
            "400 m ahead at 36 km/h"
        );
        assert_eq!(badge(&sim), ["90", "M"]);
        sim.checkpoint("map-beside-radar");
    }
    #[test]
    fn far_off_course_the_chevrons_still_point_back_to_the_junction() {
        let course = [
            (0.0, 0.0),
            (0.0, 1_500.0),
            (600.0, 1_500.0),
            (600.0, 5_000.0),
        ];
        let sensors = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];
        for radar in [false, true] {
            let paired = if radar { &sensors[..] } else { &sensors[..3] };
            let mut sim = on_the_map_with(&course, &[(0.0, 0.0), (0.0, 12_000.0)], paired);
            for (ridden, back) in [(3_200.0, "1.7"), (5_000.0, "3.5"), (11_500.0, "10")] {
                ride_to(&mut sim, ridden);
                assert_eq!(badge(&sim), [back, "KM", "TURN BACK"]);
                let screen = sim.screen();
                let chevrons = (100..250)
                    .flat_map(|y| (0..screen.width).map(move |x| (x, y)))
                    .filter(|&(x, y)| at(&screen, x, y) == AMBER)
                    .count();
                assert!(
                    chevrons >= 60,
                    "radar {radar}, {back} km back: {chevrons} amber px between the rider and the junction"
                );
                if !radar && back == "3.5" {
                    sim.checkpoint("map-offcourse-far");
                }
            }
        }
    }

    fn scale(screen: &Screen) -> (usize, String) {
        let bar = (0..screen.width)
            .skip_while(|&x| at(screen, x, 304) != SECONDARY)
            .take_while(|&x| at(screen, x, 304) == SECONDARY)
            .count();
        let label: Vec<String> = screen
            .frame()
            .read()
            .into_iter()
            .filter(|run| run.y >= 290 && run.x < 110)
            .map(|run| run.text)
            .collect();
        (bar, label.join(" "))
    }

    fn metres_per_px(screen: &Screen) -> f64 {
        let (px, label) = scale(screen);
        let metres: f64 = label.trim_end_matches(" M").parse().unwrap();
        metres / px as f64
    }

    #[test]
    fn the_map_shows_about_40_s_ahead_and_eases_from_speed_to_speed() {
        let course = [(0.0, 0.0), (0.0, 9_000.0)];
        let mut sim = on_the_map_with(&course, &course, &["HEART RATE", "POWER METER"]);
        for (kmh, bar) in [
            (10.0, (67, "50 M")),
            (25.0, (72, "100 M")),
            (40.0, (45, "100 M")),
        ] {
            sim.ride_at(kmh, 210.0, 150.0, 88.0);
            sim.advance(seconds(60));
            assert_eq!(scale(&sim.screen()), (bar.0, bar.1.into()), "{kmh} km/h");
        }
        sim.ride_at(10.0, 210.0, 150.0, 88.0);
        sim.advance(seconds(60));
        sim.ride_at(40.0, 210.0, 150.0, 88.0);
        let mut seen = vec![metres_per_px(&sim.screen())];
        for _ in 0..30 {
            sim.advance(seconds(1));
            seen.push(metres_per_px(&sim.screen()));
        }
        let (first, last) = (seen[0], *seen.last().unwrap());
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0] - 0.01 * first),
            "10 to 40 km/h narrows on the way: {seen:?}"
        );
        let halfway = seen
            .iter()
            .position(|&m| m >= (first + last) / 2.0)
            .unwrap();
        assert!(halfway >= 3, "half the way out in {halfway} s: {seen:?}");
        sim.ride_at(0.0, 0.0, 150.0, 0.0);
        sim.advance(seconds(30));
        sim.expect_text("AUTO PAUSED");
        sim.ride_at(40.0, 210.0, 150.0, 88.0);
        while scale(&sim.screen()).1.is_empty() {
            sim.advance(seconds(1));
        }
        assert_eq!(metres_per_px(&sim.screen()), last, "standing still held it");
    }

    #[test]
    fn the_badge_counts_the_next_turn_down_from_300_m_at_any_speed() {
        let course = [(0.0, 0.0), (0.0, 2_000.0), (400.0, 2_000.0)];
        for kmh in [10.0, 25.0, 40.0] {
            let mut sim = on_the_map(&course, &course);
            sim.ride_at(kmh, 210.0, 150.0, 88.0);
            ride_to(&mut sim, 1_500.0);
            let soon = ride_until_cue(&mut sim, "turn_soon", 1_800.0);
            let ahead = 2_000.0 - soon;
            assert!(
                (285.0..=315.0).contains(&ahead),
                "{kmh} km/h: opened {ahead} m ahead"
            );
            let mut read: Vec<u32> = vec![];
            while sim.world().distance_m < 1_950.0 {
                let shown = badge(&sim);
                assert_eq!(
                    shown.get(1).map(String::as_str),
                    Some("M"),
                    "{kmh} km/h: {shown:?}"
                );
                read.push(shown[0].parse().unwrap());
                sim.advance(seconds(1));
            }
            assert!(read[0] <= 300, "{kmh} km/h: {read:?}");
            assert!(
                read.windows(2).all(|w| w[1] <= w[0]),
                "{kmh} km/h counts down: {read:?}"
            );
        }
    }
}

#[test]
fn bare_runs_on_buttons_alone() {
    use device_api::input::InputSource;
    use device_api::observation::Availability;
    use shell_simulator::models::BARE;
    use shell_simulator::replay::first_difference;

    let mut sim = Sim::on(BARE).boot();
    let frame = sim.screen();
    assert_eq!((frame.width, frame.height), (240, 320));
    let controls = sim.device.input.controls();
    assert_eq!(controls.touch, Availability::Unsupported);
    assert_eq!(controls.buttons, BARE.buttons);
    sim.checkpoint("home on bare");

    let mut changed = 0;
    for &button in BARE.buttons {
        let before = sim.screen().text();
        sim.press(button);
        sim.advance(seconds(1));
        changed += usize::from(sim.screen().text() != before);
        sim.press(Button::TopLeft);
        sim.advance(seconds(1));
    }
    assert!(changed > 0, "no button changed the screen");

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture.trace).unwrap();
    assert_eq!(header.device, "bare");
    let replayed = Sim::replay(&capture);
    assert_eq!(replayed.device.model, BARE);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
}
