use super::*;
use device_api::ant::{Admission, Discovery, Identity, LinkState, OperationId, ScanState};
use device_api::gps::FixState;
use std::vec::Vec;

const HEADER: Header<'static> = Header {
    version: VERSION,
    build: [7; 20],
    boot: 42,
    profile: 1,
    config: b"prefs",
    media: b"rides@1234",
};

/// A recorder's loop: seal a full block and carry on in the next.
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

/// Every kind of event, timed around passes as a recorder emits them.
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
        Event::StorageFault {
            op: StorageOp::Write,
            sector: 1_955_841,
        },
        Event::Entropy(0x9e37_79b9_7f4a_7c15),
        Event::Page("ride"),
        Event::Toast("LAP 1 05:00"),
        Event::Sound(21),
        Event::RideState("recording"),
        Event::Shown("W=210 BPM=142 KM/H=28.0"),
        Event::Log {
            level: Level::Warn,
            target: "terrain",
            text: "no PSRAM for the tile cache",
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
fn a_torn_header_is_refused() {
    let bytes = encode(&sample(), 4096);
    let header = block(&bytes, 0).unwrap().1;
    assert!(decode(&bytes[..header - 1]).is_err());
}

#[test]
fn a_tag_this_version_does_not_know_is_skipped() {
    let mut payload = std::vec![0u8; 64];
    let mut w = Writer::new(&mut payload, 0);
    w.byte(SHORT_PASS | 10);
    w.record(120, |w| w.bytes(b"from a later version"));
    w.byte(SHORT_PASS | 10);
    let len = w.len;
    let mut bytes = std::vec![0u8; 256];
    let header = encode_header(&HEADER, &mut bytes).unwrap();
    bytes[header + 2..header + 2 + len].copy_from_slice(&payload[..len]);
    let end = header + seal(&mut bytes[header..], len).unwrap();
    let (got, finish) = decoded(&bytes[..end]);
    assert_eq!(
        got,
        [Event::Pass { now_ms: 10 }, Event::Pass { now_ms: 20 }]
    );
    assert_eq!(finish, Some(End::Complete));
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
fn a_pass_ten_milliseconds_on_is_one_byte() {
    let passes: Vec<_> = (1..=1000).map(|i| Event::Pass { now_ms: i * 10 }).collect();
    let bytes = encode(&passes, 4096);
    let header = block(&bytes, 0).unwrap().1;
    let opening = 2 + 4 + 1 + 1 + 1;
    assert_eq!(bytes.len() - header, 1000 + opening);
}

#[test]
fn a_record_larger_than_a_block_is_refused() {
    let mut encoder = Encoder::new(std::vec![0u8; 64]);
    let text = core::str::from_utf8(&[b'x'; 80]).unwrap();
    assert_eq!(encoder.push(&Event::Command(text)), Err(Full::Record));
    assert!(encoder.is_empty());
}
