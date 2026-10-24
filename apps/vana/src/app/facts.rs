//! Ride facts in the app: written when a ride is saved and again once its
//! heart-rate recovery is known, and read back for the week's training stress
//! and a saved ride's summary.
//!
//! After boot the files are read into `Kept` a few directory slots per tick
//! while no ride is open; a save or a cleared journal then updates it in
//! place. Without a FILES volume nothing is kept and every reader shows what
//! it did before: stress at the FTP now, no HR recovery and no climbs.
use super::Runtime;
use super::recording::{Medium, Ride};
use crate::ride::facts::{self, Facts, KEEP, Record};
use crate::ride::recovery::Reading;
use crate::ride::zones::TrainingProfile;
use device_api::bulk::ReadWrite;
use firmware_services::files;

/// A failed read of the files is tried again this long after.
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scan {
    /// The next directory slot to read.
    At(usize),
    Retry {
        at: u64,
    },
    Done,
}

impl Default for Scan {
    fn default() -> Self {
        Self::At(0)
    }
}

#[derive(Default)]
pub(super) struct Kept {
    records: heapless::Vec<Record, KEEP>,
    scan: Scan,
    /// Changes with `records`, so totals made from them are made again.
    pub(super) revision: u32,
    /// The ride just saved, while its heart-rate recovery is measuring.
    measuring: Option<Record>,
}

impl Kept {
    /// What the saved ride `ride_id` of `medium`, started at `started_utc_ms`,
    /// keeps beside its journal.
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

    /// Keep what was just written, in place of the file it deleted.
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
        let known = self
            .records
            .iter()
            .any(|kept| (kept.journal, kept.ride_id) == (record.journal, record.ride_id));
        if !known && self.records.push(record).is_ok() {
            self.changed();
        }
    }

    /// Write the facts of the ride just saved on `medium`, with the profile
    /// in force and the heart-rate recovery as far as it is measured.
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
        let record = Record {
            journal: medium.journal(),
            ride_id: saved.ride_id,
            started_utc_ms: saved.first_utc_ms,
            facts: Facts {
                profile: Some(profile),
                recovery_bpm: match recovery {
                    Some(Reading::Recovered(bpm)) => Some(bpm),
                    _ => None,
                },
                climbing: Some(ride.climbing),
            },
        };
        self.measuring = matches!(recovery, Some(Reading::Measuring(_))).then_some(record);
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
        self.measuring = None;
        self.changed();
        if let Err(error) = facts::forget(bulk, journal) {
            log::warn!(target: "sdk.facts", "forget_failed error={error:?}");
        }
    }
}

impl Runtime {
    /// Add the heart-rate recovery once it is measured, and read the files
    /// while no ride is open.
    pub(super) fn service_facts(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        if let Some(mut record) = self.facts.measuring {
            match self.recovery.reading(now) {
                Some(Reading::Measuring(_)) => {}
                Some(Reading::Recovered(bpm)) => {
                    self.facts.measuring = None;
                    record.facts.recovery_bpm = Some(bpm);
                    self.facts.write(bulk, record);
                }
                None => self.facts.measuring = None,
            }
        }
        if self.ride.active().riding() {
            return;
        }
        let slot = match self.facts.scan {
            Scan::Done => return,
            Scan::Retry { at } if now < at => return,
            Scan::Retry { .. } => 0,
            Scan::At(slot) => slot,
        };
        let kept = &mut self.facts;
        kept.scan = match facts::read(bulk, slot, |record| kept.read(record)) {
            Ok(Some(next)) => Scan::At(next),
            Ok(None) => Scan::Done,
            Err(_) => Scan::Retry {
                at: now.saturating_add(RETRY_MS),
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: Option<u64> = Some(1_760_000_000_000);

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
        let find = |kept: &Kept, ride| kept.find(Medium::Bulk, ride, START).copied();
        assert_eq!(find(&kept, 2).and_then(|f| f.recovery_bpm), Some(30));
        assert!(find(&kept, 1).is_some());
        assert!(kept.find(Medium::Internal, 1, START).is_none());
        let revision = kept.revision;
        kept.saved(record(3, None), Some(facts::file(b'b', 1)));
        assert!(find(&kept, 1).is_none());
        assert!(find(&kept, 3).is_some());
        assert_ne!(kept.revision, revision);
    }
}
