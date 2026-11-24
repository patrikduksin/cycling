use super::Runtime;
use crate::ride::log::Status;
use crate::screens::chrome::Tone;
use crate::screens::ride::summary;
use crate::screens::workout::{Page, Text};
use crate::workout::ftp::{Offer, Source};
use core::fmt::Write;
use device_api::input::Role;

impl Runtime {
    pub(super) fn summary_offer(&self) -> Option<Offer> {
        if !self.ride_saved() {
            return None;
        }
        self.training
            .ftp
            .summary_offer(self.preferences.profile.ftp_watts)
    }

    pub(super) fn ride_saved(&self) -> bool {
        !self.ride.pending() && self.ride.active().status() == Status::Saved
    }

    pub(super) fn summary_pages(&self) -> usize {
        match self.navigation.page() {
            Page::Summary if self.summary_offer().is_some() => {
                summary::pages(self.has_elevation()) + 1
            }
            _ => summary::pages(self.has_elevation()),
        }
    }

    #[cfg(test)]
    pub(super) fn offer_shown(&self) -> bool {
        matches!(
            self.ftp_question(),
            Some(crate::workout::ftp::Question::Summary(_))
        )
    }

    pub(super) fn answer_ftp_offer(&mut self, button: Role, now: u64) {
        let Some(offer) = self.summary_offer() else {
            return;
        };
        let mut text = Text::<20>::new();
        let tone = match button {
            Role::Act => {
                let mut profile = self.preferences.profile;
                profile.ftp_watts = offer.suggested_watts;
                self.preferences.set(profile, now);
                let _ = write!(text, "FTP {} W SAVED", offer.suggested_watts);
                Tone::Good
            }
            Role::Back => {
                let _ = write!(text, "FTP KEPT {} W", offer.current_watts);
                Tone::Info
            }
            _ => return,
        };
        self.training.ftp.answered(Source::RideSummary);
        self.navigation.toast(text.as_str(), tone, now);
    }
}
