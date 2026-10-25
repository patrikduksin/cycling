//! CLIMB AHEAD: VANA reads the road ahead from an offline terrain pack in the
//! MMC TERRAIN volume and shows its climbs before and while riding them.
#![cfg(feature = "cycling")]
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::route::{Point, Route, to_gpx};
use shell_simulator::sim::{NoteKind, Sim, seconds};

const SPEED: u16 = 777;
const POWER: u16 = 1234;

fn data(name: &str) -> String {
    format!(
        "{}/../../packages/terrain/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Route G-21 from Santiago's edge up to Farellones, as the pack has it.
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

/// A rider at the foot of G-21 with the Farellones pack installed.
fn rider() -> Sim {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    let pack = std::fs::read(data("farellones.terrain")).unwrap();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &pack);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(g21());
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["SPEED/CAD", "POWER METER"]);
    sim
}

fn ride_to(sim: &mut Sim, metres: f64) {
    while sim.world().distance_m < metres {
        sim.advance(seconds(1));
    }
}

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

/// A number shown after `label`, such as the remaining kilometres.
fn field(sim: &Sim, label: &str) -> f64 {
    let value = sim
        .screen()
        .field(label)
        .unwrap_or_else(|| panic!("no {label}; screen:\n{}", sim.screen().text()));
    value
        .trim_end_matches('%')
        .parse()
        .unwrap_or_else(|_| panic!("{label} shows {value:?}"))
}

#[test]
fn the_map_shows_the_climbs_up_g21_before_and_while_riding_them() {
    let mut sim = rider();
    sim.start_ride();
    sim.ride_at(14.0, 240.0, 0.0, 80.0);
    // G-21 starts on a short rise to about 410 m: the map has the rider on
    // a climb from the first fixes, and brings up the climb page.
    ride_until_shown(&mut sim, "CLIMBING", 100.0);
    assert_eq!(sim.screen().page().as_deref(), Some("4/6"));
    // Over its top it shows the first ramp, 700 m from the start, once the
    // map is sure of the road again past a fork.
    let shown = ride_until_shown(&mut sim, "CLIMB IN", 600.0);
    assert!((400.0..550.0).contains(&shown), "at {shown:.0} m");
    assert_eq!(sim.screen().page().as_deref(), Some("4/6"));
    assert!(sim.screen().field("LENGTH KM").is_some());
    for label in ["AVG", "GAIN M", "POWER", "HEART", "0% UP"] {
        assert!(
            sim.screen().contains(label),
            "{label}:\n{}",
            sim.screen().text()
        );
    }
    sim.checkpoint("climb ahead from the map at the foot of G-21");

    // On the ramp the page counts down to its top, the rider moving along
    // the whole climb.
    let on = ride_until_shown(&mut sim, "CLIMBING", 1_200.0);
    assert!((600.0..1_100.0).contains(&on), "at {on:.0} m");
    let first = field(&sim, "UP TO GO M");
    sim.checkpoint("climb page counting down to the top");
    ride_to(&mut sim, on + 150.0);
    sim.expect_text("CLIMBING");
    let later = field(&sim, "UP TO GO M");
    assert!(later < first, "{later} m to go, was {first}");
    assert!(!sim.screen().contains("0% UP"), "{}", sim.screen().text());
    sim.checkpoint("climb page nearer the top");
}
