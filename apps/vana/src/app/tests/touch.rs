use super::*;
use crate::ride::log::Status;
use crate::screens::home::SENSORS_TILE;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;
use device_api::input::Point;

fn touch(runtime: &mut Runtime, radio: &mut Radio, points: &[(u16, u16)], now: u64) {
    for (i, &(x, y)) in points.iter().enumerate() {
        runtime.input(Input::Touch(Point { x, y }), now + 30 * i as u64, radio);
    }
    runtime.input(Input::Release, now + 30 * points.len() as u64, radio);
}

#[test]
fn tapping_a_home_tile_opens_it() {
    let mut radio = Radio::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = Page::Home;
    touch(&mut runtime, &mut radio, &[(120, 40)], 1000);
    assert!(runtime.navigation.page == Page::Ready);
    runtime.navigation.page = Page::Home;
    let y = (SENSORS_TILE.top + 20) as u16;
    touch(&mut runtime, &mut radio, &[(120, y)], 2000);
    assert!(runtime.navigation.page == Page::Sensors);
    assert_eq!(runtime.navigation.home_cursor, 1);
}

#[test]
fn swipes_change_ride_pages_and_touch_never_controls_the_ride() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);

    touch(
        &mut runtime,
        &mut radio,
        &[(200, 150), (140, 152), (60, 155)],
        2000,
    );
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);
    touch(&mut runtime, &mut radio, &[(40, 150), (200, 150)], 3000);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    touch(&mut runtime, &mut radio, &[(40, 150), (200, 150)], 4000);
    assert_eq!(
        runtime.navigation.ride_page,
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
    assert!(runtime.navigation.page == Page::Ride);
}

#[test]
fn state_changes_are_announced_once_in_a_toast() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
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
        .announce(runtime.ride.active().status(), 1000);
    assert_eq!(text(&mut runtime, 1000).as_deref(), Some("RECORDING"));
    runtime.navigation.announce(Status::Recording, 1500);
    assert_eq!(text(&mut runtime, 3100), None, "expired and not repeated");
    // A lap outranks the confirmation it follows.
    runtime.input(press(Button::TopLeft), 3200, &mut radio);
    let lap = text(&mut runtime, 3200).unwrap();
    assert!(lap.starts_with("LAP 1 "), "{lap}");
    runtime.input(press(Button::BottomRight), 6000, &mut radio);
    for step in 0..4 {
        tick(&mut runtime, &mut journal, &mut radio, 6000 + step * 50);
    }
    assert_eq!(runtime.ride.active().status(), Status::Paused);
    // The paused screen says PAUSED; no toast repeats it.
    runtime.navigation.announce(Status::Paused, 6300);
    assert_eq!(text(&mut runtime, 6300), None);
}

#[test]
fn a_ride_without_wheel_speed_notes_auto_pause_is_off_once() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
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
    let mut runtime = Runtime::new(Profile::HeartRate);
    let down = |runtime: &mut Runtime, radio: &mut Radio, (x, y), now| {
        runtime.input(Input::Touch(Point { x, y }), now, radio)
    };
    let sensors = (120, (SENSORS_TILE.top + 20) as u16);

    // A finger down on Sensors while BACK returns Home lifts over the RIDE
    // card: Home never saw it begin, so it opens nothing.
    runtime.navigation.page = Page::Sensors;
    down(&mut runtime, &mut radio, (120, 40), 1000);
    runtime.input(press(Button::TopLeft), 1100, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
    runtime.input(Input::Release, 1200, &mut radio);
    assert!(runtime.navigation.page == Page::Home);

    // A contact that began on Home ends while the TURN OFF sheet is open;
    // after the sheet is dismissed the very next tap counts.
    down(&mut runtime, &mut radio, sensors, 2000);
    runtime.input(press(Button::TopLeft), 2100, &mut radio);
    assert!(runtime.power.view().is_some());
    runtime.input(Input::Release, 2200, &mut radio);
    down(&mut runtime, &mut radio, sensors, 2300);
    runtime.input(press(Button::BottomLeft), 2600, &mut radio);
    assert!(runtime.power.view().is_none());
    runtime.input(Input::Release, 2700, &mut radio);
    assert!(runtime.navigation.page == Page::Home, "began on the sheet");
    touch(&mut runtime, &mut radio, &[sensors], 3000);
    assert!(runtime.navigation.page == Page::Sensors);

    // An idle Cancel, as on companion framing loss, loses no later tap.
    runtime.navigation.page = Page::Home;
    runtime.input(Input::Cancel, 4000, &mut radio);
    touch(&mut runtime, &mut radio, &[sensors], 4100);
    assert!(runtime.navigation.page == Page::Sensors);
}

/// A finger dragged from `from` to `to` over `ms`, reported every 10 ms as
/// the C606's touch poll does, rested `hold_ms`, then lifted.
fn drag(
    runtime: &mut Runtime,
    radio: &mut Radio,
    from: (u16, u16),
    to: (u16, u16),
    ms: u64,
    hold_ms: u64,
    now: u64,
) -> u64 {
    let steps = (ms / 10).max(1);
    for step in 0..=steps {
        let at = |a: u16, b: u16| {
            (i64::from(a) + (i64::from(b) - i64::from(a)) * step as i64 / steps as i64) as u16
        };
        let point = Point {
            x: at(from.0, to.0),
            y: at(from.1, to.1),
        };
        let time = now + step * 10;
        runtime.navigation.touches.tick(time);
        runtime.input(Input::Touch(point), time, radio);
    }
    let lift = now + steps * 10 + hold_ms;
    runtime.navigation.touches.tick(lift);
    runtime.input(Input::Release, lift, radio);
    lift
}

/// The Workouts page from its top, with its built-in workouts.
fn workouts() -> (Runtime, Radio) {
    let radio = Radio::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = Page::Workouts;
    runtime.show_focus(List::Workouts, 0);
    assert!(
        runtime.training.rows() >= 8,
        "a list longer than the screen"
    );
    (runtime, radio)
}

/// Screen row of card `index`'s middle with the page at `offset`.
fn card_y(index: usize, offset: i32) -> u16 {
    let layout = workouts::LAYOUT;
    let (top, bottom) = layout.span(index);
    (layout.top as i32 + (top + bottom) / 2 - offset) as u16
}

use crate::screens::touch::{Control, List};
use crate::screens::workouts;

#[test]
fn scrolling_from_a_card_never_focuses_or_chooses_it() {
    let (mut runtime, mut radio) = workouts();
    let start = card_y(2, 0);
    // The finger rests on the third card until it lights up...
    runtime.input(Input::Touch(Point { x: 120, y: start }), 1000, &mut radio);
    runtime.navigation.touches.tick(1000 + 160);
    assert_eq!(
        runtime.navigation.touches.pressed(),
        Some(Control::Workout(2))
    );
    // ...then drags the list up and lets go still.
    let lift = drag(
        &mut runtime,
        &mut radio,
        (120, start),
        (120, start - 150),
        400,
        200,
        1200,
    );
    assert_eq!(
        runtime.navigation.touches.pressed(),
        None,
        "the scroll took it"
    );
    runtime.list_frame(lift + 2000);
    assert_eq!(
        runtime.list_offset(List::Workouts, lift + 2000),
        140,
        "1:1 less the slop"
    );
    assert!(runtime.navigation.page == Page::Workouts, "nothing chosen");
    assert_eq!(runtime.training.picker_cursor, 0, "the focus stays put");
    assert!(runtime.training.selected.is_none());
}

#[test]
fn a_press_first_brings_a_focus_that_scrolled_away_back() {
    let (mut runtime, mut radio) = workouts();
    let lift = drag(
        &mut runtime,
        &mut radio,
        (120, 290),
        (120, 90),
        400,
        200,
        1000,
    );
    runtime.list_frame(lift + 2000);
    let offset = runtime.list_offset(List::Workouts, lift + 2000);
    let layout = workouts::LAYOUT;
    assert!(!layout.whole(0, offset), "the focus scrolled away");
    // ACT scrolls the focus back into view and chooses nothing.
    runtime.input(press(Button::BottomRight), lift + 2100, &mut radio);
    assert!(runtime.navigation.page == Page::Workouts);
    runtime.list_frame(lift + 4000);
    assert_eq!(runtime.list_offset(List::Workouts, lift + 4000), 0);
    // Scrolled away again, MOVE focuses the first whole card in view
    // without scrolling; ACT then chooses it.
    let lift = drag(
        &mut runtime,
        &mut radio,
        (120, 290),
        (120, 90),
        400,
        200,
        lift + 5000,
    );
    runtime.list_frame(lift + 2000);
    let offset = runtime.list_offset(List::Workouts, lift + 2000);
    let first = layout.first_whole(runtime.training.rows(), offset).unwrap();
    runtime.input(press(Button::BottomLeft), lift + 2100, &mut radio);
    assert_eq!(runtime.training.picker_cursor, first);
    runtime.list_frame(lift + 3000);
    assert_eq!(runtime.list_offset(List::Workouts, lift + 3000), offset);
    runtime.input(press(Button::BottomRight), lift + 3500, &mut radio);
    assert!(runtime.navigation.page == Page::Ready);
}

#[test]
fn a_tap_chooses_its_card_but_a_tap_that_stops_a_coasting_list_does_not() {
    let (mut runtime, mut radio) = workouts();
    // A flick: the list coasts on after the finger lifts.
    let lift = drag(
        &mut runtime,
        &mut radio,
        (120, 280),
        (120, 180),
        50,
        0,
        1000,
    );
    runtime.list_frame(lift + 100);
    assert!(runtime.kinetic(List::Workouts).moving());
    // A tap mid-list on the moving list stops it and chooses nothing.
    touch(&mut runtime, &mut radio, &[(120, 200)], lift + 100);
    assert!(runtime.navigation.page == Page::Workouts);
    runtime.list_frame(lift + 300);
    assert!(!runtime.kinetic(List::Workouts).moving());
    // On the list at rest, a tap chooses the card under it and focuses it.
    let offset = runtime.list_offset(List::Workouts, lift + 300);
    let layout = workouts::LAYOUT;
    let card = layout.first_whole(runtime.training.rows(), offset).unwrap();
    touch(
        &mut runtime,
        &mut radio,
        &[(120, card_y(card, offset))],
        lift + 400,
    );
    assert!(runtime.navigation.page == Page::Ready);
    assert_eq!(runtime.training.picker_cursor, card);
}
