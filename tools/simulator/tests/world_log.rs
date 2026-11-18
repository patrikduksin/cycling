use firmware_services::trace::{Event, decode};
use shell_simulator::ant::device_type;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::twin::script::declared;
use std::collections::BTreeMap;

const POWER: u16 = 1234;

fn size(body: usize) -> usize {
    2 + (1..).find(|bytes| body < 1 << (7 * bytes)).unwrap_or(1) + body
}

fn ridden() -> Sim {
    let mut sim = Sim::new().boot();
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(30.0, 210.0, 0.0, 90.0);
    sim.advance(seconds(180));
    sim
}

#[test]
fn a_ride_writes_what_each_capability_gave_vana() {
    let trace = ridden().capture();
    let (_, events) = decode(&trace).expect("the trace decodes");
    let mut methods: BTreeMap<String, usize> = BTreeMap::new();
    for event in events {
        if let Event::Call(call) = event {
            let (capability, method) =
                declared(call.method).expect("every world record names a declared method");
            *methods
                .entry(format!("{}::{}", capability.name, method.name))
                .or_default() += 1;
        }
    }
    for expected in [
        "Settings::load",
        "OwnedFlash::read",
        "OwnedFlash::program",
        "Display::geometry",
        "InputSource::take_edge",
        "InputObservation::snapshot",
        "Sensors::startup_status",
        "Sensors::snapshot",
        "PowerControl::status",
        "Positioning::snapshot",
        "Ant::request",
        "Ant::channels",
        "Ant::take_packet",
        "Ble::control",
        "Sound::snapshot",
        "Server::room",
        "AccessPoint::state",
    ] {
        assert!(
            methods.contains_key(expected),
            "no {expected} record in {methods:?}"
        );
    }
}

#[test]
fn a_riding_pass_costs_few_bytes_of_world_records() {
    let sim = ridden();
    let trace = sim.capture();
    let (_, events) = decode(&trace).expect("the trace decodes");
    let riding_from = sim.device.now - 120_000;
    let (mut passes, mut bytes) = (0usize, 0usize);
    let mut by_method: BTreeMap<String, usize> = BTreeMap::new();
    let mut now = 0;
    for event in events {
        match event {
            Event::Pass { now_ms } => {
                now = now_ms;
                if now >= riding_from {
                    passes += 1;
                    bytes += 1;
                }
            }
            Event::Call(call) if now >= riding_from => {
                let (capability, method) = declared(call.method).unwrap();
                bytes += size(call.body.len());
                *by_method
                    .entry(format!("{}::{}", capability.name, method.name))
                    .or_default() += size(call.body.len());
            }
            _ => {}
        }
    }
    let per_pass = bytes as f64 / passes as f64;
    println!(
        "{passes} riding passes, {bytes} bytes of passes and world records: {per_pass:.1} a pass"
    );
    for (method, bytes) in &by_method {
        println!("  {method}: {:.2} a pass", *bytes as f64 / passes as f64);
    }
    assert!(passes > 1_000, "{passes} passes in the last two minutes");
    assert!(per_pass < 24.0, "{per_pass:.1} bytes a riding pass");
}

#[test]
fn a_radio_command_writes_the_control_status_it_read() {
    let mut sim = Sim::new().boot();
    sim.advance(seconds(5));
    let mut reply = String::new();
    sim.device
        .command(
            b"CMD 1 WIFI CONFIG WPA2 54657374 70617373776f7264",
            &mut reply,
        )
        .expect("the command parses");
    sim.device
        .command(b"CMD 2 BLE FORGET", &mut reply)
        .expect("the command parses");
    let trace = sim.capture();
    let (_, events) = decode(&trace).expect("the trace decodes");
    let mut after_command = Vec::new();
    let mut commands = 0;
    for event in events {
        match event {
            Event::Command(_) => commands += 1,
            Event::Call(call) if commands > 0 => {
                let (capability, method) = declared(call.method).unwrap();
                after_command.push(format!("{}::{}", capability.name, method.name));
            }
            _ => {}
        }
    }
    for expected in [
        "Wifi::request",
        "Wifi::control",
        "Ble::request",
        "Ble::control",
    ] {
        assert!(
            after_command.iter().any(|method| method == expected),
            "no {expected} after the commands: {after_command:?}"
        );
    }
}

#[test]
fn a_trace_read_back_from_its_file_alone_replays_faithful() {
    let dir = std::env::temp_dir().join(format!("trace-alone-{}", std::process::id()));
    let path = dir.join("ride.trace");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, ridden().capture()).unwrap();
    let alone = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let report = Sim::replay(&alone).device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    let faithful = report.faithful.expect("a replay says what it proved");
    assert_eq!(faithful.ended, None);
    assert!(faithful.physical > 0, "{faithful}");
}

/// A session recording into the card's STORE for a minute, then reset
/// without warning: by a panic when `panic`, or by a power cut. The trace
/// it recorded, the file it recorded into, and the next session's trace.
fn reset_while_recording(panic: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    use shell_simulator::bulk::Presence;
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = sim.boot();
    sim.ant_sensor(device_type::POWER, POWER);
    sim.advance(seconds(60));
    let name = sim
        .device
        .trace_file()
        .expect("the session records into STORE");
    let recorded = sim.trace();
    if panic {
        sim.crash();
    } else {
        sim.cut_power();
    }
    sim.advance(seconds(20));
    let file = sim.store_file(&name).expect("every record reads back");
    (recorded, file, sim.trace())
}

fn last_pass(trace: &[u8]) -> Option<u64> {
    let (_, events) = decode(trace).expect("the trace decodes");
    events
        .filter_map(|event| match event {
            Event::Pass { now_ms } => Some(now_ms),
            _ => None,
        })
        .last()
}

fn before(next: &[u8]) -> (Option<Option<u64>>, Option<u32>) {
    let (_, events) = decode(next).expect("the trace decodes");
    let (mut horizon, mut salvaged) = (None, None);
    for event in events {
        match event {
            Event::Horizon { last_pass_ms, .. } => horizon = Some(last_pass_ms),
            Event::Salvaged { blocks } => salvaged = Some(blocks),
            _ => {}
        }
    }
    (horizon, salvaged)
}

#[test]
fn a_panic_keeps_the_blocks_its_spool_held_and_a_power_cut_loses_them() {
    let (recorded, file, next) = reset_while_recording(true);
    assert!(
        file == recorded,
        "after a panic the file is the whole trace"
    );
    let (horizon, salvaged) = before(&next);
    assert!(salvaged.is_some_and(|blocks| blocks > 0), "{salvaged:?}");
    assert_eq!(
        horizon,
        Some(last_pass(&recorded)),
        "the horizon is its end"
    );

    let (recorded, file, next) = reset_while_recording(false);
    assert!(
        file.len() < recorded.len(),
        "a power cut loses what the spool held: {} of {} bytes",
        file.len(),
        recorded.len()
    );
    assert!(
        recorded.starts_with(&file),
        "the file is what the trace held to its last record"
    );
    let (horizon, salvaged) = before(&next);
    assert_eq!(salvaged, None, "a power-on boot salvages nothing");
    assert_eq!(
        horizon,
        Some(last_pass(&file)),
        "the horizon is what survived"
    );
}
