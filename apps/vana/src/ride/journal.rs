//! The recorder's view of one ride medium.
//!
//! A journal scans, initializes, clears and appends in bounded steps and owns
//! its layout. The recorder owns ride state and decides what to append. Both
//! media hold the same slots, so one scanner and one exporter
//! decode either. Construct a journal for one service call from its retained
//! state and the borrowed storage handle.

use crate::ride::log::{
    Catalog, Entry, SLOT_SIZE, SLOTS, SLOTS_PER_SECTOR, Scanner, Sector, Slot, commit_word, decode,
    recommittable, tear,
};
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

enum Recommit {
    Whole,
    Torn,
    Unknown,
}

/// Scan progress of the internal 1 MiB journal.
pub struct InternalState {
    scanner: Option<Scanner>,
    sector: Sector,
    torn: Option<usize>,
}

impl Default for InternalState {
    fn default() -> Self {
        Self {
            scanner: Some(Scanner::default()),
            sector: Sector::default(),
            torn: None,
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

    fn recommit(&mut self, index: usize, expected: Option<&Entry>) -> Recommit {
        let mut slot = Slot::default();
        if self.store.ride_read_slot(index, &mut slot).is_err() {
            return Recommit::Unknown;
        }
        let Some(entry) =
            recommittable(&slot).filter(|entry| expected.is_none_or(|expected| entry == expected))
        else {
            return Recommit::Torn;
        };
        if self.store.ride_commit_slot(index, &commit_word()).is_err()
            || self.store.ride_read_slot(index, &mut slot).is_err()
        {
            return Recommit::Unknown;
        }
        match decode(&slot) {
            Some(read) if read == entry => Recommit::Whole,
            _ => Recommit::Torn,
        }
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
        if let Some(torn) = self.state.torn
            && torn / SLOTS_PER_SECTOR == sector
        {
            let at = torn % SLOTS_PER_SECTOR * SLOT_SIZE;
            tear(&mut self.state.sector.0[at..at + SLOT_SIZE]);
        }
        scanner.accept(&self.state.sector);
        let Some(catalog) = scanner.finish() else {
            return Scan::More;
        };
        let newest = scanner.newest();
        self.state.scanner = None;
        // A cut commit program can leave the newest slot's word marginal,
        // read whole on one boot and torn on a later one (#444).
        let Some(newest) = newest else {
            return Scan::Done(catalog);
        };
        match self.recommit(newest, None) {
            Recommit::Whole => Scan::Done(catalog),
            Recommit::Torn if self.state.torn.is_none() => {
                self.state.torn = Some(newest);
                self.state.scanner = Some(Scanner::default());
                Scan::More
            }
            Recommit::Torn | Recommit::Unknown => Scan::Failed,
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
        let committed = self
            .store
            .ride_write_slot(index, &slot)
            .map(|()| self.store.ride_commit_slot(index, &commit_word()));
        // The commit may have reached flash even when verification cannot
        // complete; the boot scanner reconciles this slot. A commit program
        // that failed can leave the word marginal, read whole now and torn
        // after a reboot, so it is programmed again first.
        let verified = match committed {
            Ok(Err(_)) => matches!(self.recommit(index, Some(entry)), Recommit::Whole),
            _ => {
                self.store.ride_read_slot(index, &mut verify).is_ok()
                    && decode(&verify).as_ref() == Some(entry)
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::{COMMIT_OFFSET, Commit, Kind, REGION_SIZE, SECTOR_SIZE, encode};
    use std::vec;
    use std::vec::Vec;

    struct Flash {
        bytes: Vec<u8>,
        marginal: Option<(usize, [u8; 4])>,
        whole: bool,
        cut_commits: u8,
        lost_after_commit: Option<usize>,
        committed: Vec<usize>,
    }

    impl Flash {
        fn new() -> Self {
            Self {
                bytes: vec![0xff; REGION_SIZE],
                marginal: None,
                whole: true,
                cut_commits: 0,
                lost_after_commit: None,
                committed: Vec::new(),
            }
        }

        fn read(&self, at: usize, output: &mut [u8]) {
            output.copy_from_slice(&self.bytes[at..at + output.len()]);
            if let Some((word, whole)) = self.marginal
                && self.whole
                && (at..at + output.len()).contains(&word)
            {
                output[word - at..word - at + 4].copy_from_slice(&whole);
            }
        }
    }

    impl RideStorage for Flash {
        type Error = ();
        fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), ()> {
            self.read(sector * SECTOR_SIZE, &mut output.0);
            Ok(())
        }
        fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
            if self.lost_after_commit == Some(slot) && self.committed.contains(&slot) {
                return Err(());
            }
            self.read(slot * SLOT_SIZE, &mut output.0);
            Ok(())
        }
        fn ride_erase_sector(&mut self, _: usize) -> Result<(), ()> {
            unreachable!("the internal journal never erases")
        }
        fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
            let stored = &mut self.bytes[slot * SLOT_SIZE..(slot + 1) * SLOT_SIZE];
            for (stored, new) in stored.iter_mut().zip(data.0) {
                *stored &= new;
            }
            Ok(())
        }
        fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), ()> {
            let at = slot * SLOT_SIZE + COMMIT_OFFSET;
            let word = &mut self.bytes[at..at + 4];
            if self.cut_commits > 0 {
                self.cut_commits -= 1;
                let mut whole = [0; 4];
                for (whole, (stored, new)) in whole.iter_mut().zip(word.iter().zip(commit.0)) {
                    *whole = stored & new;
                }
                word[..2].copy_from_slice(&whole[..2]);
                self.marginal = Some((at, whole));
                return Err(());
            }
            for (stored, new) in word.iter_mut().zip(commit.0) {
                *stored &= new;
            }
            if self.marginal.is_some_and(|(word, _)| word == at) {
                self.marginal = None;
            }
            self.committed.push(slot);
            Ok(())
        }
    }

    fn entry(sequence: u32) -> Entry {
        let kind = if sequence == 0 {
            Kind::Start
        } else {
            Kind::Pause
        };
        Entry::event(kind, 1, sequence, u64::from(sequence) * 1000)
    }

    fn journal(count: usize, cut: bool) -> Flash {
        let mut flash = Flash::new();
        for index in 0..count {
            flash.cut_commits = u8::from(cut && index + 1 == count);
            let slot = encode(&entry(index as u32));
            flash.ride_write_slot(index, &slot).unwrap();
            let _ = flash.ride_commit_slot(index, &commit_word());
        }
        flash
    }

    fn boot(flash: &mut Flash, whole: bool) -> Catalog {
        flash.whole = whole;
        let mut state = InternalState::default();
        loop {
            match Internal::new(&mut state, flash).scan() {
                Scan::More => {}
                Scan::Done(catalog) => return catalog,
                other => panic!("the scan ended {other:?}"),
            }
        }
    }

    #[test]
    fn a_marginal_commit_word_read_whole_at_boot_is_programmed_again() {
        let mut flash = journal(3, true);
        assert_eq!(boot(&mut flash, true).valid_slots, 3);
        assert_eq!(flash.marginal, None, "the boot completed the program");
        assert_eq!(boot(&mut flash, false).valid_slots, 3, "a later boot");
    }

    #[test]
    fn an_append_whose_commit_program_fails_programs_it_again() {
        let mut flash = journal(1, false);
        flash.cut_commits = 1;
        let mut state = InternalState::default();
        let appended = Internal::new(&mut state, &mut flash).append(1, &entry(1), false);
        assert_eq!(
            appended,
            Append::Verified {
                checkpoint_due: false
            }
        );
        assert_eq!(flash.marginal, None);
        assert_eq!(boot(&mut flash, false).valid_slots, 2);
    }

    #[test]
    fn a_newest_slot_that_cannot_be_read_back_fails_the_scan() {
        let mut flash = journal(3, true);
        flash.lost_after_commit = Some(2);
        flash.whole = true;
        let mut state = InternalState::default();
        let ended = loop {
            match Internal::new(&mut state, &mut flash).scan() {
                Scan::More => {}
                other => break other,
            }
        };
        assert!(
            matches!(ended, Scan::Failed),
            "no recovery is written over it"
        );
    }

    #[test]
    fn an_append_whose_commit_fails_twice_is_uncertain() {
        let mut flash = journal(1, false);
        flash.cut_commits = 2;
        flash.whole = true;
        let mut state = InternalState::default();
        let appended = Internal::new(&mut state, &mut flash).append(1, &entry(1), false);
        assert_eq!(appended, Append::Uncertain);
    }

    /// #444's open case: the boot after the cut reads the word torn, so the
    /// slot is no valid slot to program again, and the next append goes after
    /// it. Each later boot reads the slot whole or torn, so its entry comes and
    /// goes.
    #[test]
    fn a_commit_word_read_torn_at_its_boot_comes_and_goes_on_later_boots() {
        let mut flash = journal(3, true);
        assert_eq!(boot(&mut flash, false).valid_slots, 2);
        let mut state = InternalState::default();
        let appended = Internal::new(&mut state, &mut flash).append(3, &entry(3), false);
        assert!(matches!(appended, Append::Verified { .. }));
        assert_eq!(boot(&mut flash, true).valid_slots, 4);
        assert_eq!(boot(&mut flash, false).valid_slots, 3);
    }
}
