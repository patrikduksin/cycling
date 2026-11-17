//! Deterministic random input for host property tests.
//!
//! Property tests draw every case from a fixed seed, so a failure names the
//! case that reproduces it, and cap their cases so `cycling test` stays
//! quick. The generator is `firmware_services::testing::Rng`, shared by every
//! crate's property tests through that crate's `testing` feature.

use crate::ride::log::{Altitude, LapStart, Sample};

pub use firmware_services::testing::Rng;
use std::vec::Vec;

/// Ride-shaped draws on the shared generator.
pub trait Draw {
    fn lap(&mut self) -> LapStart;
    fn sample(&mut self) -> Sample;
}

impl Draw for Rng {
    fn lap(&mut self) -> LapStart {
        self.pick(&[
            LapStart::Ride,
            LapStart::Manual,
            LapStart::Distance,
            LapStart::Step,
            LapStart::DistanceStep,
        ])
    }

    /// Any reading a sensor or the GPS can report, at any active time.
    fn sample(&mut self) -> Sample {
        Sample {
            active_ms: self.next(),
            utc_ms: self.option(|rng| rng.range(1_500_000_000_000, 2_500_000_000_000)),
            location_e7: self.option(|rng| {
                (
                    rng.range(0, 1_800_000_000) as i32 - 900_000_000,
                    (rng.range(0, 3_600_000_000) as i64 - 1_800_000_000) as i32,
                )
            }),
            heart_bpm: self.option(|rng| rng.next() as u16),
            cadence_tenths: self.option(|rng| rng.next() as u16),
            battery_percent: self.option(Self::byte),
            speed_mm_s: self.option(|rng| rng.next() as u32),
            power_watts: self.option(|rng| rng.next() as u16),
            gradient_tenths: self.option(|rng| rng.next() as i16),
            altitude: self.option(|rng| Altitude::from_dm(rng.next() as i32)),
            lap: self.one_in(4).then(|| self.lap()),
        }
    }
}

/// One of a sweep's parts, split so nextest runs them on separate cores.
pub(crate) struct Share {
    k: usize,
    seen: usize,
}

impl Share {
    pub(crate) const fn new(k: usize) -> Self {
        assert!(k < 8);
        Self { k, seen: 0 }
    }

    pub(crate) fn takes(&mut self) -> bool {
        self.seen += 1;
        (self.seen - 1) % 8 == self.k
    }
}

macro_rules! split_in_eight {
    ($sweep:ident) => {
        mod $sweep {
            use $crate::testing::Share;
            #[test]
            fn share_0() {
                super::$sweep(Share::new(0));
            }
            #[test]
            fn share_1() {
                super::$sweep(Share::new(1));
            }
            #[test]
            fn share_2() {
                super::$sweep(Share::new(2));
            }
            #[test]
            fn share_3() {
                super::$sweep(Share::new(3));
            }
            #[test]
            fn share_4() {
                super::$sweep(Share::new(4));
            }
            #[test]
            fn share_5() {
                super::$sweep(Share::new(5));
            }
            #[test]
            fn share_6() {
                super::$sweep(Share::new(6));
            }
            #[test]
            fn share_7() {
                super::$sweep(Share::new(7));
            }
        }
    };
}
pub(crate) use split_in_eight;

/// How an interrupted sector erase leaves its sector. NOR erase only sets
/// bits, and one cut by power leaves cells across the whole sector partly
/// erased.
#[derive(Clone, Copy)]
pub(crate) struct EraseTear {
    pub(crate) progress: u16,
    pub(crate) seed: u64,
}

impl EraseTear {
    pub(crate) fn draw(rng: &mut Rng) -> Self {
        let progress = match rng.below(4) {
            0 => rng.pick(&[0, 1, 255, 256]),
            _ => rng.below(257) as u16,
        };
        Self {
            progress,
            seed: rng.next(),
        }
    }

    pub(crate) fn apply(self, stored: &mut [u8]) {
        let mut rng = Rng::new(self.seed);
        for byte in stored.iter_mut() {
            for bit in 0..8 {
                if rng.below(256) < u64::from(self.progress) {
                    *byte |= 1 << bit;
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Cut {
    pub(crate) at: u64,
    pub(crate) seed: u64,
}

/// The two NOR sectors of `Region::Intent`, with power cut at any mutation.
/// It models an interrupted NOR erase and program conservatively: a cut
/// erase leaves bits across the sector partly erased, a cut program leaves
/// its bytes partly programmed, and a cut commit word is marginal, so each
/// later read returns it whole or as it was left until a program completes
/// over it or the sector is erased.
pub(crate) struct IntentFlash {
    pub(crate) bytes: std::vec::Vec<u8>,
    pub(crate) mutations: u64,
    pub(crate) cut: Option<Cut>,
    pub(crate) off: bool,
    marginal: std::vec::Vec<(usize, u8)>,
    reads: Rng,
}

impl Default for IntentFlash {
    fn default() -> Self {
        Self {
            bytes: std::vec![0xff; 2 * 4096],
            mutations: 0,
            cut: None,
            off: false,
            marginal: std::vec::Vec::new(),
            reads: Rng::new(0),
        }
    }
}

impl IntentFlash {
    pub(crate) fn power_on(&mut self, read_seed: u64) {
        self.off = false;
        self.cut = None;
        self.reads = Rng::new(read_seed);
    }

    fn mutation(&mut self) -> Result<Option<Rng>, ()> {
        if self.off {
            return Err(());
        }
        let at = self.mutations;
        self.mutations += 1;
        match self.cut {
            Some(cut) if cut.at == at => {
                self.off = true;
                Ok(Some(Rng::new(cut.seed)))
            }
            _ => Ok(None),
        }
    }
}

impl device_api::storage::OwnedFlash for IntentFlash {
    type Error = ();
    fn availability(
        &self,
        _region: device_api::storage::Region,
    ) -> device_api::observation::Availability {
        device_api::observation::Availability::Ready
    }
    fn geometry(&self, region: device_api::storage::Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: if region == device_api::storage::Region::Intent {
                self.bytes.len()
            } else {
                0
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
        for &(at, whole) in &self.marginal {
            if let Some(byte) = at.checked_sub(offset).and_then(|i| output.get_mut(i))
                && self.reads.one_in(2)
            {
                *byte = whole;
            }
        }
        Ok(())
    }
    fn program(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), ()> {
        let Some(mut tear) = self.mutation()? else {
            for (stored, new) in self.bytes[offset..].iter_mut().zip(bytes) {
                *stored &= new;
            }
            self.marginal
                .retain(|&(at, _)| !(offset..offset + bytes.len()).contains(&at));
            return Ok(());
        };
        let landed = tear.below(bytes.len() as u64 + 1) as usize;
        for (i, (stored, new)) in self.bytes[offset..].iter_mut().zip(bytes).enumerate() {
            let target = *stored & new;
            *stored &= match i.cmp(&landed) {
                core::cmp::Ordering::Less => *new,
                core::cmp::Ordering::Equal => new | tear.byte(),
                core::cmp::Ordering::Greater => 0xff,
            };
            if bytes.len() == 4 && *stored != target {
                self.marginal.push((offset + i, target));
            }
        }
        Err(())
    }
    fn erase(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        length: usize,
    ) -> Result<(), ()> {
        let tear = self.mutation()?;
        self.marginal
            .retain(|&(at, _)| !(offset..offset + length).contains(&at));
        let Some(mut tear) = tear else {
            self.bytes[offset..offset + length].fill(0xff);
            return Ok(());
        };
        EraseTear {
            progress: tear.below(257) as u16,
            seed: tear.next(),
        }
        .apply(&mut self.bytes[offset..offset + length]);
        Err(())
    }
}

/// A rule every reached state keeps, by name.
pub(crate) type Holds<S> = (&'static str, fn(&S) -> bool);
/// A rule every transition keeps, by name: before, event, after.
pub(crate) type Legal<S, E> = (&'static str, fn(&S, E, &S) -> bool);

/// What a breadth-first search of every state reachable from `inits` found.
pub(crate) struct Report<E> {
    /// The frontier emptied within the budget; only then is the search
    /// exhaustive, up to a fingerprint collision.
    pub closed: bool,
    pub states: usize,
    /// Names of the states reached, each once, in the order first reached.
    pub reached: Vec<&'static str>,
    /// Each broken rule once, with the shortest event path that breaks it.
    pub broken: Vec<Broken<E>>,
}

pub(crate) struct Broken<E> {
    pub rule: &'static str,
    pub init_index: usize,
    pub path: Vec<E>,
}

#[derive(Clone, Copy)]
enum Step<E> {
    Init(usize),
    Edge { parent: usize, event: E },
}

/// Search breadth-first from `inits` through `next`, checking `holds` on
/// visited states and `legal` on every generated edge. Deduplication uses
/// 128-bit fingerprints from `S::hash`; callers must hash every distinction
/// relevant to state predicates or future behavior. Fingerprint collisions
/// can omit states. Discovering more than `budget` states truncates the search.
pub(crate) fn explore<S: Clone + core::hash::Hash, E: Copy>(
    inits: &[S],
    alphabet: impl Fn(&S) -> Vec<E>,
    next: impl Fn(&S, E) -> S,
    name: impl Fn(&S) -> &'static str,
    holds: &[Holds<S>],
    legal: &[Legal<S, E>],
    budget: usize,
) -> Report<E> {
    use std::collections::{HashSet, VecDeque};
    let names = holds
        .iter()
        .map(|(name, _)| name)
        .chain(legal.iter().map(|(name, _)| name));
    let unique: HashSet<_> = names.clone().collect();
    assert_eq!(unique.len(), names.count(), "rule names are unique");
    let mut trail: Vec<Step<E>> = Vec::new();
    let path = |trail: &[Step<E>], mut at: usize, last: Option<E>| {
        let mut path: Vec<E> = last.into_iter().collect();
        loop {
            match trail[at] {
                Step::Init(index) => {
                    path.reverse();
                    return (index, path);
                }
                Step::Edge { parent, event } => {
                    path.push(event);
                    at = parent;
                }
            }
        }
    };
    let mut report = Report {
        closed: false,
        states: 0,
        reached: Vec::new(),
        broken: Vec::new(),
    };
    let record = |report: &mut Report<E>, rule: &'static str, (init_index, path)| {
        if report.broken.iter().all(|broken| broken.rule != rule) {
            report.broken.push(Broken {
                rule,
                init_index,
                path,
            });
        }
    };
    let mut seen = HashSet::new();
    let mut frontier = VecDeque::new();
    let mut admit = |state: S, from: Step<E>, trail: &mut Vec<Step<E>>, report: &mut Report<E>| {
        if !seen.insert(fingerprint(&state)) {
            return None;
        }
        let at = trail.len();
        trail.push(from);
        let named = name(&state);
        if !report.reached.contains(&named) {
            report.reached.push(named);
        }
        Some((at, state))
    };
    for (index, init) in inits.iter().enumerate() {
        if let Some(admitted) = admit(init.clone(), Step::Init(index), &mut trail, &mut report) {
            frontier.push_back(admitted);
        }
    }
    if trail.len() > budget {
        report.states = trail.len();
        return report;
    }
    while let Some((at, state)) = frontier.pop_front() {
        for (rule, keeps) in holds {
            if !keeps(&state) {
                record(&mut report, rule, path(&trail, at, None));
            }
        }
        for event in alphabet(&state) {
            let after = next(&state, event);
            for (rule, keeps) in legal {
                if !keeps(&state, event, &after) {
                    record(&mut report, rule, path(&trail, at, Some(event)));
                }
            }
            let from = Step::Edge { parent: at, event };
            if let Some(admitted) = admit(after, from, &mut trail, &mut report) {
                if trail.len() > budget {
                    report.states = trail.len();
                    return report;
                }
                frontier.push_back(admitted);
            }
        }
    }
    report.closed = true;
    report.states = trail.len();
    report
}

fn fingerprint(state: &impl core::hash::Hash) -> u128 {
    use core::hash::Hasher;
    struct Twin(std::hash::DefaultHasher, std::hash::DefaultHasher);
    impl Hasher for Twin {
        fn write(&mut self, bytes: &[u8]) {
            self.0.write(bytes);
            self.1.write(bytes);
        }
        fn finish(&self) -> u64 {
            self.0.finish()
        }
    }
    let mut salted = std::hash::DefaultHasher::new();
    salted.write_u64(0x5eed);
    let mut twin = Twin(std::hash::DefaultHasher::new(), salted);
    state.hash(&mut twin);
    u128::from(twin.0.finish()) << 64 | u128::from(twin.1.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(budget: usize) -> Report<u8> {
        explore(
            &[0u8],
            |_| std::vec![1, 2],
            |state, step| (state + step) % 10,
            |state| if state % 2 == 0 { "even" } else { "odd" },
            &[("below seven", |state| *state < 7)],
            &[("never wraps", |before, _, after| after > before)],
            budget,
        )
    }

    #[test]
    fn explore_reaches_every_state_and_names_the_shortest_path_to_each_broken_rule() {
        let report = count(100);
        assert!(report.closed);
        assert_eq!(report.states, 10);
        assert_eq!(report.reached, ["even", "odd"]);
        let path = |rule| {
            let broken = report
                .broken
                .iter()
                .find(|broken| broken.rule == rule)
                .unwrap();
            (broken.init_index, broken.path.clone())
        };
        assert_eq!(path("below seven"), (0, std::vec![1, 2, 2, 2]));
        assert_eq!(path("never wraps"), (0, std::vec![2, 2, 2, 2, 2]));
    }

    #[test]
    fn a_broken_rule_names_its_init_even_past_a_duplicate_init() {
        let report = explore(
            &[0u8, 0, 5],
            |_| std::vec::Vec::<u8>::new(),
            |state, _| *state,
            |_| "any",
            &[("not five", |state| *state != 5)],
            &[],
            100,
        );
        assert!(report.closed);
        assert_eq!(report.states, 2);
        assert_eq!(report.broken[0].init_index, 2);
        assert!(report.broken[0].path.is_empty());
    }

    #[test]
    fn explore_past_its_budget_is_not_closed() {
        let report = count(5);
        assert!(!report.closed);
        assert!(report.states > 5);
    }
}
