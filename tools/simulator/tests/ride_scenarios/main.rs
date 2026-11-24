//! VANA behaviour in a simulated world: sensors, rides, losses and recovery.
use device_api::ant::{LinkState, Request};
use device_api::ble_transport::Link;
use device_api::gps::FixState;
use device_api::input::Role;
use device_api::positioning::Positioning;
use firmware_services::gps::unix_seconds;
use shell_simulator::ant::{BatteryReport, ZeroReply, device_type};
use shell_simulator::bulk::Presence;
use shell_simulator::models;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Need, NoteKind, Screen, Sim, minutes, seconds};
use shell_simulator::trace::timeline::{Kind as Traced, session_clock};
use shell_simulator::trace::world::{self, World};
use shell_simulator::world::{Side, Vehicle};
use vana::ride::fit::Refusal;
use vana::ride::log::{
    Entry, Kind, LapStart, SLOT_SIZE, Sample, Slot, commit_word, decode, encode,
};
use vana::screens::chrome::{AMBER, GREEN, RED, SECONDARY, WHITE, ZONES};

mod arrival_chime;
mod auto_pause;
mod bulk_media;
mod climb;
mod clock;
mod course_following;
mod course_map;
mod laps;
mod marks;
mod mutations;
mod navigation;
mod phone_uploads;
mod power_and_battery;
mod radar;
mod replay;
mod replay_inputs;
mod ride_lifecycle;
mod riding;
mod saver_mode;
mod screens;
mod sensor_batteries;
mod sensors;
mod storage;
mod support;
mod track_map;
mod wheel;
mod workouts;
mod zones_and_profile;

use support::*;

/// The milestones VANA reports for `cycling startup-timing --sim`, which
/// reads them from `STARTUP_TIMING_OUT`. Simulated timings are modelled.
#[test]
fn startup_timing() {
    let mut sim = Sim::new();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    let lit = wait_lit(&mut sim);
    sim.checkpoint("power-on: brand flash");
    sim.wait_usable();
    let home = sim.device.now;
    sim.checkpoint("power-on: home ready");
    assert!(home - lit <= 500, "brand flash {} ms", home - lit);
    assert!(home <= seconds(5), "Home at {home} ms uptime");
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 180.0, 130.0, 85.0);
    sim.advance(seconds(10));
    sim.pause_ride();
    sim.resume_ride();
    let (status, timing) = sim.command("RIDE TIMING");
    assert_eq!(status, "OK", "{timing}");
    for key in [
        "first_frame_ms",
        "home_ms",
        "storage_ms",
        "gnss_fix_ms",
        "ride_press_ms",
        "recording_ms",
    ] {
        assert!(
            !timing.contains(&format!("{key}=-")),
            "{key} missing: {timing}"
        );
    }
    if let Ok(path) = std::env::var("STARTUP_TIMING_OUT") {
        std::fs::write(path, &timing).expect("timing output is writable");
    }
    sim.stop_ride();
}
