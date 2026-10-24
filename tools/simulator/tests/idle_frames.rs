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
