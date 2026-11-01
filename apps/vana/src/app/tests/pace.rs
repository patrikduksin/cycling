//! The rest after a pass, as composition reads it: a still Home rests until
//! its next frame or `STILL_REST_MS`, and each kind of work alone keeps the
//! busy pace.
use super::presentation::Screen;
use super::*;
use crate::app::cues::Cue;
use crate::app::pace::{BUSY_REST_MS, STILL_REST_MS};
use crate::screens::workout::Page;
use device_api::input::Point;
use device_api::power::State;

const NOW: u64 = 5_000;

/// Home drawn at `NOW`, with nothing to do.
fn still_home() -> (Runtime, Screen, Radio) {
    let (mut runtime, mut screen, mut radio) = (
        Runtime::new(Profile::HeartRate),
        Screen::new(),
        Radio::new(),
    );
    runtime.navigation.page = Page::Home;
    // The first pass looks for the ride journal; later ones read nothing.
    advance(&mut runtime, &mut radio, NOW - 10);
    advance(&mut runtime, &mut radio, NOW);
    assert!(screen.present(&mut runtime, &radio, NOW), "Home drawn");
    (runtime, screen, radio)
}

fn rest(runtime: &Runtime, screen: &Screen, now: u64) -> u64 {
    runtime.pass_rest_ms(&screen.shell, State::Idle, false, now)
}

#[test]
fn a_still_home_rests_until_its_next_frame() {
    let (runtime, screen, _) = still_home();
    assert_eq!(rest(&runtime, &screen, NOW), STILL_REST_MS);
    assert_eq!(
        rest(&runtime, &screen, NOW + 980),
        20,
        "the frame is due at 1 s"
    );
    assert_eq!(rest(&runtime, &screen, NOW + 1000), BUSY_REST_MS, "due now");
}

#[test]
fn each_kind_of_work_alone_keeps_the_busy_pace() {
    type Start = fn(&mut Runtime, &mut Radio);
    let work: [(&str, Start); 5] = [
        ("a finger down", |runtime, radio| {
            runtime.input(Input::Touch(Point { x: 120, y: 4 }), NOW, radio);
        }),
        ("the turn-off sheet", |runtime, _| {
            runtime.open_power_sheet(NOW)
        }),
        ("a cue to sound", |runtime, _| {
            runtime.cue(Cue::LowBattery, NOW)
        }),
        ("a phone session to open", |runtime, _| {
            runtime.open_phone(PhoneMode::AccessPoint);
        }),
        ("a sensor search", |runtime, radio| {
            runtime.request_search(radio, NOW)
        }),
    ];
    for (what, start) in work {
        let (mut runtime, screen, mut radio) = still_home();
        start(&mut runtime, &mut radio);
        assert_eq!(rest(&runtime, &screen, NOW), BUSY_REST_MS, "{what}");
    }
}
