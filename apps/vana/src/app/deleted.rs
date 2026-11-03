//! Discarded and deleted rides in the app. The table of hidden rides on FILES
//! is read onto the shelf the composition lends (PSRAM on the device) once
//! after boot, and again each time the card becomes ready. The list, the
//! ride count, the week and FORM totals and the phone then leave those rides
//! out. Without a FILES volume or a shelf nothing is hidden, and neither the
//! discard cue nor the delete page shows.
//!
//! A discarded ride is saved as any other, with a Finish record, and then
//! hidden; once hidden, its bests, facts and workout record are never
//! written. One that could not be hidden is kept with them, and a cut
//! between the two leaves a saved ride, which the rider can delete later. A delete from a saved ride's last page waits for the next service,
//! which holds the card.
use super::Runtime;
use super::recording::Medium;
use crate::ride::deleted::{self, Error, Reason, Record, Ride, Shelf};
use crate::ride::log::{Status, Summary};
use crate::screens::chrome::Tone;
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;
use firmware_services::{files, volume};

/// A failed read of the table is tried again this long after.
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scan {
    #[default]
    Unread,
    Retry {
        at: u64,
    },
    /// The shelf holds the table, or there is none yet.
    Read,
    /// No FILES volume: read again when a card is ready.
    Absent,
    /// A table this version does not write: it hides nothing and stays.
    Foreign,
}

#[derive(Default)]
pub(super) struct Hidden {
    shelf: Option<Shelf>,
    scan: Scan,
    /// Changes with the shelf, so lists and totals made from it are made
    /// again.
    pub(super) revision: u32,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
    /// A card is in and has not failed, whatever its ride log's state.
    card_in: bool,
    /// The saved ride the rider asked to delete, for the next service.
    delete: Option<(Medium, Summary)>,
    /// The page asks about rides: END RIDE? or the list.
    asking: bool,
}

impl Hidden {
    fn shelf(&self) -> Option<&Shelf> {
        self.shelf.as_ref().filter(|_| self.scan == Scan::Read)
    }

    fn hides(&self, ride: &Ride) -> bool {
        self.shelf().is_some_and(|shelf| shelf.hides(ride))
    }

    /// Whether a ride can be hidden now: the table is read, the card is in
    /// and the table has room.
    pub(super) fn writable(&self) -> bool {
        self.card_in && self.shelf().is_some_and(|shelf| !shelf.full())
    }

    /// Whether the list may count its rides: the table was read, or a read
    /// failed and nothing is hidden until one succeeds.
    pub(super) fn settled(&self) -> bool {
        self.shelf.is_none() || self.scan != Scan::Unread
    }

    fn save(&mut self, bulk: &mut impl ReadWrite, record: &Record) -> Result<(), Error> {
        let shelf = self.shelf.as_mut().filter(|_| self.scan == Scan::Read);
        let Some(shelf) = shelf else {
            return Err(Error::Table);
        };
        let saved = deleted::save(bulk, shelf, record);
        self.revision = self.revision.wrapping_add(1);
        if let Err(error) = saved {
            log::warn!(target: "sdk.deleted", "save_failed error={error:?}");
        }
        saved
    }

    /// A cleared or initialized journal numbers its rides from 1 again. A
    /// table not read yet is read first, so its records of that journal go.
    pub(super) fn forget(&mut self, bulk: &mut impl ReadWrite, medium: Medium, now: u64) {
        if matches!(self.scan, Scan::Unread | Scan::Retry { .. }) {
            self.read(bulk, now);
        }
        let Some(shelf) = self.shelf.as_mut().filter(|_| self.scan == Scan::Read) else {
            return;
        };
        if let Err(error) = deleted::forget(bulk, shelf, medium.journal()) {
            log::warn!(target: "sdk.deleted", "forget_failed error={error:?}");
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Read the table when it is due and no ride is being written. A card
    /// that became ready is read again, and so is one without a table this
    /// version writes once a page that asks about rides opens: the host may
    /// have made FILES since.
    fn scan_step(
        &mut self,
        bulk: &mut impl ReadWrite,
        now: u64,
        (ready, busy, asking): (bool, bool, bool),
    ) {
        let opened = asking && !self.asking;
        self.asking = asking;
        if ready && !self.card_ready || opened && matches!(self.scan, Scan::Absent | Scan::Foreign)
        {
            self.scan = Scan::Unread;
            self.revision = self.revision.wrapping_add(1);
        }
        self.card_ready = ready;
        let due = match self.scan {
            Scan::Unread => true,
            Scan::Retry { at } => now >= at,
            Scan::Read | Scan::Absent | Scan::Foreign => false,
        };
        if due && !busy {
            self.read(bulk, now);
        }
    }

    fn read(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let Some(shelf) = self.shelf.as_mut() else {
            return;
        };
        let before = self.scan;
        self.scan = match deleted::read(bulk, shelf) {
            Ok(_) => Scan::Read,
            Err(Error::Table) => Scan::Foreign,
            Err(error) if absent(error) => Scan::Absent,
            Err(_) => Scan::Retry {
                at: now.saturating_add(RETRY_MS),
            },
        };
        if self.scan != Scan::Read {
            shelf.clear();
        }
        if self.scan == Scan::Read || before != self.scan {
            self.revision = self.revision.wrapping_add(1);
        }
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
    /// Lend storage for the hidden rides in memory,
    /// [`deleted::SHELF_BYTES`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::tests::formatted;

    fn hidden() -> Hidden {
        Hidden {
            shelf: Some(Shelf::new(std::vec![0; deleted::SHELF_BYTES].leak())),
            ..Hidden::default()
        }
    }

    fn record(ride_id: u32) -> Record {
        Record {
            ride: Ride::new(b'b', 9, ride_id, None, 60_000),
            reason: Reason::Deleted,
            hidden_utc_s: 0,
        }
    }

    #[test]
    fn the_table_is_read_once_the_card_is_ready_and_no_ride_is_being_written() {
        let mut card = formatted();
        let mut kept = hidden();
        kept.card_in = true;
        assert!(!kept.settled());
        kept.scan_step(&mut card, 0, (true, true, false));
        assert!(!kept.settled(), "not while a ride is being written");
        kept.scan_step(&mut card, 0, (true, false, false));
        assert!(kept.settled() && kept.writable(), "no table yet");
        kept.save(&mut card, &record(4)).unwrap();
        assert!(kept.hides(&record(4).ride));

        let mut again = hidden();
        again.scan_step(&mut card, 0, (true, false, false));
        assert!(again.hides(&record(4).ride), "read back");

        let mut blank = crate::workout::library::tests::Region::default();
        let mut none = hidden();
        none.scan_step(&mut blank, 0, (false, false, false));
        assert_eq!(none.scan, Scan::Absent);
        assert!(none.settled() && !none.writable());
        assert!(none.save(&mut blank, &record(1)).is_err());
        none.card_in = true;
        none.scan_step(&mut card, 0, (false, false, true));
        assert!(
            none.hides(&record(4).ride),
            "FILES made since, read as END RIDE? opens"
        );
        assert!(
            none.writable(),
            "a card without a ride log still keeps FILES"
        );
        none.card_in = false;
        assert!(
            !none.writable() && none.hides(&record(4).ride),
            "card pulled"
        );
    }

    #[test]
    fn without_a_shelf_nothing_is_hidden_and_the_list_counts_at_once() {
        let mut card = formatted();
        let mut bare = Hidden::default();
        bare.scan_step(&mut card, 0, (true, false, false));
        assert!(bare.settled());
        assert!(!bare.writable());
        assert!(!bare.hides(&record(1).ride));
    }
}
