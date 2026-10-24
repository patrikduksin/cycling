//! Personal bests in the app: merging a saved ride into the rider's bests,
//! announcing them on the summary, the FTP offer they can lead to and the
//! BESTS view under Settings, PROFILE.
//!
//! The bests live in a small file beside the workout outcomes
//! (`ride::bests`), not in the preference record. Only a ride that was
//! recorded live counts (`Source::Live`, never a demo ride), and only a ride
//! that finished; a ride read back from a journal never merges again. A ride
//! is never held up for the file. Where it cannot be used the BESTS view says
//! why: no store to keep it in, a damaged file (CLEAR BESTS starts over), or a
//! store that would not take the write (the merged bests stay in memory and
//! the next write keeps them). A read that failed only this time is tried
//! again, with the ride's outcome waiting.
use super::Runtime;
use super::recording::{Medium, Ride};
use crate::clock::Zone;
use crate::ride::bests::{self, Bests, Found, Outcome, suggest_ftp};
use crate::ride::log::{Source, Status};
use crate::ride::zones::TrainingProfile;
use crate::screens::bests::Mode;
use crate::screens::chrome::Tone;
use crate::screens::ride::summary;
use crate::screens::workout::{Page, Text};
use core::fmt::Write;
use device_api::bulk::ReadWrite;
use device_api::input::Button;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Store {
    /// Not read yet, or to be read again.
    #[default]
    Unread,
    Kept,
    /// No card, no FILES volume or no formatted file store.
    Unavailable,
    /// A file is there that does not decode; it is left alone.
    Damaged,
    /// The store would not take a write: the bests are in memory only.
    NotSaved,
}

/// A failed read or write is tried again this long after.
const RETRY_MS: u64 = 5_000;

/// The bests in force and what the BESTS view is doing with them.
#[derive(Default)]
pub(super) struct Kept {
    pub(super) bests: Bests,
    store: Store,
    /// BESTS is on screen, so it reads the file again once on opening.
    open: bool,
    /// ACT asked to clear them; the rider has yet to confirm.
    confirming: bool,
    /// The rider confirmed: the next service clears the file.
    clear: bool,
    /// A finished ride's outcome whose read failed this time.
    pending: Option<(Outcome, u16)>,
    /// `bests` hold a merge the store has not taken yet.
    unsaved: bool,
    retry_at: u64,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
}

impl Kept {
    /// The BESTS row's value in Settings.
    pub(super) fn summary(&self) -> Text<38> {
        let mut text = Text::new();
        let _ = match (self.store, self.bests.count()) {
            (Store::Unavailable, _) => text.push_str("NOT KEPT"),
            (Store::Damaged, _) => text.push_str("DAMAGED"),
            (Store::NotSaved, _) => text.push_str("NOT SAVED"),
            (Store::Unread, _) => Ok(()),
            (Store::Kept, 0) => text.push_str("NONE YET"),
            (Store::Kept, count) => write!(text, "{count} SET"),
        };
        text
    }

    pub(super) fn mode(&self) -> Mode {
        match self.store {
            _ if self.confirming => Mode::Confirm,
            Store::Unavailable => Mode::Unavailable,
            Store::Damaged => Mode::Damaged,
            Store::NotSaved => Mode::NotSaved,
            _ => Mode::List,
        }
    }

    /// Take a ride's outcome into the stored bests; returns what it set,
    /// which is nothing when the store could not be used.
    fn merge(&mut self, media: &mut impl ReadWrite, outcome: Outcome, day: u16, now: u64) -> Found {
        let base = if self.unsaved {
            Ok(self.bests)
        } else {
            bests::load(media)
        };
        let mut bests = match base {
            Ok(bests) => bests,
            Err(bests::Error::Media) => {
                log::warn!(target: "sdk.bests", "load_failed error=Media");
                self.pending = Some((outcome, day));
                self.retry_at = now.saturating_add(RETRY_MS);
                return Found::default();
            }
            Err(bests::Error::Damaged) => {
                self.store = Store::Damaged;
                return Found::default();
            }
            Err(_) => {
                self.store = Store::Unavailable;
                return Found::default();
            }
        };
        self.pending = None;
        let found = bests.merge(&outcome, day);
        self.bests = bests;
        if !found.any() && !self.unsaved {
            self.store = Store::Kept;
            return Found::default();
        }
        self.write(media, now);
        if self.unsaved {
            Found::default()
        } else {
            found
        }
    }

    /// Write the bests; if the store refuses, keep them in memory.
    fn write(&mut self, media: &mut impl ReadWrite, now: u64) {
        match bests::save(media, &self.bests) {
            Ok(()) => {
                self.store = Store::Kept;
                self.unsaved = false;
            }
            Err(bests::Error::Unavailable) => self.store = Store::Unavailable,
            Err(error) => {
                log::warn!(target: "sdk.bests", "save_failed error={error:?}");
                self.store = Store::NotSaved;
                self.unsaved = true;
                self.retry_at = now.saturating_add(RETRY_MS);
            }
        }
    }
}

/// What a live ride that just finished on `medium` adds up to, and the day
/// it started (0 when the clock was unknown).
fn outcome(ride: &Ride, medium: Medium, zone: Zone, now: u64) -> Option<(Outcome, u16)> {
    let saved = ride
        .recorder(medium)
        .summaries()
        .iter()
        .flatten()
        .last()
        .copied()?;
    if saved.source != Some(Source::Live) {
        return None;
    }
    let day = saved.first_utc_ms.map_or(0, |utc| {
        (zone.local(utc / 1000).0 / 86_400).min(u64::from(u16::MAX)) as u16
    });
    let clamp = |value: u64| value.min(u64::from(u32::MAX)) as u32;
    let totals = &ride.totals;
    let outcome = Outcome {
        power: ride.efforts.best(),
        distance_m: clamp(totals.distance_mm() / 1000),
        time_s: clamp(ride.active_ms(now) / 1000),
        // As the summary shows it: whole metres.
        ascent_m: (totals.ascent_dm() + 5) / 10,
    };
    Some((outcome, day))
}

/// Merge the ride that just finished on `medium` into the stored bests,
/// keeping what it set for the summary and its 20 min best for the FTP
/// offer. A ride that could not be stored claims no best.
pub(super) fn settle(
    ride: &mut Ride,
    kept: &mut Kept,
    media: &mut impl ReadWrite,
    medium: Medium,
    zone: Zone,
    now: u64,
) {
    ride.bests_found = Found::default();
    ride.ftp_suggestion = None;
    let Some((outcome, day)) = outcome(ride, medium, zone, now) else {
        return;
    };
    let found = kept.merge(media, outcome, day, now);
    ride.bests_found = found;
    if found.power[3] {
        ride.ftp_suggestion = Some(kept.bests.power[3].value.min(u32::from(u16::MAX)) as u16);
    }
}

/// The toast that opens the summary of a ride that set bests: the one best,
/// or how many. Every message fits a toast whole (see the test).
pub(super) fn news(found: &Found, bests: &Bests) -> Option<Text<24>> {
    let mut text = Text::new();
    match found.count() {
        0 => return None,
        1 => {
            if let Some(index) = found.power.iter().position(|set| *set) {
                let label = summary::effort_label(index);
                let watts = bests.power[index].value;
                let _ = write!(text, "BEST {} {watts} W", label.as_str());
            } else if found.distance {
                let _ = text.push_str("NEW LONGEST RIDE");
            } else if found.time {
                let _ = text.push_str("NEW LONGEST TIME");
            } else {
                let _ = text.push_str("NEW MOST ASCENT");
            }
        }
        count => {
            let _ = write!(text, "{count} NEW BESTS");
        }
    }
    Some(text)
}

impl Runtime {
    /// The FTP offer the summary shows as its last page: once the ride is
    /// saved, while it is still higher than the FTP in force by enough, and
    /// until the rider answers or leaves.
    pub(super) fn summary_offer(&self) -> Option<summary::FtpOffer> {
        let ride = &self.ride;
        let best = ride.ftp_suggestion?;
        if ride.pending() || ride.active().status() != Status::Saved {
            return None;
        }
        let current = self.preferences.profile.ftp_watts;
        let suggested =
            suggest_ftp(best, current).filter(|ftp| TrainingProfile::FTP_WATTS.contains(ftp))?;
        Some(summary::FtpOffer {
            suggested,
            current,
            best,
        })
    }

    /// How many pages the summary on screen has.
    pub(super) fn summary_pages(&self) -> usize {
        match self.navigation.page {
            Page::Summary if self.summary_offer().is_some() => {
                summary::pages(self.has_elevation()) + 1
            }
            _ => summary::pages(self.has_elevation()),
        }
    }

    /// Whether the summary shows the FTP offer now.
    pub(super) fn offer_shown(&self) -> bool {
        self.navigation.page == Page::Summary
            && self.navigation.summary_page == summary::pages(self.has_elevation())
            && self.summary_offer().is_some()
    }

    /// ACT takes the suggested FTP, BACK keeps the one in force. Either way
    /// the summary's own press then finishes it, as on any page.
    pub(super) fn answer_ftp_offer(&mut self, button: Button, now: u64) {
        let Some(offer) = self.summary_offer() else {
            return;
        };
        let mut text = Text::<20>::new();
        let tone = match button {
            Button::BottomRight => {
                let mut profile = self.preferences.profile;
                profile.ftp_watts = offer.suggested;
                self.preferences.set(profile, now);
                let _ = write!(text, "FTP {} W SAVED", offer.suggested);
                Tone::Good
            }
            Button::TopLeft => {
                let _ = write!(text, "FTP KEPT {} W", offer.current);
                Tone::Info
            }
            _ => return,
        };
        self.ride.ftp_suggestion = None;
        self.navigation.toast(text.as_str(), tone, now);
    }

    /// Open the BESTS view from Settings.
    pub(super) fn open_bests(&mut self, now: u64) {
        self.navigation.page = Page::Bests;
        self.navigation.page_since = now;
    }

    /// BACK returns to Settings. ACT asks to clear the bests, and then ACT
    /// confirms while BACK keeps them.
    pub(super) fn bests_input(&mut self, button: Button, held: bool) {
        let kept = &mut self.bests;
        match button {
            Button::BottomRight if kept.confirming => {
                kept.confirming = false;
                kept.clear = true;
            }
            Button::BottomRight
                if kept.store == Store::Damaged
                    || (kept.store == Store::Kept && kept.bests.count() != 0) =>
            {
                kept.confirming = true;
            }
            Button::TopLeft if kept.confirming => kept.confirming = false,
            Button::TopLeft if !held => self.navigation.page = Page::Settings,
            _ => {}
        }
    }

    /// Read the bests at boot, again when BESTS opens or the card becomes
    /// ready, and retry what failed; clear them once confirmed. Never during
    /// a ride.
    pub(super) fn service_bests(&mut self, media: &mut impl ReadWrite, now: u64) {
        let open = self.navigation.page == Page::Bests;
        let ready = self.ride.bulk.status().usable();
        let kept = &mut self.bests;
        if open && !kept.open {
            if kept.unsaved {
                kept.retry_at = 0;
            } else {
                kept.store = Store::Unread;
            }
        }
        if ready && !kept.card_ready && kept.store == Store::Unavailable {
            kept.store = Store::Unread;
        }
        kept.card_ready = ready;
        kept.open = open;
        if !open {
            kept.confirming = false;
        }
        if self.ride.pending() || self.ride.open() {
            return;
        }
        if kept.clear {
            kept.clear = false;
            match bests::save(media, &Bests::default()) {
                Ok(()) => {
                    *kept = Kept {
                        store: Store::Kept,
                        open,
                        card_ready: ready,
                        ..Kept::default()
                    };
                }
                Err(error) => {
                    log::warn!(target: "sdk.bests", "clear_failed error={error:?}");
                    self.navigation.toast("CLEAR FAILED", Tone::Alert, now);
                }
            }
            self.display.redraw();
        }
        let kept = &mut self.bests;
        if now < kept.retry_at {
            return;
        }
        if kept.unsaved && kept.pending.is_none() && open {
            kept.write(media, now);
            self.display.redraw();
        } else if let Some((outcome, day)) = kept.pending {
            kept.merge(media, outcome, day, now);
            self.display.redraw();
        } else if kept.store == Store::Unread {
            match bests::load(media) {
                Ok(bests) => {
                    kept.bests = bests;
                    kept.store = Store::Kept;
                }
                Err(bests::Error::Media) => kept.retry_at = now.saturating_add(RETRY_MS),
                Err(bests::Error::Damaged) => kept.store = Store::Damaged,
                Err(_) => kept.store = Store::Unavailable,
            }
            self.display.redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::bests::{Mark, PLAUSIBLE_WATTS};
    use crate::screens::chrome::Toast;

    fn fits(text: &Text<24>) {
        let toast = Toast::new(text.as_str(), Tone::Good);
        assert_eq!(toast.text(), text.as_bytes(), "{:?} is cut", text.as_str());
    }

    #[test]
    fn every_bests_message_fits_a_toast_whole() {
        for (index, limit) in PLAUSIBLE_WATTS.into_iter().enumerate() {
            let mut found = Found::default();
            found.power[index] = true;
            let mut bests = Bests::default();
            bests.power[index] = Mark {
                value: limit,
                day: 0,
            };
            fits(&news(&found, &bests).unwrap());
        }
        let single = [
            Found {
                distance: true,
                ..Found::default()
            },
            Found {
                time: true,
                ..Found::default()
            },
            Found {
                ascent: true,
                ..Found::default()
            },
        ];
        for found in single {
            fits(&news(&found, &Bests::default()).unwrap());
        }
        let all = Found {
            power: [true; 4],
            distance: true,
            time: true,
            ascent: true,
        };
        let text = news(&all, &Bests::default()).unwrap();
        assert_eq!(text.as_str(), "7 NEW BESTS");
        fits(&text);
        assert!(news(&Found::default(), &Bests::default()).is_none());
    }

    #[test]
    fn a_single_best_names_its_window_and_value() {
        let mut found = Found::default();
        found.power[2] = true;
        let mut bests = Bests::default();
        bests.power[2] = Mark { value: 312, day: 0 };
        assert_eq!(news(&found, &bests).unwrap().as_str(), "BEST 5 MIN 312 W");
    }
}
