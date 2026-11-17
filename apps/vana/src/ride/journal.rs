//! The recorder's view of one ride medium.
//!
//! A journal scans, initializes, clears and appends in bounded steps and owns
//! its layout. The recorder owns ride state and decides what to append. Both
//! media hold the same slots, so one scanner and one exporter
//! decode either. Construct a journal for one service call from its retained
//! state and the borrowed storage handle.

use crate::ride::log::{Catalog, Entry, SLOTS, SLOTS_PER_SECTOR, Scanner, Sector, Slot};
use crate::ride::storage::RideStorage;

/// What one scan step found.
// Boxing is unavailable without an allocator; the catalog is returned once per
// boot and every other step is small anyway.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scan {
    More,
    Done(Catalog),
    /// Nothing this journal wrote is there; only explicit initialize may write.
    Uninitialized,
    /// Another version's data is there; only FORMAT may write.
    Foreign,
    /// This device has no such medium.
    Unsupported,
    /// The medium should exist but cannot be reached now.
    Unavailable,
    /// The medium holds this journal's data but its recovery metadata cannot be
    /// trusted; it stays untouched for diagnosis.
    Corrupt,
    /// Corrupt, but readable: where the journal ends cannot be proven, and
    /// the catalog's next slot is the extent seen, for export only.
    Uncertain(Catalog),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    More,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Append {
    /// The entry is programmed and reads back; `checkpoint_due` asks for the
    /// recovery state to be saved.
    Verified { checkpoint_due: bool },
    /// The outcome is unknown: stop and rescan before writing.
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Checkpoint {
    Written,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Read {
    /// The slot's bytes are in the caller's slot.
    Slot,
    /// The medium's container around the slot is invalid: a gap, with the
    /// caller's slot zeroed.
    Invalid,
    /// The medium failed; try again later.
    Failed,
}

pub trait Journal {
    /// Slots this medium holds, including the final slot reserved for a
    /// terminal record. Zero until a scan has found the medium.
    fn slots(&self) -> usize;
    /// At most one bounded media read.
    fn scan(&mut self) -> Scan;
    /// Step `step` (from zero) of an explicit initialize, which the recorder
    /// asks only of a blank or corrupt journal. The internal journal is never
    /// either, so it keeps this refusal.
    fn initialize(&mut self, _step: usize, _seed: u64) -> Progress {
        Progress::Failed
    }
    /// Step `step` (from zero) of an explicit clear after a verified export.
    /// FORMAT wipes the internal journal instead, so it keeps this refusal.
    fn clear(&mut self, _step: usize, _seed: u64) -> Progress {
        Progress::Failed
    }
    /// Program `entry` at `index` and verify it.
    fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Append;
    /// Save recovery state so boot need not scan what it covers.
    fn checkpoint(&mut self, catalog: &Catalog) -> Checkpoint;
    /// The records since the last checkpoint already reach its cadence, as a
    /// boot can find after power was cut before a due checkpoint. One must be
    /// written before anything else is appended.
    fn checkpoint_overdue(&self) -> bool {
        false
    }
    /// The slot at `index` for export.
    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Read;
}

/// Scan progress of the internal 1 MiB journal.
pub struct InternalState {
    scanner: Option<Scanner>,
    sector: Sector,
}

impl Default for InternalState {
    fn default() -> Self {
        Self {
            scanner: Some(Scanner::default()),
            sector: Sector::default(),
        }
    }
}

/// FORMAT's wipe of internal sector `index`: true once it reads erased. Slot
/// by slot, so no sector buffer counts against the stack budget (#167).
pub fn wipe_sector<S: RideStorage>(store: &mut S, index: usize) -> bool {
    let erased = |store: &mut S| -> Option<bool> {
        let mut slot = Slot::default();
        for at in index * SLOTS_PER_SECTOR..(index + 1) * SLOTS_PER_SECTOR {
            store.ride_read_slot(at, &mut slot).ok()?;
            if !crate::ride::log::erased(&slot.0) {
                return Some(false);
            }
        }
        Some(true)
    };
    match erased(store) {
        Some(true) => true,
        Some(false) => store.ride_erase_sector(index).is_ok() && erased(store) == Some(true),
        None => false,
    }
}

/// The internal journal: fixed slots in the owned boot-flash reservation.
pub struct Internal<'a, S> {
    state: &'a mut InternalState,
    store: &'a mut S,
}

impl<'a, S: RideStorage> Internal<'a, S> {
    pub fn new(state: &'a mut InternalState, store: &'a mut S) -> Self {
        Self { state, store }
    }
}

impl<S: RideStorage> Journal for Internal<'_, S> {
    fn slots(&self) -> usize {
        SLOTS
    }

    fn scan(&mut self) -> Scan {
        let Some(scanner) = self.state.scanner.as_mut() else {
            return Scan::Failed;
        };
        let Some(sector) = scanner.next_sector() else {
            return Scan::Failed;
        };
        if self
            .store
            .ride_read_sector(sector, &mut self.state.sector)
            .is_err()
        {
            self.state.scanner = None;
            return Scan::Failed;
        }
        scanner.accept(&self.state.sector);
        match scanner.finish() {
            Some(catalog) => {
                self.state.scanner = None;
                Scan::Done(catalog)
            }
            None => Scan::More,
        }
    }

    fn append(&mut self, index: usize, entry: &Entry, _: bool) -> Append {
        let mut verify = Slot::default();
        if self.store.ride_read_slot(index, &mut verify).is_err()
            || !crate::ride::log::erased(&verify.0)
        {
            return Append::Uncertain;
        }
        let slot = crate::ride::log::encode(entry);
        let _write = self.store.ride_write_slot(index, &slot).and_then(|()| {
            self.store
                .ride_commit_slot(index, &crate::ride::log::commit_word())
        });
        // The commit may have reached flash even when verification cannot
        // complete; the boot scanner reconciles this slot.
        let verified = self.store.ride_read_slot(index, &mut verify).is_ok()
            && crate::ride::log::decode(&verify).as_ref() == Some(entry);
        if verified {
            Append::Verified {
                checkpoint_due: false,
            }
        } else {
            Append::Uncertain
        }
    }

    fn checkpoint(&mut self, _: &Catalog) -> Checkpoint {
        Checkpoint::Written
    }

    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Read {
        match self.store.ride_read_slot(index, output) {
            Ok(()) => Read::Slot,
            Err(_) => Read::Failed,
        }
    }
}
