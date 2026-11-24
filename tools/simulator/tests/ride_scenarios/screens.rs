use super::*;

/// The simulator's text reader matches light text on black, so it cannot read
/// these screens; the checks are on the colours the light look must produce.
#[test]
fn the_light_theme_turns_every_screen_white_and_keeps_its_colours() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    assert_eq!(corner(&sim), 0, "dark by default");
    sim.option("THEME", "DAY");
    sim.next_frame();
    assert_eq!(corner(&sim), WHITE, "Home turns white at once");
    let theme = vana::screens::theme::LIGHT;
    assert!(shows(&sim, theme.text), "black text on the white page");
    assert!(shows(&sim, theme.good), "the Ride button's green");
    sim.checkpoint("home in the light theme");
    sim.ride_at(28.0, 210.0, 142.0, 88.0);
    sim.start_ride();
    sim.advance(seconds(20));
    assert_eq!(corner(&sim), WHITE);
    // The Stats page draws in the light theme's own colours.
    assert!(shows(&sim, theme.speed) && shows(&sim, theme.cadence));
    sim.checkpoint("ride page in the light theme");
    sim.swipe_next();
    sim.advance(seconds(1));
    assert_eq!(corner(&sim), WHITE);
    sim.checkpoint("second ride page in the light theme");
}

#[test]
fn home_shows_the_time_the_battery_and_the_sensors_at_a_glance() {
    let mut sim = rider();
    sim.set_battery(14, false);
    sim.advance(seconds(2));
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.expect_text("Ride");
    sim.expect_text("14%");
    sim.expect_text("Wednesday 14 Oct");
    sim.expect_no_text("Just ride");
    sim.checkpoint("home before pairing");

    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.checkpoint("home with three live sensors");

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(10));
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.stop_ride();
    for _ in 0..3 {
        if sim.on_home() {
            break;
        }
        sim.press(Role::Back);
    }
    sim.wait_gone("RIDE SAVED", seconds(3)).unwrap();
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after a ride");
}

#[test]
fn tapping_home_s_widget_and_a_sensor_pairs_it_and_ride_starts() {
    let mut sim = rider();
    sim.tap(40, 125);
    sim.expect_text("NEARBY");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_no_text("NEXT");
    sim.checkpoint("sensors menu");
    sim.tap(120, 118);
    sim.expect_text("1 ADDED");
    sim.expect_text("POWER");
    sim.checkpoint("sensor connected by tap");
    sim.press(Role::Back);
    sim.wait_for("LIVE", seconds(15)).unwrap();
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after tapping a sensor");
    sim.tap(120, 273);
    sim.advance(seconds(1));
    assert_eq!(sim.vana_page(), "ride");
    assert_eq!(sim.ride_state(), "recording");
}

#[test]
fn status_bar_and_toasts_follow_the_ride_state() {
    let mut sim = rider();
    sim.set_battery(22, false);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 150.0, 88.0);
    sim.expect_text("RECORDING");
    sim.checkpoint("recording toast");
    sim.advance(seconds(5));
    sim.expect_no_text("RECORDING");
    sim.ride_page("ride/map");
    let rec = sim.screen().run("REC").expect("REC in the status bar");
    assert_eq!((rec.y < 24, rec.color), (true, RED));
    sim.checkpoint("recording, status bar only");
    sim.advance(minutes(1));
    sim.lap(1);
    sim.checkpoint("lap toast");
    sim.advance(seconds(3));

    sim.pause_ride();
    sim.advance(seconds(3));
    // The paused screen, then the ride pages with PAUSED in the status bar.
    let paused = sim.screen().run("Paused").expect("Paused");
    assert_eq!((paused.y > 24, paused.color), (true, AMBER));
    sim.checkpoint("paused screen");
    sim.show_ride_pages();
    let paused = sim
        .screen()
        .run("PAUSED")
        .expect("PAUSED in the status bar");
    assert_eq!((paused.y < 24, paused.color), (true, AMBER));
    sim.checkpoint("paused, ride pages");

    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.checkpoint("end ride?");
    sim.press(Role::Act);
    sim.wait_for("Ride saved", seconds(5)).unwrap();
    sim.checkpoint("ride done");
    assert!(sim.screen().run("REC").is_none());
}

#[test]
fn swipes_page_through_the_ride_and_taps_never_control_it() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(minutes(2));
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));

    sim.swipe_next();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
    sim.checkpoint("swiped to climb");
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/main"));
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/map"), "wraps");
    expect_map(&sim);
    sim.swipe_previous();
    assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
    sim.swipe_next();
    sim.swipe_next();
    sim.expect_text("KM/H");

    // Taps where the buttons are, and on readings, change nothing.
    for (x, y) in [(20, 300), (220, 300), (20, 20), (120, 80), (120, 150)] {
        sim.tap(x, y);
    }
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    sim.expect_no_text("End ride?");
    sim.checkpoint("still recording after taps");
}

#[test]
fn bare_runs_on_buttons_alone() {
    use device_api::input::InputSource;
    use device_api::observation::Availability;
    use shell_simulator::models::BARE;
    use shell_simulator::replay::first_difference;

    let mut sim = Sim::on(BARE).boot();
    let frame = sim.screen();
    assert_eq!((frame.width, frame.height), (240, 320));
    let controls = sim.device.input.controls();
    assert_eq!(controls.touch, Availability::Unsupported);
    assert_eq!(controls.buttons, BARE.buttons);
    sim.checkpoint("home on bare");

    let mut changed = 0;
    for button in BARE
        .buttons
        .as_slice()
        .iter()
        .filter_map(|fixture| fixture.role)
    {
        let before = sim.screen().text();
        sim.press(button);
        sim.advance(seconds(1));
        changed += usize::from(sim.screen().text() != before);
        sim.press(Role::Back);
        sim.advance(seconds(1));
    }
    assert!(changed > 0, "no button changed the screen");

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture).unwrap();
    assert_eq!(header.device, "bare");
    let replayed = Sim::replay(&capture);
    assert_eq!(replayed.device.model, BARE);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}
