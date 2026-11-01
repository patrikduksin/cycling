//! A device trace replays from the media it started on, rebuilt from its
//! header and the media pulled after the session, as `cycling trace pull`
//! pulls them through ride export and `files get`.
#![cfg(feature = "cycling")]
use device_api::server::Method;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::replay::{Capture, first_difference};
use shell_simulator::route::{Point, Route, to_gpx};
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::trace::rebuild::{self, Files, Gap, Pulled, Rides};
use vana::ride::log::{Kind, Sample};
use vana::workout::codec::encode;
use vana::workout::model::{Intensity, Length, Step, Target, Workout};

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;
const WORKOUT: &str = "workouts/tempo.vw";

fn data(name: &str) -> String {
    format!(
        "{}/../../packages/terrain/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Route G-21 up to Farellones, which the sample terrain pack holds.
fn g21() -> Route {
    let track = std::fs::read_to_string(data("farellones-g21.csv")).unwrap();
    let points: Vec<_> = track
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split(',').map(|f| f.parse::<f64>().unwrap());
            let (lat, lon, ele) = (
                fields.next().unwrap(),
                fields.next().unwrap(),
                fields.next().unwrap(),
            );
            (
                Point {
                    ele: Some(ele),
                    ..Point::new(lat, lon)
                },
                None,
            )
        })
        .collect();
    Route::from_gpx(&to_gpx("G-21", &points)).unwrap()
}

fn workout(minutes: u16) -> Vec<u8> {
    const WARMUP: Target = Target::Ftp { low: 50, high: 65 };
    let steps = [
        Step::new(Intensity::Warmup, Length::Time(300), WARMUP),
        Step::new(
            Intensity::Work,
            Length::Time(minutes * 60),
            Target::Ftp { low: 80, high: 88 },
        ),
    ];
    encode(&Workout::new("TEMPO", &steps, &[], None)).to_vec()
}

fn ride(sim: &mut Sim, seconds_long: u64, power_w: f64) {
    sim.ride_at(28.0, power_w, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(seconds_long));
    sim.stop_ride();
    sim.done();
}

fn recorded_session() -> (Sim, Vec<u8>, Session) {
    recorded_session_after(|_| {})
}

/// A device with a past: saved sensors, a ride on the internal journal and
/// one on the card's RIDES log, a workout, a course and a terrain pack, and
/// then whatever `before` does. Then the session the test pulls: a power-on
/// that rides G-21 into the pack. Returns the device after that session,
/// its trace as the device wrote it to STORE, and what the session left:
/// the frames the rider saw and the rides on both journals.
fn recorded_session_after(before: impl FnOnce(&mut Sim)) -> (Sim, Vec<u8>, Session) {
    let mut sim = Sim::new();
    sim.hash_frames();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    sim.put_file(WORKOUT, &workout(20));
    let pack = std::fs::read(data("farellones.terrain")).unwrap();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &pack);
    let mut sim = sim.boot();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ride_route(g21());
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    ride(&mut sim, 40, 200.0);
    assert_eq!(sim.rides().len(), 1, "the first ride is on internal flash");
    sim.init_bulk();
    ride(&mut sim, 40, 200.0);
    assert_eq!(sim.bulk_rides().len(), 1, "the second is on the card");
    sim.connect_phone();
    let course = to_gpx(
        "Farellones",
        &[
            (Point::new(-33.36, -70.53), None),
            (Point::new(-33.35, -70.32), None),
        ],
    );
    let (code, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(code, 200, "{body}");
    assert!(sim.file("course.dat").is_some(), "the course is on FILES");
    before(&mut sim);

    sim.device.restart().unwrap();
    sim.wait_usable();
    // Shorter, easier and lower than the rides before: it sets no best, so
    // VANA rewrites only the ride facts table, which the rebuild rewinds.
    sim.ride_route(g21());
    ride(&mut sim, 30, 150.0);
    assert!(sim.device.sync_trace(), "the session's trace is open");
    let name = sim.device.trace_file().expect("the trace file");
    let trace = sim.store_file(&name).expect("the trace reads back whole");
    let session = Session::of(&mut sim);
    assert!(
        session.frames.len() > 100,
        "{} frames",
        session.frames.len()
    );
    (sim, trace, session)
}

#[derive(Debug, PartialEq)]
struct Session {
    frames: Vec<u64>,
    journal: Vec<u8>,
    rides: Vec<(u32, Vec<Kind>, Vec<Sample>)>,
}

impl Session {
    /// The card's rides only when it holds a log VANA can export.
    fn of(sim: &mut Sim) -> Self {
        let rides = if sim.command("EXPORT BULK INFO").0 == "OK" {
            sim.bulk_rides()
        } else {
            Vec::new()
        };
        Self {
            frames: sim.frame_hashes(),
            journal: sim.ride_media(),
            rides: rides
                .iter()
                .map(|ride| (ride.id, ride.kinds(), ride.samples().copied().collect()))
                .collect(),
        }
    }
}

/// Rides after the session, on internal flash with the card out and on
/// the card again, so both journals hold more than the header names.
fn ride_on(sim: &mut Sim) {
    sim.fit_bulk(Presence::Missing);
    sim.device.restart().unwrap();
    sim.wait_usable();
    ride(sim, 30, 150.0);
    sim.fit_bulk(Presence::Ready);
    sim.device.restart().unwrap();
    sim.wait_usable();
    ride(sim, 30, 150.0);
}

/// What `cycling trace pull` copies for `trace`: the header's media lines
/// decide it, ride export reads the journals' slots up to the ends they
/// name, and `files get` reads each file they list.
fn pull(sim: &mut Sim, trace: &[u8]) -> Pulled {
    let lines = rebuild::media(trace).unwrap();
    let mut pulled = Pulled::default();
    for line in lines.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["journal", slots] if slots != "none" => {
                let (_, info) = sim.command("EXPORT INFO");
                let upper: usize = info.split_whitespace().nth(3).unwrap().parse().unwrap();
                let count = upper.min(slots.parse().unwrap());
                pulled.internal = Some(slots_of(sim, "EXPORT", count));
            }
            ["rides", "log" | "uncertain", _, head] => {
                let (_, info) = sim.command("EXPORT BULK INFO");
                let info: Vec<&str> = info.split_whitespace().collect();
                let upper: usize = info[3].parse().unwrap();
                let (store, generation) = (info[5].to_string(), info[6].parse().unwrap());
                let count = upper.min(head.parse().unwrap());
                pulled.rides = Some(Rides {
                    store: u64::from_str_radix(&store, 16).unwrap(),
                    generation,
                    slots: slots_of(sim, "EXPORT BULK", count),
                    invalid: Vec::new(),
                });
            }
            ["files", "none"] => {}
            ["files", ..] => {
                let (store, _) = files_now(sim);
                pulled.files = Some(Files {
                    store,
                    ..Files::default()
                });
            }
            ["file", name, ..] => {
                let (_, revisions) = files_now(sim);
                if let (Some(revision), Some(bytes)) = (revisions.get(name), sim.file(name)) {
                    let files = pulled.files.as_mut().unwrap();
                    files.files.insert(name.to_string(), (*revision, bytes));
                }
            }
            ["terrain", length, ..] => {
                let pack = std::fs::read(data("farellones.terrain")).unwrap();
                assert_eq!(pack.len().to_string(), length);
                pulled.terrain = Some(pack);
            }
            _ => {}
        }
    }
    pulled
}

/// The FILES store's id and each file's revision, as `files status` and
/// `files stat` read them.
fn files_now(sim: &mut Sim) -> (u64, std::collections::BTreeMap<String, u32>) {
    use firmware_services::{files, volume};
    let mut sector = [0u8; 512];
    let mut card = sim.device.bulk.host();
    let mut store = volume::open(&mut card, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    let mut revisions = std::collections::BTreeMap::new();
    dir.list(&mut store, &mut sector, |entry| {
        revisions.insert(entry.name.as_str().to_string(), entry.revision);
    })
    .unwrap();
    (dir.store_id(), revisions)
}

/// `pulled` written in the layout `cycling trace pull` writes, and read
/// back as `cycling trace rebuild` reads it.
fn through_dir(pulled: &Pulled) -> Pulled {
    let dir = std::env::temp_dir().join(format!("trace-pulled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("internal.slots"),
        pulled.internal.as_ref().unwrap(),
    )
    .unwrap();
    let rides = pulled.rides.as_ref().unwrap();
    std::fs::write(dir.join("rides.slots"), &rides.slots).unwrap();
    std::fs::write(
        dir.join("rides.epoch"),
        format!(
            "store {:016x}\ngeneration {}\ninvalid\n",
            rides.store, rides.generation
        ),
    )
    .unwrap();
    let files = pulled.files.as_ref().unwrap();
    let mut list = format!("store {:016x}\n", files.store);
    for (name, (revision, bytes)) in &files.files {
        let path = dir.join("files").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        list += &format!("file {revision} {name}\n");
    }
    for name in &files.unread {
        list += &format!("unread {name}\n");
    }
    std::fs::write(dir.join("files.list"), list).unwrap();
    std::fs::write(dir.join("terrain.pack"), pulled.terrain.as_ref().unwrap()).unwrap();
    let read = Pulled::read(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    read
}

fn slots_of(sim: &mut Sim, export: &str, count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in 0..count {
        let (status, out) = sim.command(&format!("{export} SLOT {index}"));
        assert_eq!(status, "OK", "{out}");
        let hex = out.split_whitespace().nth(3).unwrap();
        for pair in hex.as_bytes().chunks(2) {
            bytes.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
        }
    }
    bytes
}

#[test]
fn a_pulled_device_trace_replays_strict_from_the_media_it_started_on() {
    let (mut sim, trace, session) = recorded_session();
    ride_on(&mut sim);
    let pulled = through_dir(&pull(&mut sim, &trace));
    assert!(
        pulled.internal.as_ref().unwrap().len() < sim.ride_media().len(),
        "the journal grew after the session"
    );
    let (origin, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert_eq!(gaps, [], "{}", rebuild::report(&gaps));

    let capture = Capture { origin, trace };
    let mut replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
    assert!(
        Session::of(&mut replayed) == session,
        "the same frames, and the same rides on both journals"
    );
}

#[test]
fn an_internal_journal_cleared_and_filled_again_since_starts_empty() {
    let (mut sim, trace, _) = recorded_session();
    let power_on = sim.ride_media();
    let (_, internal) = sim.command("RIDE INTERNAL STATUS");
    let upper = internal
        .split_whitespace()
        .find_map(|word| word.strip_prefix("slot="))
        .unwrap()
        .to_string();
    let (status, out) = sim.command(&format!("RIDE CLEAR CONFIRM {upper}"));
    assert_eq!(status, "ACCEPTED", "{out}");
    sim.advance(seconds(10));
    sim.fit_bulk(Presence::Missing);
    sim.device.restart().unwrap();
    sim.wait_usable();
    ride(&mut sim, 60, 150.0);
    sim.fit_bulk(Presence::Ready);
    sim.device.restart().unwrap();
    sim.wait_usable();
    let pulled = pull(&mut sim, &trace);
    let slots = pulled.internal.as_ref().unwrap();
    assert!(
        slots.len() >= 2 * 256 && power_on[..slots.len()] != slots[..],
        "the device holds as many slots as at power-on, and other rides"
    );

    let (_, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert!(
        gaps.iter()
            .any(|gap| matches!(gap, Gap::JournalReplaced { .. })),
        "{gaps:?}"
    );
    let report = rebuild::report(&gaps);
    assert!(
        report.contains("cleared since; it starts empty"),
        "{report}"
    );
}

/// The owned sector the card's RIDES volume starts at.
fn rides_start(sim: &mut Sim) -> u64 {
    use firmware_services::volume;
    let mut sector = [0u8; 512];
    let Ok(volume::Layout::Table(table)) = volume::read(&mut sim.device.bulk.host(), &mut sector)
    else {
        panic!("a volume table");
    };
    table.find(&volume::RIDES).unwrap().start
}

#[test]
fn a_ride_log_vana_could_not_use_at_power_on_is_reported() {
    let (mut sim, trace, _) = recorded_session_after(|sim| {
        let rides = rides_start(sim);
        sim.device.bulk.corrupt(rides);
        sim.device.bulk.corrupt(rides + 1);
    });
    assert!(
        rebuild::media(&trace).unwrap().contains("rides unusable\n"),
        "both superblocks lost over the ride's records"
    );
    let pulled = pull(&mut sim, &trace);
    assert!(pulled.rides.is_none(), "nothing to pull");
    let (_, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert!(gaps.contains(&Gap::RidesUnusable), "{gaps:?}");
}

#[test]
fn a_ride_log_vana_kept_read_only_at_power_on_is_reported() {
    let (mut sim, trace, _) = recorded_session_after(|sim| {
        sim.ride_at(28.0, 200.0, 140.0, 85.0);
        let (status, out) = sim.command("RIDE START");
        assert_eq!(status, "ACCEPTED", "{out}");
        sim.advance(seconds(30));
        let (_, status) = sim.command("RIDE BULK STATUS");
        let head: u64 = status
            .split_whitespace()
            .find_map(|word| word.strip_prefix("slot="))
            .unwrap()
            .parse()
            .unwrap();
        let record = rides_start(sim) + firmware_services::sector_log::DATA_START + head - 2;
        sim.device.bulk.corrupt(record);
    });
    let lines = rebuild::media(&trace).unwrap();
    assert!(
        lines.contains("rides uncertain "),
        "a damaged record before valid ones of the open ride: {lines}"
    );
    let pulled = pull(&mut sim, &trace);
    let (_, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert!(
        gaps.iter()
            .any(|gap| matches!(gap, Gap::RidesUncertain { .. })),
        "{gaps:?}"
    );
}

#[test]
fn a_ride_saved_again_since_leaves_the_facts_table_out() {
    use vana::ride::facts;
    let (mut sim, trace, _) = recorded_session();
    let table = sim.file(facts::TABLE).unwrap();
    let mut again = facts::decode_table(&table)
        .next()
        .expect("a ride before power-on");
    again.facts.recovery_bpm = Some(31);
    facts::save(&mut sim.device.bulk.host(), &again).unwrap();
    let pulled = pull(&mut sim, &trace);
    let now = pulled.files.as_ref().unwrap().files[facts::TABLE].1.clone();
    assert_eq!(
        now.len(),
        table.len(),
        "a ride saved again takes no more room"
    );

    let (_, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert!(
        matches!(&gaps[..], [Gap::File { name, now: Some(_), .. }] if name == facts::TABLE),
        "{gaps:?}"
    );
}

#[test]
fn a_terrain_pack_of_the_right_length_and_another_digest_is_left_out() {
    let (mut sim, trace, _) = recorded_session();
    let mut pulled = pull(&mut sim, &trace);
    let pack = pulled.terrain.as_mut().unwrap();
    pack[1000] ^= 1;
    let (origin, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    assert!(
        matches!(&gaps[..], [Gap::Terrain { tag, .. }] if tag == "CHILE-TERRAIN-V1"),
        "{gaps:?}"
    );
    let mut sector = [0u8; 512];
    let mut card = origin.bulk.host();
    let mut terrain = firmware_services::volume::open(
        &mut card,
        &firmware_services::volume::TERRAIN,
        &mut sector,
    )
    .unwrap();
    assert_eq!(
        firmware_services::pack::status(&mut terrain, &mut sector).unwrap(),
        None,
        "the card starts without a pack"
    );
}

#[test]
fn a_file_whose_read_failed_in_the_pull_is_reported_and_left_out() {
    let (mut sim, trace, _) = recorded_session();
    let mut pulled = pull(&mut sim, &trace);
    let files = pulled.files.as_mut().unwrap();
    files.files.remove(WORKOUT).unwrap();
    files.unread.insert(WORKOUT.into());
    let (_, gaps) = rebuild::rebuild(&trace, &through_dir(&pulled)).unwrap();
    assert!(
        matches!(&gaps[..], [Gap::FileUnread { name, .. }] if name == WORKOUT),
        "{gaps:?}"
    );
    let report = rebuild::report(&gaps);
    assert!(report.contains("its read failed in the pull"), "{report}");
}

#[test]
fn a_file_changed_after_the_session_is_reported_and_left_out() {
    let (mut sim, trace, _) = recorded_session();
    sim.put_file(WORKOUT, &workout(30));
    let pulled = pull(&mut sim, &trace);
    let (origin, gaps) = rebuild::rebuild(&trace, &pulled).unwrap();
    let revision = files_now(&mut sim).1[WORKOUT];
    assert!(
        matches!(&gaps[..], [Gap::File { name, now: Some(now), .. }] if name == WORKOUT && *now == revision),
        "{gaps:?}"
    );
    let report = rebuild::report(&gaps);
    assert!(
        report.contains(&format!("file {WORKOUT}: written again since power-on"))
            && report.contains("cannot be recovered"),
        "{report}"
    );
    let mut sector = [0u8; 512];
    let mut card = origin.bulk.host();
    let mut volume =
        firmware_services::volume::open(&mut card, &firmware_services::volume::FILES, &mut sector)
            .unwrap();
    let dir = firmware_services::files::mount(&mut volume, &mut sector).unwrap();
    let mut names = Vec::new();
    dir.list(&mut volume, &mut sector, |entry| {
        names.push(entry.name.as_str().to_string())
    })
    .unwrap();
    assert!(!names.iter().any(|name| name == WORKOUT), "{names:?}");
    assert!(names.iter().any(|name| name == "course.dat"), "{names:?}");

    let capture = Capture {
        origin,
        trace: trace.clone(),
    };
    let replayed = Sim::replay(&capture);
    let difference = first_difference(&trace, &replayed.trace()).expect("a header difference");
    assert!(
        difference.starts_with("header"),
        "only the file list differs: {difference}"
    );
}
