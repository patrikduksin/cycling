use super::*;
use crate::ride::log::Status;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, POWER, SPEED};
use std::{format, string::String};

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
    // Repeat packets show the wheel has not turned: the speed decays to its bound.
    radio.data(SPEED, wheel(1024, 1), 4900);
    tick(&mut runtime, &mut journal, &mut radio, 4999);
    assert_eq!(runtime.readings.live.speed_mm_s, Some(821));
    // The transport marks the channel stale before the decoder would.
    let stale = 4900 + device_api::ant::STALE_MS;
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
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    let mut now = 1000;
    while now < 10_000 {
        radio.data(HEART_RATE, HEART_72, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 250;
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(runtime.ride.recorder.written_samples() >= 4);

    pause(&mut runtime, &mut journal, &mut radio, now);
    runtime.input(press(Button::TopLeft), now + 400, &mut radio);
    runtime.input(press(Button::TopLeft), now + 800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, now + 800);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert!(!runtime.recording());
}

/// Press BottomRight on a recording ride and let the pause land.
fn pause(runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, now: u64) {
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), now, radio);
    for step in 0..4 {
        tick(runtime, journal, radio, now + step * 50);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
}

#[test]
fn ending_a_ride_needs_a_pause_then_a_second_press_within_three_seconds() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    // BottomLeft steps through the ride pages and wraps.
    for (at, page) in [
        (1400, RidePage::Stats),
        (1800, RidePage::Lap),
        (2200, RidePage::Climb),
        (2600, RidePage::Main),
    ] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
        assert!(runtime.navigation.page == Page::Ride);
        assert_eq!(runtime.navigation.ride_page, page);
    }

    // Recording, TopLeft laps and never arms the end of the ride.
    for at in [3000, 3400] {
        runtime.input(press(Button::TopLeft), at, &mut radio);
        tick(&mut runtime, &mut journal, &mut radio, at);
        assert!(!runtime.ride.stop_confirming(at));
        assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    }
    assert_eq!(runtime.ride.laps.count(), 1, "the second lap is debounced");

    pause(&mut runtime, &mut journal, &mut radio, 4000);
    runtime.input(press(Button::TopLeft), 4400, &mut radio);
    assert!(runtime.ride.stop_confirming(4400));
    // Resuming disarms it; TopLeft laps again.
    runtime.input(press(Button::BottomRight), 4800, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 4800);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(!runtime.ride.stop_confirming(4800));
    runtime.input(press(Button::TopLeft), 7000, &mut radio);
    assert_eq!(runtime.ride.laps.count(), 2);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);

    pause(&mut runtime, &mut journal, &mut radio, 7400);
    runtime.input(press(Button::TopLeft), 7800, &mut radio);
    assert!(runtime.ride.stop_confirming(7800));
    // Too late: the second press only re-arms.
    runtime.input(press(Button::TopLeft), 10_801, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 10_801);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    runtime.input(press(Button::TopLeft), 13_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 13_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert!(!runtime.ride.stop_confirming(15_000));

    // Saved, the pages still show the ride and wrap; TopLeft goes Home.
    runtime.input(press(Button::BottomLeft), 14_400, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);
    runtime.input(press(Button::BottomLeft), 14_800, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Lap);
    runtime.input(press(Button::BottomLeft), 15_200, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    runtime.input(press(Button::BottomLeft), 15_600, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    runtime.input(press(Button::BottomLeft), 16_000, &mut radio);
    runtime.input(press(Button::TopLeft), 16_400, &mut radio);
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
    runtime.input(press(Button::BottomRight), 5000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 5000, 25_000);
    let power = runtime.ride.totals.avg_power();
    assert_eq!(power, Some(250));
    // 20 s at 250 W is 5 kJ, give or take the second the start took.
    let work = runtime.ride.totals.work_kj();
    assert!(matches!(work, Some(4..=5)), "{work:?}");

    runtime.input(press(Button::BottomRight), 25_000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 25_000, 65_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.work_kj(), work, "paused seconds count");
    runtime.input(press(Button::BottomRight), 65_000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 65_000, 85_000);
    let resumed = runtime.ride.totals.work_kj().unwrap();
    assert!((9..=10).contains(&resumed), "{resumed} kJ");

    pause(&mut runtime, &mut journal, &mut radio, 85_000);
    runtime.input(press(Button::TopLeft), 85_400, &mut radio);
    runtime.input(press(Button::TopLeft), 85_800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 85_800);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert_eq!(
        runtime.ride.totals.work_kj(),
        Some(resumed),
        "kept after stop"
    );

    runtime.input(press(Button::BottomRight), 90_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 90_000);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(runtime.ride.totals.work_kj().unwrap_or(0) <= 1, "new ride");
}

#[test]
fn laps_split_the_recorded_seconds_and_restart_with_a_new_ride() {
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
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 1000, 21_000);
    runtime.input(press(Button::TopLeft), 21_000, &mut radio);
    let first = runtime.ride.laps.previous().unwrap();
    assert_eq!(first.number, 1);
    assert_eq!(first.avg_power(), Some(250));
    assert!((19_000..=20_000).contains(&first.time_ms), "{first:?}");
    // The status row announces it for two seconds.
    assert_eq!(runtime.ride.lap_toast(22_999), Some(first));
    assert_eq!(runtime.ride.lap_toast(23_000), None);

    // Paused seconds are not lap time.
    pause(&mut runtime, &mut journal, &mut radio, 25_000);
    let before = runtime.ride.laps.current();
    ride(&mut runtime, &mut journal, &mut radio, 25_000, 45_000);
    assert_eq!(runtime.ride.laps.current(), before);

    runtime.input(press(Button::TopLeft), 45_400, &mut radio);
    runtime.input(press(Button::TopLeft), 45_800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 45_800);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert_eq!(runtime.ride.laps.count(), 1, "kept after stop");

    runtime.input(press(Button::BottomRight), 50_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 50_000);
    assert_eq!(runtime.ride.laps.count(), 0, "new ride");
    assert_eq!(runtime.ride.laps.current().number, 1);
}

#[test]
fn ride_buttons_wait_for_the_previous_request() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    // Still pending: a pause press is ignored rather than queued.
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1400);
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 1800, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1800);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
}

/// Power-only page 0x10 with 2000 W, so a second of it is two kilojoules.
const POWER_2000: [u8; 8] = [0x10, 1, 0, 90, 0, 0, 0xd0, 0x07];

/// Record one active second at 2000 W from 1000 ms, then `stop` at 2100 ms
/// and service the accepted request only at 5000 and 6000 ms.
fn delayed_stop(stop: impl FnOnce(&mut Runtime, &mut Radio)) -> Runtime {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    for now in (1000..=2000).step_by(250) {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    stop(&mut runtime, &mut radio);
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
    let runtime = delayed_stop(|runtime, radio| {
        runtime.input(press(Button::BottomRight), 2100, radio);
    });
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    // 1.1 active seconds at 2000 W, not the five seconds until the flush.
    assert_eq!(runtime.ride.totals.work_kj(), Some(2));
    assert_eq!(runtime.ride.totals.avg_power(), Some(2000));
}

#[test]
fn totals_stop_at_an_accepted_finish_while_its_flush_is_pending() {
    // Buttons end only a paused ride; a host finish can land while recording.
    let runtime = delayed_stop(|runtime, _| {
        assert!(runtime.ride.submit(|recorder, token| {
            recorder.request(
                crate::ride::control::Action::Finish,
                crate::ride::log::Source::Live,
                2100,
                token,
            )
        }));
    });
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
    assert_eq!(runtime.ride.totals.work_kj(), Some(2));
}

#[test]
fn arming_end_ride_replaces_a_showing_lap_toast() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 1000);
    runtime.input(press(Button::TopLeft), 5000, &mut radio);
    assert!(runtime.ride.lap_toast(5000).is_some());
    pause(&mut runtime, &mut journal, &mut radio, 5400);
    // Within the toast's two seconds: the prompt must show, not the lap.
    runtime.input(press(Button::TopLeft), 5800, &mut radio);
    assert!(runtime.ride.stop_confirming(5800));
    assert_eq!(runtime.ride.lap_toast(5800), None);
}

#[test]
fn climbs_bring_up_the_climb_page_and_return_unless_the_rider_paged() {
    use crate::ride::climb::{Event as Climb, Summary};
    let finished = Climb::Finished(Summary {
        start_altitude_dm: 0,
        gain_dm: 900,
        length_mm: 1_500_000,
        time_ms: 450_000,
    });
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);

    // A climb shows the climb page; its end goes back to the stats.
    runtime.navigation.follow_climb(Climb::Confirmed);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    runtime.navigation.follow_climb(finished);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);

    // Paging during a climb keeps the rider's choice when it ends.
    runtime.navigation.follow_climb(Climb::Confirmed);
    runtime.input(press(Button::BottomLeft), 2000, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    runtime.navigation.follow_climb(finished);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);

    // Already on the climb page: it stays there after the climb.
    for at in [3000, 3400, 3800] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
    }
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    runtime.navigation.follow_climb(Climb::Confirmed);
    runtime.navigation.follow_climb(finished);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);

    // Away from the ride screen climbs change nothing.
    runtime.navigation.page = Page::Home;
    runtime.navigation.ride_page = RidePage::Main;
    runtime.navigation.follow_climb(Climb::Confirmed);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
}

/// A wheel turning once per second (7.7 km/h), or standing still, each second.
struct Wheel {
    time: u16,
    count: u16,
}

impl Wheel {
    fn second(&mut self, radio: &mut Radio, now: u64, turning: bool) {
        if turning {
            self.time = self.time.wrapping_add(1024);
            self.count = self.count.wrapping_add(1);
        }
        radio.data(SPEED, wheel(self.time, self.count), now);
    }
}

/// Run `seconds` of wheel data and ticks from `from`, returning the end.
fn roll(
    (runtime, journal, radio): (&mut Runtime, &mut Journal, &mut Radio),
    wheel: &mut Wheel,
    from: u64,
    seconds: u64,
    turning: bool,
) -> u64 {
    for step in 0..seconds * 4 {
        let now = from + step * 250;
        if step % 4 == 0 {
            wheel.second(radio, now, turning);
        }
        tick(runtime, journal, radio, now);
    }
    from + seconds * 1000
}

#[test]
fn a_stop_auto_pauses_and_riding_on_auto_resumes() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        10,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);

    // The wheel needs four seconds to read zero, then three more to pause.
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        20,
        false,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert!(runtime.ride.auto.paused());
    let frozen = runtime.ride.recorder.active_ms(at);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        20,
        false,
    );
    assert_eq!(runtime.ride.recorder.active_ms(at), frozen, "stop excluded");

    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        6,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert!(!runtime.ride.auto.paused());
    assert!(runtime.ride.recorder.active_ms(at) > frozen);
}

#[test]
fn a_manual_pause_stays_paused_while_the_bike_moves() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        5,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        60,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert!(!runtime.ride.auto.paused());
}

#[test]
fn acting_on_an_auto_pause_makes_it_manual_and_the_next_press_resumes() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        25,
        false,
    );
    assert!(runtime.ride.auto.paused());
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        30,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Paused, "now manual");
    runtime.input(press(Button::BottomRight), at, &mut radio);
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        1,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
}

#[test]
fn a_lost_wheel_sensor_never_pauses_and_releases_an_auto_pause() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    let sensor = radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        25,
        false,
    );
    assert!(runtime.ride.auto.paused());
    // The sensor drops out: the auto-pause lets go rather than hold forever.
    radio.channels.receive(Event::Disconnected(sensor), at);
    for step in 0..40 {
        tick(&mut runtime, &mut journal, &mut radio, at + step * 250);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    // Recording without any speed reading is never paused.
    for step in 40..400 {
        tick(&mut runtime, &mut journal, &mut radio, at + step * 250);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
}

#[test]
fn riding_off_auto_starts_and_storage_not_ready_does_not() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        6,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert_eq!(runtime.ride.toast(at), "AUTO START");

    // Off the ride screen nothing starts.
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        20,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Ready);

    // Storage still scanning: the request is refused and reported, not forced.
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    runtime.ride.recorder = crate::ride::recorder::Recorder::default();
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        20,
        true,
    );
    assert_ne!(runtime.ride.recorder.status(), Status::Recording);
}

#[test]
fn readings_after_an_accepted_pause_do_not_reach_the_totals() {
    const POWER_200: [u8; 8] = [0x10, 1, 0, 90, 0, 0, 200, 0];
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    for now in (1000..=4000).step_by(250) {
        radio.data(POWER, POWER_200, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    runtime.input(press(Button::BottomRight), 4100, &mut radio);
    // The flush is delayed; the power meter now reports 2000 W.
    for now in [6000, 7000] {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    settle(&mut runtime, &mut journal, &mut radio, 7000);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.avg_power(), Some(200));
    assert_eq!(runtime.ride.totals.max_power(), Some(200));
}

#[test]
fn a_start_mid_step_counts_its_first_fraction() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    for now in (250..=1250).step_by(250) {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    runtime.input(press(Button::BottomRight), 1250, &mut radio);
    for now in (1500..=3000).step_by(250) {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    assert_eq!(runtime.ride.totals.max_power(), Some(2000));
}

#[test]
fn an_automatic_action_leaves_an_unread_host_completion_alone() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut output = String::new();
    let status = runtime.command("RIDE START", &mut journal, 1000, &mut output);
    assert_eq!(status, "ACCEPTED");
    let token = output.clone();
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        30,
        false,
    );
    // The stop waits: the host has not read its completion yet.
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    output.clear();
    assert_eq!(
        runtime.command("RIDE STATUS", &mut journal, at, &mut output),
        "OK"
    );
    let token = token.trim_start_matches("token=");
    assert!(
        output.contains(&format!("completed={token} result=OK")),
        "{output}"
    );
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        10,
        false,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Paused, "{at}");
}

#[test]
fn saving_an_auto_paused_ride_is_not_undone_by_movement() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        25,
        false,
    );
    assert!(runtime.ride.auto.paused());
    // First save press arms; the bike then moves on, which must not resume.
    runtime.input(press(Button::TopLeft), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        2,
        true,
    );
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    runtime.input(press(Button::TopLeft), at, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, at);
    assert_eq!(runtime.ride.recorder.status(), Status::Saved);
}

#[test]
fn a_resume_and_pause_inside_one_step_use_the_resumed_readings() {
    const POWER_0: [u8; 8] = [0x10, 1, 0, 90, 0, 0, 0, 0];
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    for now in (1000..=3000).step_by(250) {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    runtime.input(press(Button::BottomRight), 3100, &mut radio);
    let mut now = 3100;
    while now < 9000 {
        radio.data(POWER, POWER_0, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 100;
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    let work = runtime.ride.totals.work_kj();
    runtime.input(press(Button::BottomRight), now, &mut radio);
    while runtime.ride.recorder.status() != Status::Recording {
        radio.data(POWER, POWER_0, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 50;
    }
    now += 350;
    runtime.input(press(Button::BottomRight), now, &mut radio);
    // The next odometer step covers the resumed half second.
    for later in [now + 600, now + 1200] {
        radio.data(POWER, POWER_0, later);
        tick(&mut runtime, &mut journal, &mut radio, later);
    }
    settle(&mut runtime, &mut journal, &mut radio, now + 1200);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.work_kj(), work, "no stale 2000 W");
}

#[test]
fn a_new_ride_paused_before_its_first_step_still_counts_its_seconds() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(POWER, 0);
    radio.data(POWER, POWER_2000, 900);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    radio.data(POWER, POWER_2000, 1000);
    for now in [1000, 1100, 1200] {
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    radio.data(POWER, POWER_2000, 2000);
    tick(&mut runtime, &mut journal, &mut radio, 2000);
    settle(&mut runtime, &mut journal, &mut radio, 2000);
    assert_eq!(runtime.ride.recorder.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.avg_power(), Some(2000), "first 400 ms");
}

#[test]
fn one_turn_after_a_long_auto_pause_is_not_a_lost_sensor() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        90,
        false,
    );
    assert!(runtime.ride.auto.paused());
    // One turn, then still: the reference had expired, so the speed is unknown
    // while fresh packets keep arriving. That is no reason to resume.
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        1,
        true,
    );
    for _ in 0..10 {
        at = roll(
            (&mut runtime, &mut journal, &mut radio),
            &mut wheel,
            at,
            1,
            false,
        );
        assert_eq!(runtime.ride.recorder.status(), Status::Paused, "{at}");
    }
}
