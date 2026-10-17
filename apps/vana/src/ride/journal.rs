//! The recorder's view of one ride medium.
//!
//! A journal scans, initializes, clears and appends in bounded steps and owns
//! its layout. The recorder owns ride state and decides what to append. Both
//! media hold the same version-1 slots, so one scanner and one exporter
//! decode either. Construct a journal for one service call from its retained
//! state and the borrowed storage handle.

use crate::ride::log::{Catalog, Entry, SECTORS, SLOTS, Scanner, Sector, Slot};
use crate::ride::reclaim::{Clear, Media, Progress as ClearProgress};
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

pub trait Journal {
    /// Slots this medium holds, including the final slot reserved for a
    /// terminal record. Zero until a scan has found the medium.
    fn slots(&self) -> usize;
    /// At most one bounded media read.
    fn scan(&mut self) -> Scan;
    /// Step `step` (from zero) of an explicit initialize.
    fn initialize(&mut self, step: usize, seed: u64) -> Progress;
    /// Step `step` (from zero) of an explicit clear after a verified export.
    fn clear(&mut self, step: usize, seed: u64) -> Progress;
    /// Program `entry` at `index` and verify it. `Some(checkpoint)` once it
    /// verified, where `checkpoint` asks for the recovery state to be saved.
    /// `None` leaves the outcome uncertain: stop and rescan before writing.
    fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Option<bool>;
    /// Save recovery state so boot need not scan what it covers.
    fn checkpoint(&mut self, catalog: &Catalog) -> bool;
    /// The records since the last checkpoint already reach its cadence, as a
    /// boot can find after power was cut before a due checkpoint. One must be
    /// written before anything else is appended.
    fn checkpoint_overdue(&self) -> bool {
        false
    }
    /// The slot at `index` for export, or `None` when the medium fails.
    /// `Some(false)`, with zeroed output, when the medium's container around
    /// the slot is invalid.
    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Option<bool>;
}

/// Scan, initialize and clear progress of the internal 1 MiB journal.
pub struct InternalState {
    scanner: Option<Scanner>,
    sector: Sector,
    clear: Clear,
}

impl Default for InternalState {
    fn default() -> Self {
        Self {
            scanner: Some(Scanner::default()),
            sector: Sector::default(),
            clear: Clear::default(),
        }
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

    fn initialize(&mut self, step: usize, _: u64) -> Progress {
        let ok = self.store.ride_erase_sector(step).is_ok()
            && self
                .store
                .ride_read_sector(step, &mut self.state.sector)
                .is_ok()
            && crate::ride::log::erased(&self.state.sector.0);
        match (ok, step + 1 == SECTORS) {
            (false, _) => Progress::Failed,
            (true, true) => Progress::Done,
            (true, false) => Progress::More,
        }
    }

    fn clear(&mut self, step: usize, _: u64) -> Progress {
        struct StoreMedia<'a, S>(&'a mut S);
        impl<S: RideStorage> Media for StoreMedia<'_, S> {
            type Error = ();
            fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
                self.0.ride_erase_sector(sector).map_err(|_| ())
            }
            fn read_sector(
                &mut self,
                sector: usize,
                output: &mut Sector,
            ) -> Result<(), Self::Error> {
                self.0.ride_read_sector(sector, output).map_err(|_| ())
            }
        }
        if step == 0 {
            self.state.clear = Clear::default();
        }
        match self
            .state
            .clear
            .service(&mut StoreMedia(&mut *self.store), &mut self.state.sector)
        {
            Ok(ClearProgress::More) => Progress::More,
            Ok(ClearProgress::Complete) => Progress::Done,
            Err(_) => Progress::Failed,
        }
    }

    fn append(&mut self, index: usize, entry: &Entry, _: bool) -> Option<bool> {
        let mut verify = Slot::default();
        if self.store.ride_read_slot(index, &mut verify).is_err()
            || !crate::ride::log::erased(&verify.0)
        {
            return None;
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
        verified.then_some(false)
    }

    fn checkpoint(&mut self, _: &Catalog) -> bool {
        true
    }

    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Option<bool> {
        self.store.ride_read_slot(index, output).ok().map(|()| true)
    }
}
