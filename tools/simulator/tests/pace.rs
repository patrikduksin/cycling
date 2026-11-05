//! The console rests between passes while nothing needs one: about 20
//! passes a second on a still page, lit, dimmed or charging, about as many
//! as a recording ride draws, and 100 while a finger, a sheet, a coasting
//! list, a press awaiting its frame, the look-ahead or storage work needs
//! them (`vana::app::pace`). A press or a radar page ends a rest at once, a
//! harness press runs when it is due, and a strict replay of a resting
//! session is exact.
#![cfg(feature = "cycling")]
use device_api::input::{Button, Input, Point};
use firmware_services::trace::{Event, decode};
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::replay::first_difference;
use shell_simulator::route::{self, Route, to_gpx};
use shell_simulator::sim::{Sim, minutes, seconds};
use vana::screens::workout::Page;

/// Passes a second still pages run, at most 50 ms apart in 10 ms slices.
const STILL: core::ops::RangeInclusive<u64> = 15..=25;
/// Passes a second at the busy pace, one each 10 ms slice.
const BUSY: u64 = 95;

fn device() -> Sim {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim
}

fn passes(sim: &Sim) -> u64 {
    let trace = sim.trace();
    let (_, events) = decode(&trace).expect("the trace decodes");
    events
        .filter(|event| matches!(event, Event::Pass { .. }))
        .count() as u64
}

/// Passes a second over the next `ms`.
fn pace(sim: &mut Sim, ms: u64) -> u64 {
    let before = passes(sim);
    sim.advance(ms);
    (passes(sim) - before) * 1000 / ms
}

#[track_caller]
fn expect_still(sim: &mut Sim, screen: &str) {
    let pace = pace(sim, seconds(30));
    println!("{screen}: {pace} passes/s");
    assert!(STILL.contains(&pace), "{screen}: {pace} passes/s");
}

#[track_caller]
fn expect_busy(sim: &mut Sim, ms: u64, what: &str) {
    let pace = pace(sim, ms);
    println!("{what}: {pace} passes/s");
    assert!(pace >= BUSY, "{what}: {pace} passes/s");
}

/// Rides of `minutes` each saved through the terminal, as a host records
/// them, on Home.
fn saved_rides(rides: &[u64]) -> Sim {
    let mut sim = device();
    sim.ant_sensor(device_type::HEART_RATE, 4242);
    sim.ride_at(20.0, 150.0, 120.0, 80.0);
    sim.pair(&["HEART RATE"]);
    for &length in rides {
        for (command, then) in [("START", minutes(length)), ("PAUSE", 500), ("FINISH", 500)] {
            let (status, out) = sim.command(&format!("RIDE INTERNAL {command}"));
            assert_eq!(status, "ACCEPTED", "{command}: {out}");
            sim.advance(then);
            let (_, out) = sim.command("RIDE STATUS");
            assert!(out.contains("result=OK"), "{command}: {out}");
        }
    }
    sim.advance(seconds(3));
    sim
}

/// The same rides, then the rides list once its index has read them all.
fn rides_list(rides: &[u64]) -> Sim {
    let mut sim = saved_rides(rides);
    sim.home("RIDES");
    sim.wait_for(&format!("1/{}", rides.len()), seconds(5))
        .unwrap();
    sim.advance(seconds(1));
    sim
}

/// Advance until a rest is under way: the last pass ran before this slice.
fn resting(sim: &mut Sim) {
    for _ in 0..5 {
        if sim.device.clock_ms() > sim.device.now {
            return;
        }
        sim.advance(10);
    }
    panic!("the console never rested");
}

#[test]
fn still_pages_rest_between_passes() {
    let mut sim = device();
    sim.advance(seconds(3));
    let lit = pace(&mut sim, seconds(20));
    println!("home: {lit} passes/s");
    assert!(STILL.contains(&lit), "home: {lit} passes/s");
    sim.advance(seconds(10));
    assert!(sim.device.shell.dimmed(), "Home dims after 30 s");
    expect_still(&mut sim, "home dimmed");
    sim.home("SETTINGS");
    expect_still(&mut sim, "settings");
    sim.press(Button::TopLeft);
    sim.set_battery(60, true);
    expect_still(&mut sim, "home charging");
}

#[test]
fn work_on_a_page_keeps_the_busy_pace() {
    let mut sim = device();
    sim.advance(seconds(3));
    sim.device
        .input
        .push(sim.device.clock_ms(), Input::Touch(Point { x: 120, y: 4 }));
    expect_busy(&mut sim, seconds(1), "a finger resting on Home");
    sim.device.input.push(sim.device.clock_ms(), Input::Release);
    sim.advance(seconds(1));
    sim.press(Button::TopLeft);
    sim.expect_text("TURN OFF");
    expect_busy(&mut sim, seconds(2), "the turn-off sheet");
}

#[test]
fn a_ride_recording_on_a_still_page_rests() {
    let mut sim = saved_rides(&[]);
    expect_still(&mut sim, "home");
    let (status, out) = sim.command("RIDE INTERNAL START");
    assert_eq!(status, "ACCEPTED", "{out}");
    sim.advance(seconds(1));
    expect_still(&mut sim, "a ride recording on Home");
    assert_eq!(sim.device.sdk.page(), Page::Home);
}

fn frames(sim: &Sim) -> u32 {
    sim.device.shell.display_submissions
}

/// Passes and frames a second over `seconds` of a ride on the Ride page
/// with ANT+ power, heart rate and speed and cadence, each about four times
/// a second, and readings that move with every sample, as a rider's do.
fn riding_pace(seconds: u64) -> (u64, u64) {
    let mut sim = device();
    sim.ant_sensor(device_type::HEART_RATE, 1);
    sim.ant_sensor(device_type::POWER, 2);
    sim.ant_sensor(device_type::SPEED_CADENCE, 3);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(5_000);
    assert_eq!(sim.device.sdk.page(), Page::Ride);
    let (passes_before, frames_before) = (passes(&sim), frames(&sim));
    for quarter in 0..seconds * 4 {
        let swing = [0.0, 1.0, 2.0, 1.0, 0.0, -1.0, -2.0, -1.0][quarter as usize % 8];
        sim.ride_at(
            25.0 + swing,
            180.0 + 15.0 * swing,
            140.0 + swing,
            85.0 + 2.0 * swing,
        );
        sim.advance(250);
    }
    let passes = (passes(&sim) - passes_before) / seconds;
    let frames = u64::from(frames(&sim) - frames_before) / seconds;
    (passes, frames)
}

#[test]
fn a_recording_ride_passes_about_as_often_as_it_draws() {
    let (passes, frames) = riding_pace(60);
    println!("a ride: {passes} passes/s, {frames} frames/s");
    assert!(frames >= 15, "the readings glide: {frames} frames/s");
    assert!(
        passes <= frames + 10,
        "{passes} passes/s for {frames} frames/s"
    );
}

#[test]
fn a_radar_page_wakes_a_resting_ride() {
    let mut sim = device();
    sim.ant_sensor(device_type::RADAR, 4);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.advance(seconds(3));
    let mut woken = 0;
    for _ in 0..300 {
        let rested = sim.device.now;
        let resting = sim.device.clock_ms() > rested;
        let world_ms = sim.device.sim().world.time_ms + 10;
        let due = sim.device.sim().ant.page_due(device_type::RADAR, world_ms);
        sim.advance(10);
        if resting && due {
            assert!(sim.device.now > rested, "a radar page at {world_ms} ms");
            woken += 1;
        }
    }
    assert!(woken >= 4, "{woken} radar pages ended a rest");
}

/// A press the page does not answer, the second report of a hold, still
/// waits for a frame to time it, at the busy pace until that frame.
#[test]
fn a_press_waiting_for_its_frame_keeps_the_busy_pace() {
    let mut sim = device();
    sim.advance(seconds(3));
    sim.report(Button::BottomLeft);
    sim.advance(100);
    sim.report(Button::BottomLeft);
    sim.advance(10);
    assert!(sim.device.shell.frame_wanted(sim.device.now));
    expect_busy(&mut sim, 500, "a press waiting for its frame");
    assert_eq!(sim.device.sdk.page(), Page::Home);
}

#[test]
fn a_list_coasting_after_a_fling_keeps_the_busy_pace() {
    let mut sim = rides_list(&[1; 12]);
    resting(&mut sim);
    let before = sim.screen().text();
    sim.drag((120, 280), (120, 230), 50, 0);
    expect_busy(&mut sim, 500, "a list coasting");
    sim.advance(seconds(3));
    assert_ne!(sim.screen().text(), before, "the list moved");
}

/// A rider moving on Home with a terrain pack: the look-ahead's first
/// steps, some of them computing only, run at the busy pace.
#[test]
fn the_look_ahead_at_work_keeps_the_busy_pace() {
    use terrain::Status;
    let mut sim = Sim::new();
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    let pack = std::fs::read(format!(
        "{}/../../packages/terrain/tests/data/farellones.terrain",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &pack);
    let road = [(-33.367233, -70.499463), (-33.351434, -70.310506)];
    let points = road.map(|(lat, lon)| (route::Point::new(lat, lon), None));
    sim.ride_route(Route::from_gpx(&to_gpx("toward Farellones", &points)).unwrap());
    sim.ride_at(20.0, 0.0, 0.0, 0.0);
    let working = |sim: &Sim| sim.device.sdk.terrain_status() == Some(Status::Working);
    while !working(&sim) {
        assert!(sim.now() < seconds(30), "the look-ahead never worked");
        sim.advance(10);
    }
    let (before, began) = (passes(&sim), sim.now());
    while working(&sim) {
        sim.advance(10);
    }
    let worked_ms = sim.now() - began;
    let pace = (passes(&sim) - before) * 1000 / worked_ms;
    println!("the look-ahead worked {worked_ms} ms: {pace} passes/s");
    assert!(worked_ms >= 100, "worked {worked_ms} ms");
    assert!(pace >= BUSY, "worked {worked_ms} ms: {pace} passes/s");
    assert_eq!(sim.device.sdk.page(), Page::Home);
}

/// After a cold boot, Home counts the saved rides at the busy pace: each
/// step reads them from storage.
#[test]
fn counting_saved_rides_after_a_cold_boot_keeps_the_busy_pace() {
    let mut sim = saved_rides(&[1; 120]);
    sim.power_loss();
    assert!(!sim.screen().contains("120 SAVED"), "counted during boot");
    let (before, began) = (passes(&sim), sim.now());
    sim.wait_for("120 SAVED", seconds(5)).unwrap();
    let counted_ms = sim.now() - began;
    let pace = (passes(&sim) - before) * 1000 / counted_ms;
    println!("counted in {counted_ms} ms: {pace} passes/s");
    assert!(pace >= BUSY, "counted in {counted_ms} ms: {pace} passes/s");
    expect_still(&mut sim, "home with its rides counted");
}

#[test]
fn a_press_wakes_a_resting_home() {
    let pressed = |press: bool| {
        let mut sim = device();
        sim.advance(seconds(3));
        resting(&mut sim);
        let rested = sim.device.now;
        if press {
            sim.report(Button::BottomRight);
        }
        sim.advance(10);
        (sim.device.now > rested, sim.device.sdk.page())
    };
    assert_eq!(pressed(false), (false, Page::Home), "no pass falls due");
    assert_eq!(pressed(true), (true, Page::Ready), "taken in its own slice");
}

#[cfg(feature = "debug-harness")]
#[test]
fn a_harness_press_runs_when_it_is_due() {
    let mut sim = device();
    sim.advance(seconds(3));
    let id = (u64::from(sim.device.boot) << 32) | 1;
    let mut command = |line: String| {
        let (status, out) = sim.command(&line);
        assert!(
            matches!(status, "OK" | "ACCEPTED"),
            "{line}: {status} {out}"
        );
    };
    command("HARNESS OPEN 1 10000".into());
    command(format!("HARNESS {id} INPUT BEGIN 1"));
    command(format!("HARNESS {id} INPUT ADD 1 405 BUTTON 2"));
    command(format!("HARNESS {id} INPUT RUN 1"));
    sim.advance(seconds(1));
    let status = sim.command(&format!("HARNESS {id} INPUT STATUS")).1;
    assert!(status.contains("state=completed"), "{status}");
    let late: u64 = status
        .split_whitespace()
        .find_map(|field| field.strip_prefix("max_late_ms=")?.parse().ok())
        .expect("max_late_ms");
    assert!(late <= 10, "the press ran {late} ms after it was due");
    assert_eq!(sim.device.sdk.page(), Page::Ready);
}

#[test]
fn a_resting_session_with_a_press_replays_exactly() {
    let mut sim = Sim::new();
    sim.hash_frames();
    let mut sim = sim.boot();
    sim.advance(seconds(5));
    resting(&mut sim);
    sim.press(Button::BottomRight);
    sim.press(Button::TopLeft);
    sim.advance(seconds(5));
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
    let frames = sim.frame_hashes();
    assert!(!frames.is_empty() && replayed.frame_hashes() == frames);
}

/// A drag that starts while the console rests is stamped as it arrives, so
/// its fling coasts as far as one that starts just after a pass.
#[test]
fn a_fling_from_a_resting_list_coasts_as_from_a_busy_one() {
    let fling = |rested: bool| {
        let mut sim = rides_list(&[1; 12]);
        resting(&mut sim);
        while !rested && sim.device.clock_ms() > sim.device.now {
            sim.advance(10);
        }
        let before = sim.screen().text();
        sim.drag((120, 280), (120, 230), 50, 0);
        sim.advance(seconds(3));
        let after = sim.screen().text();
        assert_ne!(after, before, "the list moved");
        after
    };
    assert_eq!(fling(true), fling(false));
}

/// A saved ride opened from the still rides list reads at the busy pace,
/// as any step that reads storage does, and its page rests once it is read.
#[test]
fn reading_a_saved_ride_keeps_the_busy_pace() {
    let mut sim = rides_list(&[30, 2]);
    sim.press(Button::BottomLeft);
    sim.expect_text("2/2");
    let (before, began) = (passes(&sim), sim.now());
    sim.report(Button::BottomRight);
    sim.advance(10);
    sim.expect_text("READING");
    while sim.screen().contains("READING") {
        sim.advance(10);
        assert!(sim.now() - began < seconds(5), "still reading");
    }
    let read_ms = sim.now() - began;
    let pace = (passes(&sim) - before) * 1000 / read_ms;
    println!("reading for {read_ms} ms: {pace} passes/s");
    assert!(pace >= BUSY, "reading for {read_ms} ms: {pace} passes/s");
    assert_eq!(sim.device.sdk.page(), Page::PastRide);
    expect_still(&mut sim, "a saved ride");
}
