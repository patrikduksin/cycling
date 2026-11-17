//! The rest after a pass, as composition reads it: a still Home and a
//! recording ride rest until their next frame or deadline, or
//! `STILL_REST_MS`, and each kind of work alone keeps the busy pace.
use super::presentation::Screen;
use super::*;
use crate::app::cues::Cue;
use crate::app::navigation::To;
use crate::app::pace::{BUSY_REST_MS, STILL_REST_MS};
use crate::app::presentation::MOTION_FRAME_MS;
use crate::ride::log::Status;
use crate::screens::ride::slots::{Reading, Slots};
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, POWER};
use device_api::input::Point;
use device_api::power::State;

const NOW: u64 = 5_000;

/// Home drawn at `NOW`, with nothing to do.
fn still_home() -> (Runtime, Screen, Radio) {
    let (mut runtime, mut screen, mut radio) =
        (booted(Profile::HeartRate), Screen::new(), Radio::new());
    runtime.navigation.go(To::Page(Page::Home));
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
            runtime.input_received(Input::Touch(Point { x: 120, y: 4 }), NOW, NOW, radio);
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

/// A ride started at `NOW` and recording on the Ride page, drawn at `at`
/// after a quiet tick: the ride clock turns at `NOW + 1000`.
fn ride_drawn(at: u64) -> (Runtime, Screen, Radio, Journal) {
    let (mut journal, mut radio, mut screen) = (Journal::erased(), Radio::new(), Screen::new());
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(HEART_RATE, NOW - 1000);
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Button::BottomRight), NOW, NOW, &mut radio);
    for now in [NOW, NOW + 10, at - 10, at] {
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    assert_eq!(runtime.ride.active().status(), Status::Recording);
    assert!(screen.present(&mut runtime, &radio, at), "the ride drawn");
    (runtime, screen, radio, journal)
}

#[test]
fn a_recording_ride_rests_until_its_next_frame() {
    let (runtime, screen, ..) = ride_drawn(NOW + 100);
    assert_eq!(rest(&runtime, &screen, NOW + 100), STILL_REST_MS);
    assert_eq!(
        rest(&runtime, &screen, NOW + 580),
        20,
        "the ride page animates every 500 ms"
    );
    let (runtime, screen, ..) = ride_drawn(NOW + 900);
    assert_eq!(
        rest(&runtime, &screen, NOW + 970),
        30,
        "the ride clock turns at 1 s"
    );
}

#[test]
fn readings_gliding_on_the_ride_page_rest_until_the_next_glide_frame() {
    let (mut runtime, mut screen, mut radio, mut journal) = ride_drawn(NOW + 100);
    let mut heart = |runtime: &mut Runtime, screen: &mut Screen, bpm, now| {
        radio.data(HEART_RATE, [0, 0, 0, 0, 0, 0, 0, bpm], now);
        tick(runtime, &mut journal, &mut radio, now);
        screen.present(runtime, &radio, now);
    };
    heart(&mut runtime, &mut screen, 140, NOW + 150);
    heart(&mut runtime, &mut screen, 160, NOW + 400);
    let glide = (NOW + 400..NOW + 500)
        .find(|&now| screen.present(&mut runtime, &radio, now))
        .expect("a glide frame");
    assert_eq!(
        rest(&runtime, &screen, glide),
        MOTION_FRAME_MS,
        "the heart rate glides to its sample"
    );
}

#[test]
fn a_new_number_the_page_does_not_show_asks_for_no_more_frames() {
    let (mut journal, mut radio, mut screen) = (Journal::erased(), Radio::new(), Screen::new());
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(POWER, NOW - 1000);
    runtime.preferences.options.main_slots = Slots {
        left: Reading::Distance,
        right: Reading::Kcal,
    };
    runtime.navigation.go(To::Page(Page::Ride));
    let mut power = |runtime: &mut Runtime, screen: &mut Screen, event, rpm, now| {
        radio.data(POWER, [0x10, event, 0xff, rpm, 0, 0, 200, 0], now);
        tick(runtime, &mut journal, &mut radio, now);
        screen.present(runtime, &radio, now)
    };
    for (event, now) in (1..).zip([NOW, NOW + 250, NOW + 500, NOW + 750]) {
        power(&mut runtime, &mut screen, event, 80, now);
    }
    assert_eq!(rest(&runtime, &screen, NOW + 750), STILL_REST_MS);
    assert!(
        !power(&mut runtime, &mut screen, 5, 90, NOW + 1000),
        "the cadence is not on the page"
    );
    assert_eq!(
        rest(&runtime, &screen, NOW + 1000),
        STILL_REST_MS,
        "the frame the panel already shows is drawn"
    );
}

#[test]
fn a_finger_or_a_cue_on_a_recording_ride_keeps_the_busy_pace() {
    let (mut runtime, screen, mut radio, _) = ride_drawn(NOW + 100);
    runtime.input_received(
        Input::Touch(Point { x: 120, y: 150 }),
        NOW + 100,
        NOW + 100,
        &mut radio,
    );
    assert_eq!(rest(&runtime, &screen, NOW + 100), BUSY_REST_MS, "a finger");
    let (mut runtime, screen, ..) = ride_drawn(NOW + 100);
    runtime.cue(Cue::Lap, NOW + 100);
    assert_eq!(rest(&runtime, &screen, NOW + 100), BUSY_REST_MS, "a cue");
}

#[test]
fn a_ride_recording_on_home_rests_until_its_next_sample() {
    let (mut runtime, mut screen, mut radio, mut journal) = ride_drawn(NOW + 100);
    runtime.navigation.go(To::Page(Page::Home));
    tick(&mut runtime, &mut journal, &mut radio, NOW + 960);
    assert!(
        screen.present(&mut runtime, &radio, NOW + 960),
        "Home drawn"
    );
    assert_eq!(
        rest(&runtime, &screen, NOW + 970),
        30,
        "the sample is due at 1 s"
    );
}
