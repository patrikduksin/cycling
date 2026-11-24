//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
use device_api::ant::{LinkState, Request};
use device_api::ble_transport::Link;
use device_api::gps::FixState;
use device_api::input::Role;
use device_api::positioning::Positioning;
use firmware_services::gps::unix_seconds;
use shell_simulator::ant::{BatteryReport, ZeroReply, device_type};
use shell_simulator::bulk::Presence;
use shell_simulator::models;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Need, NoteKind, Screen, Sim, minutes, seconds};
use shell_simulator::trace::timeline::{Kind as Traced, session_clock};
use shell_simulator::trace::world::{self, World};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::fit::Refusal;
use vana::ride::log::{
    Entry, Kind, LapStart, SLOT_SIZE, Sample, Slot, commit_word, decode, encode,
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
fn power_meter_dropout_blanks_watts_and_recovers() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("PWR 3S W", "190");
    sim.checkpoint("power reading");

    // A short loss keeps the channel tracking; readings blank, then resume.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "--");
    sim.checkpoint("power meter out of range");

    sim.set_present(device_type::POWER, POWER, true);
    let waited = sim.wait_for("190", seconds(10)).unwrap();
    assert!(waited < seconds(3), "recovered after {waited} ms");
    sim.expect_field("PWR 3S W", "190");
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

    sim.start_ride();
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

/// The simulated internal journal's slots.
const SLOTS: usize = shell_simulator::storage::DATA_BYTES / SLOT_SIZE;

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
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "free_s"), "600", "{status}");
    sim.home("RIDE");
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
    // Power fails ten bytes into the very first START record.
    sim.fail_storage(0, 10);
    sim.home("RIDE");
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
    sim.expect_field("PWR 3S W", "190");

    // Beyond the companion's search timeout the channel closes.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(40));
    sim.expect_field("PWR 3S W", "--");
    sim.checkpoint("power meter gone for 40 s");

    sim.set_present(device_type::POWER, POWER, true);
    sim.wait_for("190", seconds(30)).unwrap();
    sim.expect_field("PWR 3S W", "190");
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
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    assert_eq!(corner(&sim), 0, "dark by default");
    sim.option("THEME", "LIGHT");
    sim.next_frame();
    assert_eq!(corner(&sim), WHITE, "Home turns white at once");
    let theme = vana::screens::theme::LIGHT;
    assert!(shows(&sim, theme.text), "black text on the white page");
    assert!(shows(&sim, theme.good), "the Ride button's green");
    sim.checkpoint("home in the light theme");
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.start_ride();
    sim.advance(seconds(20));
    assert_eq!(corner(&sim), WHITE);
    // The Stats page draws in the light theme's own colours.
    assert!(shows(&sim, theme.speed) && shows(&sim, theme.cadence));
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
fn a_speed_and_cadence_sensor_alone_drives_cadence_and_its_low_alert() {
    let mut sim = rider();
    // The power meter is in range but not saved: only the SPD/C crank speaks.
    sim.pair(&["SPEED/CAD"]);
    sim.option("CADENCE_LOW", "60");
    sim.ride_at(28.0, 0.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.expect_field("CAD RPM", "85");
    sim.expect_no_text("CADENCE LOW");
    sim.checkpoint("cadence from the speed and cadence sensor");
    sim.ride_at(28.0, 0.0, 0.0, 55.0);
    sim.wait_for("CADENCE LOW 55", seconds(30)).unwrap();
    sim.checkpoint("alert: cadence below the limit");
    sim.ride_at(28.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(8));
    sim.expect_field("CAD RPM", "0");
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
    sim.expect_field("HEART BPM", "--");

    // Selected: a 10 s scan, then connection and discovery.
    sim.select_ble_heart_strap();
    sim.advance(seconds(9));
    sim.expect_field("HEART BPM", "--");
    sim.wait_for_field("HEART BPM", "131", seconds(5));
    sim.checkpoint("ble heart rate");

    // The link survives a short gap; the reading goes stale, then the link drops.
    sim.ble_heart_strap(false);
    sim.advance(seconds(3));
    assert_eq!(sim.device.sim_mut().ble.snapshot.link, Link::Connected);
    sim.advance(seconds(4));
    sim.expect_field("HEART BPM", "--");
    assert_ne!(sim.device.sim_mut().ble.snapshot.link, Link::Connected);
    sim.checkpoint("strap out of range");

    // Back in range, it reconnects on the next scan after the backoff.
    sim.advance(seconds(20));
    sim.ble_heart_strap(true);
    sim.wait_for_field("HEART BPM", "131", seconds(30));

    // Forgotten, it is never used again even in range.
    let (status, _) = sim.command("BLE FORGET");
    assert_eq!(status, "ACCEPTED");
    sim.advance(seconds(20));
    sim.expect_field("HEART BPM", "--");
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

fn ride_page(sim: &mut Sim, name: &str) {
    sim.ride_page(name);
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

/// The number Ride done shows over `caption`, such as "5.0" over "distance".
fn done_stat(sim: &Sim, caption: &str) -> Option<String> {
    let runs = sim.screen().frame().read();
    let label = runs.iter().find(|run| run.text == caption)?;
    runs.iter()
        .filter(|run| run.scale > label.scale && run.x == label.x && run.y < label.y)
        .max_by_key(|run| run.y)
        .map(|run| run.text.clone())
}

#[test]
fn stats_page_totals_a_steady_ten_minutes() {
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
    sim.need(Need::Wifi);
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
fn ascent_counts_the_climbs_of_a_hilly_loop() {
    let mut sim = Sim::new().boot();
    sim.need(Need::Barometer);
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
    ride_page(&mut sim, "ride/main");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "CLIMB M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    // The final climb brought up the climb page; back to the totals.
    ride_page(&mut sim, "ride/main");
    expect_near(&mut sim, "DIST KM", 4.0, 0.05);
    // The 3 m band and smoothing may leave the last metres of the final climb.
    expect_near(&mut sim, "CLIMB M", 100.0, 5.0);
    sim.checkpoint("stats after one hilly lap");

    // The exports carry the barometric elevation: a 600 m start, up to 650 m,
    // down to 550 m, and the session's ascent and descent.
    sim.stop_ride();
    let climbed: f64 = done_stat(&sim, "climbed").unwrap().parse().unwrap();
    assert!((95.0..=105.0).contains(&climbed), "climbed {climbed} m");
    sim.checkpoint("ride done after the hilly lap");
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
    let fit = ride
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
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
}

/// The FIT session of the newest saved ride, its fields by number.
fn saved_session(sim: &mut Sim) -> std::collections::BTreeMap<u8, u64> {
    let fit = sim
        .rides()
        .last()
        .expect("a saved ride")
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
    fit_messages(&fit, 18).remove(0)
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

const ZONE_GREY: u16 = ZONES[0];
const ZONE_BLUE: u16 = ZONES[1];
const ZONE_GREEN: u16 = ZONES[2];
const ZONE_YELLOW: u16 = ZONES[3];
const ZONE_ORANGE: u16 = ZONES[4];
const ZONE_RED: u16 = ZONES[5];
const ZONE_PURPLE: u16 = ZONES[6];

/// The main page's power (first) or heart rate (second) zone label, such as
/// "Z4", and the zone colour filling its field.
#[track_caller]
fn zone_chip(sim: &Sim, heart: bool) -> (String, u16) {
    let screen = sim.screen();
    let mut zones: Vec<_> = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.smooth && run.text.len() == 2 && run.text.starts_with('Z'))
        .collect();
    zones.sort_by_key(|run| run.x);
    let run = zones
        .get(usize::from(heart))
        .unwrap_or_else(|| panic!("no zone label; screen:\n{}", screen.text()));
    // The zone fills the field; its tag is written in the fill's ink.
    let fill = screen.pixels[run.y * screen.width + run.x - 2];
    (run.text.clone(), fill)
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
        sim.press(Role::Back);
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
        sim.expect_field("PWR 3S W", &format!("{watts}"));
        expect_chip(&mut sim, false, power.0, power.1);
        expect_chip(&mut sim, true, heart.0, heart.1);
        sim.checkpoint(&format!("{watts} W in {}", power.0));
    }
    // A lost reading takes its zone away.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "--");
    let (heart, _) = zone_chip(&sim, false);
    assert_eq!(heart, "Z5", "only heart rate's zone is left");
    sim.checkpoint("no power, no zone colour");
}

#[test]
fn raising_ftp_moves_the_same_watts_down_a_zone() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 0.0, 90.0);
    sim.advance(seconds(10));
    expect_chip(&mut sim, false, "Z5", ZONE_ORANGE);
    sim.checkpoint("250 W at FTP 215");
    sim.stop_ride();
    go_home(&mut sim);

    sim.set_profile(250, 185);

    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("PWR 3S W", "250");
    expect_chip(&mut sim, false, "Z4", ZONE_YELLOW);
    sim.checkpoint("250 W at FTP 250");
}

#[test]
fn the_body_set_in_settings_survives_power_loss() {
    let mut sim = rider();
    sim.profile("WEIGHT", "75");
    sim.profile("AGE", "30");
    sim.profile("SEX", "M");
    sim.advance(seconds(5));

    sim.power_loss();
    let (_, profile) = sim.command("PROFILE");
    assert!(
        profile.contains("weight=75 age=30 sex=m stored=current"),
        "{profile}"
    );
}

#[test]
fn training_profile_survives_power_loss() {
    let mut sim = rider();
    let (_, before) = sim.command("PROFILE");
    assert_eq!(
        before, "ftp=215 lthr=185 weight=0 age=0 sex=- stored=missing persistence=saved",
        "a fresh device has the defaults and stores nothing until the first edit"
    );
    sim.set_profile(280, 170);
    sim.advance(seconds(5));

    sim.power_loss();
    let (status, after) = sim.command("PROFILE");
    assert_eq!(status, "OK");
    assert_eq!(
        after,
        "ftp=280 lthr=170 weight=0 age=0 sex=- stored=current persistence=saved"
    );

    // The terminal edits the same profile; an edit that never settled before
    // power was cut is not saved.
    let (status, reply) = sim.command("PROFILE LTHR 175");
    assert_eq!(status, "OK");
    assert_eq!(
        reply,
        "ftp=280 lthr=175 weight=0 age=0 sex=- stored=current persistence=pending"
    );
    assert_eq!(sim.command("PROFILE FTP 9000").0, "INVALID");
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=170 weight=0 age=0 sex=- stored=current persistence=saved"
    );
    sim.command("PROFILE LTHR 175");
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=0 age=0 sex=- stored=current persistence=saved"
    );
    sim.command("PROFILE WEIGHT 72");
    sim.command("PROFILE AGE 41");
    sim.command("PROFILE SEX M");
    assert_eq!(sim.command("PROFILE WEIGHT 12").0, "INVALID");
    assert_eq!(sim.command("PROFILE SEX X").0, "INVALID");
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=72 age=41 sex=m stored=current persistence=saved"
    );
    // Shell preferences saved later keep the profile.
    assert_eq!(sim.command("BRIGHTNESS 70").0, "OK");
    assert_eq!(sim.command("SAVE").0, "OK");
    sim.power_loss();
    assert!(sim.command("SETTINGS").1.contains("brightness=70"));
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=72 age=41 sex=m stored=current persistence=saved"
    );
}

#[test]
fn options_set_from_the_terminal_apply_at_once_and_survive_power_loss() {
    let mut sim = rider();
    let defaults = "auto_pause=on sounds=on auto_lap=5 wheel=2136 wheel_mode=auto hr_low=0 hr_high=0 power_low=0 power_high=0 cadence_low=0 fuel=0 radar=on sunset=on theme=dark daylight=auto display=normal brightness=auto zone=chile";
    let (status, before) = sim.command("OPTION");
    assert_eq!(status, "OK");
    assert!(before.starts_with(defaults), "{before}");
    for (name, value) in [
        ("AUTO_PAUSE", "OFF"),
        ("SOUNDS", "OFF"),
        ("AUTO_LAP", "START"),
        ("WHEEL", "2100"),
        ("HR_LOW", "100"),
        ("HR_HIGH", "170"),
        ("POWER_LOW", "150"),
        ("POWER_HIGH", "300"),
        ("CADENCE_LOW", "70"),
        ("FUEL", "20"),
        ("RADAR", "OFF"),
        ("SUNSET", "OFF"),
        ("THEME", "LIGHT"),
        ("DAYLIGHT", "NIGHT"),
        ("DISPLAY", "SAVER"),
        ("BRIGHTNESS", "60"),
        ("ZONE", "UTC-4"),
    ] {
        let (status, reply) = sim.command(&format!("OPTION {name} {value}"));
        assert_eq!(status, "OK", "OPTION {name} {value}: {reply}");
    }
    let set = "auto_pause=off sounds=off auto_lap=start wheel=2100 wheel_mode=manual hr_low=100 hr_high=170 power_low=150 power_high=300 cadence_low=70 fuel=20 radar=off sunset=off theme=light daylight=night display=saver brightness=60 zone=utc-4";
    assert_eq!(
        sim.command("OPTION").1,
        format!("{set} stored=missing persistence=pending"),
        "a fresh device stores nothing until the first edit settles"
    );
    sim.advance(100);
    let shell = sim.command("SETTINGS").1;
    assert!(shell.contains("brightness=60 auto=false"), "{shell}");
    for refused in [
        "OPTION AUTO_LAP 101",
        "OPTION WHEEL 90",
        "OPTION HR_LOW 180",
        "OPTION FUEL 25",
        "OPTION BRIGHTNESS 2",
        "OPTION ZONE UTC+20",
        "OPTION SOUNDS",
        "OPTION DECK MAP",
    ] {
        assert_eq!(sim.command(refused).0, "INVALID", "{refused}");
    }
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("OPTION").1,
        format!("{set} stored=current persistence=saved")
    );
    assert_eq!(sim.command("OPTION BRIGHTNESS AUTO").0, "OK");
    assert_eq!(sim.command("OPTION WHEEL_MODE AUTO").0, "OK");
    let reply = sim.command("OPTION").1;
    assert!(
        reply.contains("wheel=2100 wheel_mode=auto") && reply.contains("brightness=auto"),
        "{reply}"
    );
}

fn fit_laps(sim: &mut Sim) -> Vec<std::collections::BTreeMap<u8, u64>> {
    let fit = sim
        .rides()
        .pop()
        .expect("a saved ride")
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
    fit_messages(&fit, 19)
}

#[track_caller]
fn assert_near(what: &str, value: f64, expected: f64, tolerance: f64) {
    assert!(
        (value - expected).abs() <= tolerance,
        "{what}: {value}, expected {expected} within {tolerance}"
    );
}

#[test]
fn manual_laps_keep_each_effort() {
    manual_laps(&mut rider());
}

fn manual_laps(sim: &mut Sim) {
    sim.need(Need::Barometer);
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
    assert_eq!(sim.ride_state(), "recording");

    sim.ride_at(33.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(4));
    let toast = sim.lap(2);
    assert!(
        toast.starts_with("LAP 2 04:0") && (toast.ends_with(" 250W") || toast.ends_with(" 249W")),
        "{toast}"
    );

    sim.ride_at(24.0, 150.0, 130.0, 85.0);
    sim.advance(minutes(3));
    sim.stop_ride();
    // The saved ride counts the three laps, the open one included.
    assert_eq!(saved_session(sim)[&26], 3, "FIT laps");
    let laps = fit_laps(sim);
    assert_eq!(laps.len(), 3);
    for (lap, km, km_h, watts, bpm) in [(1, 2.2, 33.0, 250.0, 150.0), (2, 1.2, 24.0, 150.0, 130.0)]
    {
        let lap = &laps[lap];
        assert_near("lap km", lap[&9] as f64 / 100_000.0, km, 0.05);
        assert_near("lap km/h", lap[&13] as f64 * 0.0036, km_h, 0.2);
        assert_near("lap W", lap[&19] as f64, watts, 1.0);
        assert_near("lap bpm", lap[&15] as f64, bpm, 1.0);
    }
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
    sim.wait_for("LAP 1 10:", seconds(30)).unwrap();
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

/// A 2 km loop ridden at 30 km/h with AUTO LAP set to AT START; `jitter_m`
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

    sim.wait_for("LAP 3 04:", seconds(300)).unwrap();
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
        sim.press(Role::Back);
        sim.expect_no_text("End ride?");
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");

    // Paused, the whole screen says so; TopLeft asks, BACK keeps riding.
    sim.pause_ride();
    sim.expect_text("Ride time");
    sim.checkpoint("paused");
    sim.press(Role::Back);
    sim.expect_text("End ride?");
    assert_eq!(sim.vana_page(), "end_ride");
    sim.checkpoint("end ride?");
    // The question waits; nothing ends the ride but an answer.
    sim.advance(seconds(10));
    sim.expect_text("End ride?");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=paused"), "{status}");
    sim.press(Role::Back);
    sim.expect_text("Paused");
    sim.expect_no_text("End ride?");
    // Asked again and answered with ACT: saved, then Ride done.
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.wait_for("Ride saved", seconds(5)).unwrap();
    assert_eq!(sim.vana_page(), "ride_done");
    sim.checkpoint("ride done");
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
    sim.press(Role::Act);
    assert_eq!(sim.ride_state(), "paused");
    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.expect_no_text("LAP 1 ");
    sim.checkpoint("end ride asked over a lap toast");
    sim.press(Role::Act);
    sim.wait_for("Ride saved", seconds(5)).unwrap();

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
    let mut sim = hill(Sim::new());
    sim.need(Need::Barometer);
    sim
}

/// `sim`, booted, with the hill route and its sensors paired.
fn hill(sim: Sim) -> Sim {
    let mut sim = sim.boot();
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

/// Ride on until the trace sees the ride page `page`, returning where along the route.
#[track_caller]
fn ride_until_page(sim: &mut Sim, page: &str, before_m: f64) -> f64 {
    while sim.ride_page_name().as_deref() != Some(page) {
        assert!(
            sim.world().distance_m < before_m,
            "no page {page} by {before_m} m; screen:\n{}",
            sim.screen().text()
        );
        sim.advance(seconds(1));
    }
    sim.next_frame();
    let at = sim.world().distance_m;
    sim.note(NoteKind::Check, format!("page {page} at {at:.0} m"));
    at
}

/// The climb page's grade: its largest percentage.
#[track_caller]
fn expect_grade(sim: &mut Sim, expected: f64, tolerance: f64) {
    sim.need(Need::Barometer);
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
    ride_page(&mut sim, "ride/climb");
    ride_to(&mut sim, 800.0);
    sim.expect_text("ALTITUDE M");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    sim.expect_field("ASCENT M", "0");
    expect_grade(&mut sim, 0.0, 0.5);
    sim.expect_text("180");
    sim.checkpoint("climb page on the flat");

    // Up at 14 km/h. 20 m of gain confirms the climb about 330 m in.
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1200.0);
    sim.expect_text("ALTITUDE M");
    let confirmed = ride_until_shown(&mut sim, "CLIMB DIST KM", 1600.0);
    assert!(
        (1300.0..1450.0).contains(&confirmed),
        "climb confirmed at {confirmed:.0} m"
    );

    ride_to(&mut sim, 1750.0);
    expect_grade(&mut sim, 6.0, 0.5);
    expect_near(&mut sim, "CLIMB ELEV M", 45.0, 4.0);
    expect_near(&mut sim, "CLIMB DIST KM", 0.75, 0.1);
    sim.expect_text("260");
    sim.checkpoint("climb page mid-climb");

    ride_to(&mut sim, 2510.0);
    sim.expect_text("CLIMB DIST KM");
    expect_near(&mut sim, "CLIMB ELEV M", 90.0, 4.0);
    expect_near(&mut sim, "CLIMB DIST KM", 1.5, 0.1);
    sim.checkpoint("climb page at the top");

    // Over the top at 40 km/h the climb ends within about 200 m.
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let ended = ride_until_shown(&mut sim, "ALTITUDE M", 2800.0);
    assert!(ended > 2500.0, "climb ended at {ended:.0} m");
    ride_to(&mut sim, 4300.0);
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(20));
    sim.expect_text("ALTITUDE M");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    expect_near(&mut sim, "ASCENT M", 90.0, 4.0);
    sim.checkpoint("climb page after the descent");
}

#[test]
fn in_saver_the_climb_page_comes_up_already_climbing() {
    let mut sim = hill_rider();
    sim.option("DISPLAY", "SAVER");
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 500.0);
    sim.pause_ride();
    sim.advance(WALL_AND_RIDE_SECONDS_APART_MS);
    sim.resume_ride();
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    while sim.ride_page_name().as_deref() != Some("ride/climb") {
        assert!(
            sim.world().distance_m < 1600.0,
            "no climb page; screen:\n{}",
            sim.screen().text()
        );
        sim.advance(50);
    }
    sim.next_frame();
    sim.expect_text("CLIMB DIST KM");
    sim.checkpoint("saver: climb page brought up by the climb");
}

#[test]
fn a_climb_brings_up_the_climb_page_and_hands_back_the_main_page() {
    let mut sim = hill_rider();
    sim.need(Need::Barometer);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    let shown = ride_until_page(&mut sim, "ride/climb", 1600.0);
    assert!((1300.0..1450.0).contains(&shown), "at {shown:.0} m");
    sim.expect_text("CLIMB DIST KM");
    sim.checkpoint("climb page brought up by the climb");

    ride_to(&mut sim, 2500.0);
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let back = ride_until_page(&mut sim, "ride/main", 2800.0);
    assert!(back > 2500.0, "back at {back:.0} m");
    expect_near(&mut sim, "SPD KM/H", 40.0, 0.3);
    sim.checkpoint("main page again over the top");
}

#[test]
fn a_silent_barometer_mid_climb_is_a_gap_not_a_flat_road() {
    let mut sim = hill_rider();
    sim.need(Need::Barometer);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_until_shown(&mut sim, "CLIMB DIST KM", 1600.0);
    ride_to(&mut sim, 1600.0);

    // 400 m of climbing without pressure readings: the climb page stays.
    sim.set_barometer(false);
    while sim.world().distance_m < 2000.0 {
        sim.advance(seconds(1));
        assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
        sim.expect_text("CLIMB DIST KM");
    }
    sim.checkpoint("climbing through a barometer gap");
    sim.set_barometer(true);
    ride_to(&mut sim, 2400.0);
    sim.expect_text("CLIMB DIST KM");
    expect_near(&mut sim, "CLIMB ELEV M", 84.0, 5.0);
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

#[track_caller]
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

    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after reboot");
    sim.start_ride();
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "205");
    sim.expect_field("HEART BPM", "138");
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
    sim.expect_field("HEART BPM", "131");
    sim.checkpoint("late strap reading");
}

#[test]
fn disconnect_is_remembered_across_reboot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.home("SENSORS");
    sim.disconnect_sensor("HEART RATE");
    assert_eq!(
        sim.device
            .sdk
            .saved_sensors()
            .count_of(device_type::HEART_RATE),
        0
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
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home without the removed sensor");
}

fn power_meter_on_channel(sim: &Sim) -> Option<u16> {
    let channel = shown_channel(sim, device_type::POWER)?;
    (channel.link == LinkState::Connected).then_some(channel.selected?.device_number)
}

fn shown_channel(sim: &Sim, kind: u8) -> Option<device_api::ant::Snapshot> {
    use device_api::ant::Ant;
    sim.device.sim().ant.channel(kind, sim.device.now)
}

#[test]
fn two_power_meters_stay_saved_and_the_one_in_range_takes_the_channel() {
    let mut sim = rider();
    const SPARE: u16 = 5151;
    sim.ant_sensor(device_type::POWER, SPARE);
    sim.home("SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("POWER METER");
    sim.connect_sensor("POWER METER");
    sim.expect_text("2 ADDED");
    sim.checkpoint("two power meters added");
    let saved = sim.device.sdk.saved_sensors();
    assert_eq!(saved.count_of(device_type::POWER), 2);
    let in_use = saved.active(device_type::POWER).unwrap().1.device_number;
    let other = if in_use == POWER { SPARE } else { POWER };
    go_home(&mut sim);
    let mut waited = 0;
    while power_meter_on_channel(&sim) != Some(in_use) {
        assert!(
            waited < seconds(30),
            "{:?}",
            shown_channel(&sim, device_type::POWER)
        );
        sim.advance(100);
        waited += 100;
    }

    let scans = |sim: &Sim| {
        let log = &sim.device.sim().ant.log;
        log.iter()
            .filter(|(_, request)| matches!(request, Request::Scan { .. }))
            .count()
    };
    let searched = scans(&sim);
    sim.set_present(device_type::POWER, in_use, false);
    let mut waited = 0;
    while power_meter_on_channel(&sim) != Some(other) {
        assert!(
            waited < seconds(90),
            "{:?}",
            shown_channel(&sim, device_type::POWER)
        );
        sim.advance(100);
        waited += 100;
    }
    let saved = sim.device.sdk.saved_sensors();
    assert_eq!(saved.count_of(device_type::POWER), 2);
    assert_eq!(
        saved.active(device_type::POWER).unwrap().1.device_number,
        other
    );
    assert_eq!(
        scans(&sim),
        searched + 1,
        "one background discovery chose it"
    );
    sim.checkpoint("the other bike's power meter in use");

    sim.power_loss();
    wait_connected(&mut sim, device_type::POWER, seconds(15));
    assert_eq!(power_meter_on_channel(&sim), Some(other));
}

#[test]
fn saved_sensors_keep_the_training_profile() {
    let mut sim = rider();
    sim.set_profile(250, 175);
    sim.advance(seconds(5));
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(1));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=250 lthr=175 weight=0 age=0 sex=- stored=current persistence=saved"
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
    let latency = sim.wait_for("Auto paused", seconds(15)).unwrap();
    assert!(latency <= seconds(5), "auto-pause took {latency} ms");
    println!("auto-pause latency {latency} ms");
    let paused = sim.screen().run("Auto paused").expect("Auto paused");
    assert_eq!(paused.color, AMBER);
    sim.expect_text("Ride time");
    sim.expect_text("Ride on to resume");
    sim.checkpoint("auto paused at the light");
    sim.advance(seconds(60));
    sim.expect_text("Auto paused");
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
    sim.advance(seconds(5));
    assert!(sim.rides().is_empty());
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("waiting at standstill on Home");
    sim.ride_at(20.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO START", seconds(15)).unwrap();
    assert!(recording(&mut sim));
    sim.checkpoint("auto started");
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
fn rolling_off_with_the_sheet_open_starts_nothing_until_it_closes() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.swipe((120, 4), (120, 200));
    sim.expect_text("Sensors");
    sim.ride_at(25.0, 0.0, 140.0, 80.0);
    sim.advance(seconds(30));
    assert_eq!(sim.vana_page(), "home");
    assert_eq!(sim.ride_state(), "ready");
    // The first press may only wake the dimmed screen.
    for _ in 0..2 {
        if sim.screen().contains("Sensors") {
            sim.press(Role::Back);
        }
    }
    for _ in 0..30 {
        if sim.ride_state() == "recording" {
            break;
        }
        sim.advance(seconds(1));
    }
    assert_eq!(sim.vana_page(), "ride", "{}", sim.screen().text());
}

#[test]
fn riding_with_gps_only_shows_speed_and_distance() {
    for doppler in [true, false] {
        let mut sim = gps_rider(doppler);
        sim.ride_at(25.0, 0.0, 140.0, 0.0);
        for _ in 0..30 {
            if sim.ride_state() == "recording" {
                break;
            }
            sim.advance(seconds(1));
        }
        assert_eq!(sim.vana_page(), "ride", "{}", sim.screen().text());
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
fn gps_only_rider_rolling_off_starts_the_ride() {
    let mut sim = gps_rider(true);
    sim.advance(seconds(5));
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
    sim.checkpoint("power-on: home ready");
    assert!(home - lit <= 500, "brand flash {} ms", home - lit);
    assert!(home <= seconds(5), "Home at {home} ms uptime");
    assert!(sim.on_home(), "{}", sim.screen().text());
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
    sim.press(Role::Back);
    sim.expect_text("TURN OFF");
    sim.checkpoint("turn off sheet");
    // BACK keeps it on.
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.press(Role::Back);
    sim.report(Role::Act);
    sim.advance(80);
    sim.expect_text("TURNING OFF");
    sim.checkpoint("turning off");
    sim.advance(seconds(1));
    assert!(sim.is_off(), "device still on");
    sim.checkpoint("off");
    // Presses other than the power-on hold do nothing while off.
    sim.press(Role::Act);
    sim.press(Role::Back);
    assert!(sim.is_off());
    sim.advance(minutes(1));
    let usable = sim.turn_on();
    assert!(usable <= seconds(5), "Home {usable} ms after turning on");
    sim.checkpoint("Home after turning on");

    // Separate top-left taps never add up to a hold.
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    for _ in 0..shell_simulator::sim::POWER_ON_REPORTS * 2 {
        sim.press(Role::Back);
    }
    assert!(sim.is_off(), "taps turned the device on");

    // Holding the top-left button, as the live window does, also turns it on.
    sim.hold(Role::Back, seconds(2));
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
    assert!(sim.on_home(), "{}", sim.screen().text());

    sim.press(Role::Back);
    sim.report(Role::Act);
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
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn turned_off_on_usb_power_the_device_starts_again_in_charging_standby() {
    let mut sim = hashed().boot();
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    let ridden = sim.device.boot;
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    assert!(sim.on_home(), "{}", sim.screen().text());
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
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    let capture = pulled;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
    assert!(
        replayed.frame_hashes() == sim.frame_hashes(),
        "the same frames"
    );
    let faithful = report.faithful.expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{faithful}");
    let samples: Vec<usize> = (sim.rides().iter())
        .map(|ride| ride.samples().count())
        .collect();
    assert_eq!(samples, [61], "the ride was saved whole");
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
fn a_fast_drain_during_a_ride_warns_once_that_the_battery_may_not_last() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.set_battery(82, false);
    // A ride drains at 40 %/h, a point every 90 seconds.
    sim.start_ride();
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
    sim.press(Role::Back);
    sim.expect_no_text("TURN OFF");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    // Paused, TopLeft only asks to end the ride; BACK answers no.
    sim.pause_ride();
    sim.press(Role::Back);
    sim.expect_no_text("TURN OFF");
    sim.expect_text("End ride?");
    sim.press(Role::Back);
    sim.resume_ride();
    sim.stop_ride();
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    assert!(sim.on_home(), "{}", sim.screen().text());

    sim.set_battery(60, false);
    sim.press(Role::Move);
    sim.advance(AUTO_OFF_MS - AUTO_OFF_WARNING_MS + seconds(2));
    sim.expect_text("PRESS ANY BUTTON");
    sim.checkpoint("idle off warning");
    // A press keeps it on; the first press may only wake a dimmed screen.
    sim.press(Role::Move);
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
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 225");
    sim.expect_text("CHANGES NOT");
    // The background retry saves it.
    sim.advance(seconds(10));
    assert!(sim.command("PROFILE").1.contains("persistence=saved"));

    // A new edit whose turn-off save fails is reported again, not dropped.
    sim.command("PROFILE FTP 250");
    sim.fail_storage(0, 0);
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 250");
    sim.expect_text("CHANGES NOT");
    // Turning off again now saves it if it can.
    sim.advance(seconds(5));
    sim.press(Role::Back);
    sim.press(Role::Act);
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
    let (_, status) = sim.command("RIDE STATUS");
    assert!(
        status.contains("medium=internal") && status.contains("fallback=1"),
        "{status}"
    );
    assert_eq!(room_hours(&status), 4, "{status}");
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    assert!(bulk.contains("state=needs_init"), "{bulk}");
    assert_eq!(sim.device.bulk.writes(), 0, "never formatted implicitly");

    sim.init_bulk();
    // The volume table and ten log metadata sectors (superblocks and ring),
    // not the 3 GB region.
    assert_eq!(sim.device.bulk.writes(), 12);
    sim.advance(seconds(1));
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("medium=bulk"), "{status}");
    assert_eq!(room_hours(&status), 2330, "{status}");
}

fn room_hours(status: &str) -> u64 {
    status_field(status, "free_s").parse::<u64>().unwrap() / 3600
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
    assert_session_replays_faithfully(&sim);
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
    sim.press(Role::Act);
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
    sim.press(Role::Act);
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
    sim.press(Role::Act);
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
    let (_, status) = sim.command("RIDE STATUS");
    let room: u64 = status_field(&status, "free_s").parse().unwrap();
    assert!(room < 60, "{status}");
    sim.home("RIDE");
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
    let active_ms: u64 = status_field(&status, "active_ms").parse().unwrap();
    assert!((20_000..24_000).contains(&active_ms), "{status}");

    // The next ride goes to internal flash, and the screen says so.
    sim.done();
    sim.home("RIDE");
    sim.wait_for("RECORDING INT", seconds(3)).unwrap();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
    sim.stop_ride();

    // After a reboot nothing is shown from the MMC ride: rides go internal.
    sim.power_loss();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
    assert_eq!(status_field(&status, "fallback"), "1", "{status}");
    assert_eq!(room_hours(&status), 4, "{status}");
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
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "internal", "{status}");
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
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
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
    // Ride done shows the saved ride; Home stays put for the next one.
    sim.done();
    sim.advance(seconds(2));
    sim.init_bulk();
    // The MMC's first write, the START record, fails.
    sim.fail_bulk(0, 0);
    let (result, _) = sim.command("RIDE START");
    assert_eq!(result, "ACCEPTED");
    sim.advance(seconds(2));
    sim.expect_no_text("Ride saved");
    sim.checkpoint("the MMC fails its first START");
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "medium"), "bulk", "{status}");
    assert_eq!(status_field(&status, "state"), "error", "{status}");
}

#[test]
fn home_shows_the_time_the_battery_and_the_sensors_at_a_glance() {
    let mut sim = rider();
    sim.set_battery(14, false);
    sim.advance(seconds(2));
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.expect_text("Ride");
    sim.expect_text("14%");
    sim.expect_text("Wednesday 14 Oct");
    sim.expect_no_text("Just ride");
    sim.checkpoint("home before pairing");

    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.checkpoint("home with three live sensors");

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.stop_ride();
    for _ in 0..3 {
        if sim.on_home() {
            break;
        }
        sim.press(Role::Back);
    }
    sim.wait_gone("RIDE SAVED", seconds(3)).unwrap();
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after a ride");
}

#[test]
fn tapping_home_s_widget_and_a_sensor_pairs_it_and_ride_starts() {
    let mut sim = rider();
    sim.tap(40, 125);
    sim.expect_text("NEARBY");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_no_text("NEXT");
    sim.checkpoint("sensors menu");
    sim.tap(120, 118);
    sim.expect_text("1 ADDED");
    sim.expect_text("POWER");
    sim.checkpoint("sensor connected by tap");
    sim.press(Role::Back);
    sim.wait_for("LIVE", seconds(15)).unwrap();
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after tapping a sensor");
    sim.tap(120, 273);
    sim.advance(seconds(1));
    assert_eq!(sim.vana_page(), "ride");
    assert_eq!(sim.ride_state(), "recording");
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
    sim.ride_page("ride/map");
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
    let paused = sim.screen().run("Paused").expect("Paused");
    assert_eq!((paused.y > 24, paused.color), (true, AMBER));
    sim.checkpoint("paused screen");
    sim.show_ride_pages();
    let paused = sim
        .screen()
        .run("PAUSED")
        .expect("PAUSED in the status bar");
    assert_eq!((paused.y < 24, paused.color), (true, AMBER));
    sim.checkpoint("paused, ride pages");

    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.checkpoint("end ride?");
    sim.press(Role::Act);
    sim.wait_for("Ride saved", seconds(5)).unwrap();
    sim.checkpoint("ride done");
    assert!(sim.screen().run("REC").is_none());
}

#[test]
fn swipes_page_through_the_ride_and_taps_never_control_it() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(2));
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));

    sim.swipe_next();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
    sim.checkpoint("swiped to climb");
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/map"), "wraps");
    expect_map(&sim);
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("KM/H");

    // Taps where the buttons are, and on readings, change nothing.
    for (x, y) in [(20, 300), (220, 300), (20, 20), (120, 80), (120, 150)] {
        sim.tap(x, y);
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    sim.expect_no_text("End ride?");
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
    sim.press(Role::Back);
    sim.advance(seconds(3));
}

#[test]
fn holding_move_on_the_workout_page_extends_the_step_without_a_mark() {
    let mut sim = training_rider();
    sim.start_workout("VO2 MAX 5X3");
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
    sim.start_workout("VO2 MAX 5X3");
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
fn a_thirty_thirty_runs() {
    let mut sim = training_rider();
    sim.start_workout("VO2 MAX 30-30S");
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
    sim.start_workout("FTP TEST 20 MIN");
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
    sim.press(Role::Act);
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
    sim.start_workout("FTP RAMP TEST");
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
    sim.wait_for("NEW FTP", seconds(2)).unwrap();
    sim.expect_text("221");
    let beeps = beeps_since(&sim, before);
    assert!(beeps.contains(&10), "the step cue: {beeps:?}");
    sim.checkpoint("ramp result offered");
    assert_eq!(sim.workout()["done"], "1", "the workout ends with the ramp");
    sim.press(Role::Back);
    sim.expect_text("FTP KEPT 215 W");
    assert_eq!(
        sim.ride_state(),
        "recording",
        "BACK on the sheet neither laps nor stops"
    );
    let (_, profile) = sim.command("PROFILE");
    assert!(profile.starts_with("ftp=215 "), "{profile}");
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/main"),
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
    // The host puts the workout on FILES; listing the workouts finds it.
    sim.put_file("workouts/over-unders.vw", &bytes);
    let (_, list) = sim.command("WORKOUT LIST");
    assert!(
        list.contains("uploads=1 skipped=1 upload0=workouts/over-unders.vw:OVER_UNDERS"),
        "{list}"
    );
    sim.start_workout("OVER UNDERS");
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

#[test]
fn the_ride_lifecycle_walks_from_home_to_ride_done() {
    let mut sim = rider();
    sim.set_battery(80, false);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);

    let chimes = sim.device.sim_mut().speaker.played.len();
    sim.home("RIDE");
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
    sim.expect_text("Paused");
    sim.expect_text("for 00:12");
    sim.checkpoint("paused screen");
    // A swipe shows the ride pages under PAUSED, MOVE pages them, and past
    // the last page the paused screen comes back.
    sim.show_ride_pages();
    sim.checkpoint("paused, ride pages");
    for _ in 0..8 {
        if sim.screen().contains("Ride time") {
            break;
        }
        sim.press(Role::Move);
    }
    sim.expect_text("Ride time");

    // BACK asks; BACK again keeps the ride.
    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.checkpoint("end ride?");
    sim.press(Role::Back);
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

    // Now end it: Ride done, and the saved ride keeps its totals.
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "ride_done");
    sim.expect_text("Ride saved");
    sim.checkpoint("ride done");
    let session = saved_session(&mut sim);
    // The second before 220 W was set rides at the lead-in power; where it
    // falls in the sampling second moves the average by a watt.
    assert!(
        (219..=220).contains(&session[&20]),
        "FIT avg power {session:?}"
    );
    assert!(session[&34].abs_diff(220) <= 1, "FIT NP {session:?}");
    assert_eq!(session[&26], 2, "FIT laps");
    sim.done();
    sim.checkpoint("home after the ride");
}

#[test]
fn rolling_off_from_home_starts_and_stopping_auto_pauses() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_for("AUTO START", seconds(10)).unwrap();
    assert_eq!(sim.vana_page(), "ride");
    sim.checkpoint("auto start from home");
    sim.advance(minutes(1));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("Auto paused", seconds(15)).unwrap();
    sim.expect_text("Ride on to resume");
    sim.checkpoint("auto paused screen");
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_gone("Auto paused", seconds(15)).unwrap();
    sim.expect_no_text("Ride time");
    assert!(recording(&mut sim));
    sim.checkpoint("auto resumed onto the ride pages");
}

/// Riding on from Ride done starts a new free ride by itself.
#[test]
fn riding_on_from_ride_done_starts_a_new_ride() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_for("AUTO START", seconds(10)).unwrap();
    sim.advance(minutes(1));
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "ride_done");
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    sim.wait_for("AUTO START", seconds(15)).unwrap();
    assert_eq!(sim.vana_page(), "ride");
    assert!(recording(&mut sim));
    sim.checkpoint("riding on from ride done starts a ride");
}

/// A ride started from the host while Ride done shows takes the rider to
/// its controls, so the buttons can pause and end it.
#[test]
fn a_ride_started_remotely_from_ride_done_shows_its_controls() {
    let mut sim = rider();
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "ride_done");
    assert_eq!(sim.command("RIDE START").0, "ACCEPTED");
    sim.advance(seconds(2));
    assert_eq!(sim.vana_page(), "ride");
    sim.pause_ride();
    sim.stop_ride();
    assert_eq!(sim.rides().len(), 2);
    assert_eq!(sim.vana_page(), "ride_done");
}

/// The patterns played since the `from`th.
fn played_since(sim: &Sim, from: usize) -> Vec<u8> {
    let played = &sim.device.sim().speaker.played;
    played[from..].iter().map(|(_, id)| *id).collect()
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
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.hold(Role::Move, 1000);
    let page = sim.ride_page_name().expect("a page");
    assert_eq!(page, "ride/climb", "one page on, not two");
    sim.advance(seconds(3));
    assert!(marks(sim).is_empty(), "a hold of a second marks nothing");
    sim.expect_no_text("MARK");

    let pressed = sim.device.clock_ms();
    let from = sim.device.sim().speaker.played.len();
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(
        sim.ride_page_name().as_ref(),
        Some(&page),
        "the page put back"
    );
    assert_eq!(
        marks(sim),
        [format!("1 pressed {} on {page}", session_clock(pressed))]
    );
    sim.checkpoint("marked");
    sim.advance(seconds(3));
    assert_eq!(played_since(sim, from), [0, 0, 0], "middle, middle, middle");
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 2");
    assert_eq!(sim.ride_page_name(), Some(page), "the page put back");
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
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.hold(Role::Move, 1400);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/climb"),
        "one page on, not two"
    );
    sim.checkpoint("held-move-one-page");
    sim.advance(seconds(1));
    sim.press(Role::Move);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/map"),
        "a new press still pages"
    );
}

#[test]
fn holding_move_on_home_marks_and_keeps_the_focus() {
    let mut sim = rider();
    sim.advance(seconds(3));
    assert!(sim.on_home(), "{}", sim.screen().text());
    let ring = |sim: &Sim| sim.screen().pixels[232 * 240 + 120];
    let focus = vana::screens::theme::DARK.focus;
    assert_eq!(ring(&sim), focus);
    let pressed = sim.device.clock_ms();
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(ring(&sim), focus, "Ride keeps the focus");
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
    sim.hold_while(Role::Move, 1600, |sim, held| {
        if held == 500 {
            sim.report(Role::Act);
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
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert!(auto_paused(&mut sim));
    assert_eq!(sim.seen().page.expect("a page").1, "ride/paused");
}

fn climbing() -> Sim {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_page("ride/map");
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1200.0);
    sim
}

#[test]
fn the_climb_page_coming_up_during_a_mark_stays() {
    let on_climb = |sim: &mut Sim| sim.ride_page_name().as_deref() == Some("ride/climb");
    let shown = ms_until(climbing, on_climb);
    let mut sim = climbing();
    sim.advance(shown - 700);
    assert!(!on_climb(&mut sim), "{:?}", sim.ride_page_name());
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    let pages = traced(&sim, Traced::Page);
    let climb = pages.iter().rposition(|page| page == "ride/climb");
    assert_eq!(
        &pages[climb.expect("the climb page came up")..],
        ["ride/climb"],
        "nothing took the climb page away: {pages:?}"
    );
    ride_to(&mut sim, 4500.0);
    assert_eq!(
        sim.seen().page.expect("a page").1,
        "ride/map",
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
    sim.need(Need::Barometer);
    sim.start_ride();
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    sim.advance(seconds(5));
    sim.ride_page("ride/map");
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
    sim.hold(Role::Move, 1600);
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
    sim.hold(Role::Move, 1600);
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
    sim.hold_while(Role::Move, 1600, |sim, held| {
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
    sim.press(Role::Move);
    assert!(!sim.screen().white("RIDE"), "{}", sim.screen().text());
    sim.advance(seconds(5));
    sim.press(Role::Back);
    sim.expect_text("TURN OFF");
    sim.hold(Role::Move, 1600);
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

#[test]
fn an_unsaved_brightness_change_is_reported_before_turning_off() {
    let mut sim = rider();
    sim.option("BRIGHTNESS", "25");
    sim.advance(seconds(5));
    sim.fail_storage(0, 0);
    sim.option("BRIGHTNESS", "75");
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost the brightness");
    sim.expect_text("CHANGES NOT");
    sim.checkpoint("brightness not saved, still on");
    // The background retry stores it, and it survives a power loss.
    sim.advance(seconds(10));
    sim.power_loss();
    assert_eq!(sim.device.shell.settings.brightness, 75);
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

    sim.option("DAYLIGHT", "DAY");
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
    // The automatic theme is light by day and dark by night.
    sim.option("THEME", "AUTO");
    sim.next_frame();
    assert_eq!(corner(&sim), WHITE, "day");
    // A ride keeps the screen awake.
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.set_gps_fix(false);
    sim.advance(minutes(38));
    sim.stop_ride();
    go_home(&mut sim);
    assert_eq!(corner(&sim), 0, "night");
    sim.checkpoint("night without a fix");
}

/// Receiver UTC in ms at simulated time `t_ms`, for a receiver left on its
/// default clock: 10:00 UTC on 14 October 2026 at t=0.
fn receiver_utc_ms(t_ms: u64) -> u64 {
    unix_seconds(*b"141026", *b"100000").unwrap() as u64 * 1000 + t_ms
}

fn cyclist(sim: &mut Sim) {
    sim.pair(&["SPEED/CAD"]);
}

/// Press Ride on Home, then ride at speed; the simulated time of the press.
fn press_start(sim: &mut Sim) -> u64 {
    let pressed = sim.now();
    sim.start_ride();
    sim.ride_at(25.0, 150.0, 130.0, 80.0);
    pressed
}

/// The ride's start, as the FIT export counts it back from the first dated sample.
fn fit_start_ms(ride: &shell_simulator::rides::Ride) -> u64 {
    ride.fit_plan(&models::C606.product)
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
    assert_eq!(
        ride.fit_plan(&models::C606.product).err(),
        Some(Refusal::NoClock)
    );
}

fn clock_reads(date: (u8, u8, u8), local: &str, day: &str) -> Sim {
    let mut sim = rider();
    sim.set_gps_clock(16 * 3600, date);
    sim.advance(seconds(5));
    sim.expect_text(local);
    sim.checkpoint(&format!("home at 16:00 UTC, {day}"));
    sim
}

/// Santiago keeps UTC-4 in winter. A rider in Magallanes or Aysén picks
/// UTC-3, which holds all year.
#[test]
fn the_clock_reads_santiago_winter_time_in_june() {
    let mut sim = clock_reads((21, 6, 26), "12:00", "JUN 21");
    sim.option("ZONE", "UTC-3");
    sim.advance(seconds(1));
    assert!(
        sim.screen().contains("1:00") || sim.screen().contains("1:01"),
        "{}",
        sim.screen().text()
    );
    sim.checkpoint("time zone fixed at utc-3");
}

/// Santiago keeps UTC-3 in summer.
#[test]
fn the_clock_reads_santiago_summer_time_in_december() {
    clock_reads((21, 12, 26), "1:00", "DEC 21");
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

    // The library lists it; choosing it and starting a ride plays it.
    sim.start_workout("FROM THE PHONE");
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
    sim.ride_page("ride/map");
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
    sim.wait_for("Auto paused", seconds(20)).unwrap();
    sim.swipe_next();
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
    sim.option("AUTO_LAP", "START");
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.start_ride();
    ride_to(&mut sim, 4_500.0);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("ride/map");
    let before = ridden_track_pixels(&sim);
    sim.checkpoint("the map before the power cut");

    sim.power_loss();
    let (_, status) = sim.command("RIDE STATUS");
    assert_eq!(status_field(&status, "state"), "recovered", "{status}");
    sim.start_ride();
    sim.advance(seconds(20));
    sim.ride_page("ride/map");
    let after = ridden_track_pixels(&sim);
    sim.note(
        NoteKind::Check,
        format!("map ink {before} px before, {after} px after"),
    );
    assert!(after * 10 >= before * 9, "{after} px against {before} px");
    sim.checkpoint("the map after the restart, the whole track");

    // On round the loop: the ride's start is where the lap closes.
    sim.ride_page("ride/main");
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
    sim.ride_page("ride/map");
    let before = ridden_track_pixels(&sim);
    sim.checkpoint("the map after five hours");

    sim.power_loss();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert!(status.contains("state=recovered"), "{status}");
    sim.home("RIDE");
    sim.press(Role::Act);
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.ride_page("ride/map");
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
    assert_session_replays_faithfully(&sim);
}

#[test]
fn a_ride_that_moves_to_internal_memory_keeps_the_track_and_the_start() {
    let mut sim = bulk_rider();
    sim.init_bulk();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_route(Route::loop_around(Point::new(-33.45, -70.66), 3_000.0, 0.0));
    sim.option("AUTO_LAP", "START");
    sim.ride_at(30.0, 220.0, 0.0, 90.0);
    sim.start_ride();
    ride_to(&mut sim, 4_500.0);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(20));
    sim.ride_page("ride/map");
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
    sim.ride_page("ride/map");
    let after = ridden_track_pixels(&sim);
    sim.note(
        NoteKind::Check,
        format!("map ink {before} px before the move, {after} px after"),
    );
    sim.checkpoint("the map after the move, the whole track");
    assert!(after * 10 >= before * 9, "{after} px against {before} px");

    // On round the loop: the ride's start is where the lap closes.
    sim.ride_page("ride/main");
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
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/map"));
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
    assert!(
        (played.len()..=played.len() + 1).contains(&sounds.len()),
        "the trace has every note the speaker played: {sounds:?} {played:?}"
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
    sim.disconnect_phone();
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
    sim.ride_page("ride/map");
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

#[test]
fn an_auto_pause_before_following_keeps_the_direction_in_sight() {
    let course = gpx_through("straight", &[(0.0, 0.0), (0.0, 7_000.0)]);
    let mut sim = with_a_course(&course);
    let wrong_way = gpx_through("wrong way", &[(0.0, 3_000.0), (0.0, 2_000.0)]);
    sim.ride_route(Route::from_gpx(&wrong_way).unwrap());
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(24.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "AROUND", 150.0);
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(15)).unwrap();
    sim.advance(seconds(10));
    assert_eq!(sim.ride_state(), "paused");
    let pages = traced(&sim, Traced::Page);
    assert!(
        !pages.iter().any(|page| page == "ride/paused"),
        "not even for the passes before the next frame: {pages:?}"
    );
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/main"),
        "the ride page, not the paused screen:\n{}",
        sim.screen().text()
    );
    sim.expect_no_text("Ride time");
    sim.expect_text("AROUND");
    sim.expect_text("AUTO PAUSED");
    sim.checkpoint("auto paused before following, turn around in sight");

    sim.ride_page("ride/map");
    expect_map(&sim);
    assert_eq!(map_badge(&sim), ["TURN", "AROUND"]);
    sim.checkpoint("auto paused before following, on the map");
    sim.press(Role::Move);
    sim.expect_text("Ride time");
    assert_eq!(sim.vana_page(), "ride");
    sim.swipe_next();
    sim.expect_no_text("Ride time");
    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.press(Role::Back);
    sim.expect_no_text("End ride?");

    sim.ride_at(24.0, 180.0, 0.0, 85.0);
    sim.press(Role::Act);
    sim.wait_for("RESUMED", seconds(3)).unwrap();
    sim.advance(seconds(5));
    sim.pause_ride();
    sim.expect_text("Ride time");
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
    sim.ride_page("ride/map");
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
    sim.disconnect_phone();
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
    sim.ride_page("ride/map");
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
fn holding_move_on_the_map_stops_following_the_course() {
    let mut sim = following_the_loop_on_the_map();
    sim.hold(Role::Move, 1_600);
    sim.expect_text("COURSE OFF");
    assert_eq!(map_badge(&sim), Vec::<String>::new(), "no turn to show");
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/map"),
        "the map stays"
    );
    assert!(
        !sim.screen().text().contains("MARK"),
        "the hold stops, not marks"
    );
    sim.checkpoint("following stopped by a held MOVE");
    ride_to(&mut sim, 1_200.0);
    assert!(!sim.screen().text().contains("FOLLOWING"));
    sim.hold(Role::Move, 1_600);
    sim.expect_text("MARK 1");
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
    sim.ride_page("ride/map");
    sim
}

#[test]
fn before_following_the_map_comes_back_on_the_leg_ridden_beside_meanwhile() {
    let mut sim = riding_too_far_beside_a_zigzag_to_follow();
    ride_to(&mut sim, 375.0);
    sim.ride_page("ride/main");
    ride_to(&mut sim, 1_000.0);
    sim.ride_page("ride/map");
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
        pair.both(|sim| sim.ride_page("ride/map"));
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
        self.away.ride_page("ride/main");
        self.both(|_| {});
    }

    #[track_caller]
    fn come_back(&mut self, what: &str) {
        self.away.ride_page("ride/map");
        self.both(|_| {});
        self.away.checkpoint(what);
        self.same_map(what);
    }

    #[track_caller]
    fn ride_to(&mut self, metres: f64) {
        while self.watched.world().distance_m < metres {
            self.both(|sim| sim.advance(seconds(1)));
            if self.away.ride_page_name().as_deref() == Some("ride/map") {
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
    let heard =
        cues_after_acting_before_chime(chime_ms, 0, seconds(15), |sim| sim.vehicle(closing_car()));
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
fn pressed_as_the_rider_arrives_the_chime_still_sounds(button: Role, cue: &str) {
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
    pressed_as_the_rider_arrives_the_chime_still_sounds(Role::Back, "lap");
}

#[test]
fn pausing_in_the_pass_that_arrives_keeps_the_arrival_chime() {
    pressed_as_the_rider_arrives_the_chime_still_sounds(Role::Act, "pause");
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
    sim.option("SUNSET", "OFF");
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
    sim.option("SUNSET", "OFF");
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
    // The trace's two decimals: the Stats page shows one.
    let shown = sim
        .reading(vana::compose::data::Metric::Distance)
        .expect("a distance");
    (shown.parse().expect("kilometres"), travelled)
}

/// The wheel size and mode from `OPTION`, such as "wheel=2136 wheel_mode=auto".
fn wheel_option(sim: &mut Sim) -> String {
    let (_, options) = sim.command("OPTION");
    options
        .split_whitespace()
        .filter(|field| field.starts_with("wheel"))
        .collect::<Vec<_>>()
        .join(" ")
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

    assert_eq!(wheel_option(&mut sim), "wheel=2136 wheel_mode=auto");

    // Ride 1 measures with the wrong size and learns the right one, but keeps
    // the wrong one until the ride ends, so its distance never jumps, and
    // writes nothing to the record until then.
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(4));
    let early_speed = reading(&sim, "SPD KM/H");
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
    let late_speed = reading(&sim, "SPD KM/H");
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
    // Stopped, so Ride done does not start the next ride.
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
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

    assert_eq!(
        wheel_option(&mut sim),
        format!("wheel={learned} wheel_mode=auto")
    );

    // Ride 2 measures with the learned size.
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let speed = reading(&sim, "SPD KM/H");
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

    sim.option("WHEEL", "2155");
    assert_eq!(wheel_option(&mut sim), "wheel=2155 wheel_mode=manual");

    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(15));
    let speed = reading(&sim, "SPD KM/H");
    assert!((speed - 28.0 * 2155.0 / 2200.0).abs() < 0.2, "{speed}");
    assert!(
        wheel_fields(&mut sim).contains("windows=0"),
        "no learning while manual"
    );
    // Stopped, so Ride done does not start the next ride.
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.stop_ride();
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");

    sim.power_loss();
    assert_eq!(wheel_option(&mut sim), "wheel=2155 wheel_mode=manual");
}

fn battery(sim: &mut Sim, kind: u8, number: u16, report: BatteryReport) {
    sim.ant_sensor(kind, number).battery = Some(report);
}

fn run_color(sim: &Sim, text: &str) -> u16 {
    let screen = sim.screen();
    let run = screen
        .run(text)
        .or_else(|| screen.find(text).into_iter().next());
    run.unwrap_or_else(|| panic!("no {text:?}; screen:\n{}", screen.text()))
        .color
}

#[test]
fn a_low_battery_turns_sensors_amber_on_their_pages() {
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

    sim.home("SENSORS");
    sim.open_sensor_detail("HEART RATE");
    sim.expect_text("2.4");
    assert_eq!(run_color(&sim, "2.4"), AMBER);
    sim.checkpoint("heart rate detail with a low battery");
    sim.press(Role::Back);
    sim.open_sensor_detail("POWER METER");
    sim.expect_text("2.9");
    assert_eq!(run_color(&sim, "2.9"), WHITE);
    sim.checkpoint("power meter detail with a good battery");
}

/// Whether Home's status widget shows `color`, as its sensors' dots do.
fn widget_shows(sim: &Sim, color: u16) -> bool {
    let screen = sim.screen();
    let (left, right, top, bottom) = (8, 170, 105, 142);
    (top..bottom).any(|y| (left..right).any(|x| screen.pixels[y * screen.width + x] == color))
}

fn connected_strap(sim: &mut Sim, level: Option<u8>) {
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(level);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
}

#[test]
fn a_low_ble_strap_battery_turns_its_dot_amber_on_home() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(80));
    let theme = vana::screens::theme::DARK;
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert!(
        widget_shows(&sim, theme.good),
        "a live strap's dot is green"
    );
    assert!(!widget_shows(&sim, theme.warn));
    let mut sim = rider();
    connected_strap(&mut sim, Some(12));
    assert!(
        widget_shows(&sim, theme.warn),
        "a low battery's dot is amber"
    );
    assert!(!widget_shows(&sim, theme.bad));
    sim.checkpoint("home with a low ble heart strap battery");
}

#[test]
fn a_critical_ble_strap_battery_is_red_and_the_next_read_clears_it() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(3));
    let theme = vana::screens::theme::DARK;
    assert!(
        widget_shows(&sim, theme.bad),
        "a critical battery's dot is red"
    );
    sim.checkpoint("home with a critical ble heart strap battery");
    // A fresh cell shows on the strap's next minute re-read.
    sim.ble_heart_strap_battery(Some(100));
    sim.advance(seconds(65));
    assert!(!widget_shows(&sim, theme.bad));
    assert!(widget_shows(&sim, theme.good));
}

#[test]
fn a_critical_ant_battery_is_red_on_home_and_a_fresh_one_clears_it() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(5, 2_100),
    );
    sim.advance(seconds(12));
    let theme = vana::screens::theme::DARK;
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert!(
        widget_shows(&sim, theme.bad),
        "a critical battery's dot is red"
    );
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(1, 3_000),
    );
    sim.advance(seconds(12));
    assert!(!widget_shows(&sim, theme.bad));
    assert!(widget_shows(&sim, theme.good));
}

#[test]
fn reserved_battery_values_and_battery_pages_never_disturb_readings() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
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
        assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
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
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.expect_field("PWR 3S W", "205");
    sim.expect_field("HEART BPM", "138");
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(
        ride.samples()
            .skip(5)
            .all(|s| s.power_watts == Some(205) && s.heart_bpm == Some(138)),
        "battery pages leaked into the recording"
    );
}

#[test]
fn a_failed_ble_link_is_left_off_home() {
    let mut sim = rider();
    sim.ble_heart_strap(false);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
    sim.device.sim_mut().ble.snapshot.link = Link::Failed;
    sim.advance(seconds(1));
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    // Home shows what a device with no sensor shows at the same moment.
    let mut bare = rider();
    bare.advance(sim.now() - bare.now());
    assert!(sim.screen().pixels == bare.screen().pixels);
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
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(20)).unwrap();
    sim.checkpoint("power meter calibrating");
    sim.wait_for("Zeroed", seconds(5)).unwrap();
    sim.expect_text("offset -37");
    assert_eq!(run_color(&sim, "Zeroed, offset -37"), GREEN);
    sim.checkpoint("power meter calibrated with its offset");
    sim.expect_text("ANT+");
    sim.wait_gone("Zeroed", seconds(12)).unwrap();
}

#[test]
fn a_power_meter_that_refuses_to_zero_ends_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Failure);
    sim.press(Role::Act);
    sim.wait_for("Keep cranks still", seconds(5)).unwrap();
    assert_eq!(run_color(&sim, "Keep cranks still"), AMBER);
    sim.checkpoint("power meter calibration failed");
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(1)).unwrap();
    sim.wait_for("Keep cranks still", seconds(5)).unwrap();
}

#[test]
fn a_silent_power_meter_times_out_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Silence);
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(1)).unwrap();
    sim.advance(seconds(8));
    sim.expect_text("Zeroing");
    sim.checkpoint("power meter still waiting for its answer");
    sim.wait_for("Keep cranks still", seconds(4)).unwrap();
    sim.checkpoint("power meter calibration timed out");
}

#[test]
fn calibrating_a_power_meter_never_disturbs_a_later_ride() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(512));
    sim.press(Role::Act);
    sim.wait_for("Zeroed", seconds(5)).unwrap();
    sim.press(Role::Back);
    sim.press(Role::Back);
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.expect_field("PWR 3S W", "205");
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
fn assert_replays_exactly(mut recorded: Sim, scenario: fn(&mut Sim), bulk: bool) -> Vec<u8> {
    scenario(&mut recorded);
    let capture = recorded.capture();
    println!("{}", trace_size(&capture, recorded.device.now));
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    if let Some(difference) = shell_simulator::replay::first_difference(&capture, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
    // Media accesses are checked through the rerecorded bytes and digests rather than by comparing
    // final journal contents.
    let faithful = report.faithful.expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{faithful}");
    assert!(faithful.physical > 0, "{faithful}");
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
    let rides = if bulk {
        recorded.bulk_rides()
    } else {
        recorded.rides()
    };
    assert!(
        rides.iter().any(|ride| ride.samples().count() > 30),
        "a ride was saved"
    );
    capture
}

#[track_caller]
fn assert_session_replays_faithfully(sim: &Sim) {
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    if let Some(difference) = shell_simulator::replay::first_difference(&capture, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
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
    let middle = capture.len() / 2;
    capture[middle] ^= 0x40;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.count(Kind::Truncated), 1, "{report}");
    assert!(
        replayed.device.now < sim.device.now,
        "the replay stops where the trace breaks"
    );
    let difference = first_difference(&capture, &replayed.trace()).unwrap();
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
    let (_, events) = decode(&capture).unwrap();
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

const SET_UTC_S: u64 = 1_793_521_800;

fn a_clock_set_from_the_host(sim: &mut Sim) {
    sim.advance(seconds(2));
    sim.expect_text("--:--");
    assert_eq!(
        sim.command(&format!("TIME SET {SET_UTC_S}")),
        ("OK", format!("unix_seconds={SET_UTC_S}"))
    );
    sim.advance(seconds(1));
    sim.expect_text("5:30");
    sim.checkpoint("home shows the set clock");
}

#[test]
fn a_clock_set_from_the_host_shows_on_home_and_outlives_a_crash() {
    let mut sim = hashed().boot();
    a_clock_set_from_the_host(&mut sim);
    sim.crash();
    sim.wait_usable();
    sim.expect_text("5:30");
    let (status, time) = sim.command("TIME");
    assert_eq!(status, "OK");
    assert!(
        time.contains("source=set"),
        "a crash keeps the clock: {time}"
    );
    let shown = time
        .split("unix_seconds: Some(")
        .nth(1)
        .and_then(|rest| rest.split(')').next());
    let shown: u64 = shown.and_then(|seconds| seconds.parse().ok()).expect(&time);
    assert!((SET_UTC_S..SET_UTC_S + 60).contains(&shown), "{time}");
    sim.power_loss();
    sim.expect_text("--:--");
    let (_, time) = sim.command("TIME");
    assert!(time.contains("source=none"), "power loss: {time}");
}

#[test]
fn a_clock_set_from_the_host_replays_exactly() {
    let mut sim = hashed().boot();
    a_clock_set_from_the_host(&mut sim);
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    if let Some(difference) = shell_simulator::replay::first_difference(&capture, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
    assert_eq!(sim.frame_hashes(), replayed.frame_hashes());
}

#[test]
fn the_receiver_overrules_a_set_clock() {
    let mut sim = rider();
    sim.advance(seconds(5));
    sim.expect_text("7:00");
    sim.command(&format!("TIME SET {SET_UTC_S}"));
    sim.advance(seconds(2));
    let (_, time) = sim.command("TIME");
    assert!(time.contains("source=gnss"), "{time}");
    sim.expect_text("7:00");
    sim.expect_no_text("5:30");
}

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
    use firmware_services::network_time::{Anchor, Source, offer};
    offer(Anchor::new(Source::Sntp, WIFI_UTC_MS + 365 * 86_400_000, 0));
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_dated_by_wifi, false);
    let (_, events) = firmware_services::trace::decode(&capture).unwrap();
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
fn a_trace_names_the_terrain_pack_and_the_working_memory_its_session_started_with() {
    use firmware_services::trace::origin::{Found, Memory};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.install_terrain(b"CHILE-V1\0\0\0\0\0\0\0\0", &[7u8; 2048]);
    let mut sim = equip(sim.boot());
    a_ride_on_new_bulk_media(&mut sim);

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture).unwrap();
    let Found::Read(terrain) = header.terrain else {
        panic!("the pack: {:?}", header.terrain);
    };
    assert!(terrain.ready);
    assert_eq!(terrain.length, 2048);
    assert_eq!(&terrain.tag[..8], b"CHILE-V1");
    let opened = world::opened(&capture);
    assert_eq!((opened.memory, opened.redacted), (Memory::ALL, false));
}

#[test]
fn a_replay_gives_vana_only_the_working_memory_the_recording_had() {
    use firmware_services::trace::{Event, decode};
    let mut sim = equip(Sim::new().boot());
    sim.advance(seconds(5));
    let mut capture = sim.capture();
    assert!(sim.device.sdk.detach_terrain().is_some());
    let trace: &'static [u8] = Box::leak(capture.clone().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let events: Vec<Event<'static>> = events
        .map(|event| match event {
            Event::Session {
                mut memory,
                redacted,
            } => {
                memory.terrain = false;
                Event::Session { memory, redacted }
            }
            event => event,
        })
        .collect();
    capture = shell_simulator::trace::surgery::encode(&header, events);
    let mut replayed = Sim::replay(&capture);
    assert!(
        replayed.device.sdk.detach_terrain().is_none(),
        "no tile cache, as the recording had none"
    );
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
    assert_eq!(pulled.len(), capture.len());
    assert!(pulled == capture, "the file is the trace");
    capture = pulled;

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
    let (frames, replayed_frames) = (sim.frame_hashes(), replayed.frame_hashes());
    assert!(frames.len() > 100);
    assert!(frames == replayed_frames, "the same frames from power-on");
    let faithful = report.faithful.expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{faithful}");
    assert!(
        sim.bulk_rides()
            .iter()
            .any(|ride| ride.samples().count() > 30),
        "a ride was saved"
    );
}

#[test]
fn a_new_store_replays_with_the_entropy_it_was_named_from() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_on_new_bulk_media, true);
    let entropy = count_world(&capture, |world| matches!(world, World::Entropy(_)));
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
fn a_ride_replayed_changed_on_the_same_build_saves_the_same_ride() {
    use shell_simulator::replay::{Mode, Options};
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let changed = Options {
        mode: Mode::Changed,
        model: false,
    };
    let mut replayed = Sim::replay_as(&recorded.capture(), changed);
    let report = replayed.device.report().unwrap();
    assert_eq!(
        report.verdict(),
        Ok("the replay re-recorded the trace byte for byte".into()),
        "{report}"
    );
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
            Event::Call(call) => match world::read(&call) {
                Some(World::AntRequest {
                    operation: device_api::ant::AntOperation::Connect(peer),
                    result: Ok(_),
                }) => Some(peer.device_type),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn with_the_first_connects_swapped(trace: &[u8]) -> Vec<u8> {
    use device_api::ant::AntOperation;
    use firmware_services::trace::wire::{Wire, Writer};
    use firmware_services::trace::{Event, decode};
    let trace: &'static [u8] = Box::leak(trace.to_vec().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let connects: Vec<(usize, usize)> = (events.iter().enumerate())
        .filter_map(|(at, event)| match event {
            Event::Call(call) => match world::read(call) {
                Some(World::AntRequest {
                    operation: AntOperation::Connect(_),
                    result: result @ Ok(_),
                }) => {
                    let mut bytes = [0u8; 64];
                    let mut w = Writer::new(&mut bytes, call.now_ms);
                    result.put(&mut w);
                    Some((at, call.body.len() - w.written().unwrap()))
                }
                _ => None,
            },
            _ => None,
        })
        .take(2)
        .collect();
    let [(first, first_args), (second, second_args)] = connects[..] else {
        panic!("two connections the radio accepted")
    };
    let (Event::Call(mut a), Event::Call(mut b)) = (events[first], events[second]) else {
        unreachable!()
    };
    let a_body = [&b.body[..second_args], &a.body[first_args..]].concat();
    let b_body = [&a.body[..first_args], &b.body[second_args..]].concat();
    a.body = Vec::leak(a_body);
    b.body = Vec::leak(b_body);
    events[first] = Event::Call(a);
    events[second] = Event::Call(b);
    shell_simulator::trace::surgery::encode(&header, events)
}

#[test]
fn a_ride_recorded_power_meter_first_replays_on_a_modelled_branch_and_saves_the_sensor_data() {
    use shell_simulator::replay::{Kind, Mode, Options};
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let mut capture = recorded.capture();
    capture = with_the_first_connects_swapped(&capture);
    let modelled = Options {
        mode: Mode::Changed,
        model: true,
    };
    let mut replayed = Sim::replay_as(&capture, modelled);
    let (power, heart, speed) = (
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::SPEED_CADENCE,
    );
    assert_eq!(connected_in_order(&capture), [power, heart, speed]);
    assert_eq!(
        connected_in_order(&replayed.trace()),
        [heart, power, speed],
        "this build connects the strap first"
    );
    let report = replayed.device.report().unwrap();
    let branched = report.branched.as_ref().expect("the replay branched");
    assert!(report.verdict().is_err(), "a branch fails the verdict");
    assert!(report.count(Kind::Modelled) >= 1, "{report}");
    assert!(report.count(Kind::NotRequested) >= 1, "{report}");
    assert!(
        branched.detail.contains("Ant::request"),
        "the branch names the call: {}",
        branched.detail
    );
    assert_eq!(sensor_data(&mut replayed), sensor_data(&mut recorded));
}

fn count_events(
    trace: &[u8],
    matches: impl Fn(&firmware_services::trace::Event<'_>) -> bool,
) -> usize {
    let (_, events) = firmware_services::trace::decode(trace).unwrap();
    events.filter(|event| matches(event)).count()
}

fn count_world(trace: &[u8], matches: impl Fn(&World<'_>) -> bool) -> usize {
    count_events(trace, |event| match event {
        firmware_services::trace::Event::Call(call) => {
            world::read(call).is_some_and(|world| matches(&world))
        }
        _ => false,
    })
}

fn body_read(trace: &[u8]) -> usize {
    let (_, events) = firmware_services::trace::decode(trace).unwrap();
    events
        .filter_map(|event| match event {
            firmware_services::trace::Event::Call(call) => match world::read(&call) {
                Some(World::ServerRead { part, .. }) => Some(part.len()),
                _ => None,
            },
            _ => None,
        })
        .sum()
}

fn a_course_uploaded_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.disconnect_phone();
    sim.advance(seconds(1));
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
}

#[test]
fn a_course_uploaded_from_the_phone_mid_session_replays_exactly() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let capture = assert_replays_exactly(equip(sim.boot()), a_course_uploaded_from_the_phone, true);
    let read = body_read(&capture);
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    assert_eq!(read, course.len(), "the whole body is in the trace");
    let joined = count_world(&capture, |world| {
        matches!(
            world,
            World::AccessPointState(device_api::server::AccessPointState::Up { clients: 1 })
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
    let lost: u32 = events
        .filter_map(|event| match event {
            Event::Lost { blocks, .. } => Some(blocks),
            _ => None,
        })
        .sum();
    assert_eq!(
        (body_read(&pulled), lost),
        (max, 0),
        "the whole body, nothing lost"
    );
    assert!(peak < capacity / 8, "the drain keeps up: {peak} B");
}

fn a_redacted_phone_session_then_a_ride() -> (Sim, [String; 5]) {
    let mut sim = hashed();
    sim.need(Need::Wifi);
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
    sim.disconnect_phone();
    sim.advance(seconds(1));
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    (sim, secrets)
}

#[test]
fn a_redacted_phone_session_replays_on_placeholders_and_holds_no_secret() {
    use firmware_services::trace::{Event, decode};
    let (sim, secrets) = a_redacted_phone_session_then_a_ride();
    let capture = sim.capture();
    let text = String::from_utf8_lossy(&capture);
    for secret in &secrets {
        assert!(!text.contains(secret.as_str()), "the trace holds {secret}");
    }
    let said = count_events(&capture, |event| {
        matches!(event, Event::Session { redacted: true, .. })
    });
    assert_eq!(said, 1, "the trace says it replaced its secrets");
    let (_, events) = decode(&capture).unwrap();
    let sessions: Vec<String> = events
        .filter_map(|event| match event {
            Event::Call(call) => match world::read(&call) {
                Some(World::ServerRequest(Some(request))) => Some(request.session.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        sessions.iter().any(|session| session == "****************"),
        "{sessions:?}"
    );

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    let faithful = report.faithful.clone().expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{report}");
    assert!(faithful.exclusions.is_empty(), "{report}");
    let placeholder = replayed.device.sdk.phone_status();
    assert!(placeholder.is_none(), "the session closed for the ride");
}

fn with_the_last_frame_unseen(mut capture: Vec<u8>) -> Vec<u8> {
    use firmware_services::trace::{Event, decode};
    let trace: &'static [u8] = Box::leak(capture.clone().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let last = events
        .iter()
        .rposition(|event| matches!(event, Event::Panel(_)))
        .unwrap();
    events[last] = Event::Panel(0xdead_beef);
    capture = shell_simulator::trace::surgery::encode(&header, events);
    capture
}

#[test]
fn a_redacted_phone_session_leaves_a_later_difference_first() {
    let (sim, _) = a_redacted_phone_session_then_a_ride();
    let replayed = Sim::replay(&with_the_last_frame_unseen(sim.capture()));
    let report = replayed.device.report().unwrap();
    let first = report.first.as_ref().expect("the altered frame diverges");
    assert_eq!(first.kind, shell_simulator::replay::Kind::Seen, "{report}");
    assert!(
        first.detail.contains("recorded Panel(3735928559)"),
        "{report}"
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
    use firmware_services::trace::{Medium, StorageOp};
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_journal_write_tears,
        false,
    );
    let torn = count_world(&capture, |world| {
        matches!(
            world,
            World::Media(StorageOp::Write, access)
                if access.span.medium == Medium::Data && access.fault.is_some()
        )
    });
    assert_eq!(torn, 1, "one torn write");
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
    use firmware_services::trace::{Medium, StorageOp};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_fails, false);
    let torn = count_world(&capture, |world| {
        matches!(
            world,
            World::Media(StorageOp::Write, access)
                if access.span.medium == Medium::Bulk && access.fault.is_some()
        )
    });
    assert_eq!(torn, 1, "the torn write");
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
    use firmware_services::trace::{Medium, StorageError};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_goes_missing, false);
    let missing = count_world(&capture, |world| match world {
        World::Media(_, access) => {
            access.span.medium == Medium::Bulk && access.fault == Some(StorageError::Unavailable)
        }
        World::NoCard(error) => *error == StorageError::Unavailable,
        _ => false,
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
    use shell_simulator::trace::world::Output;
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_panel_and_backlight_refuse,
        false,
    );
    for output in [Output::Panel, Output::Backlight] {
        let refused = count_world(
            &capture,
            |world| matches!(world, World::Refused { output: refused, .. } if *refused == output),
        );
        assert!(refused > 0, "{output:?} refused");
    }
    let changes = count_world(&capture, |world| matches!(world, World::Backlight(_)));
    assert_eq!(changes, 3, "ready, failed, ready again");
}

fn a_ride_then_turned_off(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    sim.done();
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "turned off");
    sim.advance(seconds(2));
}

#[test]
fn a_ride_ended_by_turning_off_replays_exactly() {
    use firmware_services::trace::Event;
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_then_turned_off, false);
    let asked = count_world(&capture, |world| {
        matches!(world, World::PowerRequest { result: Ok(()), .. })
    });
    let off = count_events(&capture, |event| matches!(event, Event::PoweredOff));
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
                Event::PoweredOff => asked = asked.or(passes.last().copied()),
                Event::Call(call)
                    if matches!(world::read(&call), Some(World::PowerRequest { .. })) =>
                {
                    asked = asked.or(passes.last().copied())
                }
                _ => {}
            }
        }
        (passes, asked)
    };
    let (recorded, asked) = passes(&sim.capture());
    let asked = asked.expect("VANA asked to turn off");
    let pulled = sim.store_file(&name).expect("every record reads back");
    let (filed, _) = passes(&pulled);
    let before = recorded.iter().position(|&at| at == asked).unwrap();
    assert_eq!(
        filed,
        recorded[..before],
        "every pass before the one that asked"
    );
    let capture = pulled;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

mod mutations {
    use super::*;
    use firmware_services::trace::{Event, decode};
    use shell_simulator::replay::first_difference;

    pub struct Recording {
        pub trace: &'static [u8],
        frames: Vec<u64>,
        backlight: (u8, u32),
    }

    type Outputs = (Vec<u64>, (u8, u32));

    /// Media accesses are checked by rerecording them, so this helper compares frames and backlight
    /// state.
    fn outputs(sim: &Sim) -> Outputs {
        let power = sim.device.power.0.borrow();
        (sim.frame_hashes(), (power.brightness, power.writes))
    }

    pub fn record(sim: Sim, scenario: fn(&mut Sim)) -> Recording {
        let mut sim = sim;
        scenario(&mut sim);
        let capture = sim.capture();
        let (frames, backlight) = outputs(&sim);
        Recording {
            trace: Vec::leak(capture),
            frames,
            backlight,
        }
    }

    /// A world record's method, or another event's name.
    pub fn kind(event: &Event<'_>) -> String {
        if let Event::Call(call) = event {
            return shell_simulator::twin::script::Named(call.method).to_string();
        }
        let debug = format!("{event:?}");
        debug
            .split([' ', '(', '{'])
            .next()
            .unwrap_or_default()
            .into()
    }

    fn replayed(event: &Event<'_>) -> bool {
        matches!(
            event,
            Event::Call(_)
                | Event::Pass { .. }
                | Event::Command(_)
                | Event::NetworkTime(_)
                | Event::PoweredOff
                | Event::Panel(_)
        )
    }

    pub fn kinds(trace: &[u8]) -> Vec<String> {
        let (_, events) = decode(trace).unwrap();
        let kinds: std::collections::BTreeSet<String> =
            events.filter(replayed).map(|event| kind(&event)).collect();
        kinds.into_iter().collect()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mutation {
        Drop,
        Alter,
    }

    fn altered(event: Event<'static>) -> Option<Event<'static>> {
        Some(match event {
            Event::Call(mut call) => {
                let mut body = call.body.to_vec();
                *body.last_mut()? ^= 1;
                call.body = Vec::leak(body);
                Event::Call(call)
            }
            Event::NetworkTime(mut time) => {
                time.unix_seconds = time.unix_seconds.map(|seconds| seconds + 7 * 3_600);
                Event::NetworkTime(time)
            }
            Event::Pass { now_ms } => Event::Pass { now_ms: now_ms + 5 },
            Event::Panel(digest) => Event::Panel(digest ^ 1),
            _ => return None,
        })
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
        Some(shell_simulator::trace::surgery::encode(&header, events))
    }

    pub fn with_too_large(trace: &'static [u8]) -> Vec<u8> {
        let (header, events) = decode(trace).unwrap();
        let mut events: Vec<Event<'static>> = events.collect();
        let second = events
            .iter()
            .position(|event| matches!(event, Event::Pass { .. }))
            .unwrap()
            + 1;
        let (capability, operation) = Event::Command("").id();
        events.insert(
            second,
            Event::TooLarge {
                capability,
                operation,
            },
        );
        shell_simulator::trace::surgery::encode(&header, events)
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
    }

    pub fn replay(recording: &Recording, trace: Vec<u8>) -> Caught {
        let faithful = shell_simulator::replay::Options::default();
        let mut replayed = match Sim::replaying(&trace, faithful, None) {
            Ok(replayed) => replayed,
            Err(boot) => {
                return Caught {
                    report: Some(boot.to_string()),
                    visible: false,
                    frames: false,
                    handed: false,
                };
            }
        };
        replayed.hash_frames();
        replayed.replay_until(u64::MAX);
        let report = replayed.device.report().unwrap();
        let report = report
            .first
            .map(|first| format!("{:?} at {} ms", first.kind, first.at_ms));
        let outputs = outputs(&replayed);
        let frames = shown(&outputs.0) != shown(&recording.frames);
        let visible = outputs != (recording.frames.clone(), recording.backlight);
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

    /// Dropping a record can produce a valid trace of a different world, so compare replayed inputs
    /// and outputs as well as divergence reports. Check the unmutated trace first to validate the
    /// oracle.
    #[track_caller]
    pub fn assert_every_mutation_shows(recording: &Recording, holds: &[&str]) {
        let clean = replay(recording, recording.trace.to_vec());
        assert!(!clean.any(), "the unmutated trace replays clean: {clean:?}");
        let kinds = kinds(recording.trace);
        let absent: Vec<&&str> = (holds.iter())
            .filter(|kind| !kinds.iter().any(|held| held == **kind))
            .collect();
        assert!(
            absent.is_empty(),
            "the trace holds no {absent:?}: {kinds:?}"
        );
        let (mut missed, mut silent) = (Vec::new(), Vec::new());
        for kind in &kinds {
            for how in [Mutation::Drop, Mutation::Alter] {
                let Some(trace) = mutated(recording.trace, kind, how) else {
                    continue;
                };
                let caught = replay(recording, trace);
                if caught.frames && caught.report.is_none() {
                    silent.push((kind, how));
                }
                if !caught.any() {
                    missed.push((kind, how));
                }
            }
        }
        assert!(missed.is_empty(), "mutations that did not show: {missed:?}");
        assert!(
            silent.is_empty(),
            "replays that showed other frames and reported nothing: {silent:?}"
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
    sim.press(Role::Act);
    sim.advance(seconds(40));
    sim.stop_ride();
}

#[test]
fn every_sensor_input_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_short_ride_with_every_sensor);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "Pass",
            "Command",
            "NetworkTime",
            "Panel",
            "InputSource::take_edge",
            "InputObservation::snapshot",
            "Ant::availability",
            "Ant::discoveries",
            "Ant::channels",
            "Ant::take_packet",
            "Ant::request",
            "Ble::snapshot",
            "Ble::take_packet",
            "Ble::take_battery",
            "Positioning::snapshot",
            "Sensors::startup_status",
            "Sensors::snapshot",
            "Sound::snapshot",
            "Sound::play",
            "PowerControl::status",
            "Power::availability",
            "Settings::load",
            "OwnedFlash::read",
            "OwnedFlash::program",
        ],
    );
    let marked = mutations::with_too_large(recording.trace);
    let caught = mutations::replay(&recording, marked);
    assert!(caught.report.is_some(), "{caught:?}");
}

fn a_power_meter_calibrated(sim: &mut Sim) {
    calibrate_power_meter(sim, ZeroReply::Offset(-37));
    sim.press(Role::Act);
    sim.wait_for("Zeroed", seconds(25)).unwrap();
}

#[test]
fn every_ant_send_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_power_meter_calibrated);
    mutations::assert_every_mutation_shows(&recording, &["Ant::send_status", "Ant::request"]);
}

fn a_course_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.disconnect_phone();
    sim.advance(seconds(2));
}

#[test]
fn every_phone_input_mutated_shows_in_its_replay() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let recording = mutations::record(equip(sim.boot()), a_course_from_the_phone);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "AccessPoint::state",
            "AccessPoint::start",
            "AccessPoint::entropy",
            "Server::request",
            "Server::read",
            "Server::write",
            "Server::finish",
            "ReadWrite::entropy",
            "ReadWrite::owned_read",
            "ReadWrite::owned_write",
        ],
    );
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
        &[
            "Display::submit",
            "Power::brightness",
            "OwnedFlash::program",
        ],
    );
}

#[test]
fn every_card_fault_mutated_shows_in_its_replay() {
    for scenario in [a_ride_whose_card_fails, a_ride_whose_card_goes_missing] {
        let mut sim = hashed();
        sim.fit_bulk(Presence::Ready);
        let recording = mutations::record(equip(sim.boot()), scenario);
        mutations::assert_every_mutation_shows(
            &recording,
            &["ReadWrite::owned_read", "ReadWrite::owned_write"],
        );
    }
}

#[test]
fn every_power_input_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_ride_then_turned_off);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "PowerControl::request",
            "PowerControl::status",
            "PoweredOff",
        ],
    );
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
    expect_page(sim, "ride/climb");
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
}

#[test]
fn a_ride_paged_by_the_harness_replays_exactly() {
    use firmware_services::trace::Event;
    let capture =
        assert_replays_exactly(equip(hashed().boot()), a_ride_paged_by_the_harness, false);
    let pressed = count_events(&capture, |event| matches!(event, Event::Synthetic(_)));
    assert_eq!(pressed, 1, "the harness pressed once");
}

fn a_ride_paged_by_a_press(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(seconds(30));
    sim.press(Role::Move);
    expect_page(sim, "ride/climb");
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
            numbers: ["SPD KM/H", "PWR 3S W", "HEART BPM", "CAD RPM"]
                .map(|label| screen.field(label)),
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
        sim.press(Role::Move);
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
fn saver_mode_is_kept_over_a_restart_and_draws_only_what_arrives() {
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

    assert!(sim.command("OPTION").1.contains("display=normal"));
    sim.option("DISPLAY", "SAVER");
    sim.advance(seconds(5));

    sim.power_loss();
    assert!(sim.command("PROFILE").1.contains("stored=current"));
    let (_, options) = sim.command("OPTION");
    assert!(options.contains("display=saver"), "{options}");
    sim.advance(seconds(3));

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
    never_dim(&mut sim);
    assert!(sim.command("OPTION").1.contains("brightness=auto"));
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
    sim.checkpoint("brightness AUTO in a dim room");
    assert_eq!(
        backlight_in(&mut sim, None),
        50,
        "reading lost: back to 50%"
    );

    sim.option("DISPLAY", "SAVER");
    let saver_bright = backlight_in(&mut sim, Some(1_500));
    let saver_dim = backlight_in(&mut sim, Some(68));
    assert!(
        saver_bright < bright && saver_dim < dim,
        "{saver_bright} {saver_dim}"
    );
    assert!(saver_bright <= 60 && saver_dim <= 18);

    sim.option("BRIGHTNESS", "50");
    sim.advance(seconds(1));
    assert!(!sim.device.shell.auto());
    for light in [Some(40), Some(4_000), None] {
        assert_eq!(backlight_in(&mut sim, light), 50, "{light:?}");
    }
    sim.checkpoint("brightness fixed at 50% ignores the light");
}

/// Keep the backlight from dimming, at the dim level it has.
fn never_dim(sim: &mut Sim) {
    let (_, settings) = sim.command("SETTINGS");
    let level = status_field(&settings, "dim_brightness").to_string();
    let (status, out) = sim.command(&format!("IDLE 0 {level}"));
    assert_eq!(status, "OK", "IDLE 0 {level}: {out}");
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
        sim.disconnect_phone();
        sim.advance(seconds(1));
        sim.ride_route(Route::from_gpx(&gpx_spaced("route", route, spacing)).unwrap());
        sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
        sim
    }

    fn start_following(sim: &mut Sim) {
        sim.start_ride();
        ride_until_shown(sim, "FOLLOWING COURSE", 150.0);
        sim.advance(seconds(1));
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
    #[ignore = "the Stats page has no chip tile yet (#517)"]
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
    fn a_turn_banner_moves_no_reading_and_leaves_the_top_line() {
        let course = straight_then(-1_000.0);
        let mut sim = on_course(&course, &course);
        sim.ride_at(24.6, 182.0, 141.0, 84.0);
        start_following(&mut sim);
        ride_to(&mut sim, 6_600.0);
        let at_rest = readings_at(&sim);
        assert_eq!(at_rest.len(), 6, "{at_rest:?}");

        let soon = ride_until_only(&mut sim, "turn_soon", &TURN_SOON, 6_850.0);
        assert!((6_690.0..6_710.0).contains(&soon), "{soon}");
        ride_until_sign(&mut sim, BANNER, &["120", "M"], 6_900.0);
        assert_eq!(readings_at(&sim), at_rest, "the banner moves no reading");
        assert!(
            sim.screen().text().contains("0:"),
            "the top line's time stays"
        );
        sim.checkpoint("ride-turns");

        let now = ride_until_only(&mut sim, "turn_now", &TURN_NOW, 7_000.0);
        assert!((6_960.0..6_980.0).contains(&now), "{now}");
        ride_to(&mut sim, 7_100.0);
        assert_eq!(readings_at(&sim), at_rest, "nor does it leaving");
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
    #[ignore = "the Stats page has no arrival totals yet (#517)"]
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
        sim.disconnect_phone();
        sim.advance(seconds(1));
        sim.ride_route(Route::from_gpx(route).unwrap());
        sim.ant_sensor(device_type::RADAR, RADAR);
        sim.pair(sensors);
        sim.ride_at(36.0, 210.0, 150.0, 88.0);
        sim.start_ride();
        ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
        sim.ride_page("ride/map");
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
        ride_to(&mut sim, 905.0);
        let screen = sim.screen();
        let puck = whole_puck_bounds(&screen, RIDER_BESIDE_RADAR).expect("the puck, whole");
        assert!(puck[0] >= 30, "its ring starts at x {}, cut", puck[0]);
        assert_eq!(
            scale(&screen),
            (50, "100 M".into()),
            "400 m ahead at 36 km/h"
        );
        // About 90 m to the turn; the sign steps by ten metres.
        let badge = badge(&sim);
        assert!(badge == ["90", "M"] || badge == ["80", "M"], "{badge:?}");
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
fn bare_shows_no_grade_without_a_barometer() {
    let mut sim = hill(Sim::on(shell_simulator::models::BARE));
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "ride/climb");
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1750.0);
    let screen = sim.screen();
    let grades: Vec<_> = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.text.ends_with('%'))
        .map(|run| run.text)
        .collect();
    assert!(
        grades.contains(&"--%".to_string())
            && grades
                .iter()
                .all(|grade| ["%", "--%"].contains(&grade.as_str())),
        "screen:\n{}",
        screen.text()
    );
    sim.expect_field("ALTITUDE M", "--");
    sim.checkpoint("climb page without a barometer");
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
    for button in BARE
        .buttons
        .as_slice()
        .iter()
        .filter_map(|fixture| fixture.role)
    {
        let before = sim.screen().text();
        sim.press(button);
        sim.advance(seconds(1));
        changed += usize::from(sim.screen().text() != before);
        sim.press(Role::Back);
        sim.advance(seconds(1));
    }
    assert!(changed > 0, "no button changed the screen");

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture).unwrap();
    assert_eq!(header.device, "bare");
    let replayed = Sim::replay(&capture);
    assert_eq!(replayed.device.model, BARE);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

#[test]
fn a_refused_frame_replays_as_refused_and_says_so() {
    let mut sim = hashed().boot();
    let mut out = String::new();
    assert_eq!(
        sim.device.fixture("FIXTURE DISPLAY_FAIL", &mut out),
        "OK",
        "{out}"
    );
    sim.press(Role::Act);
    sim.advance(seconds(2));
    let replayed = Sim::replay(&sim.capture());
    let report = replayed.device.report().expect("a replay");
    let faithful = report.faithful.clone().expect("a replay");
    assert_eq!(faithful.ended, None, "{report}");
    assert!(
        faithful
            .exclusions
            .contains(&shell_simulator::replay::Exclusion::RefusedFrames(1)),
        "{report}"
    );
}
