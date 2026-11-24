use super::*;

/// The ride: the course, with a wrong turn 400 m along its third leg, 175 m
/// north and back, and on past the end for a little.
fn course_ride_gpx() -> String {
    let mut legs = COURSE_LOOP[..3].to_vec();
    legs.extend([(900.0, 1275.0), (905.0, 1450.0), (920.0, 1274.0)]);
    legs.extend_from_slice(&COURSE_LOOP[3..]);
    legs.push((0.0, 300.0));
    gpx_through("loop with a wrong turn", &legs)
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
