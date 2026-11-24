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

#[test]
fn tapping_the_banner_opens_the_map() {
    let course = straight_then(1_000.0);
    let mut sim = on_course(&course, &course);
    sim.ride_at(30.0, 180.0, 150.0, 85.0);
    start_following(&mut sim);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/main"),
        "a loaded course keeps the map in the cycle"
    );
    ride_until_cue(&mut sim, "turn_soon", 6_900.0);
    sim.tap(120, 200);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/main"),
        "not on the sign"
    );
    sim.tap(120, 60);
    expect_map(&sim);
    sim.checkpoint("the banner tapped opens the map");
}
