//! The course on the map page. The design scenario rides the course the map
//! mockups draw and checkpoints each map screen under its mockup's name in
//! `docs/design/course-navigation`, for `cycling mockup-compare`.
use super::*;
use device_api::server::Method;

const MOCKUP: &str = include_str!("../../../../docs/design/course-navigation/map-approach.svg");
/// map-approach is 400 m across with its rider at (80, 266), 250 m before
/// the turn.
const PX_PER_M: f64 = 240.0 / 400.0;
const RIDER: (f64, f64) = (80.0, 266.0);
/// Where the ride starts, south of map-approach's rider.
const LEAD_M: f64 = 1_000.0;
/// The course keeps no point within 5 m of the last; the mockup's points
/// are 3 to 6 m apart, so the route is walked again at this spacing.
const SPACING_M: f64 = 5.5;

/// The route the mockup draws, in metres east and north of the ride's
/// start, then on north so the course does not end in view.
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
            ((x - RIDER.0) / PX_PER_M, LEAD_M + (RIDER.1 - y) / PX_PER_M)
        })
        .collect();
    // The ride in, then each stretch between the drawing's corners at an
    // even spacing, so every corner is a point.
    let mut course = vec![(0.0, 0.0)];
    let mut stretch = vec![drawn[0]];
    for (k, &p) in drawn.iter().enumerate().skip(1) {
        stretch.push(p);
        if k + 1 == drawn.len() || bend(stretch[stretch.len() - 2], p, drawn[k + 1]) > 0.5 {
            course.extend(evenly(&stretch));
            stretch = vec![p];
        }
    }
    let &(east, north) = drawn.last().unwrap();
    course.push((east, north + 3_000.0));
    course
}

/// How far a line through `b` turns, in radians.
fn bend(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    let turn = (c.1 - b.1).atan2(c.0 - b.0) - (b.1 - a.1).atan2(b.0 - a.0);
    turn.sin().atan2(turn.cos()).abs()
}

/// Points along `stretch` at the even spacing nearest `SPACING_M` above,
/// ending at its end.
fn evenly(stretch: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let lengths: Vec<f64> = stretch
        .windows(2)
        .map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1))
        .collect();
    let total: f64 = lengths.iter().sum();
    let count = (total / SPACING_M).floor().max(1.0);
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

/// Ride the last metres to `metres` slowly and stand there a moment, so the
/// fix the map shows, which may trail a second behind, is where the rider
/// stands.
fn arrive(sim: &mut Sim, metres: f64) {
    ride_to(sim, metres - 20.0);
    sim.ride_at(4.0, 120.0, 150.0, 60.0);
    ride_to(sim, metres - 0.6);
    sim.ride_at(0.0, 0.0, 150.0, 0.0);
    sim.advance(seconds(2));
    sim.ride_at(36.0, 210.0, 150.0, 88.0);
}

/// A rider with every sensor who loaded `course` from the phone and rides
/// `route` at 36 km/h, 10 m a fix, on the map page.
fn on_the_map(course: &[(f64, f64)], route: &[(f64, f64)]) -> Sim {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.format_files();
    sim.connect_phone();
    let gpx = gpx_through("course", course);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", gpx.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.press(Button::TopLeft);
    sim.advance(seconds(1));
    sim.ride_route(Route::from_gpx(&gpx_through("route", route)).unwrap());
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(36.0, 210.0, 150.0, 88.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    sim.ride_page("5/5");
    sim
}

/// The badge's words, top-left over the map.
fn badge(sim: &Sim) -> Vec<String> {
    sim.screen()
        .frame()
        .read()
        .into_iter()
        .filter(|run| (30..90).contains(&run.y) && run.x < 160)
        .map(|run| run.text)
        .collect()
}

#[test]
fn the_map_steps_in_towards_a_turn_and_leads_back_to_it_once_missed() {
    let course = mockup_course();
    let route = [(0.0, 0.0), (0.0, LEAD_M + 600.0)];
    let mut sim = on_the_map(&course, &route);
    arrive(&mut sim, LEAD_M);
    assert_eq!(badge(&sim), ["240", "M"], "to where the turn starts");
    sim.expect_text("100 M");
    sim.checkpoint("map-approach");

    arrive(&mut sim, LEAD_M + 210.0);
    assert_eq!(badge(&sim), ["30", "M"]);
    sim.checkpoint("map-close");

    let off = ride_until_cue(&mut sim, "off_course", LEAD_M + 395.0);
    assert!(off < LEAD_M + 395.0, "{off}");
    arrive(&mut sim, LEAD_M + 390.0);
    assert_eq!(badge(&sim), ["140", "M", "TURN BACK"]);
    sim.checkpoint("map-offcourse");
}
