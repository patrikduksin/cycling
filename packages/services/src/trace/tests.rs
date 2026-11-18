use super::*;
use std::string::{String, ToString};
use std::vec::Vec;

const HEADER: Header<'static> = Header {
    build: [7; 20],
    device: "c606",
    boot: 42,
    terrain: origin::Found::Absent,
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

fn sample() -> Vec<Event<'static>> {
    let mut ble = [0u8; 60];
    ble[..2].copy_from_slice(&[0, 142]);
    std::vec![
        Event::Pass { now_ms: 3_300 },
        Event::Pass { now_ms: 3_310 },
        Event::NetworkTime(crate::network_time::Snapshot {
            unix_seconds: Some(1_793_534_400),
            millis: 250,
            local_minutes: Some(720),
            age_ms: Some(5_000),
            status: crate::network_time::Status::Fresh,
        }),
        Event::Command("RIDE STATUS"),
        Event::PoweredOff,
        Event::Lost {
            blocks: 3,
            bytes: 12_000,
        },
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
        Event::Synthetic(Input::Button { id: 1, code: 1 }),
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
fn the_header_names_the_device_that_recorded_it() {
    let bytes = encode(&sample(), 4096);
    assert_eq!(decode(&bytes).unwrap().0.device, "c606");
    for device in ["", "C606", "6c06", "c606/../x", "a-name-past-sixteen"] {
        let mut bytes = std::vec![0u8; 128];
        let len = encode_header(&Header { device, ..HEADER }, &mut bytes).unwrap();
        assert_eq!(
            decode(&bytes[..len]).err(),
            Some(Error::Header),
            "{device:?}"
        );
    }
}

#[test]
fn a_record_with_an_unknown_tag_ends_the_trace_as_corrupt() {
    let mut payload = std::vec![0u8; 64];
    let mut w = Writer::new(&mut payload, 0);
    w.byte(SHORT_PASS | 10);
    w.byte(0);
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
    w.byte(0);
    w.record(tag::FORGET, |w| {
        Medium::Bulk.put(w);
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
    let mut bytes = [0u8; 128];
    let mut w = Writer::new(&mut bytes, 5);
    packet.put(&mut w);
    let len = w.len;
    let mut r = Reader::new(&bytes[..len], 5);
    assert_eq!(device_api::ble_transport::Packet::take(&mut r), packet);
    assert_eq!(r.read(), Some(len));
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
    let opening = 2 + 4 + 1 + 1 + 1 + 1;
    assert_eq!(bytes.len() - header, 1000 + opening);
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
fn committed_traces_reencode_byte_for_byte() {
    let traces: [(&str, &[u8]); 2] = [
        (
            "laps-and-a-menu",
            include_bytes!("../../../../tools/simulator/tests/traces/laps-and-a-menu.trace"),
        ),
        (
            "power-dropout-after-descent",
            include_bytes!(
                "../../../../tools/simulator/tests/traces/power-dropout-after-descent.trace"
            ),
        ),
    ];
    for (name, bytes) in traces {
        let (header, mut all) = decode(bytes).unwrap();
        assert!(all.by_ref().count() > 1000, "{name} has events");
        assert_eq!(all.end(), Some(End::Complete), "{name} ends whole");
        let mut out = std::vec![0u8; MAX_BLOCK];
        let len = encode_header(&header, &mut out).unwrap();
        out.truncate(len);
        let (_, mut start) = block(bytes, 0).unwrap();
        while start < bytes.len() {
            let mut events = Events::resume(
                bytes,
                Cursor {
                    block: start,
                    at: 0,
                    now: 0,
                    end: None,
                    checked: None,
                },
            );
            assert_eq!(
                events.step(),
                Ok(None),
                "{name}: a block opens with its clock"
            );
            let mut encoder = Encoder::new(std::vec![0u8; MAX_BLOCK]);
            encoder.now = events.cursor.now;
            encoder.restart();
            loop {
                match events.step() {
                    Ok(Some(event)) => encoder.push(&event).unwrap(),
                    Ok(None) | Err(End::Complete) => break,
                    Err(end) => panic!("{name}: block at {start} ends {}", end.name()),
                }
            }
            out.extend_from_slice(encoder.sealed());
            start = block(bytes, start).unwrap().1;
        }
        assert!(out == bytes, "{name} re-encodes to other bytes");
    }
}
