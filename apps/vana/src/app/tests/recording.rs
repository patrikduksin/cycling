use super::*;
use crate::ride::log::Status;
use crate::screens::ride::RidePage;
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
    // While a ride is open TopLeft only steps through the ride pages.
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);
    runtime.input(press(Button::TopLeft), 1800, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);

    runtime.input(press(Button::BottomRight), 2200, &mut radio);
    assert!(runtime.ride.stop_confirming(2200));
    // Too late: the second press only re-arms.
    runtime.input(press(Button::BottomRight), 7201, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 7201);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 12_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 12_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert!(!runtime.ride.stop_confirming(12_000));

    // Saved, the pages still show the ride; past the last one is Home.
    runtime.input(press(Button::TopLeft), 14_400, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);
    runtime.input(press(Button::TopLeft), 14_800, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
}

#[test]
fn totals_count_recorded_seconds_survive_pause_and_restart_with_a_new_ride() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    let ride = |runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, from, until| {
        for now in (from..until).step_by(250) {
            radio.data(POWER, POWER_250, now);
            tick(runtime, journal, radio, now);
        }
    };
    // Before the start nothing is counted.
    ride(&mut runtime, &mut journal, &mut radio, 1000, 5000);
    assert_eq!(runtime.ride.totals.work_kj(), None);
    runtime.input(press(Button::BottomLeft), 5000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 5000, 25_000);
    let power = runtime.ride.totals.avg_power();
    assert_eq!(power, Some(250));
    // 20 s at 250 W is 5 kJ, give or take the second the start took.
    let work = runtime.ride.totals.work_kj();
    assert!(matches!(work, Some(4..=5)), "{work:?}");

    runtime.input(press(Button::BottomLeft), 25_000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 25_000, 65_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.work_kj(), work, "paused seconds count");
    runtime.input(press(Button::BottomLeft), 65_000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 65_000, 85_000);
    let resumed = runtime.ride.totals.work_kj().unwrap();
    assert!((9..=10).contains(&resumed), "{resumed} kJ");

    runtime.input(press(Button::BottomRight), 85_000, &mut radio);
    runtime.input(press(Button::BottomRight), 85_400, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 85_400);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert_eq!(
        runtime.ride.totals.work_kj(),
        Some(resumed),
        "kept after stop"
    );

    runtime.input(press(Button::BottomLeft), 90_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 90_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(runtime.ride.totals.work_kj().unwrap_or(0) <= 1, "new ride");
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

/// Power-only page 0x10 with 2000 W, so a second of it is two kilojoules.
const POWER_2000: [u8; 8] = [0x10, 1, 0, 90, 0, 0, 0xd0, 0x07];

/// Record one active second at 2000 W from 1000 ms, then press `stop` at 2100 ms
/// and service the accepted request only at 5000 and 6000 ms.
fn delayed_stop(stop: &[(Button, u64)]) -> Runtime {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    for now in (1000..=2000).step_by(250) {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    for &(button, at) in stop {
        runtime.input(press(button), at, &mut radio);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    for now in [5000, 6000] {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    settle(&mut runtime, &mut journal, &mut radio, 6000);
    assert_eq!(runtime.ride.recorder.active_ms(6000), 1100);
    runtime
}

#[test]
fn totals_stop_at_an_accepted_pause_while_its_flush_is_pending() {
    let runtime = delayed_stop(&[(Button::BottomLeft, 2100)]);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    // 1.1 active seconds at 2000 W, not the five seconds until the flush.
    assert_eq!(runtime.ride.totals.work_kj(), Some(2));
    assert_eq!(runtime.ride.totals.avg_power(), Some(2000));
}

#[test]
fn totals_stop_at_an_accepted_finish_while_its_flush_is_pending() {
    let runtime = delayed_stop(&[(Button::BottomRight, 1700), (Button::BottomRight, 2100)]);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert_eq!(runtime.ride.totals.work_kj(), Some(2));
}
