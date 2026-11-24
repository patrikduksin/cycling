//! The glue of one recording pass. `Runtime::record` runs `Ride`'s pass steps
//! and does between them what only the runtime can, in the order
//! `ride::current` describes.
use super::Runtime;
use crate::ride::log::{Sample, Status};
use crate::ride::medium::Medium;
use crate::ride::settle::{self, Saved};
use crate::ride::storage::RideStorage;
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;

impl Runtime {
    /// Where both ride journals stood when their boot scans ended; `None`
    /// while either still scans. A scan step writes nothing, so this is the
    /// media as they were at power-on.
    pub fn journals_at_boot(&self) -> Option<crate::ride::current::Journals> {
        self.ride.journals_at_boot()
    }

    pub fn recorder_busy(&self) -> bool {
        self.ride.busy()
    }

    /// Rides on a volatile journal that the phone has not confirmed: every
    /// controlled way of losing RAM waits until this is false.
    pub fn holds_unsynced(&self) -> bool {
        self.sync.count() > 0
    }

    /// Pause, resume or start the ride from its speed source (see `ride::auto`),
    /// through the same request path as the buttons.
    pub(super) fn auto_control(&mut self, now: u64) {
        // A ride moving to internal media keeps its automatic pause: the
        // continuation passes through Ready and Recording before it pauses.
        // An unread host completion is the host's own and stays put.
        if self.ride.pending() || self.ride.moving() {
            return;
        }
        // Not while the sheet is open: a hand on the screen is not a start,
        // and its hold starts again once the sheet closes. Rolling off rides
        // free, as only Home's workout card plays today's workout.
        let starts = matches!(self.navigation.page(), Page::Ride | Page::Home | Page::Done)
            && !self.sheet_open();
        let motion = self.readings.motion;
        let automatic = self.preferences.options.auto_pause;
        if let Some(call) = self.ride.auto_update(now, motion, starts, automatic) {
            self.ride_auto(call, now);
        }
    }

    pub(super) fn record(
        &mut self,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
        sample: Sample,
    ) {
        let clock = self.latency_clock;
        let clock = &|| clock.map_or(now, |clock| clock());
        let serviced = self.ride.service(store, bulk, now, sample, clock);
        for medium in serviced.reset.into_iter().flatten() {
            // The reset has reported its outcome already; a card that
            // refuses keeps the old records, as a card that is out does.
            let _ = settle::reset(medium, bulk);
        }
        if self.ride.open_pass(&serviced, now, &sample) {
            self.notices.clear_lap_toast();
            self.training.player = self.training.start();
            self.training.events.clear();
            // The last workout's summary belongs to the ride it ended with.
            self.training.last = None;
            self.ride
                .set_laps_by_distance(self.training.player.is_none());
        }
        self.protect_open_ride();
        let ended = self.ride.finish_pass();
        let (ride, player) = (&self.ride, self.training.player.as_ref());
        let saved = |medium: Medium| Saved {
            medium,
            summary: ride
                .recorder(medium)
                .summaries()
                .iter()
                .flatten()
                .last()
                .copied(),
            player,
        };
        let finished = ended.iter().any(Option::is_some);
        settle::settle(ended.map(|ended| ended.map(saved)), bulk);
        self.ride.after_settle(now);
        // The ride saved its workout's record: the next ride is free.
        if finished {
            self.leave_workout();
        }
        // Stamp milestones after the synchronous writes, so their cost shows:
        // storage once neither journal scans, recording once the ride does.
        let done = clock().max(now);
        let ride = &self.ride;
        if serviced.was_scanning && !ride.scanning() {
            let status = ride.recorder(ride.medium()).status();
            self.timing
                .storage(done, status.name(), status != Status::Error);
        }
        if ride.active().status() == Status::Recording {
            self.timing.recording(done);
        }
        self.ride_serviced(serviced.before, now);
    }
}
