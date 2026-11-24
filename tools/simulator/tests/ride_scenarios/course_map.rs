use super::*;
use device_api::server::Method;
use vana::screens::chrome::BLACK;

const MOCKUP: &str = include_str!("../../../../docs/design/course-navigation/map-approach.svg");
const SANTIAGO: &[u8] =
    include_bytes!("../../../../packages/terrain/tests/data/santiago-centro.terrain");
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
        if k + 1 == drawn.len() || turn_angle(stretch[stretch.len() - 2], p, drawn[k + 1]) > 0.5 {
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
    include_str!("../../../../packages/terrain/tests/data/santiago-centro-course.gpx");
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
