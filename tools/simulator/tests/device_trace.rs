//! A device's trace comes alone, without the media its session started on.
//! A strict replay runs on what the trace holds: the card's size and volume
//! table for its own trace file, every byte VANA read, and the TERRAIN pack
//! its header names, found in a directory of pack files.
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::replay::{Exclusion, Mode};
use shell_simulator::route::{Point, Route, to_gpx};
use shell_simulator::session::{Session, Step};
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::trace::file::{self, Media};
use std::path::{Path, PathBuf};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/terrain/tests/data")
        .join(name)
}

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
            let point = Point {
                ele: Some(ele),
                ..Point::new(lat, lon)
            };
            (point, None)
        })
        .collect();
    Route::from_gpx(&to_gpx("G-21", &points)).unwrap()
}

fn recorded(dir: &Path) -> PathBuf {
    let mut sim = Sim::new();
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    let pack = std::fs::read(data("farellones.terrain")).unwrap();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &pack);
    sim.power_loss();
    sim.ant_sensor(device_type::POWER, 1234);
    sim.ride_route(g21());
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(14.0, 240.0, 0.0, 80.0);
    while !sim.screen().contains("CLIMBING") {
        assert!(sim.world().distance_m < 300.0, "{}", sim.screen().text());
        sim.advance(seconds(1));
    }
    assert_eq!(sim.command("STORE STATUS").0, "OK");
    sim.advance(seconds(2));
    let path = dir.join("ride.trace");
    std::fs::write(&path, sim.capture().trace).unwrap();
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_trace_alone_replays_strict_with_the_pack_its_header_names() {
    let dir = scratch("device-trace");
    let trace = recorded(&dir);
    let (capture, media) = file::load(&trace).unwrap();
    assert_eq!(media, Media::Blank, "nothing was saved beside the trace");

    let packs = dir.join("packs");
    std::fs::create_dir(&packs).unwrap();
    let missing = Session::replaying_with(&capture, Mode::Strict, &packs)
        .err()
        .expect("no pack file to read TERRAIN from");
    let missing = missing.to_string();
    assert!(missing.contains("CHILE-TERRAIN-V1"), "{missing}");
    assert!(missing.contains(&packs.display().to_string()), "{missing}");

    std::fs::write(packs.join("other.terrain"), [7u8; 600]).unwrap();
    std::fs::copy(data("farellones.terrain"), packs.join("a-pack.terrain")).unwrap();
    let mut replay = Session::replaying_with(&capture, Mode::Strict, &packs).unwrap();
    let mut store = None;
    loop {
        match replay.replay_step() {
            Step::End => break,
            Step::Command { line, reply, .. } if line.ends_with("STORE STATUS") => {
                store = Some(reply);
            }
            _ => {}
        }
    }
    let report = replay.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    let faithful = report.faithful.unwrap();
    assert_eq!(faithful.ended, None, "{faithful}");
    assert!(
        matches!(faithful.exclusions[..], [Exclusion::TerrainByDigest(reads)] if reads > 0),
        "{faithful}"
    );
    let store = store.expect("the replay ran the command");
    assert!(
        store.contains(" trace=recording "),
        "the replay's own trace file is open on its card: {store}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
