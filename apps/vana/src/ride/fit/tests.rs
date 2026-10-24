use super::*;
use crate::ride::log::{LapStart, Source, commit_word, encode};
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
fn an_impossible_cadence_exports_as_the_highest_one() {
    let mut fast = sample(1_000, true);
    fast.cadence_tenths = Some(u16::MAX);
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &[fast]).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 2, 1_000)),
    ];
    let (_, bytes) = file(&slots, None).unwrap();
    let record = decode(&bytes).into_iter().find(|m| m.global == 20).unwrap();
    assert_eq!(record.get(4), Some(254));
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
    assert!(bytes.len() < 140_000, "{} bytes", bytes.len());
    let session = decode(&bytes).into_iter().find(|m| m.global == 18).unwrap();
    assert_eq!(session.get(34), Some(200), "normalized power");
}

#[test]
fn altitude_fills_records_and_ascent_and_descent() {
    // 100 m up to 130 m in 10 m steps, then back down, with one sample
    // in the middle that has no reading.
    let at = |active_ms: u64, dm: Option<i32>| Sample {
        altitude: dm.map(log::Altitude::from_dm),
        ..sample(active_ms, true)
    };
    let up: Vec<Sample> = (1..=4)
        .map(|i| at(i * 1000, Some(1000 + (i as i32 - 1) * 100)))
        .collect();
    let down: Vec<Sample> = (5..=8)
        .map(|i| at(i * 1000, (i != 6).then_some(1300 - (i as i32 - 4) * 100)))
        .collect();
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &up).unwrap()),
        slot(&Entry::batch(Source::Live, 3, 2, &down).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 3, 8_000)),
    ];
    let (plan, bytes) = file(&slots, None).unwrap();
    assert_eq!(bytes.len() as u32, plan.size);
    let messages = decode(&bytes);
    let records: Vec<&Decoded> = messages.iter().filter(|m| m.global == 20).collect();
    // (metres + 500) * 5: 100 m is 3000, 130 m is 3150.
    let altitudes: Vec<Option<u64>> = records.iter().map(|r| r.get(2)).collect();
    assert_eq!(
        altitudes,
        [
            Some(3000),
            Some(3050),
            Some(3100),
            Some(3150),
            Some(3100),
            None,
            Some(3000),
            Some(2950)
        ]
    );
    let enhanced: Vec<Option<u64>> = records.iter().map(|r| r.get(78)).collect();
    assert_eq!(enhanced, altitudes);
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    assert_eq!((session.get(22), session.get(23)), (Some(30), Some(40)));
    let lap = messages.iter().find(|m| m.global == 19).unwrap();
    assert_eq!((lap.get(21), lap.get(22)), (Some(30), Some(40)));
}

#[test]
fn a_ride_without_altitude_has_none_and_no_ascent() {
    let (_, bytes) = file(&ride(true, 0), None).unwrap();
    let messages = decode(&bytes);
    assert!(
        messages
            .iter()
            .filter(|m| m.global == 20)
            .all(|r| r.get(2).is_none() && r.get(78).is_none())
    );
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    assert_eq!((session.get(22), session.get(23)), (None, None));
}

fn record_times(bytes: &[u8]) -> Vec<u64> {
    let t = fit_time(T0);
    decode(bytes)
        .iter()
        .filter(|m| m.global == 20)
        .map(|r| r.get(253).unwrap() - t)
        .collect()
}

#[test]
fn an_unreadable_slot_takes_the_clock_again() {
    // The damaged slot hid a pause and resume of a minute.
    let mut late = sample(2_000, true);
    late.utc_ms = Some(T0 + 62_000);
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &[sample(1_000, true)]).unwrap()),
        Slot([0x33; 256]),
        slot(&Entry::batch(Source::Live, 3, 4, &[late]).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 5, 2_000)),
    ];
    let (_, bytes) = file(&slots, None).unwrap();
    assert_eq!(record_times(&bytes), [1, 62]);
}

#[test]
fn a_resumed_stretch_dated_part_way_is_dated_from_its_start() {
    let mut slots = ride(true, 60_000);
    let mut second: Vec<Sample> = (5..=8).map(|i| sample(i * 1000, true)).collect();
    for (index, sample) in second.iter_mut().enumerate() {
        sample.utc_ms = (index >= 2).then(|| T0 + sample.active_ms + 60_000);
    }
    slots[4] = slot(&Entry::batch(Source::Live, 3, 4, &second).unwrap());
    let (_, bytes) = file(&slots, None).unwrap();
    assert_eq!(record_times(&bytes), [1, 2, 3, 4, 65, 66, 67, 68]);
    let t = fit_time(T0);
    let events: Vec<u64> = decode(&bytes)
        .iter()
        .filter(|m| m.global == 21)
        .map(|m| m.get(253).unwrap() - t)
        .collect();
    assert_eq!(events, [0, 4, 64, 68]);
}

#[test]
fn time_between_the_last_sample_and_a_pause_counts() {
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &[sample(1_000, true)]).unwrap()),
        slot(&Entry::event(Kind::Pause, Source::Live, 3, 2, 1_500)),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 3, 1_500)),
    ];
    let (_, bytes) = file(&slots, None).unwrap();
    let session = decode(&bytes).into_iter().find(|m| m.global == 18).unwrap();
    assert_eq!(session.get(9), Some(1_200), "8 m/s for 1.5 s");
    assert_eq!(session.get(8), Some(1_500));
}

#[test]
fn a_start_outside_fit_dates_is_refused() {
    let slots = ride(false, 0);
    for start in [0, T0 / 1000, u64::MAX - 1] {
        assert_eq!(
            file(&slots, Some(start)).unwrap_err(),
            Refusal::BadClock,
            "{start}"
        );
    }
}

#[test]
fn a_later_stretch_outside_fit_dates_is_refused() {
    // The resumed stretch carries a clock far past FIT's last date.
    for pause_wall_ms in [u64::MAX / 2, 200 * 365 * 86_400_000] {
        assert_eq!(
            file(&ride(true, pause_wall_ms), None).unwrap_err(),
            Refusal::BadClock,
            "{pause_wall_ms}"
        );
    }
}

fn lapped_ride() -> Vec<Slot> {
    let mark = |active: u64, lap| Sample {
        lap: Some(lap),
        utc_ms: Some(T0 + active + 60_000),
        ..sample(active, true)
    };
    let mut first: Vec<Sample> = (1..=4).map(|i| sample(i * 1000, true)).collect();
    first[0].lap = Some(LapStart::Ride);
    let mut second: Vec<Sample> = (5..=8)
        .map(|i| {
            let mut s = sample(i * 1000, true);
            s.utc_ms = s.utc_ms.map(|v| v + 60_000);
            s
        })
        .collect();
    second[0] = mark(5_000, LapStart::Manual);
    second[2] = mark(7_000, LapStart::Distance);
    let mut slots = ride(true, 60_000);
    slots[1] = slot(&Entry::batch(Source::Live, 3, 1, &first).unwrap());
    slots[4] = slot(&Entry::batch(Source::Live, 3, 4, &second).unwrap());
    slots
}

/// Ride 3 as a journal can hold it: batches whose clock arrives late, jumps
/// or never comes, pauses of any wall time, damaged slots, another ride's
/// records, and any end.
fn arbitrary_ride(rng: &mut crate::testing::Rng) -> Vec<Slot> {
    let mut slots = vec![slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0))];
    let mut sequence = 1;
    let mut active_ms = 0;
    let mut offset_ms = T0 + rng.below(86_400_000);
    for _ in 0..rng.range(0, 40) {
        match rng.below(10) {
            0..=5 => {
                let samples: Vec<Sample> = (0..rng.range(1, 4))
                    .map(|_| {
                        active_ms += rng.pick(&[0, 1, 999, 1_000, 1_001, 30_000]);
                        let mut sample = rng.sample();
                        sample.active_ms = active_ms;
                        sample.utc_ms = match rng.below(8) {
                            0 => None,
                            1 => Some(offset_ms.saturating_sub(rng.below(600_000)) + active_ms),
                            2 => sample.utc_ms,
                            _ => Some(offset_ms + active_ms),
                        };
                        sample
                    })
                    .collect();
                slots.push(slot(
                    &Entry::batch(Source::Live, 3, sequence, &samples).unwrap(),
                ));
            }
            6 => {
                slots.push(slot(&Entry::event(
                    Kind::Pause,
                    Source::Live,
                    3,
                    sequence,
                    active_ms,
                )));
                sequence += 1;
                let any = rng.below(86_400_000);
                offset_ms += rng.pick(&[0, 1_000, 3_600_000, any]);
                slots.push(slot(&Entry::event(
                    Kind::Resume,
                    Source::Live,
                    3,
                    sequence,
                    active_ms,
                )));
            }
            7 => slots.push(Slot(core::array::from_fn(|_| rng.byte()))),
            8 => {
                let mut other = slot(&Entry::batch(Source::Live, 9, 1, &[rng.sample()]).unwrap());
                if rng.one_in(2) {
                    let at = rng.below(252) as usize;
                    other.0[at] ^= 1 << rng.below(8);
                }
                slots.push(other);
            }
            _ => sequence += 1,
        }
        sequence += 1;
    }
    let end = rng.pick(&[
        Kind::Finish,
        Kind::Recovered,
        Kind::Full,
        Kind::Start,
        Kind::Samples,
    ]);
    match end {
        Kind::Start => slots.push(slot(&Entry::event(Kind::Start, Source::Live, 4, 0, 0))),
        Kind::Samples => {}
        kind => slots.push(slot(&Entry::event(
            kind,
            Source::Live,
            3,
            sequence,
            active_ms,
        ))),
    }
    slots
}

#[test]
fn each_marked_lap_becomes_a_lap_message() {
    let (plan, bytes) = file(&lapped_ride(), None).unwrap();
    assert_eq!(plan.laps, 3);
    let messages = decode(&bytes);
    let t = fit_time(T0);
    let laps: Vec<&Decoded> = messages.iter().filter(|m| m.global == 19).collect();
    let field = |number| laps.iter().map(|lap| lap.get(number)).collect::<Vec<_>>();
    assert_eq!(field(254), [Some(0), Some(1), Some(2)], "message index");
    assert_eq!(
        field(24),
        [Some(0), Some(2), Some(7)],
        "manual, distance, end"
    );
    assert_eq!(
        field(2),
        [Some(t), Some(t + 4), Some(t + 66)],
        "each starts as the last ends"
    );
    assert_eq!(field(253), [Some(t + 4), Some(t + 66), Some(t + 68)], "end");
    assert_eq!(field(8), [Some(4_000), Some(2_000), Some(2_000)], "timer");
    assert_eq!(
        field(7),
        [Some(4_000), Some(62_000), Some(2_000)],
        "elapsed, the pause in lap 2"
    );
    assert_eq!(field(9), [Some(3_200), Some(1_600), Some(1_600)], "8 m/s");
    assert_eq!(field(19), [Some(200); 3], "avg power");
    assert_eq!(field(15), [Some(140); 3], "avg heart rate");
    let lat = |active: i32| Some(u64::from(semicircles(-333_646_900 + active) as u32));
    assert_eq!(
        field(3),
        [lat(1000), lat(5000), lat(7000)],
        "first position"
    );
    assert_eq!(field(5), [lat(4000), lat(6000), lat(8000)], "last position");
    let globals: Vec<u16> = messages.iter().map(|m| m.global).collect();
    let first_lap = globals.iter().position(|g| *g == 19).unwrap();
    assert_eq!(globals[first_lap - 2..first_lap + 3], [20, 21, 19, 21, 20]);
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    assert_eq!(session.get(26), Some(3), "num_laps");
    assert_eq!(
        session.get(8),
        Some(8_000),
        "the laps add up to the session"
    );
}

#[test]
fn a_ride_without_lap_marks_is_one_lap_of_the_whole_ride() {
    let (plan, bytes) = file(&ride(true, 60_000), None).unwrap();
    assert_eq!(plan.laps, 1);
    let messages = decode(&bytes);
    let laps: Vec<&Decoded> = messages.iter().filter(|m| m.global == 19).collect();
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    assert_eq!(laps.len(), 1);
    assert_eq!(laps[0].get(24), Some(7), "session end");
    for (in_lap, in_session) in [(2, 2), (7, 7), (8, 8), (9, 9), (13, 14), (19, 20)] {
        assert_eq!(
            laps[0].get(in_lap),
            session.get(in_session),
            "lap field {in_lap}"
        );
    }
}

#[test]
fn a_slot_of_four_laps_stays_within_its_bound() {
    let mut out = Counter::default();
    let mut export = Export::start(3, None, None, &mut out);
    let slots = ride(true, 0);
    export.slot(&slots[0], &mut out);
    export.slot(&slots[1], &mut out);
    export.slot(&slots[2], &mut out);
    let marked: Vec<Sample> = (5..=8)
        .map(|i| Sample {
            lap: Some(LapStart::Manual),
            ..sample(i * 1000, true)
        })
        .collect();
    let before = out.0;
    export.slot(&slots[3], &mut out);
    export.slot(
        &slot(&Entry::batch(Source::Live, 3, 4, &marked).unwrap()),
        &mut out,
    );
    assert_eq!(LAP.record_size(), LAP_RECORD);
    assert_eq!(out.0 - before, MAX_SLOT_OUTPUT - 7, "all but a pause");
}

#[test]
fn lap_distances_add_up_to_the_session_to_the_centimetre() {
    // 15 mm a second: each one-sample lap holds a centimetre and a half.
    let samples: Vec<Sample> = (1..=4u64)
        .map(|i| Sample {
            speed_mm_s: Some(15),
            lap: Some(if i == 1 {
                LapStart::Ride
            } else {
                LapStart::Manual
            }),
            ..sample(i * 1000, true)
        })
        .collect();
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &samples).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 2, 4_000)),
    ];
    let (_, bytes) = file(&slots, None).unwrap();
    let messages = decode(&bytes);
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    let laps: u64 = messages
        .iter()
        .filter(|m| m.global == 19)
        .map(|lap| lap.get(9).unwrap())
        .sum();
    assert_eq!(session.get(9), Some(6), "60 mm");
    assert_eq!(Some(laps), session.get(9));
}

#[test]
fn lap_ascent_and_descent_add_up_to_the_session() {
    // Up 0.45 m a second for 30 s, then down, with a lap every 7 samples:
    // each lap's own climb has a fraction of a metre to round.
    let samples: Vec<Sample> = (1..=60u64)
        .map(|i| {
            let dm = 1000 + 45 * i.min(60 - i) as i32 / 10;
            Sample {
                altitude: Some(log::Altitude::from_dm(dm)),
                lap: (i % 7 == 1).then_some(if i == 1 {
                    LapStart::Ride
                } else {
                    LapStart::Manual
                }),
                ..sample(i * 1000, true)
            }
        })
        .collect();
    let mut slots = vec![slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0))];
    for (index, batch) in samples.chunks(4).enumerate() {
        slots.push(slot(
            &Entry::batch(Source::Live, 3, index as u32 + 1, batch).unwrap(),
        ));
    }
    slots.push(slot(&Entry::event(
        Kind::Finish,
        Source::Live,
        3,
        16,
        60_000,
    )));
    let (plan, bytes) = file(&slots, None).unwrap();
    assert_eq!(plan.laps, 9);
    let messages = decode(&bytes);
    let laps: Vec<&Decoded> = messages.iter().filter(|m| m.global == 19).collect();
    let session = messages.iter().find(|m| m.global == 18).unwrap();
    for (in_lap, in_session) in [(21, 22), (22, 23)] {
        let sum: u64 = laps.iter().map(|lap| lap.get(in_lap).unwrap()).sum();
        assert!(session.get(in_session).unwrap() > 0);
        assert_eq!(Some(sum), session.get(in_session), "lap field {in_lap}");
    }
}

#[test]
fn any_ride_becomes_a_valid_file_of_its_planned_size_with_ordered_times() {
    let (mut written, mut refused, mut lapped) = (0, 0, 0);
    for seed in 0..1_500 {
        let mut rng = crate::testing::Rng::new(seed);
        let slots = arbitrary_ride(&mut rng);
        let start = rng.option(|rng| T0 + rng.below(1_000_000));
        // `file` checks the planned size against the bytes written.
        let Ok((plan, bytes)) = file(&slots, start) else {
            refused += 1;
            continue;
        };
        written += 1;
        // The reader checks both CRCs and the declared size.
        let messages = decode(&bytes);
        let records = messages.iter().filter(|m| m.global == 20).count();
        assert_eq!(records as u32, plan.samples, "seed {seed}");
        let times: Vec<u64> = messages
            .iter()
            .filter(|m| matches!(m.global, 19..=21))
            .map(|m| m.get(253).expect("timestamped"))
            .collect();
        assert!(
            times.windows(2).all(|pair| pair[0] <= pair[1]),
            "seed {seed}: {times:?}"
        );
        // Every lap mark starts a lap, and the laps add up to the session.
        let laps: Vec<&Decoded> = messages.iter().filter(|m| m.global == 19).collect();
        let session = messages.iter().find(|m| m.global == 18).unwrap();
        assert_eq!(laps.len(), usize::from(plan.laps), "seed {seed}");
        assert_eq!(session.get(26), Some(u64::from(plan.laps)), "seed {seed}");
        // A session total at its field's largest value saturated, so its laps cannot add up.
        for (in_lap, in_session, largest) in [
            (8, 8, u64::from(u32::MAX - 1)),
            (9, 9, u64::from(u32::MAX - 1)),
            (21, 22, u64::from(u16::MAX - 1)),
            (22, 23, u64::from(u16::MAX - 1)),
        ] {
            let total = session.get(in_session);
            if total.is_some_and(|total| total >= largest) {
                continue;
            }
            // A lap without a reading leaves its field invalid.
            let values: Vec<u64> = laps.iter().filter_map(|lap| lap.get(in_lap)).collect();
            let sum = (!values.is_empty()).then(|| values.iter().sum::<u64>());
            assert_eq!(sum, total, "seed {seed}: lap field {in_lap}");
        }
        lapped += usize::from(plan.laps > 2);
    }
    assert!(
        written > 1_000 && refused > 0 && lapped > 500,
        "{written} written, {refused} refused, {lapped} with laps"
    );
}

#[test]
fn arbitrary_slots_never_break_the_export() {
    for seed in 0..1_000 {
        let mut rng = crate::testing::Rng::new(seed);
        let mut slots = arbitrary_ride(&mut rng);
        for _ in 0..rng.range(1, 8) {
            let index = rng.below(slots.len() as u64) as usize;
            let at = rng.below(256) as usize;
            slots[index].0[at] = rng.byte();
        }
        if let Ok((plan, bytes)) = file(&slots, Some(T0)) {
            decode(&bytes);
            assert_eq!(plan.size as usize, bytes.len(), "seed {seed}");
        }
    }
}

#[test]
fn a_stored_cadence_past_the_sensor_range_exports_capped() {
    let mut wild = sample(1_000, true);
    wild.cadence_tenths = Some(u16::MAX);
    let slots = [
        slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        slot(&Entry::batch(Source::Live, 3, 1, &[wild]).unwrap()),
        slot(&Entry::event(Kind::Finish, Source::Live, 3, 2, 1_000)),
    ];
    let (_, bytes) = file(&slots, None).unwrap();
    let record = decode(&bytes).into_iter().find(|m| m.global == 20).unwrap();
    assert_eq!(record.get(4), Some(254));
}
