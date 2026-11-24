//! Ride done's owner: what the saved ride shows, worked out as the screen
//! opens, and its picture placed where the screen's tree puts it.
use super::Runtime;
use crate::compose::builtin::ScreenId;
use crate::compose::registry::PICTURE;
use crate::ride::log::Status;
use crate::screens::done::{Done, Kept};
use crate::screens::workout::Page;

impl Runtime {
    pub(super) fn ride_done(&self) -> Done {
        let zone = self.preferences.options.zone;
        let day = self
            .shown
            .live()
            .utc_ms
            .map(|ms| zone.local(ms / 1000).day());
        Done::new(
            day,
            super::course::crumbs(&self.track),
            self.ride.power_trace(),
            &self.preferences.profile,
        )
    }

    fn kept(&self) -> Kept {
        match self.ride.active().status() {
            _ if self.ride.pending() => Kept::Saving,
            Status::Recording | Status::Paused => Kept::Saving,
            Status::Error => Kept::Failed,
            _ if self.shown_ride_unsynced() => Kept::Unsynced,
            _ => Kept::Saved,
        }
    }

    pub(super) fn refresh_done(&mut self) {
        if self.navigation.page() != Page::Done {
            return;
        }
        let kept = self.kept();
        self.done.keep(kept);
        let Some(rect) = self
            .composition
            .laids()
            .get(ScreenId::RideDone)
            .and_then(|laid| laid.box_of(PICTURE))
        else {
            return;
        };
        let track = super::course::crumbs(&self.track);
        self.done.place(rect, track, &self.device.scaled);
    }
}
