//! Read a saved ride back from its journal and add it up again, for ride
//! history: the same totals, zone times and best efforts the summary shows
//! after a live ride.
//!
//! A journal holds rides one after another, so slot ride ids never fall.
//! The replay finds the ride's first slot by bisection, then reads its slots
//! in order until the ride ends. Each `step` reads at most a given number of
//! slots, so a long ride never holds up a frame. Reading is read-only.
//!
//! What the journal does not keep stays unknown: laps and climbs live only
//! in RAM during a ride, and altitude is not recorded, so ascent comes from
//! the recorded gradient over the distance ridden.
//!
//! The replay itself is a few words of position; it adds up into `Sums` the
//! caller lends it, so the device keeps one set of ride sums, not two.
use crate::ride::log::{Entry, Kind, Sample};
use crate::ride::totals::{Efforts, Totals};
use crate::ride::zones::{TrainingProfile, ZoneTime};

/// The longest gap between two samples that still counts as ridden time,
/// as the odometer caps its steps.
const MAX_STEP_MS: u64 = 3000;
/// Probes past an unreadable slot before bisection gives up on it.
const PROBES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The first slot of the ride lies in `lo..hi`.
    Find {
        lo: usize,
        hi: usize,
    },
    /// Reading the ride from slot `next`.
    Read {
        next: usize,
    },
    Done,
}

/// How far a replay has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Still searching for or reading the ride.
    Reading,
    /// The whole ride is added up.
    Done,
    /// The ride was not found in the journal.
    Missing,
}

/// What a replay adds up into.
pub struct Sums<'a> {
    pub totals: &'a mut Totals,
    pub zone_time: &'a mut ZoneTime,
    pub efforts: &'a mut Efforts,
}

impl Sums<'_> {
    /// Start from nothing, as a new ride does.
    pub fn clear(&mut self) {
        *self.totals = Totals::default();
        *self.zone_time = ZoneTime::default();
        *self.efforts = Efforts::default();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Replay {
    ride_id: u32,
    upper: usize,
    phase: Phase,
    found: bool,
    last_ms: Option<u64>,
    rise_mm: u64,
    /// Recorded time, from the ride's last record.
    pub active_ms: u64,
}

impl Replay {
    /// Replay ride `ride_id` from a journal whose slots below `upper` are
    /// written.
    pub fn new(ride_id: u32, upper: usize) -> Self {
        Self {
            ride_id,
            upper,
            phase: Phase::Find { lo: 0, hi: upper },
            found: false,
            last_ms: None,
            rise_mm: 0,
            active_ms: 0,
        }
    }

    pub fn ride_id(&self) -> u32 {
        self.ride_id
    }

    /// Stop: the journal cannot be read. The ride counts as missing.
    pub fn abandon(&mut self) {
        self.phase = Phase::Done;
        self.found = false;
    }

    pub fn progress(&self) -> Progress {
        match self.phase {
            Phase::Done if self.found => Progress::Done,
            Phase::Done => Progress::Missing,
            _ => Progress::Reading,
        }
    }

    /// Read up to `budget` slots through `read`, which returns the decoded
    /// entry of a slot, or `None` for one that cannot be read or decoded,
    /// adding the ride's samples into `sums`.
    pub fn step(
        &mut self,
        budget: usize,
        profile: &TrainingProfile,
        sums: &mut Sums<'_>,
        mut read: impl FnMut(usize) -> Option<Entry>,
    ) -> Progress {
        let mut left = budget;
        while left > 0 {
            match self.phase {
                Phase::Done => break,
                Phase::Find { lo, hi } if lo >= hi => {
                    self.phase = Phase::Read { next: lo };
                }
                Phase::Find { lo, hi } => {
                    let mid = lo + (hi - lo) / 2;
                    // An unreadable slot: the next readable one decides.
                    let mut probe = mid;
                    let mut entry = None;
                    while probe < hi && probe < mid + PROBES && left > 0 {
                        left -= 1;
                        entry = read(probe);
                        if entry.is_some() {
                            break;
                        }
                        probe += 1;
                    }
                    if left == 0 && entry.is_none() && probe < hi && probe < mid + PROBES {
                        // Out of budget mid-probe: try this bisection again.
                        break;
                    }
                    self.phase = match entry {
                        Some(entry) if entry.ride_id < self.ride_id => {
                            Phase::Find { lo: probe + 1, hi }
                        }
                        _ => Phase::Find { lo, hi: mid },
                    };
                }
                Phase::Read { next } if next >= self.upper => self.phase = Phase::Done,
                Phase::Read { next } => {
                    left -= 1;
                    self.phase = Phase::Read { next: next + 1 };
                    let Some(entry) = read(next) else {
                        continue;
                    };
                    if entry.ride_id != self.ride_id {
                        // Before the ride's first readable slot nothing is
                        // lost; past the ride, it has ended.
                        if entry.ride_id > self.ride_id {
                            self.phase = Phase::Done;
                        }
                        continue;
                    }
                    self.found = true;
                    self.active_ms = self.active_ms.max(entry.active_ms);
                    match entry.kind {
                        Kind::Samples => {
                            for sample in &entry.samples[..usize::from(entry.count)] {
                                self.add(sample, profile, sums);
                            }
                        }
                        Kind::Finish | Kind::Recovered | Kind::Full => self.phase = Phase::Done,
                        Kind::Start | Kind::Pause | Kind::Resume => {}
                    }
                }
            }
        }
        self.progress()
    }

    fn add(&mut self, sample: &Sample, profile: &TrainingProfile, sums: &mut Sums<'_>) {
        // Active time runs only while recording, so pauses leave no gap.
        let elapsed = match self.last_ms {
            Some(last) => sample.active_ms.saturating_sub(last),
            None => sample.active_ms.min(1000),
        }
        .min(MAX_STEP_MS);
        self.last_ms = Some(sample.active_ms);
        self.active_ms = self.active_ms.max(sample.active_ms);
        if elapsed == 0 {
            return;
        }
        let speed = sample.speed_mm_s.or(sample.demo_speed_mm_s).unwrap_or(0);
        let distance_mm = u64::from(speed) * elapsed / 1000;
        sums.totals.add(elapsed, distance_mm, sample, None);
        sums.zone_time.add(elapsed, sample, profile);
        if let Some(watts) = sample.power_watts {
            sums.efforts.add_ms(watts, elapsed);
        }
        if let Some(grade) = sample.gradient_tenths.filter(|grade| *grade > 0) {
            // Tenths of a percent: rise = distance × grade / 1000.
            self.rise_mm += distance_mm * grade as u64 / 1000;
        }
    }

    /// Ascent from the recorded gradient, in decimetres.
    pub fn ascent_dm(&self) -> u32 {
        (self.rise_mm / 100).min(u64::from(u32::MAX)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Source;
    use std::vec::Vec;

    #[derive(Default)]
    struct Owned {
        totals: Totals,
        zone_time: ZoneTime,
        efforts: Efforts,
    }

    impl Owned {
        fn sums(&mut self) -> Sums<'_> {
            Sums {
                totals: &mut self.totals,
                zone_time: &mut self.zone_time,
                efforts: &mut self.efforts,
            }
        }
    }

    fn sample(active_ms: u64, watts: u16) -> Sample {
        Sample {
            active_ms,
            utc_ms: None,
            location_e7: None,
            demo_speed_mm_s: None,
            heart_bpm: Some(140),
            cadence_tenths: Some(900),
            battery_percent: None,
            speed_mm_s: Some(10_000),
            power_watts: Some(watts),
            gradient_tenths: Some(50),
            altitude: None,
        }
    }

    /// A journal of `rides`, each `seconds` long at its power, with START,
    /// four-sample batches and FINISH, rides one after another.
    fn journal(rides: &[(u32, u64, u16)]) -> Vec<Entry> {
        let mut slots = Vec::new();
        for &(ride_id, seconds, watts) in rides {
            let mut sequence = 0;
            let mut next = |slots: &mut Vec<Entry>, mut entry: Entry| {
                entry.sequence = sequence;
                sequence += 1;
                slots.push(entry);
            };
            next(
                &mut slots,
                Entry::event(Kind::Start, Source::Live, ride_id, 0, 0),
            );
            let samples: Vec<_> = (1..=seconds).map(|s| sample(s * 1000, watts)).collect();
            for chunk in samples.chunks(4) {
                let batch = Entry::batch(Source::Live, ride_id, 0, chunk).unwrap();
                next(&mut slots, batch);
            }
            let finish = Entry::event(Kind::Finish, Source::Live, ride_id, 0, seconds * 1000);
            next(&mut slots, finish);
        }
        slots
    }

    fn replay(slots: &[Entry], ride_id: u32, budget: usize) -> (Replay, Owned, usize) {
        let mut replay = Replay::new(ride_id, slots.len());
        let mut owned = Owned::default();
        let (mut reads, mut steps) = (0, 0);
        while replay.progress() == Progress::Reading {
            steps += 1;
            assert!(steps < 10_000, "replay never ends");
            replay.step(
                budget,
                &TrainingProfile::default(),
                &mut owned.sums(),
                |index| {
                    reads += 1;
                    slots.get(index).copied()
                },
            );
        }
        (replay, owned, reads)
    }

    #[test]
    fn a_ride_adds_up_as_it_did_live() {
        let slots = journal(&[(1, 600, 100), (2, 1200, 250), (3, 300, 400)]);
        let (ride, sums, reads) = replay(&slots, 2, 16);
        assert_eq!(ride.progress(), Progress::Done);
        assert_eq!(ride.active_ms, 1_200_000);
        // 1200 s at 10 m/s.
        assert_eq!(sums.totals.distance_mm(), 12_000_000);
        assert_eq!(sums.totals.avg_power(), Some(250));
        assert_eq!(sums.totals.avg_heart_bpm(), Some(140));
        assert_eq!(sums.efforts.best(), [Some(250); 4]);
        // 5% over 12 km climbs 600 m.
        assert_eq!(ride.ascent_dm(), 6000);
        // Bisection, then the ride's own slots and the next ride's first.
        let own = 1200 / 4 + 2;
        assert!(reads <= own + 16, "{reads} reads for {own} slots");
    }

    #[test]
    fn the_budget_bounds_every_step() {
        let slots = journal(&[(5, 400, 200), (6, 400, 300)]);
        let mut replay = Replay::new(6, slots.len());
        let mut owned = Owned::default();
        while replay.progress() == Progress::Reading {
            let mut reads = 0;
            replay.step(3, &TrainingProfile::default(), &mut owned.sums(), |index| {
                reads += 1;
                slots.get(index).copied()
            });
            assert!(reads <= 3);
        }
        assert_eq!(owned.totals.avg_power(), Some(300));
    }

    #[test]
    fn unreadable_slots_are_skipped_and_a_missing_ride_says_so() {
        let mut slots: Vec<Option<Entry>> = journal(&[(1, 200, 100), (2, 200, 200)])
            .into_iter()
            .map(Some)
            .collect();
        // A torn slot in the middle of the journal, inside ride 2.
        let torn = slots.len() - 10;
        slots[torn] = None;
        let read = |slots: &[Option<Entry>], id| {
            let mut replay = Replay::new(id, slots.len());
            let mut owned = Owned::default();
            while replay.progress() == Progress::Reading {
                replay.step(8, &TrainingProfile::default(), &mut owned.sums(), |i| {
                    slots.get(i).copied().flatten()
                });
            }
            (replay.progress(), owned.totals.avg_power())
        };
        assert_eq!(read(&slots, 2), (Progress::Done, Some(200)));
        assert_eq!(read(&slots, 7).0, Progress::Missing);
        assert_eq!(read(&slots, 1).1, Some(100));
    }
}
