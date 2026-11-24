use super::*;

fn marks_on_a_ride(sim: &mut Sim) {
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.hold(Role::Move, 1000);
    let page = sim.ride_page_name().expect("a page");
    assert_eq!(page, "ride/climb", "one page on, not two");
    sim.advance(seconds(3));
    assert!(marks(sim).is_empty(), "a hold of a second marks nothing");
    sim.expect_no_text("MARK");

    let pressed = sim.device.clock_ms();
    let from = sim.device.sim().speaker.played.len();
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(
        sim.ride_page_name().as_ref(),
        Some(&page),
        "the page put back"
    );
    assert_eq!(
        marks(sim),
        [format!("1 pressed {} on {page}", session_clock(pressed))]
    );
    sim.checkpoint("marked");
    sim.advance(seconds(3));
    assert_eq!(played_since(sim, from), [0, 0, 0], "middle, middle, middle");
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 2");
    assert_eq!(sim.ride_page_name(), Some(page), "the page put back");
    assert_eq!(marks(sim).len(), 2);
    sim.advance(seconds(30));
    sim.stop_ride();
}

#[test]
fn holding_move_marks_the_moment_and_leaves_the_ride_as_it_was() {
    let mut sim = rider();
    marks_on_a_ride(&mut sim);
    let toasts = traced(&sim, Traced::Toast);
    assert!(
        !toasts.iter().any(|toast| toast.starts_with("LAP")),
        "a mark takes no lap: {toasts:?}"
    );
    let states = traced(&sim, Traced::Ride);
    let paused = states.iter().filter(|state| *state == "paused").count();
    assert_eq!(paused, 1, "only the pause that ends the ride: {states:?}");
}

#[test]
fn holding_move_on_a_ride_page_steps_one_page_however_long_it_is_held() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.hold(Role::Move, 1400);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/climb"),
        "one page on, not two"
    );
    sim.checkpoint("held-move-one-page");
    sim.advance(seconds(1));
    sim.press(Role::Move);
    assert_eq!(
        sim.ride_page_name().as_deref(),
        Some("ride/map"),
        "a new press still pages"
    );
}

#[test]
fn holding_move_on_home_marks_and_keeps_the_focus() {
    let mut sim = rider();
    sim.advance(seconds(3));
    assert!(sim.on_home(), "{}", sim.screen().text());
    let ring = |sim: &Sim| sim.screen().pixels[232 * 240 + 120];
    let focus = vana::screens::theme::DARK.focus;
    assert_eq!(ring(&sim), focus);
    let pressed = sim.device.clock_ms();
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert_eq!(ring(&sim), focus, "Ride keeps the focus");
    assert_eq!(
        marks(&sim),
        [format!("1 pressed {} on home", session_clock(pressed))]
    );
}

/// A pause the rider paged away from, then a resume: the paused screen
/// stays hidden in the view until the next pause brings it back.
fn paused_and_paged_away(sim: &mut Sim) {
    sim.pause_ride();
    sim.show_ride_pages();
    sim.resume_ride();
    sim.advance(seconds(5));
}

#[test]
fn pausing_during_a_mark_keeps_the_paused_screen() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    paused_and_paged_away(&mut sim);
    sim.hold_while(Role::Move, 1600, |sim, held| {
        if held == 500 {
            sim.report(Role::Act);
        }
    });
    sim.expect_text("MARK 1");
    assert_eq!(sim.ride_state(), "paused");
    assert_eq!(sim.seen().page.expect("a page").1, "ride/paused");
    assert_eq!(marks(&sim).len(), 1);
}

/// How long after `setup` its ride first meets `reached`, stepping a tenth
/// of a second.
fn ms_until(setup: fn() -> Sim, reached: fn(&mut Sim) -> bool) -> u64 {
    let mut sim = setup();
    let mut waited = 0;
    while !reached(&mut sim) {
        assert!(waited < seconds(120), "never reached");
        sim.advance(100);
        waited += 100;
    }
    waited
}

fn stopping_at_a_light() -> Sim {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    paused_and_paged_away(&mut sim);
    sim.advance(seconds(30));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim
}

#[test]
fn an_auto_pause_during_a_mark_keeps_the_paused_screen() {
    let auto_paused = |sim: &mut Sim| {
        sim.seen()
            .ride
            .is_some_and(|(_, state)| state == "auto_paused")
    };
    let pauses = ms_until(stopping_at_a_light, auto_paused);
    let mut sim = stopping_at_a_light();
    sim.advance(pauses - 700);
    assert!(!auto_paused(&mut sim));
    sim.hold(Role::Move, 1600);
    sim.expect_text("MARK 1");
    assert!(auto_paused(&mut sim));
    assert_eq!(sim.seen().page.expect("a page").1, "ride/paused");
}

#[test]
fn input_lost_during_a_hold_marks_nothing() {
    let mut sim = rider();
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    sim.hold_while(Role::Move, 1600, |sim, held| {
        if held == 500 {
            let at = sim.device.clock_ms();
            sim.device.input.push(at, device_api::input::Input::Cancel);
        }
    });
    sim.advance(seconds(1));
    sim.expect_no_text("MARK");
    assert!(marks(&sim).is_empty(), "{:?}", marks(&sim));
}

#[test]
fn a_move_that_closes_the_power_sheet_marks_nothing_however_long_it_is_held() {
    let mut sim = rider();
    sim.advance(seconds(3));
    sim.press(Role::Move);
    assert!(!sim.screen().white("RIDE"), "{}", sim.screen().text());
    sim.advance(seconds(5));
    sim.press(Role::Back);
    sim.expect_text("TURN OFF");
    sim.hold(Role::Move, 1600);
    sim.expect_no_text("TURN OFF");
    sim.advance(seconds(1));
    sim.expect_no_text("MARK");
    assert!(marks(&sim).is_empty(), "{:?}", marks(&sim));
    assert!(
        !sim.screen().white("RIDE"),
        "the focus stays where the tap left it: {}",
        sim.screen().text()
    );
}

#[test]
fn a_ride_with_marks_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), marks_on_a_ride, false);
}
