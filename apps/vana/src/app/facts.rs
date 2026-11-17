//! Ride facts in the app: written again once a ride's heart-rate recovery is
//! known, and read back for the training stress of the rides list and FORM,
//! and for a saved ride's summary.
use super::Runtime;
use crate::kept::facts::record::Shelf;
use device_api::bulk::ReadWrite;

impl Runtime {
    /// Lend storage for the ride facts in memory,
    /// [`SHELF_BYTES`](crate::kept::facts::record::SHELF_BYTES), so FORM and the
    /// rides list find each ride's.
    pub fn attach_ride_facts(&mut self, slots: &'static mut [u8]) {
        self.facts.attach(Shelf::new(slots));
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
