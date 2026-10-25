use super::*;
use crate::ride::log::Status;
use crate::screens::ride::{PageChoice, RidePage};
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert!(runtime.ride.internal.written_samples() >= 4);

    pause(&mut runtime, &mut journal, &mut radio, now);
    runtime.input(press(Button::TopLeft), now + 400, &mut radio);
    runtime.input(press(Button::BottomRight), now + 800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, now + 800);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
    assert!(!runtime.recording());
}

#[test]
fn an_update_across_workout_steps_puts_each_part_in_its_steps_lap() {
    use crate::ride::metrics::Step as Odometer;
    use crate::workout::model::{Intensity, Length, Step, Target, Workout};
    use crate::workout::player::{Event, Player};
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    let mut now = 1000;
    while now < 5000 {
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 250;
    }
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    const STEPS: [Step; 4] = [
        Step::new(Intensity::Work, Length::Distance(100), Target::Free),
        Step::new(Intensity::Work, Length::Distance(100), Target::Free),
        Step::new(Intensity::Work, Length::Distance(100), Target::Free),
        Step::new(Intensity::Work, Length::Open, Target::Free),
    ];
    runtime.ride.workout = Some(Player::new(Workout::new("D", &STEPS, &[], None)));
    let laps_before = runtime.ride.laps.count();
    let profile = runtime.preferences.profile;
    // Three seconds over 300 m: each 100 m step ends in its own lap.
    runtime.ride.count(
        now + 3000,
        Odometer {
            elapsed_ms: 3000,
            distance_mm: 300_000,
        },
        &crate::ride::log::Sample::default(),
        None,
        &profile,
    );
    assert_eq!(runtime.ride.laps.count(), laps_before + 3);
    for back in 0..3 {
        let lap = runtime.ride.laps.finished(back).unwrap();
        assert_eq!(lap.distance_mm, 100_000, "lap {back}: {lap:?}");
    }
    let started = runtime
        .ride
        .workout_events
        .iter()
        .filter(|event| matches!(event, Event::Started(_)))
        .count();
    assert_eq!(started, 3, "no step event lost");
}

/// Press BottomRight on a recording ride and let the pause land.
fn pause(runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, now: u64) {
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), now, radio);
    for step in 0..4 {
        tick(runtime, journal, radio, now + step * 50);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
}

#[test]
fn ending_a_ride_needs_a_pause_then_an_answer_to_end_ride() {
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
        (2600, RidePage::Map),
        (3000, RidePage::Back),
        (3400, RidePage::Main),
    ] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
        assert!(runtime.navigation.page == Page::Ride);
        assert_eq!(runtime.navigation.ride_page, page);
    }

    // Recording, TopLeft laps and never asks to end the ride.
    for at in [3800, 4200] {
        runtime.input(press(Button::TopLeft), at, &mut radio);
        tick(&mut runtime, &mut journal, &mut radio, at);
        assert!(runtime.navigation.page == Page::Ride);
        assert_eq!(runtime.ride.internal.status(), Status::Recording);
    }
    assert_eq!(runtime.ride.laps.count(), 1, "the second lap is debounced");

    // Paused, TopLeft asks "End ride?"; BACK keeps the ride paused.
    pause(&mut runtime, &mut journal, &mut radio, 4600);
    runtime.input(press(Button::TopLeft), 5000, &mut radio);
    assert!(runtime.navigation.page == Page::EndRide);
    runtime.input(press(Button::TopLeft), 5400, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);
    // MOVE on the question does nothing; it waits as long as it takes.
    runtime.input(press(Button::TopLeft), 5800, &mut radio);
    runtime.input(press(Button::BottomLeft), 6200, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 60_000);
    assert!(runtime.navigation.page == Page::EndRide);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    // ACT answers: the ride saves and its summary opens.
    runtime.input(press(Button::BottomRight), 60_400, &mut radio);
    assert!(runtime.navigation.page == Page::Summary);
    settle(&mut runtime, &mut journal, &mut radio, 60_400);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
    assert!(runtime.navigation.page == Page::Summary);

    // The summary pages wrap; ACT finishes, to Home.
    assert_eq!(runtime.navigation.summary_page, 0);
    runtime.input(press(Button::BottomLeft), 63_000, &mut radio);
    assert_eq!(runtime.navigation.summary_page, 1);
    runtime.input(press(Button::BottomLeft), 63_400, &mut radio);
    runtime.input(press(Button::BottomLeft), 63_800, &mut radio);
    assert_eq!(runtime.navigation.summary_page, 0);
    runtime.input(press(Button::BottomRight), 64_200, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
}

#[test]
fn a_lap_is_kept_in_the_journal_and_read_back_by_replay_and_fit() {
    use crate::ride::log::{Entry, Kind, LapStart, decode};
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    let mut now = 1000;
    while now < 12_000 {
        if now == 6_100 {
            runtime.input(press(Button::TopLeft), now, &mut radio);
        }
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 100;
    }
    assert_eq!(runtime.ride.laps.count(), 1);
    pause(&mut runtime, &mut journal, &mut radio, now);
    runtime.input(press(Button::TopLeft), now + 400, &mut radio);
    runtime.input(press(Button::BottomRight), now + 800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, now + 800);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);

    let entries: std::vec::Vec<Entry> = (0..runtime.ride.internal.next_slot())
        .map(|index| decode(&journal.slot(index)).expect("every slot reads"))
        .collect();
    let marks: std::vec::Vec<(u64, LapStart)> = entries
        .iter()
        .filter(|entry| entry.kind == Kind::Samples)
        .flat_map(|entry| &entry.samples[..usize::from(entry.count)])
        .filter_map(|sample| sample.lap.map(|lap| (sample.active_ms, lap)))
        .collect();
    assert_eq!(
        marks,
        [(100, LapStart::Ride), (6_000, LapStart::Manual)],
        "{entries:?}"
    );

    let mut replay = crate::ride::replay::Replay::new(1, entries.len());
    let mut sums = crate::ride::replay::Sums {
        totals: &mut crate::ride::totals::Totals::default(),
        zone_time: &mut crate::ride::zones::ZoneTime::default(),
        efforts: &mut crate::ride::totals::Efforts::default(),
        trace: &mut crate::ride::elevation::Trace::default(),
    };
    let profile = runtime.preferences.profile;
    while replay.step(64, &profile, &mut sums, |i| entries.get(i).copied())
        == crate::ride::replay::Progress::Reading
    {}
    assert_eq!(replay.laps(), Some(2), "as the live summary counts them");

    let mut out = firmware_services::fit::Counter::default();
    let slots = (0..entries.len()).map(|index| journal.slot(index));
    let plan = crate::ride::fit::write_all(
        slots,
        1,
        Default::default(),
        Some(1_789_000_000_000),
        &mut out,
    )
    .unwrap();
    assert_eq!(plan.laps, 2);
    assert_eq!(plan.size as usize, out.0);
}

struct Failing {
    journal: Journal,
    fail: bool,
}

impl RideStorage for Failing {
    type Error = ();
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), ()> {
        self.journal.ride_read_sector(sector, output)
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
        self.journal.ride_read_slot(slot, output)
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), ()> {
        self.journal.ride_erase_sector(sector)
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
        if self.fail {
            return Err(());
        }
        self.journal.ride_write_slot(slot, data)
    }
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), ()> {
        if self.fail {
            return Err(());
        }
        self.journal.ride_commit_slot(slot, commit)
    }
}

fn ask_to_end(runtime: &mut Runtime, store: &mut impl RideStorage, radio: &mut Radio) {
    on_ride_page(runtime);
    runtime.input(press(Button::BottomRight), 1000, radio);
    tick(runtime, store, radio, 1000);
    runtime.input(press(Button::BottomRight), 2000, radio);
    for step in 0..4 {
        tick(runtime, store, radio, 2000 + step * 50);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    runtime.input(press(Button::TopLeft), 2400, radio);
    assert!(runtime.navigation.page == Page::EndRide);
}

fn host(
    runtime: &mut Runtime,
    store: &mut impl RideStorage,
    now: u64,
    words: &str,
) -> (&'static str, String) {
    let mut output = String::new();
    let status = runtime.command(words, store, &mut NoBulk, now, &mut output);
    (status, output)
}

fn toast_text(runtime: &mut Runtime, now: u64) -> Option<String> {
    runtime
        .toast(now)
        .map(|toast| String::from_utf8(toast.text().to_vec()).unwrap())
}

#[test]
fn a_host_resume_while_asking_returns_to_the_ride_before_the_next_frame() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    ask_to_end(&mut runtime, &mut journal, &mut radio);
    host(&mut runtime, &mut journal, 2500, "RIDE STATUS");
    assert_eq!(
        host(&mut runtime, &mut journal, 2500, "RIDE RESUME").0,
        "ACCEPTED"
    );
    tick(&mut runtime, &mut journal, &mut radio, 2600);
    let (_, status) = host(&mut runtime, &mut journal, 2600, "RIDE STATUS");
    assert!(status.starts_with("state=recording "), "{status}");
    assert!(status.contains(" page=ride "), "{status}");
}

#[test]
fn a_host_finish_while_asking_opens_the_summary_before_the_next_frame() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    ask_to_end(&mut runtime, &mut journal, &mut radio);
    host(&mut runtime, &mut journal, 2500, "RIDE STATUS");
    assert_eq!(
        host(&mut runtime, &mut journal, 2500, "RIDE FINISH").0,
        "ACCEPTED"
    );
    settle(&mut runtime, &mut journal, &mut radio, 2600);
    let (_, status) = host(&mut runtime, &mut journal, 5000, "RIDE STATUS");
    assert!(status.starts_with("state=saved "), "{status}");
    assert!(status.contains(" page=summary "), "{status}");
    runtime.input(press(Button::BottomRight), 5000, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
}

#[test]
fn a_refused_answer_to_end_ride_returns_to_the_ride_and_says_storage_not_ready() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    ask_to_end(&mut runtime, &mut journal, &mut radio);
    let nobody_waits = 0;
    assert!(runtime.ride.internal.request(
        crate::ride::control::Action::Resume,
        crate::ride::log::Source::Live,
        2500,
        nobody_waits
    ));
    runtime.input(press(Button::BottomRight), 2800, &mut radio);
    assert!(runtime.navigation.page == Page::Ride);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert_eq!(
        toast_text(&mut runtime, 2800).as_deref(),
        Some("STORAGE NOT READY")
    );
}

#[test]
fn a_failed_finish_read_by_the_host_stays_on_the_summary_and_lets_the_rider_leave() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let mut store = Failing {
        journal,
        fail: false,
    };
    ask_to_end(&mut runtime, &mut store, &mut radio);
    runtime.input(press(Button::BottomRight), 2800, &mut radio);
    assert!(runtime.navigation.page == Page::Summary);
    store.fail = true;
    for step in 0..20 {
        tick(&mut runtime, &mut store, &mut radio, 2800 + step * 100);
    }
    let (_, status) = host(&mut runtime, &mut store, 5000, "RIDE STATUS");
    assert!(status.starts_with("state=error "), "{status}");
    assert!(status.contains(" page=summary "), "{status}");
    // The failed finish is the rider's: the host's read neither reports nor
    // clears it.
    assert!(status.ends_with(" pending=0"), "{status}");
    assert!(matches!(
        runtime.ride.request,
        crate::app::recording::Request::Completed(result) if !result.ok
    ));
    let mut screen = super::presentation::Screen::new();
    assert!(screen.present(&mut runtime, &radio, 5000));
    assert!(runtime.navigation.page == Page::Summary);
    runtime.input(press(Button::BottomRight), 5400, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
}

#[test]
fn the_paused_screen_comes_first_and_paging_returns_to_it() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    pause(&mut runtime, &mut journal, &mut radio, 2000);
    runtime.navigation.announce(Status::Paused, 2200);
    assert!(!runtime.navigation.pause_hidden, "the paused screen shows");
    runtime.input(press(Button::BottomLeft), 2600, &mut radio);
    assert!(runtime.navigation.pause_hidden);
    assert_eq!(
        runtime.navigation.ride_page,
        RidePage::Main,
        "the page before"
    );
    for at in [3000, 3400, 3800] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
    }
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    runtime.input(press(Button::BottomLeft), 4200, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Map);
    runtime.input(press(Button::BottomLeft), 4600, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Back);
    runtime.input(press(Button::BottomLeft), 5000, &mut radio);
    assert!(
        !runtime.navigation.pause_hidden,
        "back to the paused screen"
    );
    // Paging on goes round every page again, starting at the first.
    let mut seen = std::vec::Vec::new();
    for at in (5400..9400).step_by(400).take(7) {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
        let navigation = &runtime.navigation;
        seen.push(navigation.pause_hidden.then_some(navigation.ride_page));
    }
    let pages = RidePage::ALL.map(Some);
    assert_eq!(seen[..6], pages, "every page");
    assert_eq!(seen[6], None, "then the paused screen");
    // A swipe back from the paused screen shows the last page.
    runtime.navigation.ride_page_chosen(RidePage::Main);
    runtime.navigation.pause_hidden = true;
    runtime.navigation.pause_wrapped = false;
    runtime.previous_ride_page();
    assert!(!runtime.navigation.pause_hidden);
    runtime.previous_ride_page();
    assert_eq!(
        (
            runtime.navigation.pause_hidden,
            runtime.navigation.ride_page
        ),
        (true, RidePage::Back)
    );
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert_eq!(runtime.ride.totals.work_kj(), work, "paused seconds count");
    runtime.input(press(Button::BottomRight), 65_000, &mut radio);
    ride(&mut runtime, &mut journal, &mut radio, 65_000, 85_000);
    let resumed = runtime.ride.totals.work_kj().unwrap();
    assert!((9..=10).contains(&resumed), "{resumed} kJ");

    pause(&mut runtime, &mut journal, &mut radio, 85_000);
    runtime.input(press(Button::TopLeft), 85_400, &mut radio);
    runtime.input(press(Button::BottomRight), 85_800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 85_800);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
    assert_eq!(
        runtime.ride.totals.work_kj(),
        Some(resumed),
        "kept after stop"
    );

    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 90_000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 90_000);
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
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
    runtime.input(press(Button::BottomRight), 45_800, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 45_800);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
    assert_eq!(runtime.ride.laps.count(), 1, "kept after stop");

    on_ride_page(&mut runtime);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 1800, &mut radio);
    tick(&mut runtime, &mut journal, &mut radio, 1800);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    for now in [5000, 6000] {
        radio.data(POWER, POWER_2000, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
    }
    settle(&mut runtime, &mut journal, &mut radio, 6000);
    assert_eq!(runtime.ride.internal.active_ms(6000), 1100);
    runtime
}

#[test]
fn totals_stop_at_an_accepted_pause_while_its_flush_is_pending() {
    let runtime = delayed_stop(|runtime, radio| {
        runtime.input(press(Button::BottomRight), 2100, radio);
    });
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    // 1.1 active seconds at 2000 W, not the five seconds until the flush.
    assert_eq!(runtime.ride.totals.work_kj(), Some(2));
    assert_eq!(runtime.ride.totals.avg_power(), Some(2000));
}

#[test]
fn totals_stop_at_an_accepted_finish_while_its_flush_is_pending() {
    // Buttons end only a paused ride; a host finish can land while recording.
    let runtime = delayed_stop(|runtime, _| {
        assert!(
            runtime
                .ride
                .submit(crate::app::Medium::Internal, |recorder, token| {
                    recorder.request(
                        crate::ride::control::Action::Finish,
                        crate::ride::log::Source::Live,
                        2100,
                        token,
                    )
                })
        );
    });
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
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
    // Within the toast's two seconds: the question replaces the lap.
    runtime.input(press(Button::TopLeft), 5800, &mut radio);
    assert!(runtime.navigation.page == Page::EndRide);
    assert_eq!(runtime.ride.lap_toast(5800), None);
}

/// Ride `metres` on the barometer's climbs at 5 m/s along `altitude_dm(m)`,
/// from where the last stretch ended, and let the page follow the road.
fn ride_barometer(runtime: &mut Runtime, metres: u32, altitude_dm: impl Fn(u32) -> i32) {
    for m in (5..=metres).step_by(5) {
        runtime.ride.climbs.add(1000, 5000, Some(altitude_dm(m)));
        runtime.follow_road();
    }
}

/// Up 400 m at 8% (32 m), confirmed by the barometer.
fn barometer_climb(runtime: &mut Runtime) {
    ride_barometer(runtime, 400, |m| m as i32 * 8 / 10);
    assert!(runtime.ride.climbs.climbing().is_some());
}

/// Level after the climb, until the barometer ends it.
fn barometer_top(runtime: &mut Runtime) {
    ride_barometer(runtime, 400, |_| 320);
    assert!(runtime.ride.climbs.climbing().is_none());
}

#[test]
fn climbs_bring_up_the_climb_page_and_return_unless_the_rider_paged() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);

    // A climb shows the climb page; its end goes back to the stats.
    barometer_climb(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    barometer_top(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Stats);

    // Paging during a climb keeps the rider's choice when it ends, and the
    // climb does not take them back.
    barometer_climb(&mut runtime);
    runtime.input(press(Button::BottomLeft), 2000, &mut radio);
    assert_eq!(runtime.navigation.ride_page, RidePage::Map);
    ride_barometer(&mut runtime, 200, |m| 320 + m as i32 * 8 / 10);
    assert_eq!(runtime.navigation.ride_page, RidePage::Map);
    barometer_top(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Map);

    // Already on the climb page: it stays there after the climb.
    for at in [2600, 3000, 3400, 3800, 4200] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
    }
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    barometer_climb(&mut runtime);
    barometer_top(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);

    // Away from the ride screen climbs change nothing.
    runtime.navigation.page = Page::Home;
    runtime.navigation.ride_page = RidePage::Main;
    barometer_climb(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
}

fn hide(runtime: &mut Runtime, pages: &[RidePage]) {
    let choice = pages
        .iter()
        .fold(PageChoice::default(), |choice, page| choice.toggled(*page));
    runtime.preferences.options.ride_pages = choice;
}

#[test]
fn move_and_swipes_skip_the_pages_the_rider_turned_off() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    hide(
        &mut runtime,
        &[RidePage::Stats, RidePage::Lap, RidePage::Map],
    );
    on_ride_page(&mut runtime);
    let mut seen = std::vec::Vec::new();
    for at in [1000, 1400, 1800, 2200] {
        runtime.input(press(Button::BottomLeft), at, &mut radio);
        seen.push(runtime.navigation.ride_page);
    }
    assert_eq!(
        seen,
        [
            RidePage::Climb,
            RidePage::Back,
            RidePage::Main,
            RidePage::Climb
        ]
    );
    runtime.previous_ride_page();
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    runtime.previous_ride_page();
    assert_eq!(runtime.navigation.ride_page, RidePage::Back);
}

#[test]
fn a_page_turned_off_under_the_rider_gives_way_to_main() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.navigation.ride_page_chosen(RidePage::Lap);
    hide(&mut runtime, &[RidePage::Lap]);
    assert!(!runtime.ride_pages().contains(RidePage::Lap));
    assert_eq!(runtime.ride_pages().first(), RidePage::Main);
    assert_eq!(
        runtime.ride_pages().following(RidePage::Main),
        RidePage::Stats
    );
}

#[test]
fn a_climb_does_not_bring_up_a_climb_page_the_rider_turned_off() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    hide(&mut runtime, &[RidePage::Climb]);
    on_ride_page(&mut runtime);
    barometer_climb(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    assert!(runtime.navigation.toast.is_none(), "silent, no toast");
    barometer_top(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);

    climb_on_the_map(&mut runtime, 0.0);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    assert!(runtime.navigation.toast.is_none());

    hide(&mut runtime, &[]);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb, "back on");
}

/// A runtime with a look-ahead whose forecast has a 5% climb starting
/// `foot` metres ahead, pinned to the ride.
fn climb_on_the_map(runtime: &mut Runtime, foot: f64) {
    let arena = std::vec![0u8; crate::app::TERRAIN_ARENA_BYTES].leak();
    let terrain = std::boxed::Box::new(crate::app::Terrain::new(arena));
    runtime.attach_terrain(std::boxed::Box::leak(terrain));
    let road = move |m: f64| (m - foot).clamp(0.0, 1000.0) * 0.05;
    map_says(runtime, 1, road);
}

/// A new forecast of `road(metres)`, followed by the pin as recording does.
fn map_says(runtime: &mut Runtime, generation: u32, road: impl Fn(f64) -> f64) {
    runtime
        .forecast_mut()
        .update(&crate::ride::climb_ahead::tests::ahead(generation, 5, road));
    runtime.pin_for_test(true);
}

/// Ride `metres` on the odometer, at 5 m/s, following the road each second.
fn ride_on(runtime: &mut Runtime, metres: u64) {
    for _ in 0..metres / 5 {
        let sample = runtime.readings.live;
        runtime.ride.totals.add(1000, 5000, &sample, None);
        // The look-ahead steps each pass, with or without a new forecast.
        runtime.pin_for_test(false);
        runtime.follow_road();
    }
}

#[test]
fn the_map_brings_the_page_up_before_the_foot_and_takes_it_away_past_the_top() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    climb_on_the_map(&mut runtime, 300.0);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    // Up the 1 km climb: the barometer saying nothing keeps it, as does a
    // map that loses the climb in its last metres.
    ride_on(&mut runtime, 1200);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    map_says(&mut runtime, 2, |m| -m * 0.02);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    // At 100 m past the top it stays; beyond, it lets go.
    ride_on(&mut runtime, 200);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    ride_on(&mut runtime, 10);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
}

#[test]
fn the_barometer_ending_a_climb_leaves_the_page_while_the_map_climbs_on() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    climb_on_the_map(&mut runtime, 0.0);
    barometer_climb(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    barometer_top(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    // The map shows the rider turned off the climb: the road is clear.
    map_says(&mut runtime, 2, |_| 0.0);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
}

#[test]
fn a_climb_the_map_saw_before_the_ride_screen_comes_up_on_it() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    climb_on_the_map(&mut runtime, 300.0);
    runtime.navigation.page = Page::Home;
    runtime.follow_road();
    on_ride_page(&mut runtime);
    assert_eq!(runtime.navigation.ride_page, RidePage::Main);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
}

#[test]
fn a_climb_close_after_another_comes_up_too() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    climb_on_the_map(&mut runtime, 0.0);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
    runtime.navigation.ride_page_chosen(RidePage::Main);
    // Over the top: 300 m level, then the next climb.
    ride_on(&mut runtime, 1150);
    map_says(&mut runtime, 2, |m| (m - 300.0).clamp(0.0, 800.0) * 0.06);
    runtime.follow_road();
    assert_eq!(runtime.navigation.ride_page, RidePage::Climb);
}

#[test]
fn a_forgotten_forecast_shows_no_climb() {
    let mut radio = Radio::new();
    let mut journal = Journal::erased();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    climb_on_the_map(&mut runtime, 0.0);
    assert!(runtime.forecast().current().is_some());
    runtime.forecast_mut().forget();
    assert!(runtime.forecast().current().is_none());
    assert!(runtime.forecast().upcoming().is_none());
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
    ctx: (&mut Runtime, &mut Journal, &mut Radio),
    wheel: &mut Wheel,
    from: u64,
    seconds: u64,
    turning: bool,
) -> u64 {
    roll_heard(ctx, wheel, &mut Speaker::default(), from, seconds, turning)
}

#[derive(Default)]
struct Speaker {
    played: std::vec::Vec<(u64, u8)>,
    free_at: u64,
}

impl device_api::sound::Sound for Speaker {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        const fn pattern(id: u8, frequency_hz: u16) -> device_api::sound::Pattern {
            device_api::sound::Pattern {
                id,
                frequency_hz,
                nominal_ms: 100,
                guard_ms: 0,
            }
        }
        const PATTERNS: [device_api::sound::Pattern; 3] =
            [pattern(21, 2500), pattern(0, 3000), pattern(10, 4000)];
        &PATTERNS
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        device_api::sound::Snapshot {
            state: device_api::sound::State::Elapsed,
            pattern: None,
            requested_ms: 0,
            submitted_ms: None,
            until_ms: self.free_at,
        }
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), device_api::observation::Error> {
        if now < self.free_at {
            return Err(device_api::observation::Error::Unavailable);
        }
        self.played.push((now, id));
        self.free_at = now + 100;
        Ok(())
    }
    fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
        Ok(())
    }
}

impl Speaker {
    fn heard(&self, from: u64, to: u64) -> std::vec::Vec<u8> {
        self.played
            .iter()
            .filter(|(at, _)| (from..to).contains(at))
            .map(|(_, id)| *id)
            .collect()
    }
}

fn roll_heard(
    (runtime, journal, radio): (&mut Runtime, &mut Journal, &mut Radio),
    wheel: &mut Wheel,
    speaker: &mut Speaker,
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
        let status = runtime.ride.active().status();
        runtime.navigation.announce(status, now);
        runtime.alert(speaker, now);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);

    // The wheel needs four seconds to read zero, then three more to pause.
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        20,
        false,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert!(runtime.ride.auto.paused());
    let frozen = runtime.ride.internal.active_ms(at);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        20,
        false,
    );
    assert_eq!(runtime.ride.internal.active_ms(at), frozen, "stop excluded");

    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        6,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert!(!runtime.ride.auto.paused());
    assert!(runtime.ride.internal.active_ms(at) > frozen);
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert!(!runtime.ride.auto.paused());
}

fn manually_paused(
    (runtime, journal, radio): (&mut Runtime, &mut Journal, &mut Radio),
    wheel: &mut Wheel,
    speaker: &mut Speaker,
) -> u64 {
    on_ride_page(runtime);
    radio.connected(SPEED, 0);
    let mut at = roll_heard((runtime, journal, radio), wheel, speaker, 1000, 2, true);
    runtime.input(press(Button::BottomRight), at, radio);
    at = roll_heard((runtime, journal, radio), wheel, speaker, at, 5, true);
    runtime.input(press(Button::BottomRight), at, radio);
    at = roll_heard((runtime, journal, radio), wheel, speaker, at, 1, true);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    at - 1_000
}

#[test]
fn a_manual_pause_reminds_a_rider_who_rides_on_until_they_resume() {
    const REMINDER: [u8; 3] = [0, 0, 10];
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut speaker = Speaker::default();
    let paused = manually_paused(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
    );
    let mut at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        paused,
        40,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert_eq!(speaker.heard(paused, paused + 1_000), [10, 0], "the pause");
    assert_eq!(
        speaker.heard(paused + 1_000, paused + 3_000),
        [],
        "not before the hold"
    );
    assert_eq!(
        speaker.heard(paused + 3_000, paused + 4_500),
        REMINDER,
        "reminded after riding on"
    );
    assert_eq!(speaker.heard(paused + 4_500, paused + 32_500), []);
    assert_eq!(
        speaker.heard(paused + 33_000, paused + 34_500),
        REMINDER,
        "and again 30 s later"
    );

    at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        60,
        false,
    );
    assert_eq!(speaker.heard(at - 55_000, at), [], "quiet once stopped");
    assert_eq!(runtime.ride.internal.status(), Status::Paused);

    let rode_on = at;
    at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        2,
        true,
    );
    assert_eq!(speaker.heard(rode_on, at), [], "re-armed with its hold");
    at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        3,
        true,
    );
    assert_eq!(speaker.heard(rode_on, at), REMINDER, "reminded again");
    runtime.input(press(Button::BottomRight), at, &mut radio);
    let at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        40,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert_eq!(speaker.heard(at - 40_000, at), [10], "only the resume cue");
}

#[test]
fn with_auto_pause_off_a_manual_pause_still_reminds_and_stays_paused() {
    const REMINDER: [u8; 3] = [0, 0, 10];
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.preferences.options.auto_pause = false;
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut speaker = Speaker::default();
    let paused = manually_paused(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
    );
    let at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        paused,
        10,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert_eq!(speaker.heard(paused + 3_000, at), REMINDER);
}

#[test]
fn an_auto_pause_resumes_with_the_resume_cue_and_is_never_reminded() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut speaker = Speaker::default();
    let mut at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        1000,
        2,
        true,
    );
    runtime.input(press(Button::BottomRight), at, &mut radio);
    at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        25,
        false,
    );
    assert!(runtime.ride.auto.paused());
    let from = at;
    at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        at,
        40,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert_eq!(speaker.heard(from, at), [10], "resumed, never reminded");
}

#[test]
fn muted_cues_keep_the_reminder_quiet() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.preferences.options.sounds = false;
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut speaker = Speaker::default();
    let paused = manually_paused(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
    );
    let at = roll_heard(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        &mut speaker,
        paused,
        40,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert_eq!(speaker.heard(0, at), [], "muted");
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused, "now manual");
    runtime.input(press(Button::BottomRight), at, &mut radio);
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        1,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    // Recording without any speed reading is never paused.
    for step in 40..400 {
        tick(&mut runtime, &mut journal, &mut radio, at + step * 250);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert_eq!(runtime.ride.auto_toast(at), "AUTO START");

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
    assert_eq!(runtime.ride.internal.status(), Status::Ready);

    // Storage still scanning: the request is refused and reported, not forced.
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    runtime.ride.internal = crate::ride::recorder::Recorder::default();
    roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        20,
        true,
    );
    assert_ne!(runtime.ride.internal.status(), Status::Recording);
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    assert_eq!(runtime.ride.totals.max_power(), Some(2000));
}

#[test]
fn an_automatic_action_keeps_an_unread_host_completion() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    radio.connected(SPEED, 0);
    let mut wheel = Wheel { time: 0, count: 0 };
    let mut output = String::new();
    let status = runtime.command("RIDE START", &mut journal, &mut NoBulk, 1000, &mut output);
    assert_eq!(status, "ACCEPTED");
    let token = output.clone();
    let at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        1000,
        30,
        false,
    );
    // The stop pauses the ride without waiting for the host to read its start.
    assert!(runtime.ride.auto.paused(), "{at}");
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    output.clear();
    assert_eq!(
        runtime.command("RIDE STATUS", &mut journal, &mut NoBulk, at, &mut output),
        "OK"
    );
    let token = token.trim_start_matches("token=");
    assert!(
        output.ends_with(&format!("pending=0 completed={token} result=OK")),
        "{output}"
    );
}

/// The token a host mutation was accepted under.
fn accepted(reply: (&'static str, String)) -> String {
    assert_eq!(reply.0, "ACCEPTED", "{}", reply.1);
    reply.1.trim_start_matches("token=").into()
}

#[test]
fn a_rider_press_keeps_an_unread_host_completion() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    accepted(host(&mut runtime, &mut journal, 0, "RIDE START"));
    settle(&mut runtime, &mut journal, &mut radio, 100);
    host(&mut runtime, &mut journal, 2100, "RIDE STATUS");
    // The host pauses; the pause commits, but the host has not read it yet.
    let token = accepted(host(&mut runtime, &mut journal, 3000, "RIDE PAUSE"));
    settle(&mut runtime, &mut journal, &mut radio, 3000);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    // The rider resumes before the host reads its status.
    runtime.input(press(Button::BottomRight), 6000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 6000);
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    let (status, output) = host(&mut runtime, &mut journal, 8000, "RIDE STATUS");
    assert_eq!(status, "OK");
    assert!(
        output.ends_with(&format!("pending=0 completed={token} result=OK")),
        "{output}"
    );
    // Read once; the rider's resume is never reported as the host's.
    let (_, output) = host(&mut runtime, &mut journal, 8000, "RIDE STATUS");
    assert!(output.ends_with("pending=0"), "{output}");
}

#[test]
fn a_rider_press_while_a_host_command_is_pending_is_ignored() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    accepted(host(&mut runtime, &mut journal, 0, "RIDE START"));
    settle(&mut runtime, &mut journal, &mut radio, 100);
    host(&mut runtime, &mut journal, 2100, "RIDE STATUS");
    let token = accepted(host(&mut runtime, &mut journal, 3000, "RIDE PAUSE"));
    let next = runtime.ride.next_token();
    // The recorder takes one operation at a time: a press before the host's
    // pause commits does nothing and takes no token.
    runtime.input(press(Button::BottomRight), 3000, &mut radio);
    assert_eq!(runtime.ride.next_token(), next);
    settle(&mut runtime, &mut journal, &mut radio, 3000);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    let (_, output) = host(&mut runtime, &mut journal, 5000, "RIDE STATUS");
    assert!(
        output.ends_with(&format!("pending=0 completed={token} result=OK")),
        "{output}"
    );
    // Once it has, the next press acts.
    runtime.input(press(Button::BottomRight), 5000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 5000);
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
}

#[test]
fn a_rider_completion_neither_blocks_nor_answers_the_host() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 1000);
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    // The rider's start is not the host's to read.
    let (_, output) = host(&mut runtime, &mut journal, 2100, "RIDE STATUS");
    assert!(output.ends_with("pending=0"), "{output}");
    // Nor does it hold up a host command while unread.
    runtime.input(press(Button::BottomRight), 3000, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, 3000);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    let token = accepted(host(&mut runtime, &mut journal, 5000, "RIDE RESUME"));
    settle(&mut runtime, &mut journal, &mut radio, 5000);
    let (_, output) = host(&mut runtime, &mut journal, 7000, "RIDE STATUS");
    assert!(
        output.ends_with(&format!("pending=0 completed={token} result=OK")),
        "{output}"
    );
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
    // "End ride?" is open; the bike then moves on, which must not resume.
    runtime.input(press(Button::TopLeft), at, &mut radio);
    at = roll(
        (&mut runtime, &mut journal, &mut radio),
        &mut wheel,
        at,
        2,
        true,
    );
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    runtime.input(press(Button::BottomRight), at, &mut radio);
    settle(&mut runtime, &mut journal, &mut radio, at);
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    let work = runtime.ride.totals.work_kj();
    runtime.input(press(Button::BottomRight), now, &mut radio);
    while runtime.ride.internal.status() != Status::Recording {
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
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
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
    assert_eq!(runtime.ride.internal.status(), Status::Recording);
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    radio.data(POWER, POWER_2000, 2000);
    tick(&mut runtime, &mut journal, &mut radio, 2000);
    settle(&mut runtime, &mut journal, &mut radio, 2000);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
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
        assert_eq!(runtime.ride.internal.status(), Status::Paused, "{at}");
    }
}

/// In-memory bulk media whose writes can be made to fail.
struct FlakyBulk {
    sectors: std::collections::BTreeMap<u64, [u8; 512]>,
    fail: bool,
}

impl device_api::bulk::ReadWrite for FlakyBulk {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        Ok(device_api::bulk::OwnedInfo {
            start_sector: 0,
            sectors: 16 + 10_000,
            sector_size: 512,
        })
    }
    fn owned_read(
        &mut self,
        index: u64,
        out: &mut [u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        *out = self.sectors.get(&index).copied().unwrap_or([0; 512]);
        Ok(())
    }
    fn owned_write(&mut self, index: u64, data: &[u8; 512]) -> Result<(), device_api::bulk::Error> {
        if self.fail {
            return Err(device_api::bulk::Error::Failed);
        }
        self.sectors.insert(index, *data);
        Ok(())
    }
}

fn tick_with(runtime: &mut Runtime, journal: &mut Journal, bulk: &mut FlakyBulk, now: u64) {
    runtime.tick(
        journal,
        bulk,
        now,
        &mut Radio::new(),
        &mut Absent,
        Observations {
            position: &Absent,
            network_online: false,
            input: &Absent,
            physical: no_physical_sensors(),
        },
    );
}

#[test]
fn a_ride_moving_to_internal_flash_counts_as_active_work() {
    let mut journal = Journal::erased();
    let mut bulk = FlakyBulk {
        sectors: std::collections::BTreeMap::new(),
        fail: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.internal.status() == Status::Scanning
        || runtime.ride.bulk.status() == Status::Scanning
    {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    let mut out = String::new();
    assert_eq!(
        runtime.command("RIDE BULK INIT", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.bulk.status() != Status::Ready {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    // Reading the status consumes the INIT completion.
    runtime.command("RIDE BULK STATUS", &mut journal, &mut bulk, now, &mut out);
    assert_eq!(
        runtime.command("RIDE START", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.bulk.status() != Status::Recording {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    bulk.fail = true;
    let mut window = false;
    while runtime.ride.internal.status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        assert!(now < 60_000, "the continuation never started");
        if runtime.ride.bulk.status() == Status::Error
            && runtime.ride.internal.status() != Status::Recording
        {
            // The internal start is queued but not yet written: a terminal
            // shutdown or sleep must still see active work.
            window = true;
            assert!(runtime.recording(), "at {now} ms");
        }
    }
    assert!(window, "no tick between the failure and the continuation");
    assert!(runtime.recording());
}

/// Bulk media that cannot take rides is named on the ride page by a warning
/// toast for as long as rides go to internal flash.
#[test]
fn rides_falling_back_to_internal_flash_carry_a_warning_toast() {
    use crate::app::recording::Request;
    use crate::screens::chrome::Tone;
    let mut journal = Journal::erased();
    // Never formatted: rides go to internal flash until it is initialized.
    let mut bulk = FlakyBulk {
        sectors: std::collections::BTreeMap::new(),
        fail: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.internal.status() == Status::Scanning
        || runtime.ride.bulk.status() == Status::Scanning
    {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert!(runtime.ride.fallback());
    let toast = |runtime: &mut Runtime, now| {
        let toast = runtime.toast(now).expect("a toast");
        (
            String::from_utf8(toast.text().to_vec()).unwrap(),
            toast.tone,
        )
    };
    runtime.navigation.page = Page::Ride;
    assert_eq!(toast(&mut runtime, now), ("READY INT".into(), Tone::Warn));
    let mut out = String::new();
    assert_eq!(
        runtime.command("RIDE START", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.internal.status() != Status::Recording {
        now += 100;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        assert!(now < 10_000, "never recording");
    }
    // Past the ride's first notes, the fallback is what the band names.
    now += 60_000;
    tick_with(&mut runtime, &mut journal, &mut bulk, now);
    runtime.navigation.toast = None;
    assert_eq!(
        toast(&mut runtime, now),
        ("RECORDING INT".into(), Tone::Warn)
    );
    // The bulk failure that moved a ride is what the fallback names; a
    // failed write on internal flash is never hidden behind it.
    let failed = Request::Completed(crate::ride::recorder::ResultEvent {
        action: Some(crate::ride::control::Action::Pause),
        token: 9,
        ok: false,
    });
    runtime.ride.request = failed;
    runtime.ride.request_on = Medium::Bulk;
    assert_eq!(
        toast(&mut runtime, now),
        ("RECORDING INT".into(), Tone::Warn)
    );
    runtime.ride.request_on = Medium::Internal;
    assert_eq!(
        toast(&mut runtime, now),
        ("WRITE FAILED".into(), Tone::Alert)
    );
    runtime.ride.request = Request::Idle;
    // Off the ride page nothing is shown.
    runtime.navigation.page = Page::Home;
    assert!(runtime.toast(now).is_none());
}

#[test]
fn a_continuation_waits_for_internal_recovery_to_close_the_interrupted_ride() {
    let mut journal = Journal::erased();
    let mut bulk = FlakyBulk {
        sectors: std::collections::BTreeMap::new(),
        fail: false,
    };
    let mut out = String::new();
    // An internal ride is open when the power fails; the MMC holds a store.
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.internal.status() == Status::Scanning
        || runtime.ride.bulk.status() == Status::Scanning
    {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert_eq!(
        runtime.command("RIDE BULK INIT", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.bulk.status() != Status::Ready {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    runtime.command("RIDE BULK STATUS", &mut journal, &mut bulk, now, &mut out);
    assert_eq!(
        runtime.command(
            "RIDE INTERNAL START",
            &mut journal,
            &mut bulk,
            now,
            &mut out
        ),
        "ACCEPTED"
    );
    while now < 10_000 {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Recording);

    // After the restart the MMC is ready first, and a ride starts there.
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.bulk.status() == Status::Scanning {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Scanning);
    assert_eq!(
        runtime.command("RIDE START", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.bulk.status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    // The MMC fails while internal flash still scans and has the interrupted
    // ride to close: the continuation waits for both, then starts.
    bulk.fail = true;
    while runtime.ride.bulk.status() != Status::Error {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert_eq!(runtime.ride.internal.status(), Status::Scanning);
    let mut recovered = false;
    while runtime.ride.internal.status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        recovered |= runtime.ride.internal.status() == Status::Recovered;
        assert!(now < 300_000, "the continuation never started");
    }
    assert!(
        recovered,
        "the interrupted internal ride was recovered first"
    );
    assert!(runtime.ride.moved);
    assert_eq!(runtime.ride.internal.completed(), 1);
}

#[test]
fn a_host_action_replaces_the_riders_failure_notice() {
    use crate::app::recording::Request;
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    on_ride_page(&mut runtime);
    accepted(host(&mut runtime, &mut journal, 0, "RIDE START"));
    settle(&mut runtime, &mut journal, &mut radio, 100);
    host(&mut runtime, &mut journal, 2100, "RIDE STATUS");
    // A rider's write failed on internal flash; the ride page says so.
    runtime.ride.request = Request::Completed(crate::ride::recorder::ResultEvent {
        action: Some(crate::ride::control::Action::Pause),
        token: 99,
        ok: false,
    });
    runtime.ride.request_on = Medium::Internal;
    assert_eq!(
        toast_text(&mut runtime, 2100).as_deref(),
        Some("WRITE FAILED")
    );
    // The host's pause then succeeds: the notice gives way, as it would to
    // the rider's next action.
    let token = accepted(host(&mut runtime, &mut journal, 3000, "RIDE PAUSE"));
    settle(&mut runtime, &mut journal, &mut radio, 3000);
    assert_eq!(runtime.ride.internal.status(), Status::Paused);
    assert!(matches!(runtime.ride.request, Request::Idle));
    assert_ne!(
        toast_text(&mut runtime, 5000).as_deref(),
        Some("WRITE FAILED")
    );
    let (_, output) = host(&mut runtime, &mut journal, 5000, "RIDE STATUS");
    assert!(
        output.ends_with(&format!("pending=0 completed={token} result=OK")),
        "{output}"
    );
}

fn bulk_command(
    runtime: &mut Runtime,
    journal: &mut Journal,
    bulk: &mut FlakyBulk,
    now: u64,
    words: &str,
) -> (&'static str, String) {
    let mut out = String::new();
    let status = runtime.command(words, journal, bulk, now, &mut out);
    (status, out)
}

#[test]
fn a_failed_host_pause_on_mmc_is_the_hosts_and_the_ride_continues_internally() {
    use crate::app::recording::Request;
    let mut journal = Journal::erased();
    let mut bulk = FlakyBulk {
        sectors: std::collections::BTreeMap::new(),
        fail: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.internal.status() == Status::Scanning
        || runtime.ride.bulk.status() == Status::Scanning
    {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    accepted(bulk_command(
        &mut runtime,
        &mut journal,
        &mut bulk,
        now,
        "RIDE BULK INIT",
    ));
    while runtime.ride.bulk.status() != Status::Ready {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    bulk_command(
        &mut runtime,
        &mut journal,
        &mut bulk,
        now,
        "RIDE BULK STATUS",
    );
    runtime.navigation.page = Page::Ride;
    accepted(bulk_command(
        &mut runtime,
        &mut journal,
        &mut bulk,
        now,
        "RIDE START",
    ));
    while runtime.ride.bulk.status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    for _ in 0..20 {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    bulk_command(&mut runtime, &mut journal, &mut bulk, now, "RIDE STATUS");
    // The card fails as the host pauses the MMC ride.
    bulk.fail = true;
    let token = accepted(bulk_command(
        &mut runtime,
        &mut journal,
        &mut bulk,
        now,
        "RIDE PAUSE",
    ));
    while !matches!(runtime.ride.host, Request::Completed(_)) {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        assert!(now < 60_000, "the host's pause never completed");
    }
    let (_, output) = bulk_command(
        &mut runtime,
        &mut journal,
        &mut bulk,
        now,
        "RIDE BULK STATUS",
    );
    assert!(
        output.ends_with(&format!("completed={token} result=FAILED")),
        "{output}"
    );
    assert!(matches!(runtime.ride.request, Request::Idle));
    // The ride carries on from internal flash; the rider has nothing to read.
    while runtime.ride.internal.status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        assert!(now < 60_000, "the continuation never started");
    }
    for _ in 0..20 {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert!(runtime.ride.internal.riding());
    assert!(runtime.ride.moved);
    assert_eq!(runtime.ride.shown(), Medium::Internal);
    assert!(matches!(runtime.ride.request, Request::Idle));
    assert_eq!(runtime.ride.request_on, Medium::Internal);
    assert_ne!(
        toast_text(&mut runtime, now).as_deref(),
        Some("WRITE FAILED")
    );
}
