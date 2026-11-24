use super::*;
use crate::app::navigation::To;
use crate::ride::log::Status;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;
use device_api::input::Point;

pub(super) fn touch(runtime: &mut Runtime, radio: &mut Radio, points: &[(u16, u16)], now: u64) {
    for (i, &(x, y)) in points.iter().enumerate() {
        runtime.input_received(
            Input::Touch(Point { x, y }),
            now + 30 * i as u64,
            now + 30 * i as u64,
            radio,
        );
    }
    runtime.input_received(
        Input::Release,
        now + 30 * points.len() as u64,
        now + 30 * points.len() as u64,
        radio,
    );
}

fn widget(runtime: &mut Runtime) -> (u16, u16) {
    let rect = runtime.stop_rect(0);
    (
        rect.center_x() as u16,
        ((rect.top + rect.bottom) / 2) as u16,
    )
}

#[test]
fn tapping_home_s_status_widget_opens_the_sensors() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    touch(&mut runtime, &mut radio, &[(120, 40)], 1000);
    assert!(
        runtime.navigation.page() == Page::Home,
        "the clock does nothing"
    );
    let widget = widget(&mut runtime);
    touch(&mut runtime, &mut radio, &[widget], 2000);
    assert!(runtime.navigation.page() == Page::Sensors);
}

#[test]
fn swipes_change_ride_pages_and_touch_never_controls_the_ride() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), 1000, 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);

    touch(
        &mut runtime,
        &mut radio,
        &[(200, 150), (140, 152), (60, 155)],
        2000,
    );
    assert_eq!(runtime.navigation.ride_page(), RidePage::Climb);
    touch(&mut runtime, &mut radio, &[(40, 150), (200, 150)], 3000);
    assert_eq!(runtime.navigation.ride_page(), RidePage::Main);
    touch(&mut runtime, &mut radio, &[(40, 150), (200, 150)], 4000);
    assert_eq!(
        runtime.navigation.ride_page(),
        *RidePage::ALL.last().unwrap(),
        "wraps"
    );

    // Taps anywhere, including where buttons sit, leave the ride alone.
    for (i, (x, y)) in [(20, 300), (220, 300), (120, 160), (20, 20)]
        .into_iter()
        .enumerate()
    {
        touch(&mut runtime, &mut radio, &[(x, y)], 5000 + 500 * i as u64);
        tick(
            &mut runtime,
            &mut journal,
            &mut radio,
            5000 + 500 * i as u64,
        );
    }
    assert_eq!(runtime.ride.active().status(), Status::Recording);
    assert!(runtime.navigation.page() == Page::Ride);
}

#[test]
fn state_changes_are_announced_once_in_a_toast() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), 1000, 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    // Wheel speed, so auto-pause has nothing to say.
    runtime.readings.live.speed_mm_s = Some(5_000);
    let text = |runtime: &mut Runtime, now| {
        runtime
            .toast(now)
            .map(|toast| std::string::String::from_utf8(toast.text().to_vec()).unwrap())
    };
    runtime
        .navigation
        .announce(runtime.ride.active().status(), false, false, 1000);
    assert_eq!(text(&mut runtime, 1000).as_deref(), Some("RECORDING"));
    runtime
        .navigation
        .announce(Status::Recording, false, false, 1500);
    assert_eq!(text(&mut runtime, 3100), None, "expired and not repeated");
    // A lap outranks the confirmation it follows.
    runtime.input_received(press(Role::Back), 3200, 3200, &mut radio);
    let lap = text(&mut runtime, 3200).unwrap();
    assert!(lap.starts_with("LAP 1 "), "{lap}");
    runtime.input_received(press(Role::Act), 6000, 6000, &mut radio);
    for step in 0..4 {
        tick(&mut runtime, &mut journal, &mut radio, 6000 + step * 50);
    }
    assert_eq!(runtime.ride.active().status(), Status::Paused);
    // The paused screen says PAUSED; no toast repeats it.
    runtime
        .navigation
        .announce(Status::Paused, false, false, 6300);
    assert_eq!(text(&mut runtime, 6300), None);
}

#[test]
fn a_ride_without_wheel_speed_notes_auto_pause_is_off_once() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), 1000, 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);
    let text = |runtime: &mut Runtime, now| {
        runtime
            .toast(now)
            .map(|toast| std::string::String::from_utf8(toast.text().to_vec()).unwrap())
    };
    assert_eq!(text(&mut runtime, 4000).as_deref(), Some("AUTO PAUSE OFF"));
    assert_eq!(text(&mut runtime, 10_000), None);
}

#[test]
fn a_contact_never_completes_on_another_screen() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    let down = |runtime: &mut Runtime, radio: &mut Radio, (x, y), now| {
        runtime.input_received(Input::Touch(Point { x, y }), now, now, radio)
    };
    runtime.navigation.go(To::Page(Page::Home));
    let sensors = widget(&mut runtime);

    let mut screen = super::presentation::Screen::new();
    runtime.power(&mut screen.shell, &mut super::power::Switch::new(true), 0);
    runtime.navigation.go(To::Page(Page::Sensors));
    down(&mut runtime, &mut radio, (120, 40), 1000);
    runtime.input_received(press(Role::Back), 1100, 1100, &mut radio);
    assert!(runtime.navigation.page() == Page::Home);
    runtime.input_received(Input::Release, 1200, 1200, &mut radio);
    assert!(runtime.navigation.page() == Page::Home);

    // A contact that began on Home ends while the TURN OFF sheet is open;
    // after the sheet is dismissed the very next tap counts.
    down(&mut runtime, &mut radio, sensors, 2000);
    runtime.input_received(press(Role::Back), 2100, 2100, &mut radio);
    assert!(runtime.power.view().is_some());
    runtime.input_received(Input::Release, 2200, 2200, &mut radio);
    down(&mut runtime, &mut radio, sensors, 2300);
    runtime.input_received(press(Role::Move), 2600, 2600, &mut radio);
    assert!(runtime.power.view().is_none());
    runtime.input_received(Input::Release, 2700, 2700, &mut radio);
    assert!(
        runtime.navigation.page() == Page::Home,
        "began on the sheet"
    );
    touch(&mut runtime, &mut radio, &[sensors], 3000);
    assert!(runtime.navigation.page() == Page::Sensors);

    // An idle Cancel, as on companion framing loss, loses no later tap.
    runtime.navigation.go(To::Page(Page::Home));
    runtime.input_received(Input::Cancel, 4000, 4000, &mut radio);
    touch(&mut runtime, &mut radio, &[sensors], 4100);
    assert!(runtime.navigation.page() == Page::Sensors);
}

#[test]
fn a_tap_off_every_tile_and_key_tab_of_a_static_screen_does_nothing() {
    let mut runtime = booted(Profile::HeartRate);
    let panel = runtime.device.scaled.panel;
    for page in [Page::Boot, Page::Home] {
        runtime.navigation.go(To::Page(page));
        runtime.compose_shown();
        let laid = runtime.composition.laids().get(runtime.screen_id());
        let laid = laid.expect("the screen has a tree");
        let solved = laid.solved();
        let tabs = laid.controls().shown(laid.focus());
        let tiles: std::vec::Vec<_> = solved
            .focus_order()
            .map(|placed| placed.rect.rect())
            .chain(tabs.map(|cue| cue.target(runtime.device)))
            .collect();
        for y in 0..panel.height {
            for x in (0..panel.width).filter(|&x| !tiles.iter().any(|tile| tile.contains(x, y))) {
                let point = Point {
                    x: x as u16,
                    y: y as u16,
                };
                let hit = runtime.hit_test(point, 0, false);
                assert_eq!(hit.control, None, "{page:?}: a tap at {x},{y}");
            }
        }
    }
}

#[test]
fn a_move_hold_before_home_s_first_frame_back_marks_and_home_opens_on_its_start() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    runtime.focus_stop(0);
    runtime.input_received(press(Role::Act), 1000, 1000, &mut radio);
    assert_eq!(runtime.navigation.page(), Page::Sensors);
    runtime.compose_shown();
    runtime.input_received(press(Role::Back), 2000, 2400, &mut radio);
    assert_eq!(runtime.navigation.page(), Page::Home);
    // The hold starts before any frame shows Home again.
    for now in (2400..=3900).step_by(100) {
        runtime.input_received(press(Role::Move), now, now, &mut radio);
    }
    assert_eq!(runtime.toast_at(3900).unwrap().text(), b"MARK 1");
    assert_eq!(runtime.focus_index(), 1, "Home opens on Ride again");
}
