use super::*;

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

/// The whole ride lifecycle as a rider meets it: the start from Home, a lap,
/// pausing and paging, a change of mind at "End ride?", then saving it to
/// Ride done and leaving that for Home.
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

/// Riding on at Ride done starts nothing; the next ride the rider starts is
/// free of the workout the last one played, and a ride of nothing still saves.
#[test]
fn the_ride_after_a_workout_is_a_free_ride() {
    let mut sim = training_rider();
    sim.start_workout(&vana::workout::fixtures::FTP_TWENTY);
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(sim.vana_page(), "ride_done");
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(10));
    assert_eq!(sim.vana_page(), "ride_done");
    assert_eq!(sim.rides().len(), 1);
    sim.checkpoint("riding on at ride done starts nothing");
    sim.done();
    sim.start_ride();
    assert_eq!(sim.ride_state(), "recording");
    assert_eq!(sim.rides().len(), 2);
    assert_ne!(
        sim.ride_page_name().as_deref(),
        Some("ride/workout"),
        "a free ride"
    );
    sim.checkpoint("the next ride is free");

    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.stop_ride();
    sim.expect_text("Ride saved");
    assert_eq!(sim.rides().len(), 2);
}

#[test]
fn a_started_ride_auto_pauses_at_a_stop_and_resumes_riding_on() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(20.0, 0.0, 0.0, 80.0);
    assert_eq!(sim.vana_page(), "ride");
    sim.checkpoint("started from home");
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
