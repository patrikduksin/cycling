//! Ride facts in the app: written when a ride is saved and again once its
//! heart-rate recovery is known, and read back for the training stress of the
//! rides list and FORM, and for a saved ride's summary.
//!
//! The table is read onto the `Shelf` the composition lends (PSRAM on the
//! device), a few sectors per tick while no ride is open or being written:
//! after boot, and again each time the card becomes ready. The files older
//! firmware kept per ride are read after it, one directory sector per tick,
//! and then folded into the table a batch per tick. A save or a cleared
//! journal then updates the shelf in place. Without a FILES volume or a shelf
//! nothing is kept and every reader shows what it did before: stress at the
//! FTP now, no HR recovery and no climbs.
use super::Runtime;
use super::recording::{Medium, Ride};
use crate::ride::facts::{self, Error, Facts, Record, Shelf};
use crate::ride::recovery::Reading;
use crate::ride::zones::TrainingProfile;
use device_api::bulk::ReadWrite;
use firmware_services::{files, volume};

/// A failed read of the files is tried again this long after.
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scan {
    /// The next piece of the table, and the table's revision as of the first
    /// piece; a table written since is read again from the start.
    Table {
        piece: usize,
        revision: Option<u32>,
    },
    /// The next directory sector of older files.
    Files(usize),
    /// Fold the older files into the table.
    Fold,
    Retry {
        at: u64,
    },
    /// No FILES volume or no store on it: read again when a card is ready.
    Absent,
    Done,
}

impl Scan {
    const START: Self = Self::Table {
        piece: 0,
        revision: None,
    };
}

impl Default for Scan {
    fn default() -> Self {
        Self::START
    }
}

/// The ride just saved, until its heart-rate recovery is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Awaiting {
    /// Saved before the summary started measuring, as when the host ends a
    /// ride.
    Finish(Record),
    Measuring(Record),
}

#[derive(Default)]
pub(super) struct Kept {
    shelf: Option<Shelf>,
    scan: Scan,
    /// Changes with the shelf, so totals made from it are made again.
    pub(super) revision: u32,
    awaiting: Option<Awaiting>,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
}

impl Kept {
    /// What the saved ride `ride_id` of `medium`, started at `started_utc_ms`,
    /// keeps beside its journal; nothing for an undated ride.
    pub(super) fn find(
        &self,
        medium: Medium,
        ride_id: u32,
        started_utc_ms: Option<u64>,
    ) -> Option<Facts> {
        self.shelf
            .as_ref()?
            .find(medium.journal(), ride_id, started_utc_ms)
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn saved(&mut self, record: Record) {
        if let Some(shelf) = &mut self.shelf
            && shelf.put(&record, true)
        {
            self.changed();
        }
    }

    /// Keep a record read from the files, unless the shelf holds its ride
    /// from the same or a later start, as after a save.
    fn read(&mut self, record: Record) {
        if !matches!(record.journal, b'i' | b'b') {
            return;
        }
        if let Some(shelf) = &mut self.shelf
            && shelf.put(&record, false)
        {
            self.changed();
        }
    }

    /// Write the facts of the ride just saved on `medium`, with the profile
    /// in force and the heart-rate recovery as far as it is measured. An
    /// undated ride gets none: nothing could tell it from a later ride with
    /// its id.
    pub(super) fn ride_saved(
        &mut self,
        bulk: &mut impl ReadWrite,
        ride: &Ride,
        medium: Medium,
        profile: TrainingProfile,
        recovery: Option<Reading>,
    ) {
        let summaries = ride.recorder(medium).summaries();
        let Some(saved) = summaries.iter().flatten().last() else {
            return;
        };
        let Some(started_utc_ms) = saved.first_utc_ms else {
            return;
        };
        let mut record = Record {
            journal: medium.journal(),
            ride_id: saved.ride_id,
            started_utc_ms,
            facts: Facts {
                profile: Some(profile),
                recovery_bpm: None,
                climbing: Some(ride.climbing),
            },
        };
        self.awaiting = match recovery {
            Some(Reading::Recovered(bpm)) => {
                record.facts.recovery_bpm = Some(bpm);
                None
            }
            Some(Reading::Measuring(_)) => Some(Awaiting::Measuring(record)),
            None => Some(Awaiting::Finish(record)),
        };
        self.write(bulk, record);
    }

    fn write(&mut self, bulk: &mut impl ReadWrite, record: Record) {
        match facts::save(bulk, &record) {
            Ok(()) => self.saved(record),
            Err(error) => log::warn!(target: "sdk.facts", "save_failed error={error:?}"),
        }
    }

    /// A cleared or initialized journal numbers its rides from 1 again.
    pub(super) fn forget(&mut self, bulk: &mut impl ReadWrite, medium: Medium) {
        let journal = medium.journal();
        if let Some(shelf) = &mut self.shelf {
            shelf.forget(journal);
        }
        if let Some(Awaiting::Finish(record) | Awaiting::Measuring(record)) = self.awaiting
            && record.journal == journal
        {
            self.awaiting = None;
        }
        self.changed();
        if let Err(error) = facts::forget(bulk, journal) {
            log::warn!(target: "sdk.facts", "forget_failed error={error:?}");
        }
    }

    /// Follow the summary's heart-rate recovery for the ride just saved, and
    /// write it once measured.
    fn follow(&mut self, bulk: &mut impl ReadWrite, reading: Option<Reading>, riding: bool) {
        let Some(awaiting) = self.awaiting else {
            return;
        };
        let (Awaiting::Finish(mut record) | Awaiting::Measuring(mut record)) = awaiting;
        self.awaiting = match (awaiting, reading) {
            _ if riding => None,
            (_, Some(Reading::Recovered(bpm))) => {
                record.facts.recovery_bpm = Some(bpm);
                self.write(bulk, record);
                None
            }
            (_, Some(Reading::Measuring(_))) => Some(Awaiting::Measuring(record)),
            (Awaiting::Finish(_), None) => Some(awaiting),
            (Awaiting::Measuring(_), None) => None,
        };
    }

    /// Take the next step of reading the facts, unless a ride is open or
    /// being written (`busy`). A card that became ready is read from the
    /// start.
    fn scan_step(&mut self, bulk: &mut impl ReadWrite, now: u64, ready: bool, busy: bool) {
        if ready && !self.card_ready {
            if let Some(shelf) = &mut self.shelf {
                shelf.clear();
            }
            self.changed();
            self.scan = Scan::START;
        }
        self.card_ready = ready;
        if busy {
            return;
        }
        let step = match self.scan {
            Scan::Done | Scan::Absent => return,
            Scan::Retry { at } if now < at => return,
            Scan::Retry { .. } => Scan::START,
            step => step,
        };
        let next = match step {
            Scan::Table { piece, revision } => {
                match facts::read(bulk, piece, |record| self.read(record)) {
                    Ok(Some(read)) if revision.is_some_and(|r| r != read.revision) => {
                        Ok(Scan::START)
                    }
                    Ok(Some(facts::Piece {
                        revision,
                        next: Some(piece),
                    })) => Ok(Scan::Table {
                        piece,
                        revision: Some(revision),
                    }),
                    Ok(_) | Err(Error::Table) => Ok(Scan::Files(0)),
                    Err(error) => Err(error),
                }
            }
            Scan::Files(index) => facts::read_files(bulk, index, |record| self.read(record))
                .map(|next| next.map_or(Scan::Fold, Scan::Files)),
            Scan::Fold => match facts::fold(bulk) {
                Ok(true) => Ok(Scan::Fold),
                Ok(false) => Ok(Scan::Done),
                Err(error) if absent(error) => Err(error),
                // The next boot folds them; the shelf already holds them.
                Err(error) => {
                    log::warn!(target: "sdk.facts", "fold_failed error={error:?}");
                    Ok(Scan::Done)
                }
            },
            done => Ok(done),
        };
        self.scan = match next {
            Ok(next) => next,
            Err(error) if absent(error) => Scan::Absent,
            Err(_) => Scan::Retry {
                at: now.saturating_add(RETRY_MS),
            },
        };
    }
}

/// Whether a read failed because there is no card or no store to read,
/// rather than only this time.
fn absent(error: Error) -> bool {
    use device_api::bulk::Error as Media;
    matches!(
        error,
        Error::Volume(
            volume::Error::Missing
                | volume::Error::Foreign
                | volume::Error::Small
                | volume::Error::Media(Media::Unavailable | Media::Unsupported)
        ) | Error::Files(
            files::Error::Uninitialized
                | files::Error::Foreign
                | files::Error::Media(Media::Unavailable | Media::Unsupported)
        )
    )
}

impl Runtime {
    /// Lend storage for the ride facts in memory, [`facts::SHELF_BYTES`], so
    /// FORM and the rides list find each ride's.
    pub fn attach_ride_facts(&mut self, slots: &'static mut [u8]) {
        self.facts.shelf = Some(Shelf::new(slots));
        self.facts.scan = Scan::START;
    }

    /// Add the heart-rate recovery once it is measured, and read the files
    /// while no ride is open or being written.
    pub(super) fn service_facts(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let riding = self.ride.active().riding();
        self.facts.follow(bulk, self.recovery.reading(now), riding);
        let ready = self.ride.bulk.status().usable();
        let busy = self.ride.pending() || self.ride.open();
        self.facts.scan_step(bulk, now, ready, busy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u64 = 1_760_000_000_000;

    fn record(ride_id: u32, recovery_bpm: Option<i16>) -> Record {
        Record {
            journal: b'b',
            ride_id,
            started_utc_ms: START,
            facts: Facts {
                recovery_bpm,
                ..Facts::default()
            },
        }
    }

    fn kept() -> Kept {
        Kept {
            shelf: Some(Shelf::new(std::vec![0; facts::SHELF_BYTES].leak())),
            ..Kept::default()
        }
    }

    #[test]
    fn a_save_outranks_the_file_read() {
        let mut kept = kept();
        kept.saved(record(2, Some(30)));
        kept.read(record(2, None));
        kept.read(record(1, None));
        let find = |kept: &Kept, ride| kept.find(Medium::Bulk, ride, Some(START));
        assert_eq!(find(&kept, 2).and_then(|f| f.recovery_bpm), Some(30));
        assert!(find(&kept, 1).is_some());
        assert!(kept.find(Medium::Internal, 1, Some(START)).is_none());
        assert!(kept.find(Medium::Bulk, 1, None).is_none(), "undated");
        let revision = kept.revision;
        kept.saved(record(3, None));
        assert!(find(&kept, 3).is_some());
        assert_ne!(kept.revision, revision);
        let mut bare = Kept::default();
        bare.saved(record(3, None));
        assert!(
            bare.find(Medium::Bulk, 3, Some(START)).is_none(),
            "no shelf lent"
        );
    }

    #[test]
    fn the_scan_reads_the_table_and_older_files_then_folds_them_in() {
        use crate::workout::library::tests::{Region, formatted, put};
        let mut blank = Region::default();
        let mut kept = kept();
        kept.scan_step(&mut blank, 0, false, true);
        assert_eq!(kept.scan, Scan::START, "nothing read while a ride is busy");
        kept.scan_step(&mut blank, 0, false, false);
        assert_eq!(kept.scan, Scan::Absent, "no volumes on the card");
        kept.scan_step(&mut blank, RETRY_MS * 10, false, false);
        assert_eq!(kept.scan, Scan::Absent, "not retried");

        let mut card = formatted();
        for ride in 1..=200 {
            facts::save(&mut card, &record(ride, Some(12))).unwrap();
        }
        let older = Record {
            journal: b'i',
            ..record(9, Some(20))
        };
        put(
            &mut card,
            facts::file(b'i', 9).as_str(),
            &facts::encode(&older),
        );
        kept.scan_step(&mut card, 0, true, true);
        assert_eq!(kept.scan, Scan::START, "a ready card starts over");
        let mut steps = 0;
        while kept.scan != Scan::Done {
            kept.scan_step(&mut card, 0, true, false);
            steps += 1;
        }
        let pieces = (facts::SLOT * 201).div_ceil(facts::PIECE * 512);
        assert_eq!(steps, pieces + files::DIR_SECTORS + 1);
        for ride in [1, 200] {
            let found = kept.find(Medium::Bulk, ride, Some(START));
            assert_eq!(found.and_then(|f| f.recovery_bpm), Some(12), "ride {ride}");
        }
        let found = kept.find(Medium::Internal, 9, Some(START));
        assert_eq!(found.and_then(|f| f.recovery_bpm), Some(20));
        let mut last = None;
        let mut piece = Some(0);
        while let Some(index) = piece {
            piece = facts::read(&mut card, index, |record| last = Some(record))
                .unwrap()
                .and_then(|piece| piece.next);
        }
        assert_eq!(last, Some(older), "the older file folded in last");
    }

    #[test]
    fn a_table_written_during_the_scan_is_read_again() {
        let mut card = crate::workout::library::tests::formatted();
        for ride in 1..=200 {
            facts::save(&mut card, &record(ride, None)).unwrap();
        }
        let mut kept = kept();
        kept.scan_step(&mut card, 0, true, false);
        assert!(matches!(kept.scan, Scan::Table { piece: 1, .. }));
        facts::save(&mut card, &record(1, Some(40))).unwrap();
        kept.scan_step(&mut card, 0, true, false);
        assert_eq!(kept.scan, Scan::START);
        while kept.scan != Scan::Done {
            kept.scan_step(&mut card, 0, true, false);
        }
        let found = kept.find(Medium::Bulk, 200, Some(START));
        assert!(found.is_some());
    }

    #[test]
    fn a_ride_ended_before_the_summary_measures_waits_for_the_recovery() {
        let mut region = crate::workout::library::tests::formatted();
        let mut kept = Kept {
            awaiting: Some(Awaiting::Finish(record(4, None))),
            ..kept()
        };
        kept.follow(&mut region, None, false);
        assert!(kept.awaiting.is_some(), "the summary has not opened yet");
        kept.follow(&mut region, Some(Reading::Measuring(60)), false);
        kept.follow(&mut region, Some(Reading::Recovered(21)), false);
        assert_eq!(kept.awaiting, None);
        let find = kept.find(Medium::Bulk, 4, Some(START));
        assert_eq!(find.and_then(|f| f.recovery_bpm), Some(21));

        kept.awaiting = Some(Awaiting::Measuring(record(5, None)));
        kept.follow(&mut region, None, false);
        assert_eq!(kept.awaiting, None, "left the summary before the minute");
        kept.awaiting = Some(Awaiting::Finish(record(6, None)));
        kept.follow(&mut region, None, true);
        assert_eq!(kept.awaiting, None, "the next ride started");
    }

    #[test]
    fn forgetting_a_journal_keeps_the_other_journals_ride_waiting() {
        let mut region = crate::workout::library::tests::formatted();
        let internal = Record {
            journal: b'i',
            ..record(1, None)
        };
        let mut kept = Kept {
            awaiting: Some(Awaiting::Measuring(internal)),
            ..kept()
        };
        kept.forget(&mut region, Medium::Bulk);
        assert!(kept.awaiting.is_some());
        kept.forget(&mut region, Medium::Internal);
        assert_eq!(kept.awaiting, None);
    }
}
