//! Ride facts in the app: written when a ride is saved and again once its
//! heart-rate recovery is known, and read back for the week's training stress
//! and a saved ride's summary.
//!
//! The files are read into `Kept` one directory sector per tick while no
//! ride is open or being written: after boot, and again each time the card
//! becomes ready. A save or a cleared journal then updates it in place.
//! Without a FILES volume nothing is kept and every reader shows what it did
//! before: stress at the FTP now, no HR recovery and no climbs.
use super::Runtime;
use super::recording::{Medium, Ride};
use crate::ride::facts::{self, Error, Facts, KEEP, Record};
use crate::ride::recovery::Reading;
use crate::ride::zones::TrainingProfile;
use device_api::bulk::ReadWrite;
use firmware_services::{files, volume};

/// A failed read of the files is tried again this long after.
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scan {
    /// The next directory sector to read.
    At(usize),
    Retry {
        at: u64,
    },
    /// No FILES volume or no store on it: read again when a card is ready.
    Absent,
    Done,
}

impl Default for Scan {
    fn default() -> Self {
        Self::At(0)
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
    records: heapless::Vec<Record, KEEP>,
    scan: Scan,
    /// Changes with `records`, so totals made from them are made again.
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
    ) -> Option<&Facts> {
        self.records
            .iter()
            .find(|record| record.of(medium.journal(), ride_id, started_utc_ms))
            .map(|record| &record.facts)
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn saved(&mut self, record: Record, deleted: Option<files::Name>) {
        self.records.retain(|kept| {
            (kept.journal, kept.ride_id) != (record.journal, record.ride_id)
                && deleted != Some(facts::file(kept.journal, kept.ride_id))
        });
        if self.records.is_full() {
            self.records.remove(0);
        }
        let _ = self.records.push(record);
        self.changed();
    }

    /// Keep a record read from the files, unless a save already kept a newer
    /// one for its ride.
    fn read(&mut self, record: Record) {
        if !matches!(record.journal, b'i' | b'b') {
            return;
        }
        let known = self
            .records
            .iter()
            .any(|kept| (kept.journal, kept.ride_id) == (record.journal, record.ride_id));
        if !known && self.records.push(record).is_ok() {
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
            Ok(deleted) => self.saved(record, deleted),
            Err(error) => log::warn!(target: "sdk.facts", "save_failed error={error:?}"),
        }
    }

    /// A cleared or initialized journal numbers its rides from 1 again.
    pub(super) fn forget(&mut self, bulk: &mut impl ReadWrite, medium: Medium) {
        let journal = medium.journal();
        self.records.retain(|record| record.journal != journal);
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
    /// Add the heart-rate recovery once it is measured, and read the files
    /// while no ride is open or being written.
    pub(super) fn service_facts(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let riding = self.ride.active().riding();
        self.facts.follow(bulk, self.recovery.reading(now), riding);
        let ready = self.ride.bulk.status().usable();
        let kept = &mut self.facts;
        if ready && !kept.card_ready {
            kept.records.clear();
            kept.changed();
            kept.scan = Scan::At(0);
        }
        kept.card_ready = ready;
        if self.ride.pending() || self.ride.open() {
            return;
        }
        let slot = match kept.scan {
            Scan::Done | Scan::Absent => return,
            Scan::Retry { at } if now < at => return,
            Scan::Retry { .. } => 0,
            Scan::At(slot) => slot,
        };
        kept.scan = match facts::read(bulk, slot, |record| kept.read(record)) {
            Ok(Some(next)) => Scan::At(next),
            Ok(None) => Scan::Done,
            Err(error) if absent(error) => Scan::Absent,
            Err(_) => Scan::Retry {
                at: now.saturating_add(RETRY_MS),
            },
        };
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

    #[test]
    fn a_save_outranks_the_file_read_and_drops_the_record_it_deleted() {
        let mut kept = Kept::default();
        kept.saved(record(2, Some(30)), None);
        kept.read(record(2, None));
        kept.read(record(1, None));
        let find = |kept: &Kept, ride| kept.find(Medium::Bulk, ride, Some(START)).copied();
        assert_eq!(find(&kept, 2).and_then(|f| f.recovery_bpm), Some(30));
        assert!(find(&kept, 1).is_some());
        assert!(kept.find(Medium::Internal, 1, Some(START)).is_none());
        assert!(kept.find(Medium::Bulk, 1, None).is_none(), "undated");
        let revision = kept.revision;
        kept.saved(record(3, None), Some(facts::file(b'b', 1)));
        assert!(find(&kept, 1).is_none());
        assert!(find(&kept, 3).is_some());
        assert_ne!(kept.revision, revision);
    }

    #[test]
    fn a_ride_ended_before_the_summary_measures_waits_for_the_recovery() {
        let mut region = crate::workout::library::tests::formatted();
        let mut kept = Kept {
            awaiting: Some(Awaiting::Finish(record(4, None))),
            ..Kept::default()
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
            ..Kept::default()
        };
        kept.forget(&mut region, Medium::Bulk);
        assert!(kept.awaiting.is_some());
        kept.forget(&mut region, Medium::Internal);
        assert_eq!(kept.awaiting, None);
    }
}
