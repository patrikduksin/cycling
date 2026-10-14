use super::*;
use crate::ride::log::Status;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, POWER, SPEED};

const HEART_72: [u8; 8] = [0, 0, 0, 0, 0, 0, 1, 72];
/// Power-only page 0x10 with 250 W instantaneous power.
const POWER_250: [u8; 8] = [0x10, 1, 0, 90, 0, 0, 250, 0];

fn wheel(event_time: u16, revolutions: u16) -> [u8; 8] {
    let [t0, t1] = event_time.to_le_bytes();
    let [r0, r1] = revolutions.to_le_bytes();
    [0, 0, 0, 0, t0, t1, r0, r1]
}

fn on_ride_page(runtime: &mut Runtime) {
    runtime.navigation.page = Page::Ride;
}

/// Let the recorder finish any queued writes.
fn settle(runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, from: u64) {
    for step in 0..20 {
        tick(runtime, journal, radio, from + step * 100);
    }
}

#[test]
fn live_sample_combines_fresh_ant_readings_and_expires_silent_ones() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(HEART_RATE, 0);
    radio.connected(POWER, 0);
    radio.data(HEART_RATE, HEART_72, 1000);
    radio.data(POWER, POWER_250, 1000);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    let live = runtime.readings.live;
    assert_eq!((live.heart_bpm, live.power_watts), (Some(72), Some(250)));

    tick(&mut runtime, &mut journal, &mut radio, 4000);
    let live = runtime.readings.live;
    assert_eq!((live.heart_bpm, live.power_watts), (Some(72), Some(250)));
    tick(&mut runtime, &mut journal, &mut radio, 4001);
    let live = runtime.readings.live;
    assert_eq!((live.heart_bpm, live.power_watts), (None, None));
}

#[test]
fn a_new_channel_session_discards_the_previous_reading() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let heart = radio.connected(HEART_RATE, 0);
    radio.data(HEART_RATE, HEART_72, 1000);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.readings.live.heart_bpm, Some(72));

    radio.channels.receive(Event::Disconnected(heart), 1100);
    radio.channels.connect(heart, 1100).unwrap();
    radio.channels.receive(Event::Connected(heart), 1100);
    tick(&mut runtime, &mut journal, &mut radio, 1200);
    assert_eq!(runtime.readings.live.heart_bpm, None);
}

#[test]
fn wheel_speed_counts_only_while_its_sensor_is_connected_and_fresh() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(SPEED, 0);
    radio.data(SPEED, wheel(0, 0), 1000);
    radio.data(SPEED, wheel(1024, 1), 2000);
    tick(&mut runtime, &mut journal, &mut radio, 2000);
    assert_eq!(
        runtime.readings.live.speed_mm_s,
        Some(crate::ride::metrics::WHEEL_MM)
    );
    tick(&mut runtime, &mut journal, &mut radio, 4999);
    assert_eq!(
        runtime.readings.live.speed_mm_s,
        Some(crate::ride::metrics::WHEEL_MM)
    );
    // The transport marks the channel stale before the decoder would.
    let stale = 2000 + device_api::ant::STALE_MS;
    tick(&mut runtime, &mut journal, &mut radio, stale);
    assert!(radio.channel(SPEED, stale).unwrap().stale);
    assert_eq!(runtime.readings.live.speed_mm_s, None);
}

#[test]
fn a_recording_ride_writes_samples_until_finished() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(HEART_RATE, 0);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    let mut now = 1000;
    while now < 10_000 {
        radio.data(HEART_RATE, HEART_72, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 250;
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(runtime.ride.recorder.written_samples() >= 4);

    runtime.input(press(Button::BottomRight), now, &mut radio);
    runtime.input(press(Button::BottomRight), now + 400, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, now + 400);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert!(!runtime.recording());
}

#[test]
fn stop_needs_a_second_press_within_five_seconds() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    // Leaving is refused while a ride is open.
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);

    runtime.input(press(Button::BottomRight), 2000, &mut radio);
    assert!(runtime.ride.stop_confirming(2000));
    // Too late: the second press only re-arms.
    runtime.input(press(Button::BottomRight), 7001, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 7001);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 12_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 12_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert!(!runtime.ride.stop_confirming(12_000));

    runtime.input(press(Button::TopLeft), 14_400, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
}

#[test]
fn ride_buttons_wait_for_the_previous_request() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    // Still pending: a pause press is ignored rather than queued.
    runtime.input(press(Button::BottomLeft), 1400, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1400);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomLeft), 1800, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1800);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
}
