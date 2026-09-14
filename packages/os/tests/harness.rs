#![cfg(all(feature = "simulator", feature = "debug-harness"))]
use cycling_os::{
    capabilities::*,
    harness::{self, Operation},
    shell::Shell,
    simulator::{DisplayDevice, InputDevice, Memory, PowerDevice},
    terminal_protocol::{self, Command},
};
use std::{cell::Cell, fmt::Write};
type TestShell = Shell<DisplayDevice, InputDevice, PowerDevice, Memory>;
fn shell(w: usize, h: usize) -> (TestShell, DisplayDevice, InputDevice) {
    let d = DisplayDevice::new(w, h);
    let i = InputDevice::new(&[Button::TopLeft], Availability::Ready);
    let mut s = Shell::new(
        d.clone(),
        i.clone(),
        PowerDevice::new(Availability::Ready),
        Memory::default(),
        0,
    )
    .unwrap();
    s.harness = harness::State::new(7);
    s.harness.buffer = Some(harness::CaptureBuffer::Owned(
        vec![0; harness::FRAMES * w * h * 2].into_boxed_slice(),
    ));
    (s, d, i)
}
fn call(s: &mut TestShell, session: u64, op: Operation, now: u64) -> (&'static str, String) {
    let mut out = String::new();
    let status = s.harness_command(
        harness::Command {
            session,
            operation: op,
        },
        now,
        &mut out,
    );
    (status, out)
}
fn open(s: &mut TestShell) -> u64 {
    assert_eq!(
        call(
            s,
            0,
            Operation::Open {
                nonce: 9,
                lease: 1000
            },
            0
        )
        .0,
        "OK"
    );
    (7u64 << 32) | 1
}
fn event(s: &mut TestShell, session: u64, offset: u32, event: harness::Event) {
    assert_eq!(
        call(
            s,
            session,
            Operation::InputAdd {
                sequence: 1,
                offset,
                event
            },
            0
        )
        .0,
        "ACCEPTED"
    );
}
#[test]
fn production_parser_bounds_and_typed_forms() {
    assert!(matches!(
        terminal_protocol::parse(b"CMD 42 HARNESS 99 INPUT ADD 1 20 DOWN 239 319")
            .unwrap()
            .command,
        Command::Harness(harness::Command {
            session: 99,
            operation: Operation::InputAdd { offset: 20, .. }
        })
    ));
    for line in [
        "CMD 1 HARNESS CAPS extra",
        "CMD 2 HARNESS 1 INPUT ADD 1 0 BUTTON 0 extra",
        "CMD 3 HARNESS 1 INPUT ADD 1 0 LONG 0",
        "CMD 4 HARNESS OPEN 1 -1",
    ] {
        assert!(terminal_protocol::parse(line.as_bytes()).is_err());
    }
    let mut lines = terminal_protocol::Lines::default();
    for _ in 0..terminal_protocol::MAX_LINE + 1 {
        assert!(lines.push(b'x').is_none());
    }
    assert_eq!(
        lines.push(b'\n'),
        Some(Err(terminal_protocol::Error::Overlong))
    );
    for &b in b"CMD 9 HARNESS CAPS" {
        assert!(lines.push(b).is_none());
    }
    assert!(lines.push(b'\n').unwrap().is_ok());
}
#[test]
fn timed_consumer_wake_physical_counters_and_expiry_cancel() {
    let (mut s, _, physical) = shell(32, 24);
    let session = open(&mut s);
    s.settings.dim_timeout_secs = 1;
    s.tick(1000);
    assert!(s.dimmed());
    // Open a fresh session after the lease expired at the idle deadline.
    let session = if call(&mut s, session, Operation::Ping, 1000).0 == "STALE_SESSION" {
        assert_eq!(
            call(
                &mut s,
                0,
                Operation::Open {
                    nonce: 2,
                    lease: 1000
                },
                1000
            )
            .0,
            "OK"
        );
        session + 1
    } else {
        session
    };
    assert_eq!(
        call(&mut s, session, Operation::InputBegin(1), 1000).0,
        "OK"
    );
    // Queue using a consistent current device time.
    for (offset, event) in [
        (0, harness::Event::Down(31, 23)),
        (2000, harness::Event::Up),
    ] {
        assert_eq!(
            call(
                &mut s,
                session,
                Operation::InputAdd {
                    sequence: 1,
                    offset,
                    event
                },
                1000
            )
            .0,
            "ACCEPTED"
        );
    }
    assert_eq!(
        call(&mut s, session, Operation::InputRun(1), 1000).0,
        "ACCEPTED"
    );
    s.tick(1000);
    assert_eq!(s.synthetic_events, 1);
    assert_eq!(s.input_events, 0);
    assert_eq!(s.routed_events, 0);
    s.tick(2000);
    assert_eq!(s.synthetic_events, 2); // Expiry emits a cancellation through the consumer.
    physical.push(
        2001,
        Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
    );
    s.tick(2001);
    assert_eq!(s.input_events, 1);
    assert_eq!(s.routed_events, 1);
    assert_eq!(
        call(&mut s, session, Operation::Ping, 2001).0,
        "STALE_SESSION"
    );
}
#[test]
fn rejects_invalid_gestures_duplicate_execution_and_cancels_on_live_loss() {
    let (mut s, _, physical) = shell(32, 24);
    let id = open(&mut s);
    call(&mut s, id, Operation::InputBegin(1), 0);
    assert_eq!(
        call(
            &mut s,
            id,
            Operation::InputAdd {
                sequence: 1,
                offset: 0,
                event: harness::Event::Down(32, 23)
            },
            0
        )
        .0,
        "INVALID"
    );
    assert_eq!(
        call(
            &mut s,
            id,
            Operation::InputAdd {
                sequence: 1,
                offset: 0,
                event: harness::Event::Move(1, 1)
            },
            0
        )
        .0,
        "INVALID"
    );
    event(&mut s, id, 0, harness::Event::Down(0, 0));
    assert_eq!(call(&mut s, id, Operation::InputRun(1), 0).0, "INVALID");
    event(&mut s, id, 500, harness::Event::Up);
    assert_eq!(call(&mut s, id, Operation::InputRun(1), 0).0, "ACCEPTED");
    assert_eq!(
        call(&mut s, id, Operation::InputRun(1), 0).0,
        "STALE_OPERATION"
    );
    s.tick(0);
    physical.push(20, Input::Cancel);
    s.tick(20);
    let (_, status) = call(&mut s, id, Operation::InputStatus, 20);
    assert!(status.contains("state=physical_loss"));
    assert!(status.contains("held=false"));
    assert!(status.contains("delivered=1"));
}
#[test]
fn bounded_queue_overflow_and_session_replacement_reject_old_work() {
    let (mut s, _, _) = shell(16, 16);
    let id = open(&mut s);
    call(&mut s, id, Operation::InputBegin(1), 0);
    for n in 0..harness::EVENTS {
        event(&mut s, id, n as u32, harness::Event::Button(0));
    }
    assert_eq!(
        call(
            &mut s,
            id,
            Operation::InputAdd {
                sequence: 1,
                offset: 100,
                event: harness::Event::Button(0)
            },
            0
        )
        .0,
        "OVERFLOW"
    );
    s.tick(500);
    assert_eq!(s.synthetic_events, 0);
    assert!(
        call(&mut s, id, Operation::InputStatus, 500)
            .1
            .contains("state=overflow")
    );
    call(&mut s, id, Operation::Close, 500);
    assert_eq!(
        call(
            &mut s,
            0,
            Operation::Open {
                nonce: 10,
                lease: 1000
            },
            500
        )
        .0,
        "OK"
    );
    assert_eq!(call(&mut s, id, Operation::Ping, 500).0, "STALE_SESSION");
    assert_eq!(call(&mut s, id + 1, Operation::InputBegin(1), 500).0, "OK");
}
#[test]
fn captures_exact_once_submitted_pixels_crc_geometry_and_failure() {
    for (w, h) in [(240, 320), (32, 24)] {
        let (mut s, d, _) = shell(w, h);
        let id = open(&mut s);
        assert_eq!(
            call(
                &mut s,
                id,
                Operation::CaptureStart {
                    count: 1,
                    interval: 100
                },
                0
            )
            .0,
            "ACCEPTED"
        );
        let calls = Cell::new(0usize);
        s.draw_pixels(|x, y| {
            calls.set(calls.get() + 1);
            ((x * 17 + y * 29) as u16) ^ 0xa55a
        });
        assert_eq!(calls.get(), w * h);
        let expected: Vec<u8> =
            d.0.borrow()
                .pixels
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
        assert_eq!(
            &s.harness.buffer.as_ref().unwrap()[..expected.len()],
            expected.as_slice()
        );
        let meta = call(&mut s, id, Operation::CaptureMeta(1), 0).1;
        assert!(meta.contains("complete=true submitted=true physical=false"));
        assert!(meta.contains(&format!("crc32={:08x}", harness::crc32(&expected))));
        let chunk = call(
            &mut s,
            id,
            Operation::CaptureRead {
                frame: 1,
                offset: 0,
                length: 256,
            },
            0,
        );
        let mut hex = String::new();
        for b in &expected[..256] {
            write!(hex, "{b:02x}").unwrap();
        }
        assert!(chunk.1.contains(&format!("hex={hex}")));
        assert_eq!(
            call(
                &mut s,
                id,
                Operation::CaptureRead {
                    frame: 1,
                    offset: 0,
                    length: 257
                },
                0
            )
            .0,
            "INVALID"
        );
        call(
            &mut s,
            id,
            Operation::CaptureStart {
                count: 1,
                interval: 100,
            },
            0,
        );
        d.0.borrow_mut().fail_next = true;
        s.draw_pixels(|_, _| 0xffff);
        let meta = call(&mut s, id, Operation::CaptureMeta(2), 0).1;
        assert!(meta.contains("complete=false submitted=false"));
        assert_eq!(
            call(
                &mut s,
                id,
                Operation::CaptureRead {
                    frame: 2,
                    offset: 0,
                    length: 8
                },
                0
            )
            .0,
            "INCOMPLETE"
        );
        assert_eq!(
            call(&mut s, id, Operation::CaptureMeta(1), 0).0,
            "STALE_FRAME"
        );
    }
    assert_eq!(harness::crc32(b"123456789"), 0xcbf43926);
}
#[test]
fn sequence_records_achieved_time_skips_and_remains_bounded_without_host() {
    let (mut s, _, _) = shell(16, 16);
    let id = open(&mut s);
    call(
        &mut s,
        id,
        Operation::CaptureStart {
            count: 3,
            interval: 100,
        },
        0,
    );
    s.tick(0);
    s.present();
    s.tick(250);
    s.present();
    s.tick(350);
    s.present();
    let status = call(&mut s, id, Operation::CaptureStatus, 350).1;
    assert!(status.contains("state=completed requested=3 captured=3 skipped=1"));
    assert!(
        call(&mut s, id, Operation::CaptureMeta(2), 350)
            .1
            .contains("started_ms=250 ended_ms=250")
    );
    let submissions = s.display_submissions;
    s.tick(900);
    s.present();
    assert_eq!(s.display_submissions, submissions);
}

#[test]
fn bounded_rle_transfer_round_trips_runs_raw_fallback_and_pixel_boundaries() {
    fn decode(text: &str) -> (String, usize, Vec<u8>, usize) {
        let fields: std::collections::HashMap<_, _> = text
            .split_ascii_whitespace()
            .filter_map(|s| s.split_once('='))
            .collect();
        let hex = fields["hex"];
        let encoded: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert!(encoded.len() <= harness::ENCODED_BYTES);
        let bytes = if fields["encoding"] == "raw" {
            encoded.clone()
        } else {
            let mut bytes = Vec::new();
            assert_eq!(encoded.len() % 4, 0);
            for run in encoded.as_chunks::<4>().0 {
                let count = u16::from_le_bytes([run[0], run[1]]);
                assert!(count > 0);
                for _ in 0..count {
                    bytes.extend_from_slice(&run[2..4]);
                }
            }
            bytes
        };
        let length = fields["length"].parse::<usize>().unwrap();
        assert_eq!(bytes.len(), length);
        assert_eq!(
            harness::crc32(&bytes),
            u32::from_str_radix(fields["crc32"], 16).unwrap()
        );
        (fields["encoding"].to_owned(), length, bytes, encoded.len())
    }
    for (first_run, expected_encoding, expected_length) in [
        (2048, "rle565", 4096),
        (1, "raw", 256),
        (64, "raw", 256),
        (66, "rle565", 258),
    ] {
        let (mut s, _, _) = shell(32, 64);
        let id = open(&mut s);
        call(
            &mut s,
            id,
            Operation::CaptureStart {
                count: 1,
                interval: 100,
            },
            0,
        );
        let pixels: Vec<u16> = (0..2048)
            .map(|i| {
                if i < first_run {
                    0xf800
                } else {
                    (i - first_run + 1) as u16
                }
            })
            .collect();
        s.draw_pixels(|x, y| pixels[y * 32 + x]);
        let (status, text) = call(
            &mut s,
            id,
            Operation::CaptureRead {
                frame: 1,
                offset: 0,
                length: 4096,
            },
            0,
        );
        assert_eq!(status, "OK");
        let (encoding, length, decoded, encoded_length) = decode(&text);
        assert_eq!(encoding, expected_encoding);
        assert_eq!(length, expected_length);
        let expected: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        assert_eq!(decoded, expected[..length]);
        if first_run == 2048 {
            assert_eq!(encoded_length, 4);
        }
        // Host advances by actual decoded length, including a partial final request.
        let mut offset = length;
        while offset < expected.len() {
            let requested = (expected.len() - offset).min(harness::CHUNK);
            let (status, text) = call(
                &mut s,
                id,
                Operation::CaptureRead {
                    frame: 1,
                    offset: offset as u32,
                    length: requested as u16,
                },
                0,
            );
            assert_eq!(status, "OK");
            let (_, length, decoded, _) = decode(&text);
            assert!(length > 0 && length <= requested);
            assert_eq!(decoded, expected[offset..offset + length]);
            offset += length;
        }
        for (offset, length) in [(1, 256), (0, 255), (0, 4098), (4096, 2), (0, 0)] {
            assert_eq!(
                call(
                    &mut s,
                    id,
                    Operation::CaptureRead {
                        frame: 1,
                        offset,
                        length
                    },
                    0
                )
                .0,
                "INVALID"
            );
        }
        let (_, text) = call(
            &mut s,
            id,
            Operation::CaptureRead {
                frame: 1,
                offset: 4094,
                length: 2,
            },
            0,
        );
        let (encoding, length, bytes, _) = decode(&text);
        assert_eq!(encoding, "raw");
        assert_eq!(length, 2);
        assert_eq!(bytes, expected[4094..]);
    }
}
