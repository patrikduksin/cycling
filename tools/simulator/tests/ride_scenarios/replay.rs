use super::*;

#[test]
fn a_ride_with_laps_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), manual_laps, false);
}

#[test]
fn a_trace_damaged_mid_way_replays_to_where_it_breaks_and_says_so() {
    use shell_simulator::replay::{Kind, first_difference};
    let mut sim = equip(Sim::new().boot());
    manual_laps(&mut sim);
    let mut capture = sim.capture();
    let middle = capture.len() / 2;
    capture[middle] ^= 0x40;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.count(Kind::Truncated), 1, "{report}");
    assert!(
        replayed.device.now < sim.device.now,
        "the replay stops where the trace breaks"
    );
    let difference = first_difference(&capture, &replayed.trace()).unwrap();
    assert!(difference.contains("damaged"), "{difference}");
}

#[test]
fn an_auto_paused_ride_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), traffic_light_stop, false);
}

#[test]
fn a_ride_with_a_ble_strap_selected_by_command_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), ble_heart_strap, false);
}

#[test]
fn a_command_runs_after_its_pass_as_on_the_c606() {
    use firmware_services::trace::{Event, decode};
    let capture = assert_replays_exactly(equip(hashed().boot()), ble_heart_strap, false);
    let (_, events) = decode(&capture).unwrap();
    let (mut passes, mut commands) = (Vec::new(), 0);
    for event in events {
        match event {
            Event::Pass { now_ms } => passes.push(now_ms),
            Event::Command(_) => commands += 1,
            _ => {}
        }
    }
    assert!(commands > 0, "the scenario sends a command");
    let again = passes.windows(2).find(|pair| pair[1] <= pair[0]);
    assert_eq!(again, None, "a pass at or before the one before it");
}

#[test]
fn a_two_hour_ride_with_a_pause_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), two_hour_ride_with_a_pause, false);
}

#[test]
fn a_clock_set_from_the_host_replays_exactly() {
    let mut sim = hashed().boot();
    a_clock_set_from_the_host(&mut sim);
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    if let Some(difference) = shell_simulator::replay::first_difference(&capture, &replayed.trace())
    {
        panic!("the replay read or showed something else: {difference}");
    }
    assert_eq!(sim.frame_hashes(), replayed.frame_hashes());
}

fn a_ride_dated_by_wifi(sim: &mut Sim) {
    sim.sync_network_time(WIFI_UTC_MS);
    let synced = sim.now();
    cyclist(sim);
    let pressed = press_start(sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let start = fit_start_ms(&sim.rides().pop().unwrap());
    let expected = WIFI_UTC_MS + pressed - synced;
    assert!(
        start.abs_diff(expected) < 1_000,
        "the ride starts at {start}, Wi-Fi time at the press is {expected}"
    );
}

#[test]
fn a_ride_dated_by_wifi_replays_exactly_whatever_the_process_clock_holds() {
    use firmware_services::network_time::{Anchor, Source, offer};
    offer(Anchor::new(Source::Sntp, WIFI_UTC_MS + 365 * 86_400_000, 0));
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_dated_by_wifi, false);
    let (_, events) = firmware_services::trace::decode(&capture).unwrap();
    let synced = events
        .filter_map(|event| match event {
            firmware_services::trace::Event::NetworkTime(time) => time.unix_seconds,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(synced.len(), 1, "one synchronization: {synced:?}");
    assert!(synced[0].abs_diff(WIFI_UTC_MS / 1000) < 60, "{synced:?}");
}

fn a_ride_on_new_bulk_media(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(minutes(5));
    sim.pause_ride();
    sim.advance(minutes(1));
    sim.resume_ride();
    sim.advance(minutes(5));
    sim.stop_ride();
}

#[test]
fn a_ride_on_bulk_media_replays_exactly() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    assert_replays_exactly(equip(sim.boot()), a_ride_on_new_bulk_media, true);
}

#[test]
fn a_trace_names_the_terrain_pack_and_the_working_memory_its_session_started_with() {
    use firmware_services::trace::origin::{Found, Memory};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.install_terrain(b"CHILE-V1\0\0\0\0\0\0\0\0", &[7u8; 2048]);
    let mut sim = equip(sim.boot());
    a_ride_on_new_bulk_media(&mut sim);

    let capture = sim.capture();
    let (header, _) = firmware_services::trace::decode(&capture).unwrap();
    let Found::Read(terrain) = header.terrain else {
        panic!("the pack: {:?}", header.terrain);
    };
    assert!(terrain.ready);
    assert_eq!(terrain.length, 2048);
    assert_eq!(&terrain.tag[..8], b"CHILE-V1");
    let opened = world::opened(&capture);
    assert_eq!((opened.memory, opened.redacted), (Memory::ALL, false));
}

#[test]
fn a_replay_gives_vana_only_the_working_memory_the_recording_had() {
    use firmware_services::trace::{Event, decode};
    let mut sim = equip(Sim::new().boot());
    sim.advance(seconds(5));
    let mut capture = sim.capture();
    assert!(sim.device.sdk.detach_terrain().is_some());
    let trace: &'static [u8] = Box::leak(capture.clone().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let events: Vec<Event<'static>> = events
        .map(|event| match event {
            Event::Session {
                mut memory,
                redacted,
            } => {
                memory.terrain = false;
                Event::Session { memory, redacted }
            }
            event => event,
        })
        .collect();
    capture = shell_simulator::trace::surgery::encode(&header, events);
    let mut replayed = Sim::replay(&capture);
    assert!(
        replayed.device.sdk.detach_terrain().is_none(),
        "no tile cache, as the recording had none"
    );
}

#[test]
fn a_session_recorded_into_the_store_replays_exactly_from_the_file_a_host_pulls() {
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    a_ride_on_new_bulk_media(&mut sim);
    assert!(sim.device.sync_trace(), "the session records into STORE");
    let name = sim.device.trace_file().unwrap();
    assert!(name.starts_with("traces/000001-"), "{name}");
    let pulled = sim.store_file(&name).expect("every record reads back");
    let mut capture = sim.capture();
    assert_eq!(pulled.len(), capture.len());
    assert!(pulled == capture, "the file is the trace");
    capture = pulled;

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
    let (frames, replayed_frames) = (sim.frame_hashes(), replayed.frame_hashes());
    assert!(frames.len() > 100);
    assert!(frames == replayed_frames, "the same frames from power-on");
    let faithful = report.faithful.expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{faithful}");
    assert!(
        sim.bulk_rides()
            .iter()
            .any(|ride| ride.samples().count() > 30),
        "a ride was saved"
    );
}

#[test]
fn a_new_store_replays_with_the_entropy_it_was_named_from() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_on_new_bulk_media, true);
    let entropy = count_world(&capture, |world| matches!(world, World::Entropy(_)));
    assert!(entropy > 0, "the store's entropy is recorded");
}

fn paired_then_ridden(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(10));
    sim.start_ride();
    sim.advance(minutes(2));
    sim.ride_at(33.0, 250.0, 150.0, 92.0);
    sim.advance(minutes(2));
    sim.stop_ride();
}

fn sensor_data(sim: &mut Sim) -> Vec<Vec<Sample>> {
    sim.rides()
        .iter()
        .map(|ride| ride.samples().copied().collect())
        .collect()
}

#[test]
fn a_ride_replayed_changed_on_the_same_build_saves_the_same_ride() {
    use shell_simulator::replay::{Mode, Options};
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let changed = Options {
        mode: Mode::Changed,
        model: false,
    };
    let mut replayed = Sim::replay_as(&recorded.capture(), changed);
    let report = replayed.device.report().unwrap();
    assert_eq!(
        report.verdict(),
        Ok("the replay re-recorded the trace byte for byte".into()),
        "{report}"
    );
    let rides = sensor_data(&mut recorded);
    assert!(
        rides.iter().any(|ride| ride.len() > 200),
        "a ride was saved"
    );
    assert_eq!(sensor_data(&mut replayed), rides);
}

fn connected_in_order(trace: &[u8]) -> Vec<u8> {
    use firmware_services::trace::{Event, decode};
    let (_, events) = decode(trace).unwrap();
    events
        .filter_map(|event| match event {
            Event::Call(call) => match world::read(&call) {
                Some(World::AntRequest {
                    operation: device_api::ant::AntOperation::Connect(peer),
                    result: Ok(_),
                }) => Some(peer.device_type),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn with_the_first_connects_swapped(trace: &[u8]) -> Vec<u8> {
    use device_api::ant::AntOperation;
    use firmware_services::trace::wire::{Wire, Writer};
    use firmware_services::trace::{Event, decode};
    let trace: &'static [u8] = Box::leak(trace.to_vec().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let connects: Vec<(usize, usize)> = (events.iter().enumerate())
        .filter_map(|(at, event)| match event {
            Event::Call(call) => match world::read(call) {
                Some(World::AntRequest {
                    operation: AntOperation::Connect(_),
                    result: result @ Ok(_),
                }) => {
                    let mut bytes = [0u8; 64];
                    let mut w = Writer::new(&mut bytes, call.now_ms);
                    result.put(&mut w);
                    Some((at, call.body.len() - w.written().unwrap()))
                }
                _ => None,
            },
            _ => None,
        })
        .take(2)
        .collect();
    let [(first, first_args), (second, second_args)] = connects[..] else {
        panic!("two connections the radio accepted")
    };
    let (Event::Call(mut a), Event::Call(mut b)) = (events[first], events[second]) else {
        unreachable!()
    };
    let a_body = [&b.body[..second_args], &a.body[first_args..]].concat();
    let b_body = [&a.body[..first_args], &b.body[second_args..]].concat();
    a.body = Vec::leak(a_body);
    b.body = Vec::leak(b_body);
    events[first] = Event::Call(a);
    events[second] = Event::Call(b);
    shell_simulator::trace::surgery::encode(&header, events)
}

#[test]
fn a_ride_recorded_power_meter_first_replays_on_a_modelled_branch_and_saves_the_sensor_data() {
    use shell_simulator::replay::{Kind, Mode, Options};
    let mut recorded = equip(hashed().boot());
    paired_then_ridden(&mut recorded);
    let mut capture = recorded.capture();
    capture = with_the_first_connects_swapped(&capture);
    let modelled = Options {
        mode: Mode::Changed,
        model: true,
    };
    let mut replayed = Sim::replay_as(&capture, modelled);
    let (power, heart, speed) = (
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::SPEED_CADENCE,
    );
    assert_eq!(connected_in_order(&capture), [power, heart, speed]);
    assert_eq!(
        connected_in_order(&replayed.trace()),
        [heart, power, speed],
        "this build connects the strap first"
    );
    let report = replayed.device.report().unwrap();
    let branched = report.branched.as_ref().expect("the replay branched");
    assert!(report.verdict().is_err(), "a branch fails the verdict");
    assert!(report.count(Kind::Modelled) >= 1, "{report}");
    assert!(report.count(Kind::NotRequested) >= 1, "{report}");
    assert!(
        branched.detail.contains("Ant::request"),
        "the branch names the call: {}",
        branched.detail
    );
    assert_eq!(sensor_data(&mut replayed), sensor_data(&mut recorded));
}

/// A ride whose page turns on a press the harness sent over the terminal,
/// mid-ride, as a host test drives a C606.
fn a_ride_paged_by_the_harness(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(seconds(30));
    let (status, opened) = sim.command("HARNESS OPEN 1 60000");
    assert_eq!(status, "OK", "{opened}");
    let session = opened
        .split_whitespace()
        .find_map(|word| word.strip_prefix("session="))
        .unwrap()
        .to_string();
    for command in ["INPUT BEGIN 1", "INPUT ADD 1 0 BUTTON 1", "INPUT RUN 1"] {
        let (status, out) = sim.command(&format!("HARNESS {session} {command}"));
        assert!(
            matches!(status, "OK" | "ACCEPTED"),
            "{command}: {status} {out}"
        );
    }
    sim.advance(seconds(1));
    expect_page(sim, "ride/climb");
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
}

#[test]
fn a_ride_paged_by_the_harness_replays_exactly() {
    use firmware_services::trace::Event;
    let capture =
        assert_replays_exactly(equip(hashed().boot()), a_ride_paged_by_the_harness, false);
    let pressed = count_events(&capture, |event| matches!(event, Event::Synthetic(_)));
    assert_eq!(pressed, 1, "the harness pressed once");
}

fn a_ride_paged_by_a_press(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.advance(seconds(30));
    sim.press(Role::Move);
    expect_page(sim, "ride/climb");
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
}

#[track_caller]
fn expect_page(sim: &Sim, page: &str) {
    let seen = sim.seen().page.map(|(_, page)| page);
    assert_eq!(seen.as_deref(), Some(page));
}

#[test]
fn a_ride_paged_by_a_press_replays_exactly() {
    assert_replays_exactly(equip(hashed().boot()), a_ride_paged_by_a_press, false);
}

#[test]
fn a_refused_frame_replays_as_refused_and_says_so() {
    let mut sim = hashed().boot();
    let mut out = String::new();
    assert_eq!(
        sim.device.fixture("FIXTURE DISPLAY_FAIL", &mut out),
        "OK",
        "{out}"
    );
    sim.press(Role::Act);
    sim.advance(seconds(2));
    let replayed = Sim::replay(&sim.capture());
    let report = replayed.device.report().expect("a replay");
    let faithful = report.faithful.clone().expect("a replay");
    assert_eq!(faithful.ended, None, "{report}");
    assert!(
        faithful
            .exclusions
            .contains(&shell_simulator::replay::Exclusion::RefusedFrames(1)),
        "{report}"
    );
}
