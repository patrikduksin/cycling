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
