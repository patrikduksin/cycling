use super::*;

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
