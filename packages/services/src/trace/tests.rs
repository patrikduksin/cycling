use super::*;
use device_api::ant::{Admission, Discovery, Identity, LinkState, OperationId, ScanState};
use device_api::gps::FixState;
use std::string::{String, ToString};
use std::vec::Vec;

const HEADER: Header<'static> = Header {
    build: [7; 20],
    boot: 42,
    profile: 1,
    config: b"prefs",
    media: b"rides@1234",
};

fn encode(events: &[Event<'_>], block: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; block];
    let len = encode_header(&HEADER, &mut out).unwrap();
    out.truncate(len);
    let mut encoder = Encoder::new(std::vec![0u8; block]);
    for event in events {
        if encoder.push(event) == Err(Full::Block) {
            out.extend_from_slice(encoder.sealed());
            encoder.restart();
            encoder.push(event).unwrap();
        }
    }
    out.extend_from_slice(encoder.sealed());
    out
}

fn decoded(bytes: &[u8]) -> (Vec<Event<'_>>, Option<End>) {
    let (header, mut events) = decode(bytes).unwrap();
    assert_eq!(header, HEADER);
    let list = events.by_ref().collect();
    (list, events.end())
}

fn identity(device_type: u8) -> Identity {
    Identity {
        device_type,
        device_number: 4242,
        transmission_type: 1,
    }
}

fn gps() -> device_api::gps::Snapshot {
    device_api::gps::Snapshot {
        state: FixState::Fresh,
        latitude_e7: Some(-334_500_000),
        longitude_e7: Some(-706_600_000),
        satellites: Some(9),
        hdop_tenths: Some(8),
        speed_mm_s: Some(7_800),
        epoch_ms: Some(43_200_000),
        utc: Some([12, 0, 0, 1, 11, 26]),
        utc_ms: Some(1_793_534_400_000),
        age_ms: Some(120),
        bytes: 123_456,
        valid_sentences: 3_000,
        ..Default::default()
    }
}

fn sample() -> Vec<Event<'static>> {
    let mut ble = [0u8; 60];
    ble[..2].copy_from_slice(&[0, 142]);
    std::vec![
        Event::Pass { now_ms: 3_300 },
        Event::Startup(sensors::Startup::Ready),
        Event::Input(Edge {
            sequence: 1,
            received_ms: 3_295,
            input: Input::Button {
                button: Button::BottomRight,
                code: 1,
            },
        }),
        Event::Input(Edge {
            sequence: 2,
            received_ms: 3_300,
            input: Input::Touch(Point { x: 120, y: 300 }),
        }),
        Event::Input(Edge {
            sequence: 3,
            received_ms: 3_301,
            input: Input::Release,
        }),
        Event::Input(Edge {
            sequence: 4,
            received_ms: 3_301,
            input: Input::Cancel,
        }),
        Event::AntRadio(AntRadio {
            availability: Availability::Ready,
            capabilities: AntCapabilities {
                supported_types: &[11, 120, 121],
                connection_capacity: 10,
                one_peer_per_type: true,
                concurrent_scan: true,
                acknowledged_send: true,
                radio_delivery_feedback: false,
                burst: false,
                max_pending_sends: 1,
            },
            scanning: true,
            scan: ant::ScanSnapshot {
                state: ScanState::Active,
                generation: 3,
                discovery_overflows: 0,
            },
        }),
        Event::AntDiscoveries([
            Some(Discovery {
                identity: identity(11),
                rssi: -55,
                seen_ms: 3_000,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]),
        Event::Pass { now_ms: 3_310 },
        Event::AntLink {
            channel: 2,
            snapshot: Some(ant::Snapshot {
                scanning: false,
                link: LinkState::Connected,
                selected: Some(identity(120)),
                generation: 7,
                packets: 1_000,
                dropped_packets: 2,
                discovery_overflows: 0,
                transport_losses: 1,
                tx_failures: 0,
                age_ms: Some(240),
                stale: false,
            }),
        },
        Event::AntLink {
            channel: 3,
            snapshot: None,
        },
        Event::AntPacket(ant::Packet {
            identity: identity(11),
            data: [0x10, 1, 2, 3, 4, 5, 6, 7],
            received_ms: 3_312,
            generation: 7,
            loss_count: 0,
        }),
        Event::AntRequest {
            operation: ant::AntOperation::Scan(10_000),
            result: Ok(Admission::Accepted),
        },
        Event::AntRequest {
            operation: ant::AntOperation::Send {
                target: ant::Target {
                    identity: identity(11),
                    generation: 7,
                },
                data: [0x01, 0xaa, 0, 0, 0, 0, 0, 0],
            },
            result: Ok(Admission::SendQueued(OperationId(9))),
        },
        Event::AntRequest {
            operation: ant::AntOperation::Connect(identity(120)),
            result: Err(ant::Error::Busy),
        },
        Event::AntRequest {
            operation: ant::AntOperation::Disconnect(121),
            result: Err(ant::Error::Disconnected),
        },
        Event::AntRequest {
            operation: ant::AntOperation::StopScan,
            result: Ok(Admission::Accepted),
        },
        Event::AntSend(Some(ant::SendObservation {
            id: OperationId(9),
            target: ant::Target {
                identity: identity(11),
                generation: 7,
            },
            stage: ant::SendStage::BridgeReplied { accepted: true },
        })),
        Event::AntSend(None),
        Event::BleLink {
            availability: Availability::Ready,
            snapshot: ble_transport::Snapshot {
                link: ble_transport::Link::Connected,
                connections: 1,
                disconnections: 0,
                notifications: 60,
                dropped: 0,
                scan_reports: 3,
            },
        },
        Event::BlePacket(ble_transport::Packet {
            connection: 1,
            sequence: 60,
            dropped: 0,
            received_ms: 3_310,
            bytes: ble,
            length: 2,
        }),
        Event::BleBattery(ble_transport::BatteryLevel {
            connection: 1,
            raw: 87,
            received_ms: 3_000,
        }),
        Event::Position {
            availability: Availability::Ready,
            snapshot: Some(positioning::Snapshot {
                gps: gps(),
                transport: positioning::Transport::Receiving,
                sequence: 120,
                received_at_ms: Some(3_200),
                fault_at_ms: None,
            }),
        },
        Event::Physical(sensors::Snapshot {
            pressure: Observation::Fresh {
                value: sensors::Pressure {
                    pressure_centi_pa: 10_132_500,
                    temperature_centi_c: -150,
                },
                received_ms: 3_310,
            },
            motion: [
                Observation::Stale {
                    value: sensors::Motion { axes: [1, -2, 3] },
                    received_ms: 100,
                },
                Observation::Unavailable,
            ],
            identity: Observation::Unavailable,
            reports: 12,
            invalid_reports: 1,
            losses: 0,
        }),
        Event::Light(Observation::Unavailable),
        Event::Light(Observation::Fresh {
            value: sensors::Light(1_557),
            received_ms: 3_330,
        }),
        Event::Light(Observation::Stale {
            value: sensors::Light(65),
            received_ms: 3_330,
        }),
        Event::Power(Some(InputSnapshot {
            battery: Observation::Fresh {
                value: (80, 4020),
                received_ms: 3_000,
            },
            power: Observation::Fresh {
                value: 7,
                received_ms: 3_000,
            },
            button_counts: [1, 2, 3],
            touch_available: true,
            touch_errors: 0,
            companion_valid: 99,
            companion_bad_crc: 1,
            uart_errors: 0,
            input_lost: 0,
        })),
        Event::Power(None),
        Event::NetworkTime(crate::network_time::Snapshot {
            unix_seconds: Some(1_793_534_400),
            millis: 250,
            local_minutes: Some(720),
            age_ms: Some(5_000),
            status: crate::network_time::Status::Fresh,
        }),
        Event::Command("RIDE STATUS"),
        Event::StorageFault(StorageFault {
            medium: Medium::Bulk,
            attempt: 3,
            op: StorageOp::Write,
            at: 1_955_841,
            landed: 300,
            error: StorageError::Failed,
        }),
        Event::StorageFault(StorageFault {
            medium: Medium::Data,
            attempt: 0,
            op: StorageOp::Erase,
            at: 65_536,
            landed: 0,
            error: StorageError::Invalid,
        }),
        Event::AccessPoint {
            state: device_api::server::AccessPointState::Up { clients: 2 },
            reach: device_api::server::Reach {
                access_point: Some([192, 168, 4, 1]),
                station: None,
            },
        },
        Event::AccessPointRequest {
            start: true,
            result: Err(device_api::observation::Error::Unavailable),
        },
        Event::Server {
            request: Some(ServerRequest {
                id: 7,
                method: device_api::server::Method::Put,
                path: "/courses/alps.gpx",
                query: "t=1",
                session: "c0ffee",
                gzip: true,
                body: 48_000,
            }),
            room: 4096,
        },
        Event::Server {
            request: None,
            room: 0,
        },
        Event::ServerListen(true),
        Event::ServerRead {
            id: 7,
            part: b"<gpx><trk>",
            more: true,
        },
        Event::ServerRead {
            id: 7,
            part: b"",
            more: false,
        },
        Event::ServerWrite { id: 7, taken: 512 },
        Event::ServerFinish(7),
        Event::SoundState(device_api::sound::Snapshot {
            state: device_api::sound::State::Submitted,
            pattern: Some(10),
            requested_ms: 3_490,
            submitted_ms: Some(3_500),
            until_ms: 4_000,
        }),
        Event::SoundRequest {
            play: Some(10),
            result: Ok(()),
        },
        Event::SoundRequest {
            play: None,
            result: Err(device_api::observation::Error::Unavailable),
        },
        Event::PowerControl(device_api::power::Status {
            sequence: 3,
            operation: Some(device_api::power::Operation::Shutdown),
            state: device_api::power::State::Requested,
            at_ms: 3_400,
            prepare_at_ms: Some(3_450),
            failure: Some(device_api::power::Failure::Storage),
            ready: false,
        }),
        Event::PowerRequest {
            operation: device_api::power::Operation::Shutdown,
            delay_ms: 1_500,
            result: Ok(()),
        },
        Event::PoweredOff,
        Event::HostWrote,
        Event::Lost {
            blocks: 3,
            bytes: 12_000,
        },
        Event::Backlight(Availability::Failed),
        Event::Refused {
            output: Output::Panel,
            attempt: 2,
        },
        Event::Entropy(0x9e37_79b9_7f4a_7c15),
        Event::Page("ride"),
        Event::Toast("LAP 1 05:00"),
        Event::Sound {
            pattern: 21,
            cue: "radar_clear",
            note: 1,
            of: 2,
        },
        Event::Sound {
            pattern: 4,
            cue: "",
            note: 0,
            of: 0,
        },
        Event::RideState("recording"),
        Event::Mark {
            count: 2,
            pressed_ms: 2_400,
            page: "ride/main",
        },
        Event::Shown("W=210 BPM=142 KM/H=28.0"),
        Event::Panel(0x0bad_f00d),
        Event::Synthetic(Input::Button {
            button: Button::BottomLeft,
            code: 1,
        }),
        Event::Log {
            level: Level::Warn,
            target: "terrain",
            text: "no PSRAM for the tile cache",
        },
        Event::FrameTime(FrameTime {
            frames: 52,
            mean_us: 1_380,
        }),
        Event::Health(Health {
            simulated: false,
            heap_free: Some(61_234),
            heap_min: Some(48_000),
            psram_free: Some(1_204_000),
            frame_max_us: Some(41_000),
            pass_max_us: Some(180_000),
            gps_sentences: 12_000,
            gps_overflows: 2,
            gps_uart_errors: 1,
            companion_errors: 7,
            card_write_max_us: Some(9_400),
            card_recoveries: Some(1),
            spool_used: 4_096,
            spool_size: 262_144,
            wifi: Some(device_api::connectivity::WifiState::Verified),
            wifi_rssi: Some(-67),
            ble_rssi: None,
        }),
        Event::Health(Health {
            simulated: true,
            ..Health::default()
        }),
        Event::Activity(Activity {
            simulated: false,
            window_ms: 5_004,
            wakes: 1_210,
            awake_us: Some(212_000),
            main_us: Some(230_000),
            radio_us: Some(41_000),
            idle_us: Some(4_700_000),
            switches: Some(2_400),
            ticks: Some(1_300),
            hooks_us: Some(900),
            consumers: Consumers {
                cpu_mhz: Some(240),
                backlight: 0,
                frames: 3,
                gnss: Gnss::Paused,
                wifi: None,
                access_point: AccessPointState::Up { clients: 1 },
                ble: device_api::ble_transport::Link::Connected,
                ant: Availability::Ready,
                ant_scanning: true,
                ant_channels: 2,
                slept_ms: 0,
            },
        }),
        Event::Task {
            name: "console",
            polls: 1_000,
            busy_us: Some(150_000),
        },
        Event::Task {
            name: "console",
            polls: 500,
            busy_us: None,
        },
        Event::Stage {
            name: "vana.tick",
            runs: 1_000,
            busy_us: 90_000,
            max_us: 4_000,
        },
        Event::Pass { now_ms: 3_500 },
        Event::Pass { now_ms: 3_500 },
        Event::Pass { now_ms: 900_000 },
    ]
}

#[test]
fn every_event_decodes_to_what_was_recorded() {
    let events = sample();
    for block in [128, 4096] {
        let bytes = encode(&events, block);
        assert_eq!(decoded(&bytes), (events.clone(), Some(End::Complete)));
    }
}

#[test]
fn a_cut_or_damaged_tail_drops_only_the_last_block() {
    let events = sample();
    let bytes = encode(&events, 128);
    let header = block(&bytes, 0).unwrap().1;
    let mut boundaries = std::vec![header];
    while let Some((_, next)) = block(&bytes, *boundaries.last().unwrap()) {
        boundaries.push(next);
    }
    assert!(boundaries.len() > 4, "the sample spans several blocks");
    for cut in header..bytes.len() {
        let (got, end) = decoded(&bytes[..cut]);
        assert!(events.starts_with(&got), "cut at {cut}");
        let whole = boundaries.contains(&cut);
        assert_eq!(end, Some(if whole { End::Complete } else { End::Torn }));
    }
    let mut damaged = bytes.clone();
    let last = boundaries[boundaries.len() - 2];
    damaged[last + 3] ^= 0x40;
    let (got, end) = decoded(&damaged);
    assert_eq!(end, Some(End::Torn));
    assert_eq!(got, decoded(&bytes[..last]).0);
}

#[test]
fn a_damaged_block_with_blocks_after_it_ends_corrupt() {
    let events = sample();
    let bytes = encode(&events, 128);
    let header = block(&bytes, 0).unwrap().1;
    let second = block(&bytes, header).unwrap().1;
    let mut damaged = bytes.clone();
    damaged[second + 3] ^= 0x40;
    let (got, end) = decoded(&damaged);
    assert_eq!(end, Some(End::Corrupt));
    assert_eq!(got, decoded(&bytes[..second]).0);
    let mut missing = bytes[..second + 10].to_vec();
    missing.extend_from_slice(&bytes[second + 10 + 40..]);
    assert_eq!(
        decoded(&missing).1,
        Some(End::Corrupt),
        "bytes lost mid-trace"
    );
}

#[test]
fn a_torn_header_is_refused() {
    let bytes = encode(&sample(), 4096);
    let header = block(&bytes, 0).unwrap().1;
    assert!(decode(&bytes[..header - 1]).is_err());
}

#[test]
fn a_record_with_an_unknown_tag_ends_the_trace_as_corrupt() {
    let mut payload = std::vec![0u8; 64];
    let mut w = Writer::new(&mut payload, 0);
    w.byte(SHORT_PASS | 10);
    w.record(120, |w| w.bytes(b"no such record"));
    w.byte(SHORT_PASS | 10);
    let len = w.len;
    let mut bytes = std::vec![0u8; 256];
    let header = encode_header(&HEADER, &mut bytes).unwrap();
    bytes[header + 2..header + 2 + len].copy_from_slice(&payload[..len]);
    let end = header + seal(&mut bytes[header..], len).unwrap();
    let (got, finish) = decoded(&bytes[..end]);
    assert_eq!(got, [Event::Pass { now_ms: 10 }]);
    assert_eq!(finish, Some(End::Corrupt));
}

#[test]
fn decoding_resumes_from_a_cursor() {
    let events = sample();
    let bytes = encode(&events, 128);
    let (_, mut first) = decode(&bytes).unwrap();
    let head: Vec<_> = first.by_ref().take(10).collect();
    let rest: Vec<_> = Events::resume(&bytes, first.cursor()).collect();
    assert_eq!([head, rest].concat(), events);
}

#[test]
fn a_reader_resumes_over_a_trace_that_grew() {
    let events = sample();
    let mut out = std::vec![0u8; 128];
    let len = encode_header(&HEADER, &mut out).unwrap();
    let mut written = out[..len].to_vec();
    let mut encoder = Encoder::new(std::vec![0u8; 128]);
    let (_, first) = decode(&written).unwrap();
    let mut cursor = first.cursor();
    let mut read = Vec::new();
    let read_on = |bytes: &[u8], cursor: &mut Cursor, read: &mut Vec<String>| {
        let mut events = Events::resume(bytes, *cursor);
        read.extend(events.by_ref().map(|event| std::format!("{event:?}")));
        assert_eq!(events.end(), Some(End::Complete));
        *cursor = events.cursor();
    };
    for event in &events {
        if encoder.push(event) == Err(Full::Block) {
            written.extend_from_slice(encoder.sealed());
            encoder.restart();
            encoder.push(event).unwrap();
        }
        let mut now = written.clone();
        now.extend_from_slice(encoder.sealed());
        read_on(&now, &mut cursor, &mut read);
    }
    let expected: Vec<_> = events.iter().map(|e| std::format!("{e:?}")).collect();
    assert_eq!(read, expected);
}

#[test]
fn a_cursor_past_the_bytes_ends_torn_without_panicking() {
    let bytes = encode(&sample(), 128);
    let (_, mut events) = decode(&bytes).unwrap();
    events.by_ref().take(40).for_each(drop);
    let cursor = events.cursor();
    let header = block(&bytes, 0).unwrap().1;
    for cut in header..bytes.len() {
        let mut shorter = Events::resume(&bytes[..cut], cursor);
        shorter.by_ref().for_each(drop);
        assert!(shorter.end().is_some(), "cut at {cut}");
    }
}

#[test]
fn a_window_of_known_frames_has_their_mean() {
    let times_us = [1_000u32, 2_000, 6_000];
    let total_us = times_us.iter().map(|&us| u64::from(us)).sum();
    assert_eq!(
        FrameTime::of_window(3, total_us),
        Some(FrameTime {
            frames: 3,
            mean_us: 3_000
        })
    );
    assert_eq!(FrameTime::of_window(0, 0), None);
}

#[test]
fn bytes_past_the_known_fields_are_refused() {
    let mut bytes = std::vec![0u8; 512];
    let mut w = Writer::new(&mut bytes[2..], 0);
    HEADER.put(&mut w);
    w.bytes(b"an extra header field");
    let len = w.len;
    let header_end = seal(&mut bytes, len).unwrap();
    assert_eq!(decode(&bytes[..header_end]).err(), Some(Error::Header));

    let header_end = encode_header(&HEADER, &mut bytes).unwrap();
    let mut w = Writer::new(&mut bytes[header_end + 2..], 0);
    w.byte(SHORT_PASS | 10);
    w.record(tag::ENTROPY, |w| {
        w.varint(7);
        w.bytes(b"an extra field");
    });
    let len = w.len;
    let end = header_end + seal(&mut bytes[header_end..], len).unwrap();
    let (got, finish) = decoded(&bytes[..end]);
    assert_eq!(got, [Event::Pass { now_ms: 10 }]);
    assert_eq!(finish, Some(End::Corrupt));
}

#[test]
fn a_trace_of_another_version_is_refused_as_foreign() {
    for version in [0, VERSION + 1] {
        let mut bytes = std::vec![0u8; 128];
        let len = encode_header(&HEADER, &mut bytes).unwrap();
        bytes[2] = version;
        let len = seal(&mut bytes, len - FRAME).unwrap();
        assert_eq!(decode(&bytes[..len]).err(), Some(Error::Version(version)));
    }
    let mut bytes = std::vec![0u8; 128];
    bytes[2..6].copy_from_slice(&[2, 0, 2, 0]);
    let len = seal(&mut bytes, 4).unwrap();
    let error = decode(&bytes[..len]).err().unwrap();
    assert_eq!(error, Error::Version(2));
    assert_eq!(
        error.to_string(),
        std::format!(
            "trace format 2 is not this build's ({VERSION}): read it with the build that recorded it"
        )
    );
}

#[test]
fn a_ble_packet_keeps_a_length_past_its_buffer() {
    let packet = device_api::ble_transport::Packet {
        connection: 1,
        sequence: 2,
        dropped: 0,
        received_ms: 5,
        bytes: [9; 60],
        length: 61,
    };
    let events = [Event::Pass { now_ms: 5 }, Event::BlePacket(packet)];
    assert_eq!(decoded(&encode(&events, 4096)).0, events);
}

#[test]
fn a_pass_clock_near_its_end_wraps_instead_of_panicking() {
    let events = [
        Event::Pass {
            now_ms: u64::MAX - 5,
        },
        Event::Pass { now_ms: 3 },
    ];
    assert_eq!(decoded(&encode(&events, 4096)).0, events);
}

#[test]
fn a_pass_ten_milliseconds_on_is_one_byte() {
    let passes: Vec<_> = (1..=1000).map(|i| Event::Pass { now_ms: i * 10 }).collect();
    let bytes = encode(&passes, 4096);
    let header = block(&bytes, 0).unwrap().1;
    let opening = 2 + 4 + 1 + 1 + 1;
    assert_eq!(bytes.len() - header, 1000 + opening);
}

#[test]
fn an_ambient_light_reading_costs_a_few_bytes() {
    let readings: Vec<_> = (1..=300u64)
        .flat_map(|i| {
            [
                Event::Pass { now_ms: i * 333 },
                Event::Light(Observation::Fresh {
                    value: sensors::Light(1_000 + (i % 40) as u16),
                    received_ms: i * 333,
                }),
            ]
        })
        .collect();
    let bytes = encode(&readings, 4096);
    let header = block(&bytes, 0).unwrap().1;
    let per_reading = (bytes.len() - header) as f64 / 300.0;
    assert!(
        per_reading < 12.0,
        "{per_reading} bytes a reading with its pass"
    );
}

#[test]
fn a_record_larger_than_a_block_is_refused() {
    let mut encoder = Encoder::new(std::vec![0u8; 64]);
    let text = core::str::from_utf8(&[b'x'; 80]).unwrap();
    assert_eq!(encoder.push(&Event::Command(text)), Err(Full::Record));
    assert!(encoder.is_empty());
}

#[test]
fn vitals_are_due_every_five_seconds_and_every_minute_in_dark_standby() {
    let mut vitals = Vitals::default();
    let due = |vitals: &mut Vitals, from: u64, to: u64, quiet: bool| -> Vec<u64> {
        (from..to)
            .step_by(10)
            .filter(|&now| vitals.due(now, quiet))
            .collect()
    };
    assert_eq!(due(&mut vitals, 0, 15_001, false), [5_000, 10_000, 15_000]);
    assert_eq!(due(&mut vitals, 15_010, 140_000, true), [75_000, 135_000]);
    assert_eq!(due(&mut vitals, 140_000, 141_000, false), [140_000]);
}

#[test]
fn records_keep_their_fields() {
    let fields: Vec<String> = RECORD_FIELDS
        .iter()
        .map(|(record, fields)| std::format!("{record}: {}", fields.join(" ")))
        .collect();
    let pinned = "\
Point: x y
Edge: sequence received_ms input
ant::Identity: device_type device_number transmission_type
ant::Packet: identity data received_ms generation loss_count
ant::Snapshot: scanning link selected generation packets dropped_packets discovery_overflows transport_losses tx_failures age_ms stale
ant::Discovery: identity rssi seen_ms
ant::ScanSnapshot: state generation discovery_overflows
ant::Target: identity generation
ant::SendObservation: id target stage
ble_transport::Snapshot: link connections disconnections notifications dropped scan_reports
ble_transport::BatteryLevel: connection raw received_ms
device_api::gps::Snapshot: state identity latitude_e7 longitude_e7 satellites hdop_tenths speed_mm_s epoch_ms utc utc_ms age_ms bytes valid_sentences checksum_errors parse_errors overflows line_overflows long_proprietary uart_errors binary_bytes non_nmea_frames
positioning::Snapshot: gps transport sequence received_at_ms fault_at_ms
sensors::Pressure: pressure_centi_pa temperature_centi_c
sensors::Motion: axes
sensors::Identity: fields
sensors::Snapshot: pressure motion identity reports invalid_reports losses
InputSnapshot: battery power button_counts touch_available touch_errors companion_valid companion_bad_crc uart_errors input_lost
crate::network_time::Snapshot: unix_seconds millis local_minutes age_ms status
AntRadio<'a>: availability capabilities scanning scan
ServerRequest<'a>: id method path query session gzip body
Reach: access_point station
sound::Snapshot: state pattern requested_ms submitted_ms until_ms
power::Status: sequence operation state at_ms prepare_at_ms failure ready
StorageFault: medium attempt op at landed error
AntCapabilities<'a>: supported_types connection_capacity one_peer_per_type concurrent_scan acknowledged_send radio_delivery_feedback burst max_pending_sends
FrameTime: frames mean_us
Health: simulated heap_free heap_min psram_free frame_max_us pass_max_us gps_sentences gps_overflows gps_uart_errors companion_errors card_write_max_us card_recoveries spool_used spool_size wifi wifi_rssi ble_rssi
Consumers: cpu_mhz backlight frames gnss wifi access_point ble ant ant_scanning ant_channels slept_ms
Activity: simulated window_ms wakes awake_us main_us radio_us idle_us switches ticks hooks_us consumers";
    assert_eq!(
        fields.join("\n"),
        pinned,
        "a record grew or changed: give new fields a new tag"
    );
}
