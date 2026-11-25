use super::*;

fn body_read(trace: &[u8]) -> usize {
    let (_, events) = firmware_services::trace::decode(trace).unwrap();
    events
        .filter_map(|event| match event {
            firmware_services::trace::Event::Call(call) => match world::read(&call) {
                Some(World::ServerRead { part, .. }) => Some(part.len()),
                _ => None,
            },
            _ => None,
        })
        .sum()
}

fn a_course_uploaded_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.disconnect_phone();
    sim.advance(seconds(1));
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
}

#[test]
fn a_course_uploaded_from_the_phone_mid_session_replays_exactly() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let capture = assert_replays_exactly(equip(sim.boot()), a_course_uploaded_from_the_phone, true);
    let read = body_read(&capture);
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    assert_eq!(read, course.len(), "the whole body is in the trace");
    let joined = count_world(&capture, |world| {
        matches!(
            world,
            World::AccessPointState(device_api::server::AccessPointState::Up { clients: 1 })
        )
    });
    assert_eq!(joined, 1, "the phone joined once");
}

#[test]
fn the_largest_phone_upload_reaches_the_trace_file_whole() {
    use device_api::server::Method;
    use firmware_services::trace::{Event, decode};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let end = course.rfind("</gpx>").unwrap();
    let max = firmware_services::http::MAX_BODY as usize;
    let mut body = course[..end].to_string();
    body.push_str(&" ".repeat(max - course.len()));
    body.push_str(&course[end..]);
    assert_eq!(body.len(), max);
    let (status, reply) = sim.phone_request(Method::Put, "/api/course", body.as_bytes());
    assert_eq!(status, 200, "{reply}");
    sim.advance(seconds(6));
    let (peak, capacity) = sim.device.trace_spool_peak().unwrap();
    println!(
        "a {} KiB upload peaks the spool at {peak} of {capacity} B",
        max / 1024
    );
    assert!(sim.device.sync_trace());
    let pulled = sim.store_file(&sim.device.trace_file().unwrap()).unwrap();
    let (_, events) = decode(&pulled).unwrap();
    let lost: u32 = events
        .filter_map(|event| match event {
            Event::Lost { blocks, .. } => Some(blocks),
            _ => None,
        })
        .sum();
    assert_eq!(
        (body_read(&pulled), lost),
        (max, 0),
        "the whole body, nothing lost"
    );
    assert!(peak < capacity / 8, "the drain keeps up: {peak} B");
}

fn a_redacted_phone_session_then_a_ride() -> (Sim, [String; 5]) {
    let mut sim = hashed();
    sim.need(Need::Wifi);
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    sim.device.redact_secrets();
    let mut sim = equip(sim.boot());
    let (status, out) = sim.command("WIFI CONFIG WPA2 486f6d65 7365637265743132");
    assert_eq!(status, "ACCEPTED", "{out}");
    sim.device.restart().unwrap();
    let mut sim = sim.boot();
    sim.command("WIFI CONFIG WPA3 486f6d65 70617373776f7264");
    sim.init_bulk();
    sim.connect_phone();
    let status = sim.device.sdk.phone_status().expect("a phone session");
    let secrets = [
        status.token.text().to_string(),
        status.password.text().to_string(),
        "7365637265743132".to_string(),
        "70617373776f7264".to_string(),
        "secret12".to_string(),
    ];
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (code, body) = sim.phone_request(
        device_api::server::Method::Put,
        "/api/course",
        course.as_bytes(),
    );
    assert_eq!(code, 200, "{body}");
    sim.disconnect_phone();
    sim.advance(seconds(1));
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    (sim, secrets)
}

#[test]
fn a_redacted_phone_session_replays_on_placeholders_and_holds_no_secret() {
    use firmware_services::trace::{Event, decode};
    let (sim, secrets) = a_redacted_phone_session_then_a_ride();
    let capture = sim.capture();
    let text = String::from_utf8_lossy(&capture);
    for secret in &secrets {
        assert!(!text.contains(secret.as_str()), "the trace holds {secret}");
    }
    let said = count_events(&capture, |event| {
        matches!(event, Event::Session { redacted: true, .. })
    });
    assert_eq!(said, 1, "the trace says it replaced its secrets");
    let (_, events) = decode(&capture).unwrap();
    let sessions: Vec<String> = events
        .filter_map(|event| match event {
            Event::Call(call) => match world::read(&call) {
                Some(World::ServerRequest(Some(request))) => Some(request.session.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        sessions.iter().any(|session| session == "****************"),
        "{sessions:?}"
    );

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    let faithful = report.faithful.clone().expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{report}");
    assert!(faithful.exclusions.is_empty(), "{report}");
    let placeholder = replayed.device.sdk.phone_status();
    assert!(placeholder.is_none(), "the session closed for the ride");
}

fn a_phone_paired(redacting: bool) -> Vec<u8> {
    let mut sim = hashed();
    if redacting {
        sim.device.redact_secrets();
    }
    let mut sim = sim.boot();
    sim.advance(seconds(1));
    let (status, out) = sim.command("PHONE PAIR");
    assert_eq!(status, "ACCEPTED", "{out}");
    sim.advance(seconds(1));
    let (_, phone) = sim.command("PHONE");
    assert!(
        !phone.contains("paired=pending"),
        "the pairing saved: {phone}"
    );
    sim.capture()
}

#[test]
fn a_redacted_phone_pairing_replays_faithfully_and_holds_no_key() {
    use shell_simulator::trace::secret::secrets;
    let kept = secrets(&a_phone_paired(false));
    assert!(
        kept.iter()
            .any(|found| found.ends_with("a phone pairing in the shell's settings")),
        "{kept:?}"
    );
    let capture = a_phone_paired(true);
    assert_eq!(secrets(&capture), [] as [&str; 0]);

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    let faithful = report.faithful.clone().expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{report}");
}

fn with_the_last_frame_unseen(mut capture: Vec<u8>) -> Vec<u8> {
    use firmware_services::trace::{Event, decode};
    let trace: &'static [u8] = Box::leak(capture.clone().into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let last = events
        .iter()
        .rposition(|event| matches!(event, Event::Panel(_)))
        .unwrap();
    events[last] = Event::Panel(0xdead_beef);
    capture = shell_simulator::trace::surgery::encode(&header, events);
    capture
}

#[test]
fn a_redacted_phone_session_leaves_a_later_difference_first() {
    let (sim, _) = a_redacted_phone_session_then_a_ride();
    let replayed = Sim::replay(&with_the_last_frame_unseen(sim.capture()));
    let report = replayed.device.report().unwrap();
    let first = report.first.as_ref().expect("the altered frame diverges");
    assert_eq!(first.kind, shell_simulator::replay::Kind::Seen, "{report}");
    assert!(
        first.detail.contains("recorded Panel(3735928559)"),
        "{report}"
    );
}

fn a_ride_whose_journal_write_tears(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.fail_storage(0, 10);
    sim.advance(seconds(30));
}

#[test]
fn a_storage_failure_mid_ride_replays_exactly() {
    use firmware_services::trace::{Medium, StorageOp};
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_journal_write_tears,
        false,
    );
    let torn = count_world(&capture, |world| {
        matches!(
            world,
            World::Media(StorageOp::Write, access)
                if access.span.medium == Medium::Data && access.fault.is_some()
        )
    });
    assert_eq!(torn, 1, "one torn write");
}

fn a_ride_whose_card_fails(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.fail_bulk(0, 100);
    sim.advance(minutes(1));
    sim.stop_ride();
}

#[test]
fn a_bulk_failure_mid_ride_replays_exactly() {
    use firmware_services::trace::{Medium, StorageOp};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_fails, false);
    let torn = count_world(&capture, |world| {
        matches!(
            world,
            World::Media(StorageOp::Write, access)
                if access.span.medium == Medium::Bulk && access.fault.is_some()
        )
    });
    assert_eq!(torn, 1, "the torn write");
}

fn a_ride_whose_card_goes_missing(sim: &mut Sim) {
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.fit_bulk(Presence::Missing);
    sim.advance(minutes(1));
    sim.stop_ride();
}

#[test]
fn a_card_that_goes_missing_mid_ride_replays_exactly() {
    use firmware_services::trace::{Medium, StorageError};
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let capture = assert_replays_exactly(equip(sim.boot()), a_ride_whose_card_goes_missing, false);
    let missing = count_world(&capture, |world| match world {
        World::Media(_, access) => {
            access.span.medium == Medium::Bulk && access.fault == Some(StorageError::Unavailable)
        }
        World::NoCard(error) => *error == StorageError::Unavailable,
        _ => false,
    });
    assert!(missing > 0, "the card's absence is recorded");
}

fn a_ride_whose_panel_and_backlight_refuse(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(30));
    sim.device.display.0.borrow_mut().fail_next = true;
    sim.device.power.0.borrow_mut().fail = true;
    let (status, out) = sim.command("BRIGHTNESS 40");
    assert_eq!(status, "OK", "{out}");
    sim.advance(seconds(5));
    sim.device.power.0.borrow_mut().fail = false;
    sim.device.power.0.borrow_mut().available = device_api::observation::Availability::Failed;
    sim.advance(seconds(5));
    sim.device.power.0.borrow_mut().available = device_api::observation::Availability::Ready;
    sim.advance(seconds(30));
    sim.stop_ride();
}

#[test]
fn a_panel_and_backlight_that_refuse_replay_exactly() {
    use shell_simulator::trace::world::Output;
    let capture = assert_replays_exactly(
        equip(hashed().boot()),
        a_ride_whose_panel_and_backlight_refuse,
        false,
    );
    for output in [Output::Panel, Output::Backlight] {
        let refused = count_world(
            &capture,
            |world| matches!(world, World::Refused { output: refused, .. } if *refused == output),
        );
        assert!(refused > 0, "{output:?} refused");
    }
    let changes = count_world(&capture, |world| matches!(world, World::Backlight(_)));
    assert_eq!(changes, 3, "ready, failed, ready again");
}

fn a_ride_then_turned_off(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    sim.done();
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "turned off");
    sim.advance(seconds(2));
}

#[test]
fn a_ride_ended_by_turning_off_replays_exactly() {
    use firmware_services::trace::Event;
    let capture = assert_replays_exactly(equip(hashed().boot()), a_ride_then_turned_off, false);
    let asked = count_world(&capture, |world| {
        matches!(world, World::PowerRequest { result: Ok(()), .. })
    });
    let off = count_events(&capture, |event| matches!(event, Event::PoweredOff));
    assert_eq!(
        (asked, off),
        (1, 1),
        "one shutdown asked, then the power went"
    );
}

#[test]
fn a_session_turned_off_is_in_its_file_up_to_the_shutdown_request() {
    use firmware_services::trace::{Event, decode};
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.advance(seconds(1));
    let name = sim
        .device
        .trace_file()
        .expect("the session records into STORE");
    a_ride_then_turned_off(&mut sim);
    let passes = |trace: &[u8]| {
        let (_, events) = decode(trace).unwrap();
        let (mut passes, mut asked) = (Vec::new(), None);
        for event in events {
            match event {
                Event::Pass { now_ms } => passes.push(now_ms),
                Event::PoweredOff => asked = asked.or(passes.last().copied()),
                Event::Call(call)
                    if matches!(world::read(&call), Some(World::PowerRequest { .. })) =>
                {
                    asked = asked.or(passes.last().copied())
                }
                _ => {}
            }
        }
        (passes, asked)
    };
    let (recorded, asked) = passes(&sim.capture());
    let asked = asked.expect("VANA asked to turn off");
    let pulled = sim.store_file(&name).expect("every record reads back");
    let (filed, _) = passes(&pulled);
    let before = recorded.iter().position(|&at| at == asked).unwrap();
    assert_eq!(
        filed,
        recorded[..before],
        "every pass before the one that asked"
    );
    let capture = pulled;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

fn a_short_ride_with_every_sensor(sim: &mut Sim) {
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(Some(80));
    sim.select_ble_heart_strap();
    sim.sync_network_time(WIFI_UTC_MS);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(40));
    sim.press(Role::Act);
    sim.advance(seconds(40));
    sim.stop_ride();
}

#[test]
fn every_sensor_input_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_short_ride_with_every_sensor);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "Pass",
            "Command",
            "NetworkTime",
            "Panel",
            "InputSource::take_edge",
            "InputObservation::snapshot",
            "Ant::availability",
            "Ant::discoveries",
            "Ant::channels",
            "Ant::take_packet",
            "Ant::request",
            "Ble::snapshot",
            "Ble::take_packet",
            "Ble::take_battery",
            "Positioning::snapshot",
            "Sensors::startup_status",
            "Sensors::snapshot",
            "Sound::snapshot",
            "Sound::play",
            "PowerControl::status",
            "Power::availability",
            "Settings::load",
            "OwnedFlash::read",
            "OwnedFlash::program",
        ],
    );
    let marked = mutations::with_too_large(recording.trace);
    let caught = mutations::replay(&recording, marked);
    assert!(caught.report.is_some(), "{caught:?}");
}

fn a_power_meter_calibrated(sim: &mut Sim) {
    calibrate_power_meter(sim, ZeroReply::Offset(-37));
    sim.press(Role::Act);
    sim.wait_for("Zeroed", seconds(25)).unwrap();
}

#[test]
fn every_ant_send_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_power_meter_calibrated);
    mutations::assert_every_mutation_shows(&recording, &["Ant::send_status", "Ant::request"]);
}

fn a_course_from_the_phone(sim: &mut Sim) {
    use device_api::server::Method;
    sim.init_bulk();
    sim.connect_phone();
    let course = gpx_through("Cerro loop", &COURSE_LOOP);
    let (status, body) = sim.phone_request(Method::Put, "/api/course", course.as_bytes());
    assert_eq!(status, 200, "{body}");
    sim.disconnect_phone();
    sim.advance(seconds(2));
}

#[test]
fn every_phone_input_mutated_shows_in_its_replay() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let recording = mutations::record(equip(sim.boot()), a_course_from_the_phone);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "AccessPoint::state",
            "AccessPoint::start",
            "AccessPoint::entropy",
            "Server::request",
            "Server::read",
            "Server::write",
            "Server::finish",
            "ReadWrite::entropy",
            "ReadWrite::owned_read",
            "ReadWrite::owned_write",
        ],
    );
}

fn a_ride_with_faults(sim: &mut Sim) {
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.device.display.0.borrow_mut().fail_next = true;
    sim.device.power.0.borrow_mut().fail = true;
    let (status, out) = sim.command("BRIGHTNESS 40");
    assert_eq!(status, "OK", "{out}");
    sim.advance(seconds(2));
    sim.device.power.0.borrow_mut().fail = false;
    sim.advance(seconds(20));
    sim.fail_storage(0, 10);
    sim.advance(seconds(20));
}

#[test]
fn every_fault_input_mutated_shows_in_its_replay() {
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    let recording = mutations::record(equip(sim.boot()), a_ride_with_faults);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "Display::submit",
            "Power::brightness",
            "OwnedFlash::program",
        ],
    );
}

#[test]
fn every_card_fault_mutated_shows_in_its_replay() {
    for scenario in [a_ride_whose_card_fails, a_ride_whose_card_goes_missing] {
        let mut sim = hashed();
        sim.fit_bulk(Presence::Ready);
        let recording = mutations::record(equip(sim.boot()), scenario);
        mutations::assert_every_mutation_shows(
            &recording,
            &["ReadWrite::owned_read", "ReadWrite::owned_write"],
        );
    }
}

#[test]
fn every_power_input_mutated_shows_in_its_replay() {
    let recording = mutations::record(equip(hashed().boot()), a_ride_then_turned_off);
    mutations::assert_every_mutation_shows(
        &recording,
        &[
            "PowerControl::request",
            "PowerControl::status",
            "PoweredOff",
        ],
    );
}
