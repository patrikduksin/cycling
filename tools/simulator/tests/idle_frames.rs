//! An idle screen draws at its page's pace and no faster: once a second for
//! still pages, twice while something animates, such as the charging battery.
#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::sim::{Sim, seconds};

/// Frames in `WINDOW_S` idle seconds.
const WINDOW_S: u64 = 30;
const STILL: u32 = 30;
const ANIMATED: u32 = 60;

/// Frames submitted over `WINDOW_S` idle seconds, after three to settle.
fn idle_frames(sim: &mut Sim) -> u32 {
    sim.advance(seconds(3));
    let before = sim.device.shell.display_submissions;
    sim.advance(seconds(WINDOW_S));
    sim.device.shell.display_submissions - before
}

#[track_caller]
fn expect_pace(sim: &mut Sim, screen: &str, most: u32) {
    let frames = idle_frames(sim);
    println!("{screen}: {frames} frames / {WINDOW_S} s");
    assert!(
        frames <= most,
        "{screen}: {frames} frames in {WINDOW_S} idle seconds, at most {most}; screen:\n{}",
        sim.screen().text()
    );
}

/// Back to Home; the first press may only wake a dimmed screen.
fn go_home(sim: &mut Sim) {
    for _ in 0..3 {
        if sim.on_home() {
            return;
        }
        sim.press(Button::TopLeft);
    }
    panic!("not back on Home; screen:\n{}", sim.screen().text());
}

fn device() -> Sim {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim
}

#[test]
fn idle_menus_draw_once_a_second() {
    let mut sim = device();
    expect_pace(&mut sim, "home", STILL);
    sim.home("SETTINGS");
    expect_pace(&mut sim, "settings", STILL);
    go_home(&mut sim);
    sim.home("RIDES");
    expect_pace(&mut sim, "rides", STILL);
    go_home(&mut sim);
    sim.home("SENSORS");
    expect_pace(&mut sim, "sensors", STILL);
    go_home(&mut sim);
    sim.set_battery(60, true);
    expect_pace(&mut sim, "home charging", ANIMATED);
}

#[test]
fn saved_sensors_out_of_range_do_not_add_frames() {
    let mut sim = device();
    let sensors = [
        (device_type::HEART_RATE, 1),
        (device_type::POWER, 2),
        (device_type::SPEED_CADENCE, 3),
        (device_type::RADAR, 4),
    ];
    for (kind, number) in sensors {
        sim.ant_sensor(kind, number);
    }
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"]);
    for (kind, number) in sensors {
        sim.set_present(kind, number, false);
    }
    sim.power_loss();
    sim.wait_usable();
    sim.expect_text("0/4 LIVE");
    // VANA keeps asking for each saved sensor while it is away.
    expect_pace(&mut sim, "home, 4 sensors away", STILL);
    sim.set_battery(80, true);
    expect_pace(&mut sim, "home charging, 4 sensors away", ANIMATED);
}

#[test]
fn a_paused_ride_draws_twice_a_second_on_every_page() {
    let mut sim = device();
    sim.start_ride();
    sim.pause_ride();
    expect_pace(&mut sim, "paused", ANIMATED);
    sim.show_ride_pages();
    let mut pages = 0;
    while !sim.screen().contains("RIDE TIME") {
        let page = sim.screen().page().unwrap_or_default();
        expect_pace(&mut sim, &format!("paused, ride page {page}"), ANIMATED);
        sim.press(Button::BottomLeft);
        pages += 1;
        assert!(pages <= 8, "ride pages never returned to the paused screen");
    }
    assert!(pages >= 5, "visited {pages} ride pages");
}

fn frames_over(sim: &mut Sim, ms: u64) -> u32 {
    let before = sim.device.shell.display_submissions;
    sim.advance(ms);
    sim.device.shell.display_submissions - before
}

#[test]
fn a_still_home_draws_only_when_what_it_shows_changes() {
    let mut sim = device();
    sim.set_battery(80, false);
    sim.set_gps_fix(true);
    sim.ant_sensor(device_type::HEART_RATE, 1);
    sim.pair(&["HEART RATE"]);
    go_home(&mut sim);
    let next_minute_ms = 1_800_000_000_000;
    let minute_turns_in = seconds(40);
    sim.sync_network_time(next_minute_ms - minute_turns_in);
    sim.advance(seconds(3));
    sim.expect_text("1/1 LIVE");
    let clock = sim.screen().text();
    assert_eq!(frames_over(&mut sim, seconds(30)), 0, "still Home");
    assert!(frames_over(&mut sim, seconds(10)) >= 1, "the minute turned");
    assert_ne!(sim.screen().text(), clock, "the clock moved on");
    assert_eq!(frames_over(&mut sim, seconds(20)), 0, "still Home");
    sim.set_battery(12, false);
    assert!(frames_over(&mut sim, seconds(3)) >= 1, "the battery fell");
    sim.expect_text("12%");
    sim.set_present(device_type::HEART_RATE, 1, false);
    sim.wait_for("0/1 LIVE", seconds(30)).unwrap();
    sim.set_battery(12, true);
    let frames = frames_over(&mut sim, seconds(30));
    assert!(frames >= 50, "{frames} charging frames in 30 s");
}

#[test]
fn a_full_battery_on_usb_leaves_home_still() {
    let mut sim = device();
    sim.set_battery(100, true);
    let next_minute_ms = 1_800_000_000_000;
    sim.sync_network_time(next_minute_ms - seconds(50));
    sim.advance(seconds(5));
    assert_eq!(frames_over(&mut sim, seconds(30)), 0, "no fill to animate");
    sim.set_battery(90, true);
    assert!(
        frames_over(&mut sim, seconds(5)) >= 8,
        "a fill climbing from 90% animates"
    );
}

#[cfg(feature = "debug-harness")]
#[test]
fn a_capture_on_a_still_home_gets_a_frame() {
    let mut sim = device();
    assert_eq!(frames_over(&mut sim, seconds(5)), 0, "still Home");
    let id = (u64::from(sim.device.boot) << 32) | 1;
    assert_eq!(sim.command("HARNESS OPEN 1 10000").0, "OK");
    let start = sim.command(&format!("HARNESS {id} CAPTURE START 1 100"));
    assert_eq!(start.0, "ACCEPTED", "{start:?}");
    sim.advance(seconds(2));
    let status = sim.command(&format!("HARNESS {id} CAPTURE STATUS"));
    assert!(status.1.contains("state=completed"), "{status:?}");
}
