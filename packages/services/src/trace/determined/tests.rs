extern crate std;

use super::*;
use crate::testing::Rng;
use std::vec;
use std::vec::Vec;

const MEDIA: [Medium; 2] = [Medium::Data, Medium::Bulk];
const UNITS: u32 = 4_000;

struct Unbounded {
    known: [Vec<bool>; 2],
}

impl Unbounded {
    fn new() -> Self {
        Self {
            known: [vec![false; UNITS as usize], vec![false; UNITS as usize]],
        }
    }

    fn units(&mut self, at: Span) -> &mut [bool] {
        let medium = MEDIA.iter().position(|m| *m == at.medium).unwrap();
        &mut self.known[medium][at.start as usize..at.end() as usize]
    }

    fn all(&mut self, at: Span) -> bool {
        self.units(at).iter().all(|known| *known)
    }
}

fn any_span(rng: &mut Rng) -> Span {
    let medium = MEDIA[rng.below(2) as usize];
    let start = rng.below(u64::from(UNITS)) as u32;
    let longest = if rng.one_in(400) {
        UNITS - start
    } else {
        6.min(UNITS - start)
    };
    let count = rng.range(0, u64::from(longest)) as u32;
    Span {
        medium,
        start,
        count,
    }
}

fn assert_ordered(set: &Determined) {
    for pair in set.entries.windows(2) {
        let (a, b) = (pair[0].span, pair[1].span);
        assert!(
            a.count > 0 && b.count > 0,
            "an empty span is held: {a:?} {b:?}"
        );
        if a.medium == b.medium {
            assert!(
                a.end() < u64::from(b.start),
                "spans overlap or touch: {a:?} {b:?}"
            );
        } else {
            assert!((a.medium as u8) < (b.medium as u8), "media out of order");
        }
    }
}

fn run(seed: u64, steps: u32) -> u32 {
    let mut rng = Rng::new(seed);
    let mut set = Determined::new();
    let mut unbounded = Unbounded::new();
    for step in 0..steps {
        let at = any_span(&mut rng);
        match rng.below(10) {
            0..=3 => {
                set.insert(at);
                unbounded.units(at).fill(true);
            }
            4..=5 => {
                set.remove(at);
                unbounded.units(at).fill(false);
            }
            6 if rng.one_in(300) => {
                set.forget(at.medium);
                unbounded
                    .units(Span {
                        start: 0,
                        count: UNITS,
                        ..at
                    })
                    .fill(false);
            }
            7 => set.next_pass(),
            _ => {}
        }
        assert_ordered(&set);
        let probe = any_span(&mut rng);
        let covered = set.covers(probe);
        let known = unbounded.all(probe);
        assert!(
            !covered || known,
            "seed {seed} step {step}: {probe:?} covered but not determined"
        );
        if set.evictions() == 0 {
            assert_eq!(
                covered, known,
                "seed {seed} step {step}: {probe:?} missed with nothing evicted"
            );
        }
    }
    set.evictions()
}

#[test]
fn a_covered_span_is_always_determined_and_a_miss_is_only_ever_an_eviction() {
    let mut evicted = 0;
    for seed in 0..400 {
        evicted += run(seed, 2_000);
    }
    assert!(
        evicted > 0,
        "no case filled the cache, so eviction went untested"
    );
}

fn one(start: u32, count: u32) -> Span {
    Span {
        medium: Medium::Bulk,
        start,
        count,
    }
}

#[test]
fn a_span_covered_only_in_parts_is_a_miss() {
    let mut set = Determined::new();
    set.insert(one(0, 8));
    set.insert(one(10, 8));
    assert!(
        !set.covers(one(4, 10)),
        "the gap at 8..10 is not determined"
    );
    set.insert(one(8, 2));
    assert!(set.covers(one(4, 10)), "touching spans merged into one");
    assert_eq!(set.entries.len(), 1);
}

#[test]
fn the_span_used_longest_ago_is_evicted_and_a_span_used_every_pass_stays() {
    let mut set = Determined::new();
    let hot = one(0, 1);
    for at in 0..CAPACITY as u32 {
        set.insert(one(at * 2, 1));
    }
    assert_eq!(set.high_water(), CAPACITY as u16);
    for later in 0..1_000 {
        set.next_pass();
        assert!(
            set.covers(hot),
            "pass {later}: the span read every pass was evicted"
        );
        set.insert(one(10_000 + later * 2, 1));
    }
    assert_eq!(set.evictions(), 1_000);
    assert_eq!(set.high_water(), CAPACITY as u16);
}

#[test]
fn removing_the_middle_of_a_full_cache_splits_without_losing_order() {
    let mut set = Determined::new();
    for at in 0..CAPACITY as u32 {
        set.insert(one(at * 4, 3));
    }
    set.next_pass();
    assert!(set.covers(one(0, 3)));
    set.remove(one(1, 1));
    assert_ordered(&set);
    assert_eq!(set.evictions(), 1);
    assert!(
        set.covers(one(0, 1)) && set.covers(one(2, 1)),
        "both pieces were used last"
    );
    assert!(!set.covers(one(1, 1)));
}

#[test]
fn removing_up_to_the_last_address_keeps_nothing_past_it() {
    let mut set = Determined::new();
    set.insert(one(u32::MAX, 3));
    set.remove(one(u32::MAX, 2));
    assert!(
        !set.covers(one(1, 1)),
        "a piece past u32::MAX wrapped to the start"
    );
    assert!(!set.covers(one(u32::MAX, 1)));
}

#[test]
fn a_forgotten_medium_leaves_the_others_determined() {
    let mut set = Determined::new();
    set.insert(one(0, 8));
    set.insert(Span {
        medium: Medium::Data,
        start: 0,
        count: 8,
    });
    set.forget(Medium::Bulk);
    assert!(!set.covers(one(0, 8)));
    assert!(set.covers(Span {
        medium: Medium::Data,
        start: 0,
        count: 8
    }));
}

#[test]
fn an_entry_is_sixteen_bytes_and_the_set_four_kibibytes_of_them() {
    assert_eq!(core::mem::size_of::<Entry>() * CAPACITY, 4096);
}
