//! Ride facts in the app: written when a ride is saved and again once its
//! heart-rate recovery is known, and read back for the training stress of the
//! rides list and FORM, and for a saved ride's summary.
use super::Runtime;
use super::recording::Ride;
use crate::kept::facts::record::{Facts, Record, Shelf};
use crate::kept::facts::store::{Awaiting, Kept, Scan};
use crate::ride::medium::Medium;
use crate::ride::recovery::Reading;
use crate::ride::zones::TrainingProfile;
use device_api::bulk::ReadWrite;

impl Kept {
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
}

impl Runtime {
    /// Lend storage for the ride facts in memory,
    /// [`SHELF_BYTES`](crate::kept::facts::record::SHELF_BYTES), so FORM and the
    /// rides list find each ride's.
    pub fn attach_ride_facts(&mut self, slots: &'static mut [u8]) {
        self.facts.shelf = Some(Shelf::new(slots));
        self.facts.scan = Scan::START;
    }

    /// Add the heart-rate recovery once it is measured, and read the table
    /// while no ride is open or being written.
    pub(super) fn service_facts(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let riding = self.ride.active().riding();
        self.facts.follow(bulk, self.recovery.reading(now), riding);
        let ready = self.ride.bulk.status().usable();
        let busy = self.ride.pending() || self.ride.open();
        self.facts.scan_step(bulk, now, ready, busy);
    }
}
