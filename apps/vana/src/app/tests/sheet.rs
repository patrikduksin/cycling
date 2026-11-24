use super::touch::touch;
use super::*;
use crate::compose::builtin::ScreenId;
use crate::compose::input::{Action, Go};
use crate::compose::registry::{DAY_NIGHT, LEVEL, SHEET_SENSORS};
use crate::ride::log::Status;
use crate::screens::chrome::Rect;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;

fn pull(runtime: &mut Runtime, radio: &mut Radio, now: u64) {
    touch(
        runtime,
        radio,
        &[(120, 4), (121, 30), (122, 70), (122, 120)],
        now,
    );
}

fn tile(runtime: &Runtime, component: u8) -> Rect {
    let laid = runtime.composition.laids().get(ScreenId::ControlSheet);
    laid.and_then(|laid| laid.box_of(component))
        .expect("the sheet shows the tile")
}

fn centre(rect: Rect) -> (u16, u16) {
    (
        rect.center_x() as u16,
        rect.top.midpoint(rect.bottom) as u16,
    )
}

fn level(runtime: &Runtime) -> u8 {
    runtime.preferences.display.brightness
}

#[test]
fn a_pull_from_the_top_opens_the_sheet_over_a_recording_ride_and_takes_its_keys() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), 1000, 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);
    let laps = runtime.ride.laps().count();

    touch(
        &mut runtime,
        &mut radio,
        &[(120, 60), (120, 120), (120, 200)],
        2000,
    );
    assert!(!runtime.sheet_open());
    pull(&mut runtime, &mut radio, 3000);
    assert!(runtime.sheet_open());
    assert!(
        runtime.navigation.page() == Page::Ride,
        "the ride page stays under it"
    );

    let before = level(&runtime);
    runtime.input_received(press(Role::Move), 4000, 4000, &mut radio);
    assert_eq!(level(&runtime), before - 10);
    runtime.input_received(press(Role::Act), 5000, 5000, &mut radio);
    assert_eq!(level(&runtime), before);
    tick(&mut runtime, &mut journal, &mut radio, 5000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);
    assert_eq!(runtime.navigation.ride_page(), RidePage::Main);

    runtime.input_received(press(Role::Back), 6000, 6000, &mut radio);
    assert!(!runtime.sheet_open());
    assert_eq!(runtime.ride.laps().count(), laps);
    tick(&mut runtime, &mut journal, &mut radio, 7000);
    assert_eq!(runtime.ride.active().status(), Status::Recording);
}

#[test]
fn a_finger_sets_the_level_flips_the_look_and_opens_the_sensors() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    pull(&mut runtime, &mut radio, 1000);
    assert!(runtime.sheet_open());

    let bar = tile(&runtime, LEVEL);
    let y = centre(bar).1;
    let (left, right) = (bar.left as u16, bar.right as u16);
    touch(&mut runtime, &mut radio, &[(left + 2, y)], 2000);
    assert_eq!(level(&runtime), 10);
    let middle = left + (right - left) / 2;
    touch(
        &mut runtime,
        &mut radio,
        &[(left + 2, y), (middle, y)],
        3000,
    );
    assert_eq!(level(&runtime), 50, "a drag along the bar");
    touch(&mut runtime, &mut radio, &[(right - 1, y)], 4000);
    assert_eq!(level(&runtime), 100);
    assert!(runtime.sheet_open(), "the bar keeps the sheet open");

    let light = runtime.light();
    let (x, y) = centre(tile(&runtime, DAY_NIGHT));
    touch(&mut runtime, &mut radio, &[(x, y)], 5000);
    assert_eq!(runtime.light(), !light);

    let (x, y) = centre(tile(&runtime, SHEET_SENSORS));
    touch(&mut runtime, &mut radio, &[(x, y)], 6000);
    assert!(runtime.navigation.page() == Page::Sensors);
    assert!(!runtime.sheet_open());
}

#[test]
fn a_drag_up_or_a_tap_under_the_card_closes_the_sheet() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    pull(&mut runtime, &mut radio, 1000);
    touch(
        &mut runtime,
        &mut radio,
        &[(120, 250), (120, 220), (120, 160)],
        2000,
    );
    assert!(!runtime.sheet_open());
    pull(&mut runtime, &mut radio, 3000);
    touch(&mut runtime, &mut radio, &[(120, 250)], 4000);
    assert!(!runtime.sheet_open());
    assert!(
        runtime.navigation.page() == Page::Home,
        "the tap reached only the sheet"
    );
}

#[test]
fn a_tap_on_a_sign_tab_presses_its_key() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    pull(&mut runtime, &mut radio, 1000);
    let before = level(&runtime);
    let device = runtime.device;
    let tab = device.keys.of_role(Role::Move).tab(&device.scaled, true);
    touch(&mut runtime, &mut radio, &[centre(tab)], 2000);
    assert_eq!(level(&runtime), before - 10);
    assert!(runtime.sheet_open());
}

#[test]
fn a_frame_draws_the_sheet_over_the_dimmed_screen_and_its_bar_as_it_moves() {
    let mut screen = super::presentation::Screen::new();
    let mut runtime = booted(Profile::HeartRate);
    let mut radio = Radio::new();
    assert!(screen.present(&mut runtime, &radio, 3000));
    assert!(screen.present(&mut runtime, &radio, 3400));
    assert!(runtime.navigation.page() == Page::Home);
    let before = screen.panel.pixel(120, 300);
    pull(&mut runtime, &mut radio, 4000);
    assert!(screen.present(&mut runtime, &radio, 4200));
    let bar = tile(&runtime, LEVEL);
    let (x, y) = (bar.left + bar.width() * 4 / 5, bar.top + bar.height() / 2);
    let lit = screen.panel.pixel(x, y);
    for now in [5000, 5400, 5800] {
        runtime.input_received(press(Role::Move), now, now, &mut radio);
    }
    assert!(screen.present(&mut runtime, &radio, 5800));
    assert_ne!(screen.panel.pixel(x, y), lit, "the bar follows the level");
    runtime.input_received(press(Role::Back), 6200, 6200, &mut radio);
    assert!(screen.present(&mut runtime, &radio, 6200));
    assert_eq!(
        screen.panel.pixel(120, 300),
        before,
        "the screen shows again"
    );
}

#[test]
fn an_action_opens_the_sensor_list_for_home_s_status_widget() {
    let mut radio = Radio::new();
    let mut runtime = booted(Profile::HeartRate);
    runtime.navigation.go(To::Page(Page::Home));
    runtime.carry(
        Action::Go(Go::Open(Page::Sensors)),
        false,
        (1000, 1000),
        &mut radio,
    );
    assert!(runtime.navigation.page() == Page::Sensors);
}
