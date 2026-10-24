use super::*;
use crate::ride::log::{Entry, Kind, Sample, Source, commit_word, encode as encode_slot};
use std::boxed::Box;
use std::vec;
use std::vec::Vec;

/// Where a slot's commit word goes.
const COMMIT_OFFSET: usize = 252;

fn sample(active_ms: u64, utc_ms: u64, heart: u16) -> Sample {
    Sample {
        active_ms,
        utc_ms: Some(utc_ms),
        heart_bpm: Some(heart),
        ..Entry::event(Kind::Samples, Source::Live, 0, 0, 0).samples[0]
    }
}

fn slot(entry: &Entry) -> Vec<u8> {
    let mut slot = encode_slot(entry);
    slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
    slot.0.to_vec()
}

/// A journal of `rides` finished rides, each START, two sample batches and
/// FINISH; ride n starts at n hours and averages 100 + n BPM.
fn journal(rides: u32) -> Vec<Vec<u8>> {
    let mut slots = Vec::new();
    for ride in 1..=rides {
        let utc = 1_789_000_000_000 + u64::from(ride) * 3_600_000;
        let heart = 100 + ride as u16;
        slots.push(slot(&Entry::event(Kind::Start, Source::Live, ride, 0, 0)));
        for batch in 0..2u32 {
            let samples: Vec<Sample> = (0..4u64)
                .map(|i| {
                    let ms = (u64::from(batch) * 4 + i + 1) * 1000;
                    sample(ms, utc + ms, heart)
                })
                .collect();
            slots.push(slot(
                &Entry::batch(Source::Live, ride, batch + 1, &samples).unwrap(),
            ));
        }
        slots.push(slot(&Entry::event(
            Kind::Finish,
            Source::Live,
            ride,
            3,
            8_000,
        )));
    }
    slots
}

fn storage(rides: usize) -> &'static mut [u8] {
    Box::leak(vec![0u8; rides * ENTRY].into_boxed_slice())
}

fn run(index: &mut Journal, slots: &[Vec<u8>], budget: usize) -> usize {
    let mut buffer = Slot::default();
    let mut calls = 0;
    while index.scanned() < slots.len() {
        index.step(slots.len(), budget, &mut buffer, |i, out| {
            out.0.copy_from_slice(&slots[i]);
            Read::Slot
        });
        calls += 1;
    }
    calls
}

#[test]
fn every_ride_is_indexed_in_bounded_steps_with_the_catalog_summary() {
    let slots = journal(25);
    let mut index = Journal::new(storage(RIDES));
    let calls = run(&mut index, &slots, 8);
    assert_eq!(calls, slots.len().div_ceil(8));
    assert_eq!(index.len(), 25);
    let first = index.get(0).unwrap();
    assert_eq!(first.ride_id, 1);
    assert_eq!(first.active_ms, 8_000);
    assert_eq!(first.first_utc_ms, Some(1_789_003_601_000));
    assert_eq!(first.heart_average, Some(101));
    assert_eq!(first.distance_mm, None);
    assert_eq!(index.get(24).unwrap().heart_average, Some(125));
    assert_eq!(index.last_ride_id(), Some(25));
}

#[test]
fn a_new_ride_is_picked_up_and_a_cleared_journal_starts_over() {
    let slots = journal(3);
    let mut index = Journal::new(storage(RIDES));
    let mut buffer = Slot::default();
    let mut read = |i: usize, out: &mut Slot| {
        out.0.copy_from_slice(&slots[i]);
        Read::Slot
    };
    // Two rides saved so far.
    assert!(index.step(8, 100, &mut buffer, &mut read));
    assert_eq!(index.len(), 2);
    // A third is saved: only its slots are read.
    assert!(index.step(12, 100, &mut buffer, &mut read));
    assert_eq!((index.len(), index.scanned()), (3, 12));
    assert!(!index.step(12, 100, &mut buffer, &mut read));
    // Cleared, then one ride saved again.
    index.step(4, 100, &mut buffer, &mut read);
    assert_eq!((index.len(), index.last_ride_id()), (1, Some(1)));
}

#[test]
fn a_full_index_keeps_the_newest_rides() {
    let slots = journal(7);
    let mut index = Journal::new(storage(5));
    run(&mut index, &slots, 3);
    assert_eq!(index.len(), 5);
    let ids: Vec<u32> = (0..5).map(|i| index.get(i).unwrap().ride_id).collect();
    assert_eq!(ids, [3, 4, 5, 6, 7]);
}

#[test]
fn a_failed_read_retries_and_an_invalid_slot_counts_as_in_the_catalog() {
    let slots = journal(2);
    let mut index = Journal::new(storage(RIDES));
    let mut buffer = Slot::default();
    // The medium fails at slot 2: nothing past it is read.
    index.step(8, 100, &mut buffer, |i, out| {
        if i == 2 {
            return Read::Failed;
        }
        out.0.copy_from_slice(&slots[i]);
        Read::Slot
    });
    assert_eq!((index.scanned(), index.len()), (2, 0));
    // Later slot 2 reads as an invalid container. As in the catalog, ride 1
    // lost a record and never completes; ride 2 is whole.
    index.step(8, 100, &mut buffer, |i, out| {
        if i == 2 {
            return Read::Invalid;
        }
        out.0.copy_from_slice(&slots[i]);
        Read::Slot
    });
    assert_eq!((index.len(), index.last_ride_id()), (1, Some(2)));
    assert!(!index.get(0).unwrap().gap);
}

#[test]
fn an_unfinished_ride_is_not_listed() {
    let mut slots = journal(1);
    slots.push(slot(&Entry::event(Kind::Start, Source::Live, 2, 0, 0)));
    let mut index = Journal::new(storage(RIDES));
    run(&mut index, &slots, 4);
    assert_eq!(index.len(), 1);
}

/// One ride of a second-by-second `watts` pattern, as a journal records it.
fn powered(
    ride: u32,
    watts: impl Fn(u64) -> Option<u16>,
    seconds: u64,
) -> (Vec<Vec<u8>>, Vec<Sample>) {
    let mut slots = vec![slot(&Entry::event(Kind::Start, Source::Live, ride, 0, 0))];
    let samples: Vec<Sample> = (1..=seconds)
        .map(|s| Sample {
            power_watts: watts(s),
            ..sample(s * 1000, 1_789_000_000_000 + s * 1000, 120)
        })
        .collect();
    for (batch, chunk) in samples.chunks(4).enumerate() {
        let sequence = batch as u32 + 1;
        slots.push(slot(
            &Entry::batch(Source::Live, ride, sequence, chunk).unwrap(),
        ));
    }
    let sequence = samples.len().div_ceil(4) as u32 + 1;
    slots.push(slot(&Entry::event(
        Kind::Finish,
        Source::Live,
        ride,
        sequence,
        seconds * 1000,
    )));
    (slots, samples)
}

#[test]
fn a_rides_stress_matches_the_live_totals_and_a_ride_without_power_has_none() {
    use crate::ride::totals::Totals;
    // Alternating minutes at 400 W and 0 W: normalized power far above the mean.
    let surging = |s: u64| Some(if (s / 60).is_multiple_of(2) { 400 } else { 0 });
    let (mut slots, samples) = powered(1, surging, 2_400);
    slots.extend(powered(2, |_| None, 600).0);
    let mut index = Journal::new(storage(RIDES));
    run(&mut index, &slots, 16);
    assert_eq!(index.len(), 2);

    let mut totals = Totals::default();
    for sample in &samples {
        totals.add(1000, 0, sample, None);
    }
    for ftp in [150, 215, 250, 400] {
        let effort = index.get(0).unwrap().effort.expect("a ride with power");
        assert_eq!(effort.np_watts, totals.normalized_power().unwrap());
        assert_eq!(
            effort.stress_score(ftp),
            totals.stress_score(ftp),
            "ftp {ftp}"
        );
    }
    assert_eq!(index.get(1).unwrap().effort, None);
}

#[test]
fn a_ride_too_short_for_normalized_power_has_no_effort() {
    let (slots, _) = powered(1, |_| Some(300), 29);
    let mut index = Journal::new(storage(RIDES));
    run(&mut index, &slots, 16);
    assert_eq!(index.get(0).unwrap().effort, None);
}
