use crate::ride::journal::{Append, Checkpoint, Journal, Progress, Read, Scan};
use crate::ride::log::{
    COMMIT_OFFSET, Catalog, Entry, Kind, SAMPLES_PER_BATCH, SLOT_SIZE, Sample, Scanner, Slot,
    commit_word, encode, erased,
};
use core::hash::{Hash, Hasher};
use std::sync::Arc;
use std::vec::Vec;

pub(super) const SLOTS: usize = 6;
pub(super) const LAST: usize = SLOTS - 1;
const CHECKPOINT_EVERY: u8 = 3;
const IMPOSSIBLE_BATTERY: u8 = 101;
pub(super) const SAMPLE: Sample = Sample {
    active_ms: 0,
    utc_ms: Some(1_700_000_000_000),
    location_e7: Some((475_000_000, 85_000_000)),
    heart_bpm: Some(140),
    cadence_tenths: Some(900),
    battery_percent: Some(IMPOSSIBLE_BATTERY),
    speed_mm_s: Some(8_000),
    power_watts: Some(200),
    gradient_tenths: None,
    altitude: None,
    lap: None,
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Volume {
    Uninitialized,
    Formatted,
    Uncertain,
    Foreign,
    Corrupt,
    Unsupported,
    Unavailable,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Medium {
    pub(super) volume: Volume,
    pub(super) slots: Arc<[[u8; SLOT_SIZE]; SLOTS]>,
    pub(super) checkpoint: Option<Catalog>,
    since_checkpoint: u8,
    pub(super) scan_required: bool,
    pub(super) tearable: Option<usize>,
    scan_begun: bool,
}

impl Hash for Medium {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            volume,
            slots: _,
            checkpoint,
            since_checkpoint,
            scan_required,
            tearable,
            scan_begun,
        } = self;
        (
            volume,
            self.written_slots(),
            checkpoint,
            since_checkpoint,
            scan_required,
            tearable,
            scan_begun,
        )
            .hash(state);
    }
}

impl Medium {
    pub(super) fn new(volume: Volume, records: &[Entry]) -> Self {
        let mut slots = [[0xff; SLOT_SIZE]; SLOTS];
        for (slot, entry) in slots.iter_mut().zip(records) {
            *slot = committed(entry);
        }
        Self {
            volume,
            slots: Arc::new(slots),
            checkpoint: matches!(volume, Volume::Formatted | Volume::Uncertain).then(empty),
            since_checkpoint: 0,
            scan_required: false,
            tearable: None,
            scan_begun: false,
        }
    }

    pub(super) fn head(&self) -> usize {
        self.slots
            .iter()
            .rposition(|slot| !erased(slot))
            .map_or(0, |last| last + 1)
    }

    fn written_slots(&self) -> &[[u8; SLOT_SIZE]] {
        &self.slots[..self.head()]
    }

    pub(super) fn overdue(&self) -> bool {
        self.since_checkpoint >= CHECKPOINT_EVERY
    }

    fn fold(&self) -> Catalog {
        let checkpoint = self.checkpoint.unwrap_or_else(empty);
        let mut scanner = Scanner::resume(&checkpoint);
        for (index, slot) in self.slots.iter().enumerate().skip(checkpoint.next_slot) {
            scanner.accept_slot(index, slot);
        }
        scanner.catalog()
    }

    fn format(&mut self) {
        *self = Self::new(Volume::Formatted, &[]);
    }

    pub(super) fn wipe(&mut self) {
        let volume = match self.volume {
            Volume::Foreign => Volume::Formatted,
            volume => volume,
        };
        *self = Self::new(volume, &[]);
    }

    pub(super) fn tear(&mut self) {
        if let Some(index) = self.tearable.take() {
            Arc::make_mut(&mut self.slots)[index][COMMIT_OFFSET..].fill(0xff);
        }
    }

    fn takes_first_scan_step(&mut self) -> bool {
        !core::mem::replace(&mut self.scan_begun, true)
    }

    pub(super) fn power_on(&mut self) {
        self.tearable = None;
        self.scan_begun = false;
    }
}

fn empty() -> Catalog {
    Catalog {
        next_ride_id: 1,
        ..Catalog::default()
    }
}

fn committed(entry: &Entry) -> [u8; SLOT_SIZE] {
    let mut slot = encode(entry);
    slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
    slot.0
}

fn ride(records: impl IntoIterator<Item = (Kind, usize)>) -> Vec<Entry> {
    records
        .into_iter()
        .enumerate()
        .map(|(sequence, (kind, samples))| {
            let sequence = sequence as u32;
            if kind == Kind::Samples {
                let batch: Vec<Sample> = (0..samples)
                    .map(|second| Sample {
                        active_ms: second as u64 * 1_000,
                        ..SAMPLE.without_impossible_battery()
                    })
                    .collect();
                Entry::batch(1, sequence, &batch).unwrap()
            } else {
                Entry::event(kind, 1, sequence, 0)
            }
        })
        .collect()
}

pub(super) fn media() -> [(&'static str, Medium); 11] {
    let batch = (Kind::Samples, SAMPLES_PER_BATCH);
    let closed = ride([(Kind::Start, 0), (Kind::Finish, 0)]);
    let interrupted = ride([(Kind::Start, 0), batch]);
    let filled = ride([
        (Kind::Start, 0),
        batch,
        batch,
        batch,
        batch,
        (Kind::Finish, 0),
    ]);
    let mut filled_open = Medium::new(Volume::Formatted, &filled);
    filled_open.tearable = Some(LAST);
    filled_open.tear();
    let mut foreign_record = Medium::new(Volume::Formatted, &[]);
    Arc::make_mut(&mut foreign_record.slots)[0] = [0; SLOT_SIZE];
    [
        ("uninitialized", Medium::new(Volume::Uninitialized, &[])),
        ("a closed ride", Medium::new(Volume::Formatted, &closed)),
        (
            "an interrupted ride",
            Medium::new(Volume::Formatted, &interrupted),
        ),
        ("full", Medium::new(Volume::Formatted, &filled)),
        ("full with an interrupted ride", filled_open),
        ("a foreign record", foreign_record),
        ("a foreign volume", Medium::new(Volume::Foreign, &[])),
        ("unsupported", Medium::new(Volume::Unsupported, &[])),
        ("unavailable", Medium::new(Volume::Unavailable, &[])),
        ("uncertain", Medium::new(Volume::Uncertain, &closed)),
        ("corrupt", Medium::new(Volume::Corrupt, &[])),
    ]
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Answer {
    Works,
    Fails,
    WritesThenFails,
}

pub(super) struct Answering<'a> {
    pub(super) medium: &'a mut Medium,
    pub(super) answer: Answer,
    pub(super) operations: u8,
}

impl Answering<'_> {
    fn rewrite_volume(&mut self) -> Progress {
        self.operations += 1;
        if matches!(
            self.medium.volume,
            Volume::Unsupported | Volume::Unavailable
        ) {
            return Progress::Failed;
        }
        match self.answer {
            Answer::Works => {
                self.medium.format();
                Progress::Done
            }
            Answer::Fails => Progress::Failed,
            Answer::WritesThenFails => {
                self.medium.format();
                Progress::Failed
            }
        }
    }
}

impl Journal for Answering<'_> {
    fn slots(&self) -> usize {
        match self.medium.volume {
            Volume::Formatted | Volume::Uncertain => SLOTS,
            Volume::Uninitialized
            | Volume::Foreign
            | Volume::Corrupt
            | Volume::Unsupported
            | Volume::Unavailable => 0,
        }
    }

    fn scan(&mut self) -> Scan {
        self.operations += 1;
        let medium = &mut *self.medium;
        match medium.volume {
            Volume::Unsupported => return Scan::Unsupported,
            Volume::Unavailable => return Scan::Unavailable,
            _ if self.answer == Answer::Fails => return Scan::Failed,
            _ => {}
        }
        if medium.takes_first_scan_step() {
            return Scan::More;
        }
        medium.scan_required = false;
        match medium.volume {
            Volume::Uninitialized => Scan::Uninitialized,
            Volume::Foreign => Scan::Foreign,
            Volume::Corrupt => Scan::Corrupt,
            Volume::Uncertain => Scan::Uncertain(medium.fold()),
            Volume::Unsupported | Volume::Unavailable => unreachable!("answered above"),
            Volume::Formatted => {
                let from = medium
                    .checkpoint
                    .map_or(0, |checkpoint| checkpoint.next_slot);
                medium.since_checkpoint = medium.head().saturating_sub(from) as u8;
                Scan::Done(medium.fold())
            }
        }
    }

    fn initialize(&mut self, _: usize, _: u64) -> Progress {
        self.rewrite_volume()
    }

    fn clear(&mut self, _: usize, _: u64) -> Progress {
        self.rewrite_volume()
    }

    fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Append {
        self.operations += 1;
        let medium = &mut *self.medium;
        if medium.volume != Volume::Formatted || index != medium.head() || index >= SLOTS {
            medium.scan_required = true;
            return Append::Uncertain;
        }
        if self.answer != Answer::Fails {
            Arc::make_mut(&mut medium.slots)[index] = committed(entry);
            medium.since_checkpoint += 1;
            medium.tearable = Some(index);
        }
        if self.answer != Answer::Works {
            medium.scan_required = true;
            return Append::Uncertain;
        }
        Append::Verified {
            checkpoint_due: terminal || medium.overdue(),
        }
    }

    fn checkpoint(&mut self, catalog: &Catalog) -> Checkpoint {
        self.operations += 1;
        let medium = &mut *self.medium;
        if medium.volume != Volume::Formatted {
            return Checkpoint::Failed;
        }
        if self.answer != Answer::Fails {
            medium.checkpoint = Some(*catalog);
            medium.since_checkpoint = 0;
            medium.tearable = None;
        }
        if self.answer == Answer::Works {
            Checkpoint::Written
        } else {
            Checkpoint::Failed
        }
    }

    fn checkpoint_overdue(&self) -> bool {
        self.medium.overdue()
    }

    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Read {
        output.0 = self.medium.slots[index];
        Read::Slot
    }
}
