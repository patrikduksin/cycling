//! Rides on a volatile journal that the phone has not confirmed.
//!
//! A ride is protected from the pass its recording starts on a volatile
//! medium, a continuation segment included, until the phone confirms a whole
//! copy of the newest export by its receipt and FIT CRC, or until the rider
//! confirms a CLEAR that discards it. While any ride is protected, VANA shows
//! NOT SAVED and refuses every controlled way of losing RAM.

use super::Runtime;
use super::format::{Event, Scope};
use super::navigation::To;
use super::storage::Store;
use crate::ride::current::VOLATILE_STARTS;
use crate::ride::medium::Medium;
use crate::screens::workout::Page;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Protected {
    pub(crate) id: u32,
    /// The newest export's receipt, which a confirmation must quote.
    pub(crate) receipt: Option<u32>,
    /// The file CRC the newest export's writing pass wrote.
    pub(crate) crc: Option<u16>,
}

#[derive(Default)]
pub(super) struct Sync {
    protected: heapless::Vec<Protected, VOLATILE_STARTS>,
    /// The host's CLEAR waiting on LOSE n UNSAVED RIDES?, by its token.
    asked: Option<u32>,
}

state!(Sync { protected, asked });

/// What the phone's acknowledgment did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Confirmed {
    /// The ride left the protected set.
    Synced,
    /// The ride is not protected: synced already, or cleared.
    Unknown,
    /// The receipt or the CRC is not the newest export's.
    Mismatch,
}

impl Sync {
    pub(super) fn count(&self) -> usize {
        self.protected.len()
    }

    pub(super) fn protects(&self, medium: Medium, id: u32) -> bool {
        medium == Medium::Internal && self.protected.iter().any(|ride| ride.id == id)
    }

    pub(super) fn asked(&self) -> Option<u32> {
        self.asked
    }

    /// Protect ride `id`; protecting it again changes nothing.
    fn protect(&mut self, id: u32) {
        if self.protected.iter().all(|ride| ride.id != id) {
            let fresh = Protected {
                id,
                receipt: None,
                crc: None,
            };
            let pushed = self.protected.push(fresh);
            debug_assert!(pushed.is_ok(), "more protected rides than starts");
        }
    }

    /// The phone was sent ride `id` under `receipt`, a file ending in `crc`;
    /// only this newest export can confirm it.
    pub(super) fn exported(&mut self, id: u32, receipt: u32, crc: u16) {
        if let Some(ride) = self.protected.iter_mut().find(|ride| ride.id == id) {
            ride.receipt = Some(receipt);
            ride.crc = Some(crc);
        }
    }

    pub(super) fn confirm(&mut self, id: u32, receipt: u32, crc: u16) -> Confirmed {
        let Some(at) = self.protected.iter().position(|ride| ride.id == id) else {
            return Confirmed::Unknown;
        };
        let ride = self.protected[at];
        if ride.receipt != Some(receipt) || ride.crc != Some(crc) {
            return Confirmed::Mismatch;
        }
        self.protected.swap_remove(at);
        Confirmed::Synced
    }
}

impl Runtime {
    /// Protect the ride a volatile journal has open, from the pass it starts.
    pub(super) fn protect_open_ride(&mut self) {
        if let Some(id) = self.ride.volatile_ride() {
            self.sync.protect(id);
        }
    }

    /// The newest ride on the journal the ride pages show waits for the
    /// phone: its summary says SYNC TO SAVE, not RIDE SAVED.
    pub(super) fn shown_ride_unsynced(&self) -> bool {
        let medium = self.ride.shown();
        let newest = self.ride.recorder(medium).next_ride_id().wrapping_sub(1);
        self.sync.protects(medium, newest)
    }

    /// DISCARD RIDE? is offered: FILES can note the hidden ride, and the
    /// ride is not protected.
    pub(super) fn can_discard(&self) -> bool {
        self.deleted.writable()
            && self
                .ride
                .volatile_ride()
                .is_none_or(|id| !self.sync.protects(Medium::Internal, id))
    }

    /// The host asked to clear the internal journal under `token`. With
    /// unsaved rides the rider answers LOSE n UNSAVED RIDES? first.
    pub(super) fn clear_internal_rides(&mut self, token: u32) {
        if self.sync.count() == 0 {
            return self.clear_confirmed(token);
        }
        self.sync.asked = Some(token);
        self.navigation.go(To::Page(Page::LoseRides));
        self.display.redraw();
    }

    fn clear_confirmed(&mut self, token: u32) {
        self.format_event(Event::Confirm {
            scope: Scope::one(Store::Internal),
            clear: Some(token),
        });
    }

    /// Yes discards the unsaved rides with the journal; no keeps them and
    /// refuses the host's CLEAR.
    pub(super) fn lose_rides_input(&mut self, yes: bool) {
        let Some(token) = self.sync.asked else {
            return;
        };
        match yes {
            true => {
                self.sync = Sync::default();
                self.clear_confirmed(token);
            }
            false => {
                self.sync.asked = None;
                self.ride.refuse_clear(token);
                let page = self.first_page();
                self.navigation.go(To::Page(page));
                self.display.redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_export_confirms_a_ride() {
        let mut sync = Sync::default();
        sync.protect(3);
        sync.protect(3);
        assert_eq!(sync.count(), 1);
        assert_eq!(sync.confirm(3, 7, 0xbeef), Confirmed::Mismatch);
        sync.exported(3, 7, 0xbeef);
        sync.exported(3, 8, 0xbeef);
        assert_eq!(sync.confirm(3, 7, 0xbeef), Confirmed::Mismatch);
        assert_eq!(sync.confirm(3, 8, 0xbeee), Confirmed::Mismatch);
        assert!(sync.protects(Medium::Internal, 3));
        assert!(!sync.protects(Medium::Bulk, 3));
        assert_eq!(sync.confirm(3, 8, 0xbeef), Confirmed::Synced);
        assert_eq!(sync.confirm(3, 8, 0xbeef), Confirmed::Unknown);
        assert_eq!(sync.count(), 0);
    }
}
