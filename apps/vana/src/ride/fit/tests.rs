use super::*;
use crate::ride::log::{Source, commit_word, encode};
use firmware_services::fit::{FIT_EPOCH_UNIX_S, crc16};
use std::collections::BTreeMap;
use std::vec;
use std::vec::Vec;

struct Bytes(Vec<u8>);
impl Sink for Bytes {
    fn put(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// One decoded data message: global number and raw field values by number.
#[derive(Debug)]
struct Decoded {
    global: u16,
    fields: BTreeMap<u8, u64>,
}

impl Decoded {
    fn get(&self, field: u8) -> Option<u64> {
        self.fields.get(&field).copied()
    }
}

type Definition = (u16, Vec<(u8, u8, u8)>);

/// An independent minimal reader: checks both CRCs and the declared size,
/// and returns every data message with its valid fields.
fn decode(bytes: &[u8]) -> Vec<Decoded> {
    assert_eq!(bytes[0], 14);
    assert_eq!(&bytes[8..12], b".FIT");
    assert_eq!(crc16(0, &bytes[..14]), 0, "header CRC");
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 14 + size + 2, "declared size");
    assert_eq!(crc16(0, bytes), 0, "file CRC");
    let mut definitions: BTreeMap<u8, Definition> = BTreeMap::new();
    let mut messages = Vec::new();
    let mut at = 14;
    while at < 14 + size {
        let header = bytes[at];
        at += 1;
        assert_eq!(header & 0x80, 0, "no compressed timestamps");
        let local = header & 0xf;
        if header & 0x40 != 0 {
            assert_eq!(header & 0x20, 0, "no developer data");
            assert_eq!(bytes[at + 1], 0, "little endian");
            let global = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]);
            let count = usize::from(bytes[at + 4]);
            at += 5;
            let fields = (0..count)
                .map(|i| {
                    (
                        bytes[at + 3 * i],
                        bytes[at + 3 * i + 1],
                        bytes[at + 3 * i + 2],
                    )
                })
                .collect();
            at += 3 * count;
            definitions.insert(local, (global, fields));
        } else {
            let (global, fields) = definitions.get(&local).expect("defined local message");
            let mut values = BTreeMap::new();
            for &(number, size, base) in fields {
                let raw = &bytes[at..at + usize::from(size)];
                at += usize::from(size);
                if base == 0x07 {
                    continue;
                }
                let mut wide = [0u8; 8];
                wide[..raw.len()].copy_from_slice(raw);
                let value = u64::from_le_bytes(wide);
                let invalid = match base {
                    0x00 | 0x02 => 0xff,
                    0x01 => 0x7f,
                    0x83 => 0x7fff,
                    0x84 => 0xffff,
                    0x85 => 0x7fff_ffff,
                    0x86 => 0xffff_ffff,
                    0x8c => 0,
                    other => panic!("base type {other:#x}"),
                };
                if value != invalid {
                    values.insert(number, value);
                }
            }
            messages.push(Decoded {
                global: *global,
                fields: values,
            });
        }
    }
    messages
}

const T0: u64 = 1_789_000_000_000;

fn slot(entry: &Entry) -> Slot {
    let mut slot = encode(entry);
    slot.0[252..].copy_from_slice(&commit_word().0);
    slot
}

fn sample(active_ms: u64, utc: bool) -> Sample {
    Sample {
        active_ms,
        utc_ms: utc.then_some(T0 + active_ms),
        location_e7: Some((-333_646_900 + active_ms as i32, -705_155_800)),
        heart_bpm: Some(140),
        cadence_tenths: Some(905),
        speed_mm_s: Some(8_000),
        power_watts: Some(200),
        gradient_tenths: Some(-37),
        ..Sample::default()
    }
}

/// Ride 3: four samples, a pause of 60 s of wall time, four more, finish.
fn ride(utc: bool, pause_wall_ms: u64) -> Vec<Slot> {
    let later = |active: u64| {
        let mut s = sample(active, utc);
        s.utc_ms = s.utc_ms.map(|v| v + pause_wall_ms);
        s
    };
    let first: Vec<Sample> = (1..=4).map(|i| sample(i * 1000, utc)).collect();
    let second: Vec<Sample> = (5..=8).map(|i| later(i * 1000)).collect();
    vec![
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &first).unwrap()),
        slot(&Entry::event(Kind::Pause, Source::Live, 3, 2, 4_000)),
        slot(&Entry::event(Kind::Resume, Source::Live, 3, 3, 4_000)),
        slot(&Entry::batch(Source::Live, 3, 4, &second).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 5, 8_000)),
    ]
}

fn fit_time(unix_ms: u64) -> u64 {
    unix_ms / 1000 - FIT_EPOCH_UNIX_S
}

fn file(slots: &[Slot], start: Option<u64>) -> Result<(Plan, Vec<u8>), Refusal> {
    let mut out = Bytes(Vec::new());
    let plan = write_all(slots.iter(), 3, start, &mut out)?;
    assert_eq!(plan.size as usize, out.0.len());
    Ok((plan, out.0))
}

#[test]
fn a_paused_ride_becomes_a_valid_activity() {
    let (plan, bytes) = file(&ride(true, 60_000), None).unwrap();
    assert_eq!(plan.samples, 8);
    assert_eq!(plan.offset_ms, Some(T0));
    let messages = decode(&bytes);
    let globals: Vec<u16> = messages.iter().map(|m| m.global).collect();
    assert_eq!(globals[..2], [0, 23]);
    assert_eq!(globals[globals.len() - 3..], [19, 18, 34]);
    let file_id = &messages[0];
    assert_eq!(file_id.get(0), Some(4));
    assert_eq!(file_id.get(4), Some(fit_time(T0)));
    let events: Vec<(u64, u64)> = messages
        .iter()
        .filter(|m| m.global == 21)
        .map(|m| (m.get(253).unwrap(), m.get(1).unwrap()))
        .collect();
    let t = fit_time(T0);
    assert_eq!(
        events,
        [(t, 0), (t + 4, 4), (t + 64, 0), (t + 68, 4)],
        "start, pause, resume after 60 s wall time, end"
    );
    let records: Vec<&Decoded> = messages.iter().filter(|m| m.global == 20).collect();
    assert_eq!(records.len(), 8);
    let times: Vec<u64> = records.iter().map(|r| r.get(253).unwrap() - t).collect();
    assert_eq!(times, [1, 2, 3, 4, 65, 66, 67, 68]);
    let first = records[0];
    assert_eq!(first.get(3), Some(140));
    assert_eq!(first.get(4), Some(91));
    assert_eq!(first.get(6), Some(8_000));
    assert_eq!(first.get(7), Some(200));
    assert_eq!(first.get(9).map(|v| v as u16 as i16), Some(-370));
    assert_eq!(
        first.get(0).map(|v| v as u32 as i32),
        Some(semicircles(-333_645_900))
    );
    // Distance integrates wheel speed over active time only: 8 s at 8 m/s.
    assert_eq!(records[7].get(5), Some(6_400));
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    assert_eq!(session.get(5), Some(2), "cycling");
    assert_eq!(session.get(2), Some(t));
    assert_eq!(session.get(8), Some(8_000), "timer time excludes the pause");
    assert_eq!(session.get(7), Some(68_000), "elapsed time includes it");
    assert_eq!(session.get(9), Some(6_400));
    assert_eq!(session.get(14), Some(8_000));
    assert_eq!(session.get(16), Some(140));
    assert_eq!(session.get(20), Some(200));
    assert_eq!(session.get(26), Some(1));
    let lap = messages.iter().find(|m| m.global == 19).unwrap();
    assert_eq!(lap.get(8), Some(8_000));
    let activity = messages.last().unwrap();
    assert_eq!(activity.get(1), Some(1));
    assert_eq!(activity.get(253), Some(t + 68));
}

#[test]
fn rides_without_a_clock_need_a_start_time() {
    let slots = ride(false, 60_000);
    assert_eq!(file(&slots, None).unwrap_err(), Refusal::NoClock);
    let (plan, bytes) = file(&slots, Some(T0)).unwrap();
    assert_eq!(plan.offset_ms, None);
    let messages = decode(&bytes);
    let t = fit_time(T0);
    let times: Vec<u64> = messages
        .iter()
        .filter(|m| m.global == 20)
        .map(|r| r.get(253).unwrap() - t)
        .collect();
    // Without UTC the pause's wall time is unknown and counts as none.
    assert_eq!(times, [1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn a_late_clock_dates_earlier_samples_from_its_first_offset() {
    let mut slots = ride(true, 0);
    let mut early: Vec<Sample> = (1..=4).map(|i| sample(i * 1000, true)).collect();
    early[0].utc_ms = None;
    early[1].utc_ms = None;
    slots[1] = slot(&Entry::batch(Source::Live, 3, 1, &early).unwrap());
    let (_, bytes) = file(&slots, None).unwrap();
    let t = fit_time(T0);
    let times: Vec<u64> = decode(&bytes)
        .iter()
        .filter(|m| m.global == 20)
        .map(|r| r.get(253).unwrap() - t)
        .collect();
    assert_eq!(times, [1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn other_rides_invalid_slots_and_missing_readings_are_skipped() {
    let mut slots = ride(true, 0);
    slots.insert(2, Slot([0x55; 256]));
    slots.insert(
        3,
        slot(&Entry::batch(Source::Live, 9, 1, &[sample(500, true)]).unwrap()),
    );
    let bare = Sample {
        active_ms: 4_500,
        utc_ms: Some(T0 + 4_500),
        ..Sample::default()
    };
    slots.insert(6, slot(&Entry::batch(Source::Live, 3, 4, &[bare]).unwrap()));
    // The next ride's start ends an interrupted one, and later slots are ignored.
    let end = slots.len() - 1;
    slots[end] = slot(&Entry::event(Kind::Start, Source::Live, 4, 0, 0));
    slots.push(slot(
        &Entry::batch(Source::Live, 4, 1, &[sample(1_000, true)]).unwrap(),
    ));
    let (plan, bytes) = file(&slots, None).unwrap();
    assert_eq!(plan.samples, 9);
    let messages = decode(&bytes);
    let bare = messages.iter().filter(|m| m.global == 20).nth(4).unwrap();
    assert_eq!(bare.fields.keys().copied().collect::<Vec<_>>(), [5, 253]);
}

#[test]
fn an_empty_ride_is_refused() {
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 1, 0)),
    ];
    assert_eq!(file(&slots, Some(T0)).unwrap_err(), Refusal::Empty);
}

#[test]
fn stream_bounds_hold() {
    let mut out = Counter::default();
    let export = Export::start(1, None, None, &mut out);
    assert!(out.0 <= MAX_START_OUTPUT, "start {}", out.0);
    let start = out.0;
    let mut export2 = export.clone();
    let slots = ride(true, 0);
    for slot in &slots[..2] {
        let before = out.0;
        export2.slot(slot, &mut out);
        assert!(out.0 - before <= MAX_SLOT_OUTPUT);
    }
    let before = out.0;
    export2.finish(&mut out);
    assert!(
        out.0 - before <= MAX_FINISH_OUTPUT,
        "finish {}",
        out.0 - before
    );
    assert!(start > 0);
}

#[test]
fn an_hour_at_one_hertz_stays_small() {
    let mut slots = vec![slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0))];
    for batch in 0..900u64 {
        let samples: Vec<Sample> = (1..=4)
            .map(|i| sample((batch * 4 + i) * 1000, true))
            .collect();
        slots.push(slot(
            &Entry::batch(Source::Live, 3, batch as u32 + 1, &samples).unwrap(),
        ));
    }
    slots.push(slot(&Entry::event(
        Kind::Finish,
        Source::Live,
        3,
        901,
        3_600_000,
    )));
    let (plan, bytes) = file(&slots, None).unwrap();
    assert_eq!(plan.samples, 3600);
    assert!(bytes.len() < 100_000, "{} bytes", bytes.len());
    let session = decode(&bytes).into_iter().find(|m| m.global == 18).unwrap();
    assert_eq!(session.get(34), Some(200), "normalized power");
}
