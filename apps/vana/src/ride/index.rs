//! The rides a journal holds, found by reading its slots once, for lists
//! that need every ride rather than the four summaries a catalog keeps.
//!
//! An index is fed slots in order, one at a time, so a caller can spread a
//! large bulk log over many service steps. It keeps the newest `KEPT` rides.

use crate::ride::log::{self, Kind, MAX_STEP_MS, Slot, Source};

/// Rides an index keeps; older ones drop out first.
pub const KEPT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Saved,
    Recovered,
    Full,
    /// The next ride started, or the journal ended, without a terminal record.
    Interrupted,
}

impl End {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Recovered => "recovered",
            Self::Full => "full",
            Self::Interrupted => "interrupted",
        }
    }
}

/// One ride as an index found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activity {
    pub ride_id: u32,
    pub source: Source,
    pub start_slot: usize,
    /// UTC of the ride's start, from its first dated sample.
    pub start_utc_ms: Option<u64>,
    pub active_ms: u64,
    /// Wheel-speed distance; none without a wheel sensor.
    pub distance_mm: Option<u64>,
    pub samples: u32,
    pub with_position: u32,
    pub with_power: u32,
    pub with_heart: u32,
    pub end: Option<End>,
    previous_active_ms: Option<u64>,
    dated: bool,
}

impl Activity {
    fn new(ride_id: u32, source: Source, start_slot: usize) -> Self {
        Self {
            ride_id,
            source,
            start_slot,
            start_utc_ms: None,
            active_ms: 0,
            distance_mm: None,
            samples: 0,
            with_position: 0,
            with_power: 0,
            with_heart: 0,
            end: None,
            previous_active_ms: Some(0),
            dated: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Index {
    next: usize,
    rides: heapless::Deque<Activity, KEPT>,
    open: Option<Activity>,
    /// Rides seen in total, including those dropped.
    seen: u32,
    done: bool,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Index {
    pub const fn new() -> Self {
        Self {
            next: 0,
            rides: heapless::Deque::new(),
            open: None,
            seen: 0,
            done: false,
        }
    }

    /// Start over in place, without a large temporary.
    pub fn reset(&mut self) {
        self.next = 0;
        self.rides.clear();
        self.open = None;
        self.seen = 0;
        self.done = false;
    }

    /// The next slot to feed.
    pub fn next_slot(&self) -> usize {
        self.next
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn seen(&self) -> u32 {
        self.seen
    }

    /// Feed slot `self.next_slot()`; `None` for a slot the medium could not
    /// read as a container, which counts as invalid.
    pub fn accept(&mut self, slot: Option<&Slot>) {
        self.next += 1;
        let Some(entry) = slot.and_then(log::decode) else {
            return;
        };
        if entry.kind == Kind::Start {
            self.close(End::Interrupted);
            self.open = Some(Activity::new(entry.ride_id, entry.source, self.next - 1));
            self.seen += 1;
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if entry.ride_id != open.ride_id || entry.source != open.source {
            return;
        }
        open.active_ms = open.active_ms.max(entry.active_ms);
        match entry.kind {
            Kind::Samples => {
                for sample in &entry.samples[..usize::from(entry.count)] {
                    if !open.dated
                        && let Some(offset) =
                            sample.utc_ms.and_then(|u| u.checked_sub(sample.active_ms))
                    {
                        open.start_utc_ms = Some(offset);
                        open.dated = true;
                    }
                    let elapsed = sample
                        .active_ms
                        .saturating_sub(open.previous_active_ms.unwrap_or(sample.active_ms))
                        .min(MAX_STEP_MS);
                    open.previous_active_ms = Some(sample.active_ms);
                    if let Some(speed) = sample.speed_mm_s {
                        open.distance_mm =
                            Some(open.distance_mm.unwrap_or(0) + u64::from(speed) * elapsed / 1000);
                    }
                    open.samples += 1;
                    open.with_position += u32::from(sample.location_e7.is_some());
                    open.with_power += u32::from(sample.power_watts.is_some());
                    open.with_heart += u32::from(sample.heart_bpm.is_some());
                }
            }
            Kind::Resume => open.previous_active_ms = Some(entry.active_ms),
            Kind::Pause | Kind::Start => {}
            Kind::Finish => self.close(End::Saved),
            Kind::Recovered => self.close(End::Recovered),
            Kind::Full => self.close(End::Full),
        }
    }

    fn close(&mut self, end: End) {
        if let Some(mut ride) = self.open.take() {
            ride.end = Some(end);
            if self.rides.is_full() {
                self.rides.pop_front();
            }
            let _ = self.rides.push_back(ride);
        }
    }

    /// Every slot up to the journal's end was fed.
    pub fn finish(&mut self) {
        // A ride still open at the end is the ride being recorded, or one
        // recovery has not closed yet: list it as interrupted.
        self.close(End::Interrupted);
        self.done = true;
    }

    /// Rides found, oldest first.
    pub fn rides(&self) -> impl DoubleEndedIterator<Item = &Activity> {
        self.rides.iter()
    }

    pub fn find(&self, start_slot: usize, ride_id: u32) -> Option<&Activity> {
        self.rides
            .iter()
            .find(|ride| ride.start_slot == start_slot && ride.ride_id == ride_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::{Entry, Sample, commit_word, encode};

    fn slot(entry: &Entry) -> Slot {
        let mut slot = encode(entry);
        slot.0[252..].copy_from_slice(&commit_word().0);
        slot
    }

    fn sample(active_ms: u64) -> Sample {
        Sample {
            active_ms,
            utc_ms: Some(1_789_000_000_000 + active_ms),
            speed_mm_s: Some(10_000),
            power_watts: Some(180),
            ..Sample::default()
        }
    }

    #[test]
    fn finds_rides_their_start_distance_and_end() {
        let slots = [
            slot(&Entry::event(Kind::Start, Source::Live, 1, 0, 0)),
            slot(&Entry::batch(Source::Live, 1, 1, &[sample(1_000), sample(2_000)]).unwrap()),
            slot(&Entry::event(Kind::Pause, Source::Live, 1, 2, 2_000)),
            slot(&Entry::event(Kind::Resume, Source::Live, 1, 3, 2_000)),
            slot(&Entry::batch(Source::Live, 1, 4, &[sample(3_000)]).unwrap()),
            slot(&Entry::event(Kind::Finish, Source::Live, 1, 5, 3_000)),
            Slot([0x11; 256]),
            slot(&Entry::event(Kind::Start, Source::Live, 2, 0, 0)),
            slot(&Entry::batch(Source::Live, 2, 1, &[sample(1_000)]).unwrap()),
            slot(&Entry::event(Kind::Start, Source::Live, 3, 0, 0)),
        ];
        let mut index = Index::default();
        for slot in &slots {
            index.accept(Some(slot));
        }
        index.accept(None);
        index.finish();
        let rides: std::vec::Vec<_> = index.rides().copied().collect();
        assert_eq!(rides.len(), 3);
        assert_eq!(rides[0].start_slot, 0);
        assert_eq!(rides[0].end, Some(End::Saved));
        assert_eq!(rides[0].distance_mm, Some(30_000));
        assert_eq!(rides[0].samples, 3);
        assert_eq!(rides[0].with_power, 3);
        assert_eq!(rides[0].start_utc_ms, Some(1_789_000_000_000));
        assert_eq!(rides[0].active_ms, 3_000);
        assert_eq!(rides[1].end, Some(End::Interrupted));
        assert_eq!(rides[2].start_slot, 9);
        assert!(index.find(7, 2).is_some());
        assert!(index.find(7, 1).is_none());
        assert_eq!(index.next_slot(), slots.len() + 1);
    }

    #[test]
    fn a_journal_gap_counts_no_more_than_the_odometer_step() {
        let fast = |active_ms| Sample {
            speed_mm_s: Some(u32::MAX),
            ..sample(active_ms)
        };
        let mut index = Index::default();
        for entry in [
            Entry::event(Kind::Start, Source::Live, 1, 0, 0),
            Entry::batch(Source::Live, 1, 1, &[fast(1_000), fast(1 << 40)]).unwrap(),
            Entry::event(Kind::Finish, Source::Live, 1, 2, 1 << 40),
        ] {
            index.accept(Some(&slot(&entry)));
        }
        index.finish();
        let ride = index.rides().next().unwrap();
        assert_eq!(ride.distance_mm, Some(u64::from(u32::MAX) * 4));
    }

    #[test]
    fn keeps_the_newest_rides() {
        let mut index = Index::default();
        for ride in 0..(KEPT as u32 + 5) {
            index.accept(Some(&slot(&Entry::event(
                Kind::Start,
                Source::Live,
                ride + 1,
                0,
                0,
            ))));
            index.accept(Some(&slot(&Entry::event(
                Kind::Finish,
                Source::Live,
                ride + 1,
                1,
                0,
            ))));
        }
        index.finish();
        assert_eq!(index.rides().count(), KEPT);
        assert_eq!(index.rides().next().unwrap().ride_id, 6);
        assert_eq!(index.seen(), KEPT as u32 + 5);
    }
}
