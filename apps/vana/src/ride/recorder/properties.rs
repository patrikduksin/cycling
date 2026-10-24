//! Property tests of the internal journal under random rides and power loss.
//!
//! Each case rides a random sequence of start, samples, pause, resume and
//! finish requests at random service times, and cuts the power at a random
//! programmed byte: the interrupted byte keeps only some of its cleared bits,
//! as NOR flash does, and every later access fails until reboot. After every
//! boot the scan must keep each acknowledged record, count every occupied
//! slot, and recover one consistent ride.

use super::*;
use crate::ride::journal::{Internal, InternalState};
use crate::ride::log::{
    SECTOR_SIZE, SLOT_SIZE, SLOTS, Sector, Slot, commit_word, decode, encode, erased,
};
use crate::ride::storage::RideStorage;
use crate::testing::Rng;
use std::vec;
use std::vec::Vec;

/// NOR flash that loses power at one programmed byte.
struct Nor {
    bytes: Vec<u8>,
    /// Bytes programmed so far, across reboots.
    programmed: u64,
    /// Cut power while programming byte number `.0`, leaving the bits of
    /// `.1` still set in it.
    cut: Option<(u64, u8)>,
    off: bool,
    /// Slots before this one were programmed; later ones are still erased.
    used: usize,
}

impl Nor {
    fn program(&mut self, offset: usize, data: &[u8]) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        self.used = self.used.max(offset / SLOT_SIZE + 1);
        for (stored, &byte) in self.bytes[offset..offset + data.len()].iter_mut().zip(data) {
            if let Some((at, partial)) = self.cut
                && at == self.programmed
            {
                *stored &= byte | partial;
                self.cut = None;
                self.off = true;
                return Err(());
            }
            *stored &= byte;
            self.programmed += 1;
        }
        Ok(())
    }

    fn slot(&self, index: usize) -> Slot {
        let mut slot = Slot::default();
        slot.0
            .copy_from_slice(&self.bytes[index * SLOT_SIZE..][..SLOT_SIZE]);
        slot
    }
}

impl RideStorage for Nor {
    type Error = ();
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        output
            .0
            .copy_from_slice(&self.bytes[sector * SECTOR_SIZE..][..SECTOR_SIZE]);
        Ok(())
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        *output = self.slot(slot);
        Ok(())
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        self.bytes[sector * SECTOR_SIZE..][..SECTOR_SIZE].fill(0xff);
        Ok(())
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
        self.program(slot * SLOT_SIZE, &data.0)
    }
    fn ride_commit_slot(
        &mut self,
        slot: usize,
        commit: &crate::ride::log::Commit,
    ) -> Result<(), ()> {
        self.program((slot + 1) * SLOT_SIZE - 4, &commit.0)
    }
}

/// The internal journal, noting which appends it acknowledged.
struct Probe {
    nor: Nor,
    state: InternalState,
    acknowledged: Vec<(usize, Entry)>,
    /// The append in progress when the power was cut.
    attempt: Option<(usize, Entry)>,
}

impl Journal for Probe {
    fn slots(&self) -> usize {
        SLOTS
    }
    fn scan(&mut self) -> Scan {
        Internal::new(&mut self.state, &mut self.nor).scan()
    }
    fn initialize(&mut self, step: usize, seed: u64) -> Progress {
        Internal::new(&mut self.state, &mut self.nor).initialize(step, seed)
    }
    fn clear(&mut self, step: usize, seed: u64) -> Progress {
        Internal::new(&mut self.state, &mut self.nor).clear(step, seed)
    }
    fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Option<bool> {
        self.attempt = Some((index, *entry));
        let appended = Internal::new(&mut self.state, &mut self.nor).append(index, entry, terminal);
        if appended.is_some() {
            self.acknowledged.push((index, *entry));
            self.attempt = None;
        }
        appended
    }
    fn checkpoint(&mut self, catalog: &Catalog) -> bool {
        Internal::new(&mut self.state, &mut self.nor).checkpoint(catalog)
    }
    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Option<bool> {
        Internal::new(&mut self.state, &mut self.nor).read_slot(index, output)
    }
}

const fn terminal(kind: Kind) -> bool {
    matches!(kind, Kind::Finish | Kind::Recovered | Kind::Full)
}

struct Rig {
    rng: Rng,
    recorder: Recorder,
    probe: Probe,
    now: u64,
    /// Acknowledged records before this boot.
    boot_from: usize,
    seed: u64,
}

impl Rig {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            recorder: Recorder::default(),
            probe: Probe {
                nor: Nor {
                    bytes: vec![0xff; crate::ride::log::REGION_SIZE],
                    programmed: 0,
                    cut: None,
                    off: false,
                    used: 0,
                },
                state: InternalState::default(),
                acknowledged: Vec::new(),
                attempt: None,
            },
            now: 0,
            boot_from: 0,
            seed,
        }
    }

    fn service(&mut self) {
        let sample = self.rng.sample();
        let now = self.now;
        self.recorder.service(&mut self.probe, now, sample, &|| now);
    }

    /// A finished ride, then occupied invalid slots up to `free` slots
    /// from the end, so a few minutes of riding fill the journal.
    fn nearly_full(&mut self, free: usize) {
        let entries = [
            Entry::event(Kind::Start, Source::Live, 1, 0, 0),
            Entry::event(Kind::Finish, Source::Live, 1, 1, 0),
        ];
        for (index, entry) in entries.iter().enumerate() {
            let mut slot = encode(entry);
            slot.0[SLOT_SIZE - 4..].copy_from_slice(&commit_word().0);
            self.probe.nor.bytes[index * SLOT_SIZE..][..SLOT_SIZE].copy_from_slice(&slot.0);
            self.probe.acknowledged.push((index, *entry));
        }
        self.probe.nor.bytes[2 * SLOT_SIZE..(SLOTS - free) * SLOT_SIZE].fill(0);
        self.probe.nor.used = SLOTS - free;
    }

    fn boot(&mut self) {
        let seed = self.seed;
        // What the recorder counted as written is exactly what it appended.
        let written: u32 = self.probe.acknowledged[self.boot_from..]
            .iter()
            .filter(|(_, entry)| entry.kind == Kind::Samples)
            .map(|(_, entry)| u32::from(entry.count))
            .sum();
        assert_eq!(self.recorder.written_samples(), written, "seed {seed}");
        self.recorder = Recorder::default();
        self.probe.state = InternalState::default();
        self.probe.nor.off = false;
        self.probe.nor.cut = None;
        self.boot_from = self.probe.acknowledged.len();
        self.now = 0;
        let mut steps = 0;
        while self.recorder.status() == Status::Scanning {
            self.service();
            steps += 1;
            assert!(
                steps <= crate::ride::log::SECTORS,
                "seed {seed}: scan runs on"
            );
        }
        self.check();
        self.probe.attempt = None;
        if self.recorder.status() == Status::NeedsInit {
            assert!(self.recorder.initialize(1));
            while self.recorder.status() == Status::Formatting {
                self.service();
            }
            assert!(self.recorder.take_result().unwrap().ok, "seed {seed}");
        }
    }

    fn check(&self) {
        let seed = self.seed;
        let nor = &self.probe.nor;
        let catalog = *self.recorder.catalog();
        for (index, entry) in &self.probe.acknowledged {
            assert_eq!(
                decode(&nor.slot(*index)),
                Some(*entry),
                "seed {seed}: slot {index}"
            );
        }
        // Every occupied slot counts, valid or not; none is free space.
        let occupied: Vec<usize> = (0..nor.used)
            .filter(|&index| !erased(&nor.bytes[index * SLOT_SIZE..][..SLOT_SIZE]))
            .collect();
        let entries: Vec<(usize, Entry)> = occupied
            .iter()
            .filter_map(|&index| decode(&nor.slot(index)).map(|entry| (index, entry)))
            .collect();
        assert_eq!(
            catalog.next_slot,
            occupied.last().map_or(0, |last| last + 1),
            "seed {seed}"
        );
        assert_eq!(
            usize::from(catalog.valid_slots),
            entries.len(),
            "seed {seed}"
        );
        assert_eq!(
            usize::from(catalog.invalid_slots),
            occupied.len() - entries.len(),
            "seed {seed}"
        );
        let status = self.recorder.status();
        // Power cut during the first START of an empty journal more than
        // once leaves several torn slots, which the scan deliberately treats
        // as unknown data (see `Catalog::unknown_data`): no ride is lost.
        let torn_only = entries.is_empty()
            && occupied.iter().all(|&index| {
                crate::ride::log::torn_append(&nor.bytes[index * SLOT_SIZE..][..SLOT_SIZE])
            });
        assert!(
            matches!(status, Status::Ready | Status::Recovered | Status::Full)
                || (status == Status::NeedsInit && torn_only && occupied.len() > 1),
            "seed {seed}: {status:?} at slot {}",
            catalog.next_slot
        );
        // The interrupted append landed whole, or left one invalid slot.
        if let Some((index, entry)) = self.probe.attempt {
            let found = decode(&nor.slot(index));
            assert!(found.is_none() || found == Some(entry), "seed {seed}");
        }
        // Rides are numbered upwards, their records chained in order with
        // active time that never goes backwards.
        let mut open: Option<Entry> = None;
        let mut last_ride = 0;
        let mut completed = 0;
        for (index, entry) in &entries {
            if entry.kind == Kind::Start {
                assert!(entry.ride_id > last_ride, "seed {seed}: slot {index}");
                assert!(
                    open.is_none(),
                    "seed {seed}: slot {index} starts over an open ride"
                );
                last_ride = entry.ride_id;
                open = Some(*entry);
                continue;
            }
            let previous = open.expect("records belong to a started ride");
            assert_eq!(
                (entry.ride_id, entry.sequence),
                (previous.ride_id, previous.sequence + 1),
                "seed {seed}: slot {index}"
            );
            assert!(
                entry.active_ms >= previous.active_ms,
                "seed {seed}: slot {index}"
            );
            if entry.kind == Kind::Samples {
                let times = entry.samples[..usize::from(entry.count)]
                    .iter()
                    .map(|sample| sample.active_ms);
                assert!(
                    times.clone().zip(times.skip(1)).all(|(a, b)| a <= b),
                    "seed {seed}: slot {index}"
                );
            }
            if terminal(entry.kind) {
                completed += 1;
                open = None;
            } else {
                open = Some(*entry);
            }
        }
        assert_eq!(catalog.completed, completed, "seed {seed}");
        // The open ride resumes at its last committed record.
        let recovery = catalog
            .recovery
            .map(|r| (r.ride_id, r.next_sequence, r.active_ms));
        let expected = open.map(|e| (e.ride_id, e.sequence + 1, e.active_ms));
        assert_eq!(recovery, expected, "seed {seed}");
        assert_eq!(
            status == Status::Recovered,
            recovery.is_some() && catalog.next_slot < SLOTS,
            "seed {seed}"
        );
    }

    fn step(&mut self) {
        match self.rng.below(24) {
            0..=5 => {
                let action =
                    self.rng
                        .pick(&[Action::Start, Action::Pause, Action::Resume, Action::Finish]);
                let source = self.rng.pick(&[Source::Live, Source::Demo]);
                let now = self.now;
                self.recorder.request(action, source, now, 1);
                for _ in 0..self.rng.range(0, 3) {
                    self.service();
                }
                self.recorder.take_result();
            }
            6..=17 => {
                for _ in 0..self.rng.range(1, 40) {
                    self.now += self.rng.pick(&[10, 30, 250, 999, 1_000, 1_001, 3_500]);
                    self.service();
                }
            }
            18..=20 => {
                if self.probe.nor.cut.is_none() {
                    let at = self.probe.nor.programmed + self.rng.below(3 * SLOT_SIZE as u64);
                    let random = self.rng.byte();
                    let partial = self.rng.pick(&[0, 0xff, random]);
                    self.probe.nor.cut = Some((at, partial));
                }
            }
            21 => self.boot(),
            _ => self.service(),
        }
        // Only power loss fails an append: no reading makes a record that
        // does not read back.
        assert!(
            self.probe.nor.off || self.recorder.status() != Status::Error,
            "seed {}: media error without a power cut",
            self.seed
        );
        if self.probe.nor.off {
            let seed = self.seed;
            assert_eq!(self.recorder.status(), Status::Error, "seed {seed}");
            let programmed = self.probe.nor.programmed;
            self.probe.nor.off = false;
            for _ in 0..3 {
                self.now += 1_000;
                self.service();
            }
            assert_eq!(self.probe.nor.programmed, programmed, "seed {seed}");
            self.boot();
        }
    }
}

/// Cases per run; each rides about a hundred random events.
const CASES: u64 = 300;

#[test]
fn random_rides_with_power_cut_at_any_byte_keep_every_acknowledged_record() {
    for seed in 0..CASES {
        let mut rig = Rig::new(seed);
        if rig.rng.one_in(6) {
            let free = rig.rng.range(3, 24) as usize;
            rig.nearly_full(free);
        }
        rig.boot();
        for _ in 0..100 {
            rig.step();
        }
        rig.boot();
        rig.service();
        let status = rig.recorder.status();
        assert!(
            matches!(status, Status::Ready | Status::Recovered | Status::Full),
            "seed {seed}: {status:?}"
        );
        // Only a journal without room keeps a ride open.
        assert!(
            rig.recorder.catalog().recovery.is_none() || status == Status::Full,
            "seed {seed}"
        );
    }
}

#[test]
fn damaged_slots_anywhere_are_counted_and_never_written_over() {
    for seed in 0..CASES {
        let mut rig = Rig::new(seed);
        rig.boot();
        for _ in 0..40 {
            rig.step();
        }
        rig.boot();
        rig.service();
        // Bit rot, stray programming and foreign bytes in random slots,
        // including erased ones past the end of the journal.
        let used = rig.recorder.next_slot();
        for _ in 0..rig.rng.range(1, 6) {
            let index = rig.rng.below((used as u64 + 8).min(SLOTS as u64)) as usize;
            let at = index * SLOT_SIZE + rig.rng.below(SLOT_SIZE as u64) as usize;
            let byte = rig.rng.byte();
            rig.probe.nor.bytes[at] &= byte;
        }
        let before = rig.probe.nor.bytes.clone();
        rig.probe
            .acknowledged
            .retain(|(index, entry)| decode(&rig.probe.nor.slot(*index)) == Some(*entry));
        rig.probe.attempt = None;
        rig.recorder = Recorder::default();
        rig.probe.state = InternalState::default();
        let mut steps = 0;
        while rig.recorder.status() == Status::Scanning {
            rig.service();
            steps += 1;
            assert!(steps <= crate::ride::log::SECTORS);
        }
        let catalog = *rig.recorder.catalog();
        let occupied = (0..SLOTS)
            .rev()
            .find(|&index| !erased(&before[index * SLOT_SIZE..][..SLOT_SIZE]))
            .map_or(0, |last| last + 1);
        assert_eq!(catalog.next_slot, occupied, "seed {seed}");
        for _ in 0..3 {
            rig.service();
        }
        rig.recorder.request(Action::Start, Source::Live, 0, 1);
        for _ in 0..3 {
            rig.service();
        }
        for (index, (old, new)) in before
            .chunks(SLOT_SIZE)
            .zip(rig.probe.nor.bytes.chunks(SLOT_SIZE))
            .enumerate()
        {
            if old != new {
                assert!(
                    erased(old) && index >= occupied,
                    "seed {seed}: slot {index}"
                );
            }
        }
    }
}
