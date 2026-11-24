use super::*;

pub const HEART: u16 = 4242;
pub const POWER: u16 = 1234;
pub const SPEED: u16 = 777;

pub const RADAR: u16 = 31;

pub fn rider() -> Sim {
    equip(Sim::new().boot())
}

pub fn equip(mut sim: Sim) -> Sim {
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

/// The simulated internal journal's slots.
pub const SLOTS: usize = shell_simulator::storage::DATA_BYTES / SLOT_SIZE;

/// A finished ride that leaves `free` sample slots before the terminal slot.
pub fn old_ride_leaving(free: usize) -> Vec<u8> {
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

/// The panel's top-left pixel, which no screen paints over.
pub fn corner(sim: &Sim) -> u16 {
    sim.screen().pixels[0]
}

/// Whether any pixel of the panel is exactly `color`.
pub fn shows(sim: &Sim, color: u16) -> bool {
    sim.screen().pixels.contains(&color)
}

pub fn ble_heart_strap(sim: &mut Sim) {
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

pub fn two_hour_ride_with_a_pause(sim: &mut Sim) {
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
pub fn reading(sim: &Sim, label: &str) -> f64 {
    let screen = sim.screen();
    let value = screen
        .field(label)
        .unwrap_or_else(|| panic!("no {label}; screen:\n{}", screen.text()));
    value
        .parse()
        .unwrap_or_else(|_| panic!("{label} shows {value:?}; screen:\n{}", screen.text()))
}

#[track_caller]
pub fn expect_near(sim: &mut Sim, label: &str, expected: f64, tolerance: f64) {
    let value = reading(sim, label);
    assert!(
        (value - expected).abs() <= tolerance,
        "{label} shows {value}, expected {expected} ± {tolerance}; screen:\n{}",
        sim.screen().text()
    );
    sim.note(NoteKind::Check, format!("{label} shows {value}"));
}

pub fn ride_page(sim: &mut Sim, name: &str) {
    sim.ride_page(name);
}

/// The number Ride done shows over `caption`, such as "5.0" over "distance".
pub fn done_stat(sim: &Sim, caption: &str) -> Option<String> {
    let runs = sim.screen().frame().read();
    let label = runs.iter().find(|run| run.text == caption)?;
    runs.iter()
        .filter(|run| run.scale > label.scale && run.x == label.x && run.y < label.y)
        .max_by_key(|run| run.y)
        .map(|run| run.text.clone())
}

/// The FIT session of the newest saved ride, its fields by number.
pub fn saved_session(sim: &mut Sim) -> std::collections::BTreeMap<u8, u64> {
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
pub fn fit_messages(bytes: &[u8], global: u16) -> Vec<std::collections::BTreeMap<u8, u64>> {
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

/// TopLeft until Home, past the last ride page.
#[track_caller]
pub fn go_home(sim: &mut Sim) {
    for _ in 0..4 {
        if sim.on_home() {
            return;
        }
        sim.press(Role::Back);
    }
    panic!("not Home; screen:\n{}", sim.screen().text());
}

pub fn fit_laps(sim: &mut Sim) -> Vec<std::collections::BTreeMap<u8, u64>> {
    let fit = sim
        .rides()
        .pop()
        .expect("a saved ride")
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
    fit_messages(&fit, 19)
}

#[track_caller]
pub fn assert_near(what: &str, value: f64, expected: f64, tolerance: f64) {
    assert!(
        (value - expected).abs() <= tolerance,
        "{what}: {value}, expected {expected} within {tolerance}"
    );
}

pub fn manual_laps(sim: &mut Sim) {
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
    assert!(toast.starts_with("LAP 2 04:0"), "{toast}");
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
    assert_eq!(laps.len(), 3, "the open lap included");
    for (lap, km, km_h, watts, bpm) in [(1, 2.2, 33.0, 250.0, 150.0), (2, 1.2, 24.0, 150.0, 130.0)]
    {
        let lap = &laps[lap];
        assert_near("lap km", lap[&9] as f64 / 100_000.0, km, 0.05);
        assert_near("lap km/h", lap[&13] as f64 * 0.0036, km_h, 0.2);
        assert_near("lap W", lap[&19] as f64, watts, 1.0);
        assert_near("lap bpm", lap[&15] as f64, bpm, 1.0);
    }
}

/// Ride on, a second at a time, until `metres` along the route.
pub fn ride_to(sim: &mut Sim, metres: f64) {
    while sim.world().distance_m < metres {
        sim.advance(seconds(1));
    }
}

/// Ride on until `needle` shows, returning where along the route it appeared.
#[track_caller]
pub fn ride_until_shown(sim: &mut Sim, needle: &str, before_m: f64) -> f64 {
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

pub fn link(sim: &Sim, kind: u8) -> Option<LinkState> {
    shell_simulator::ant::link(&sim.device.sim().ant, kind, sim.device.now)
}

/// Connect requests the companion received since it last started.
pub fn connects(sim: &Sim, kind: u8) -> Vec<u16> {
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

pub fn recording(sim: &mut Sim) -> bool {
    sim.command("RIDE STATUS").1.contains("state=recording")
}

/// The ride clock in seconds.
pub fn elapsed_secs(sim: &Sim) -> u64 {
    let clock = sim.screen().elapsed().expect("ride clock");
    clock
        .split(':')
        .map(|part| part.parse::<u64>().unwrap())
        .fold(0, |secs, part| secs * 60 + part)
}

pub fn traffic_light_stop(sim: &mut Sim) {
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

/// Advance in firmware passes until the panel lights; returns the uptime then.
pub fn wait_lit(sim: &mut Sim) -> u64 {
    while !sim.device.lit() {
        assert!(sim.device.now < seconds(15), "panel never lit");
        sim.advance(10);
    }
    sim.device.now
}

pub fn bulk_rider() -> Sim {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    equip(sim.boot())
}

pub fn status_field(status: &str, name: &str) -> String {
    status
        .split_whitespace()
        .find_map(|field| field.strip_prefix(name)?.strip_prefix('='))
        .unwrap_or_default()
        .to_string()
}

// Structured training: workouts, the player, uploads and both FTP tests,
// ridden with simulated power.

/// A rider with power and speed paired, riding 30 km/h at an easy 120 W.
pub fn training_rider() -> Sim {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    sim
}

/// The patterns played since the `from`th.
pub fn played_since(sim: &Sim, from: usize) -> Vec<u8> {
    let played = &sim.device.sim().speaker.played;
    played[from..].iter().map(|(_, id)| *id).collect()
}

pub fn traced(sim: &Sim, kind: Traced) -> Vec<String> {
    shell_simulator::trace::timeline::read(&sim.trace())
        .expect("the session's own trace decodes")
        .entries
        .into_iter()
        .filter(|entry| entry.kind == kind && !entry.repeat)
        .map(|entry| entry.text)
        .collect()
}

pub fn marks(sim: &Sim) -> Vec<String> {
    traced(sim, Traced::Mark)
}

pub fn cyclist(sim: &mut Sim) {
    sim.pair(&["SPEED/CAD"]);
}

/// Press Ride on Home, then ride at speed; the simulated time of the press.
pub fn press_start(sim: &mut Sim) -> u64 {
    let pressed = sim.now();
    sim.start_ride();
    sim.ride_at(25.0, 150.0, 130.0, 80.0);
    pressed
}

/// The ride's start, as the FIT export counts it back from the first dated sample.
pub fn fit_start_ms(ride: &shell_simulator::rides::Ride) -> u64 {
    ride.fit_plan(&models::C606.product)
        .expect("the ride is dated")
        .offset_ms
        .expect("the plan has a start")
}

/// A GPX loop of about 5 km: north, round a bend east, down the far side
/// and back west to the start, then up the first leg again.
pub fn bend_loop_gpx() -> String {
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
pub fn gpx_through(name: &str, legs: &[(f64, f64)]) -> String {
    gpx_spaced(name, legs, 10.0)
}

pub fn gpx_spaced(name: &str, legs: &[(f64, f64)], spacing: f64) -> String {
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

/// The legs of the course the rider loads: the bend loop once round, back
/// to its start.
pub const COURSE_LOOP: [(f64, f64); 8] = [
    (0.0, 0.0),
    (0.0, 900.0),
    (500.0, 1300.0),
    (1300.0, 1250.0),
    (1700.0, 600.0),
    (1300.0, -100.0),
    (600.0, -250.0),
    (0.0, 0.0),
];

pub fn map_pixels(sim: &Sim, color: u16) -> usize {
    let screen = sim.screen();
    (22..290)
        .flat_map(|y| screen.pixels[y * screen.width..][..screen.width].iter())
        .filter(|&&pixel| pixel == color)
        .count()
}

pub fn course_pixels(sim: &Sim) -> usize {
    map_pixels(sim, vana::screens::chrome::ACCENT)
}

#[track_caller]
pub fn expect_map(sim: &Sim) {
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/map"));
    let ground = map_pixels(sim, vana::screens::chrome::rgb(0x10, 0x10, 0x12));
    assert!(ground > 30_000, "{ground} px of the map's ground");
}

pub fn map_badge(sim: &Sim) -> Vec<String> {
    sim.screen()
        .frame()
        .read()
        .into_iter()
        .filter(|run| (30..90).contains(&run.y) && run.x < 160)
        .map(|run| run.text)
        .collect()
}

pub const TURN_SOON: [u8; 2] = [21, 0];
pub const TURN_NOW: [u8; 1] = [10];
pub const OFF_COURSE: [u8; 2] = [10, 21];
pub const ON_COURSE: [u8; 2] = [21, 10];
pub const ARRIVED: [u8; 2] = [0, 10];
pub const ARRIVAL_CUE: &str = "arrived";

pub fn cues_since(sim: &Sim, from: usize) -> Vec<(String, Vec<u8>)> {
    timed_cues_since(sim, from)
        .into_iter()
        .map(|(_, cue, notes)| (cue, notes))
        .collect()
}

pub fn timed_cues_since(sim: &Sim, from: usize) -> Vec<(u64, String, Vec<u8>)> {
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

pub fn with_a_course(gpx: &str) -> Sim {
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
pub fn ride_until_cue_checking(
    sim: &mut Sim,
    cue: &str,
    before_m: f64,
    each: impl Fn(&Sim),
) -> f64 {
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
pub fn ride_until_cue(sim: &mut Sim, cue: &str, before_m: f64) -> f64 {
    ride_until_cue_checking(sim, cue, before_m, |_| {})
}

#[track_caller]
pub fn ride_until_only(sim: &mut Sim, cue: &str, notes: &[u8], before_m: f64) -> f64 {
    let from = sim.device.sim().speaker.played.len();
    let at = ride_until_cue(sim, cue, before_m);
    sim.advance(seconds(1));
    assert_eq!(played_since(sim, from), notes, "{cue} at {at:.0} m");
    at
}

/// How far VANA thinks the ride went against how far the rider really went,
/// both in km, since `start_m` on the world's odometer.
pub fn distances(sim: &mut Sim, start_m: f64) -> (f64, f64) {
    let travelled = (sim.world().distance_m - start_m) / 1000.0;
    // The trace's two decimals: the Stats page shows one.
    let shown = sim
        .reading(vana::compose::data::Metric::Distance)
        .expect("a distance");
    (shown.parse().expect("kilometres"), travelled)
}

pub fn battery(sim: &mut Sim, kind: u8, number: u16, report: BatteryReport) {
    sim.ant_sensor(kind, number).battery = Some(report);
}

/// Open the power meter's detail page and wait out the moment ACT ignores
/// after it opens, so the next ACT, on the calibrate cue, zeroes it.
pub fn calibrate_power_meter(sim: &mut Sim, reply: ZeroReply) {
    sim.ant_sensor(device_type::POWER, POWER).zero = reply;
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(3));
    sim.home("SENSORS");
    sim.open_sensor_detail("POWER METER");
    sim.checkpoint("power meter detail");
    sim.advance(seconds(1));
}

pub fn hashed() -> Sim {
    let mut sim = Sim::new();
    sim.hash_frames();
    sim
}

#[track_caller]
pub fn assert_replays_exactly(mut recorded: Sim, scenario: fn(&mut Sim), bulk: bool) -> Vec<u8> {
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
pub fn assert_session_replays_faithfully(sim: &Sim) {
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    if let Some(difference) = shell_simulator::replay::first_difference(&capture, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
}

pub fn trace_size(bytes: &[u8], uptime_ms: u64) -> String {
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

pub const WIFI_UTC_MS: u64 = 1_793_521_800_000;

pub const SET_UTC_S: u64 = 1_793_521_800;

pub fn a_clock_set_from_the_host(sim: &mut Sim) {
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

pub fn count_events(
    trace: &[u8],
    matches: impl Fn(&firmware_services::trace::Event<'_>) -> bool,
) -> usize {
    let (_, events) = firmware_services::trace::decode(trace).unwrap();
    events.filter(|event| matches(event)).count()
}

pub fn count_world(trace: &[u8], matches: impl Fn(&World<'_>) -> bool) -> usize {
    count_events(trace, |event| match event {
        firmware_services::trace::Event::Call(call) => {
            world::read(call).is_some_and(|world| matches(&world))
        }
        _ => false,
    })
}
