//! Discarded and deleted rides in the app. The list, the ride count, the week
//! and FORM totals and the phone leave out the rides `kept::deleted` hides.
//! Without a FILES volume or a shelf nothing is hidden, and neither the
//! discard cue nor the delete page shows.
//!
//! A discarded ride is saved as any other, with a Finish record, and then
//! hidden; once hidden, its bests, facts and workout record are never
//! written. One that could not be hidden is kept with them, and a cut
//! between the two leaves a saved ride, which the rider can delete later. A delete from a saved ride's last page waits for the next service,
//! which holds the card.
use super::Runtime;
use crate::kept::deleted::{Reason, Record, Ride, Scan, Shelf};
use crate::ride::log::{Status, Summary};
use crate::ride::medium::Medium;
use crate::screens::chrome::Tone;
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;

impl Runtime {
    /// Lend storage for the hidden rides in memory,
    /// [`SHELF_BYTES`](crate::kept::deleted::SHELF_BYTES).
    pub fn attach_deleted_rides(&mut self, slots: &'static mut [u8]) {
        self.deleted.shelf = Some(Shelf::new(slots));
        self.deleted.scan = Scan::Unread;
    }

    /// The bulk log's store id, which a new log on the card changes; 0 for
    /// the internal journal.
    fn store(&self, medium: Medium) -> u64 {
        match medium {
            Medium::Internal => 0,
            Medium::Bulk => self.ride.bulk_media.log().map_or(0, |log| log.store_id()),
        }
    }

    /// The table's key for the saved ride `summary` of `medium`.
    fn hidden_key(&self, medium: Medium, summary: &Summary) -> Ride {
        Ride::new(
            medium.journal(),
            self.store(medium),
            summary.ride_id,
            summary.first_utc_ms,
            summary.active_ms,
        )
    }

    /// Whether the rider discarded or deleted this saved ride.
    pub(super) fn hidden(&self, medium: Medium, summary: &Summary) -> bool {
        self.deleted.hides(&self.hidden_key(medium, summary))
    }

    /// Whether the rider discarded or deleted the saved ride `ride_id` of
    /// `medium`, for the phone, which knows rides by id: the list's ride of
    /// that id decides, else any record of the id.
    pub(super) fn hidden_id(&self, medium: Medium, ride_id: u32) -> bool {
        match self.journal_ride_id(medium, ride_id) {
            Some(summary) => self.hidden(medium, &summary),
            None => self
                .deleted
                .shelf()
                .is_some_and(|shelf| shelf.names(medium.journal(), self.store(medium), ride_id)),
        }
    }

    fn hide(
        &mut self,
        bulk: &mut impl ReadWrite,
        medium: Medium,
        summary: &Summary,
        reason: Reason,
    ) -> bool {
        let record = Record {
            ride: self.hidden_key(medium, summary),
            reason,
            hidden_utc_s: self
                .readings
                .live
                .utc_ms
                .map_or(0, |ms| u32::try_from(ms / 1000).unwrap_or(0)),
        };
        self.deleted.save(bulk, &record).is_ok()
    }

    /// Hide the ride `medium` just saved, which the rider discarded; Home
    /// then shows no last ride. Returns whether it was hidden: one that was
    /// not is kept as any other saved ride.
    pub(super) fn ride_discarded(&mut self, bulk: &mut impl ReadWrite, medium: Medium) -> bool {
        let saved = self
            .ride
            .recorder(medium)
            .summaries()
            .iter()
            .flatten()
            .last()
            .copied();
        let hidden = saved.is_some_and(|saved| self.hide(bulk, medium, &saved, Reason::Discarded));
        if hidden {
            self.history.home_last = Some(0);
        }
        self.ride.discard = super::recording::Discard::Saved { hidden };
        hidden
    }

    /// Delete the saved ride on the summary's pages at the next service.
    pub(super) fn delete_past_ride(&mut self, medium: Medium, summary: Summary) {
        self.deleted.delete = Some((medium, summary));
    }

    /// Read the table when it is due, and carry out a delete the rider asked
    /// for.
    pub(super) fn service_deleted(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let status = self.ride.bulk.status();
        let ready = status.usable();
        self.deleted.card_in = !matches!(
            status,
            Status::Unsupported | Status::Unavailable | Status::Error
        );
        let busy = self.ride.pending();
        let asking = matches!(self.navigation.page, Page::EndRide | Page::History);
        self.deleted.scan_step(bulk, now, (ready, busy, asking));
        let Some((medium, summary)) = self.deleted.delete.take() else {
            return;
        };
        if self.navigation.page != Page::PastRide {
            return;
        }
        let newest = self
            .listed(0)
            .is_some_and(|(m, s)| m == medium && s.ride_id == summary.ride_id);
        if self.hide(bulk, medium, &summary, Reason::Deleted) {
            if newest {
                self.history.home_last = Some(0);
            }
            self.history_deleted(now);
            self.navigation.toast("RIDE DELETED", Tone::Info, now);
        } else {
            self.navigation.toast("NOT DELETED", Tone::Warn, now);
        }
        self.display.redraw();
    }
}
