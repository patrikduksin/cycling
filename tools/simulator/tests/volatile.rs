use device_api::input::Role;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::models::VOLATILE;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, seconds};

fn rider() -> Sim {
    let mut sim = Sim::on(VOLATILE).boot();
    sim.ant_sensor(device_type::POWER, 1234);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER"]);
    sim
}

fn ride(sim: &mut Sim, watts: f64, secs: u64) {
    sim.start_ride();
    sim.ride_at(30.0, watts, 0.0, 90.0);
    sim.advance(seconds(secs));
    // Stopped, so rolling on does not start the next ride.
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pause_ride();
    sim.press(Role::Back);
    sim.expect_text("End ride?");
    sim.press(Role::Act);
    sim.wait_for("Sync to save", seconds(5)).unwrap();
    sim.done();
}

fn cold_boot(sim: &mut Sim) {
    sim.cut_power();
    sim.wait_usable();
    sim.pair(&["POWER METER"]);
}

fn ride_ids(sim: &mut Sim) -> Vec<u32> {
    let (status, out) = sim.command("RIDE HISTORY");
    assert_eq!(status, "OK", "{out}");
    out.split_whitespace()
        .filter_map(|field| field.strip_prefix("ride="))
        .map(|ride| ride.split(',').next().unwrap().parse().unwrap())
        .collect()
}

#[test]
fn a_restart_or_a_turn_off_waits_for_the_phone() {
    let mut sim = rider();
    ride(&mut sim, 200.0, 60);
    for command in [
        "RESTART",
        "POWER SHUTDOWN",
        "POWER SHUTDOWN AFTER 1000",
        "POWER SLEEP",
    ] {
        assert_eq!(sim.command(command).0, "UNSYNCED", "{command}");
    }
    sim.advance(seconds(5));
    assert!(!sim.is_off());
    assert_eq!(ride_ids(&mut sim).len(), 1, "the ride is still in memory");
    sim.checkpoint("home after a refused restart");
}

#[test]
fn a_cold_boot_with_the_card_in_starts_ride_ids_again_without_old_records() {
    let mut sim = rider();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    cold_boot(&mut sim);
    ride(&mut sim, 200.0, 60);
    let first = ride_ids(&mut sim);
    assert_eq!(first.len(), 1);
    cold_boot(&mut sim);
    assert!(ride_ids(&mut sim).is_empty(), "power-off loses memory");
    sim.expect_no_text("NOT SAVED");
    ride(&mut sim, 150.0, 30);
    let second = ride_ids(&mut sim);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0], first[0], "the new ride takes the old id");
}

#[test]
fn a_forced_reboot_loses_the_rides_in_memory_and_boots_ready() {
    let mut sim = rider();
    ride(&mut sim, 200.0, 60);
    sim.crash();
    sim.wait_usable();
    assert!(ride_ids(&mut sim).is_empty());
    sim.expect_no_text("NOT SAVED");
    assert_eq!(sim.command("RESTART").0, "ACCEPTED", "nothing left to lose");
}
