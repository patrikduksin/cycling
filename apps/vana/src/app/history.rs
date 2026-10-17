//! Ride history: the saved rides of both journals as a list, and one of them
//! read back onto the summary's pages.
//!
//! The list comes from the ride index the composition lends (every saved
//! ride of both journals, built in the background), plus the summaries each
//! journal's catalog keeps for rides the index has not reached yet. Without
//! an index only those catalog summaries (the last four per medium) show.
//! Entries are looked up on demand, so nothing is copied. Opening a ride replays
//! its journal records a few slots per tick, read-only, and only while that
//! journal is idle, so recording is never held up and nothing persisted
//! changes. The replay adds up into the ride sums the live ride uses: rides
//! are reachable only with no ride open, and a new ride clears them anyway.
//! Home keeps the last ride's line from before the first replay.
use super::Runtime;
use super::recording::Medium;
use crate::ride::directory::{Journal, RideIndex};
use crate::ride::log::{Status, Summary};
use crate::ride::replay::{Progress, Replay, Sums};
use crate::screens::history::{self, List, Row};
use crate::screens::ride::summary::{After, Saving};
use crate::screens::workout::{Page, Text, View};
use core::fmt::Write;
use device_api::input::Button;

/// Slots read per tick: internal flash reads are quick, the MMC slower.
const INTERNAL_BUDGET: usize = 32;
const BULK_BUDGET: usize = 8;

#[derive(Default)]
pub(super) struct History {
    /// Every saved ride of both journals, in storage the composition lends.
    pub(super) index: Option<&'static mut RideIndex>,
    /// The list was opened once: from then on the index reads the journals
    /// in the background. Until then boot and rides read nothing extra.
    indexing: bool,
    pub(super) cursor: usize,
    /// The ride on the summary's pages: its journal, its replay and its line.
    open: Option<(Medium, Replay, Summary)>,
    /// The ride the phone screen was opened for.
    pub(super) phone: Option<(Medium, u32)>,
    /// Home's last-ride distance, kept from before a replay took over the
    /// ride sums; cleared when a new ride starts.
    pub(super) home_last: Option<u64>,
}

impl Runtime {
    /// Lend storage for the ride index, so the list holds every saved ride.
    pub fn attach_ride_index(&mut self, index: &'static mut RideIndex) {
        self.history.index = Some(index);
    }

    fn indexed(&self, medium: Medium) -> Option<&Journal> {
        let index = self.history.index.as_deref()?;
        Some(match medium {
            Medium::Internal => &index.internal,
            Medium::Bulk => &index.bulk,
        })
    }

    /// One journal's rides: how many the index holds, and the first catalog
    /// summary newer than all of them.
    fn split(&self, medium: Medium) -> (usize, usize) {
        let recorder = self.ride.recorder(medium);
        let kept = usize::from(recorder.summary_count());
        let Some(journal) = self.indexed(medium) else {
            return (0, 0);
        };
        let last = journal.last_ride_id();
        let newer = (0..kept)
            .find(|&i| {
                recorder
                    .summary(i)
                    .is_some_and(|s| last.is_none_or(|last| s.ride_id > last))
            })
            .unwrap_or(kept);
        (journal.len(), newer)
    }

    /// Saved rides of one journal.
    fn journal_len(&self, medium: Medium) -> usize {
        let recorder = self.ride.recorder(medium);
        if matches!(recorder.status(), Status::Scanning | Status::NeedsInit) {
            return 0;
        }
        let (indexed, newer) = self.split(medium);
        indexed + usize::from(recorder.summary_count()) - newer
    }

    /// The `index`-th saved ride of one journal, oldest first.
    fn journal_ride(&self, medium: Medium, index: usize) -> Option<Summary> {
        let (indexed, newer) = self.split(medium);
        if index < indexed {
            return self.indexed(medium)?.get(index);
        }
        self.ride
            .recorder(medium)
            .summary(newer + index - indexed)
            .copied()
    }

    /// The `n`-th newest saved ride of both journals. Each journal's own
    /// order is certain (oldest first), so the two are merged newest first,
    /// keeping each journal's order; start times decide between journals
    /// where both are known.
    fn listed(&self, n: usize) -> Option<(Medium, Summary)> {
        let mut left = [
            self.journal_len(Medium::Internal),
            self.journal_len(Medium::Bulk),
        ];
        let media = [Medium::Internal, Medium::Bulk];
        for taken in 0.. {
            let head = |at: usize| {
                left[at]
                    .checked_sub(1)
                    .and_then(|i| self.journal_ride(media[at], i))
            };
            let (internal, bulk) = (head(0), head(1));
            let pick = match (&internal, &bulk) {
                (Some(a), Some(b)) => match (a.first_utc_ms, b.first_utc_ms) {
                    (Some(a), Some(b)) if b > a => 1,
                    _ => 0,
                },
                (Some(_), None) => 0,
                (None, Some(_)) => 1,
                (None, None) => return None,
            };
            if taken == n {
                return [internal, bulk][pick].map(|summary| (media[pick], summary));
            }
            left[pick] -= 1;
        }
        None
    }

    /// Show the saved rides, newest first, with the newest focused.
    pub(super) fn open_history(&mut self) {
        self.history.indexing = true;
        self.history.cursor = 0;
        self.history.open = None;
        self.navigation.page = Page::History;
    }

    pub(super) fn history_count(&self) -> usize {
        self.journal_len(Medium::Internal) + self.journal_len(Medium::Bulk)
    }

    /// Index a few more saved rides; the list redraws when one is added.
    pub(super) fn directory_step(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
    ) {
        if !self.history.indexing {
            return;
        }
        let Some(index) = self.history.index.as_deref_mut() else {
            return;
        };
        if self.ride.directory_step(index, store, bulk) && self.navigation.page == Page::History {
            self.display.redraw();
        }
    }

    /// MOVE focuses the next ride (held, the next page of cards), ACT opens
    /// it, BACK goes Home.
    pub(super) fn history_input(&mut self, button: Button, held: bool) {
        let count = self.history_count();
        match button {
            Button::BottomLeft if count > 0 => {
                let step = if held { history::SHOWN } else { 1 };
                self.history.cursor = if self.history.cursor + 1 >= count {
                    0
                } else {
                    (self.history.cursor + step).min(count - 1)
                };
            }
            Button::BottomRight if count > 0 => self.open_past_ride(self.history.cursor),
            Button::TopLeft => self.navigation.page = Page::Home,
            _ => {}
        }
    }

    pub(super) fn open_past_ride(&mut self, index: usize) {
        let Some((medium, summary)) = self.listed(index) else {
            return;
        };
        let ride_id = summary.ride_id;
        if self.ride.active().riding() {
            return;
        }
        // The sums are about to hold this ride: keep Home's last ride.
        if self.history.home_last.is_none() {
            let totals = &self.ride.totals;
            self.history.home_last = Some(totals.distance_mm());
        }
        self.sums().clear();
        self.history.cursor = index;
        let upper = self.ride.recorder(medium).next_slot();
        self.history.open = Some((medium, Replay::new(ride_id, upper), summary));
        self.navigation.summary_page = 0;
        self.navigation.page = Page::PastRide;
    }

    fn sums(&mut self) -> Sums<'_> {
        let ride = &mut self.ride;
        Sums {
            totals: &mut ride.totals,
            zone_time: &mut ride.zone_time,
            efforts: &mut ride.efforts,
        }
    }

    /// MOVE turns the page, BACK returns to the list, ACT sends the ride to
    /// the phone.
    pub(super) fn past_ride_input(&mut self, button: Button, held: bool) {
        match button {
            Button::BottomLeft => self.next_summary_page(1),
            Button::TopLeft if !held => self.navigation.page = Page::History,
            Button::BottomRight if !held => {
                let Some((medium, _, summary)) = self.history.open else {
                    return;
                };
                self.history.phone = Some((medium, summary.ride_id));
                self.navigation.page = Page::Phone;
            }
            _ => {}
        }
    }

    /// BACK from the phone screen returns where it was opened from.
    pub(super) fn phone_input(&mut self, button: Button, held: bool) {
        if button != Button::TopLeft || held {
            return;
        }
        self.navigation.page = if self.history.phone.take().is_some() {
            Page::PastRide
        } else {
            Page::Home
        };
    }

    /// A new ride takes the sums back; Home shows it as the last ride.
    pub(super) fn history_ride_started(&mut self) {
        self.history.open = None;
        self.history.home_last = None;
    }

    /// Read some more of the open ride, while its journal is idle.
    pub(super) fn replay_step(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
    ) {
        if self.navigation.page != Page::PastRide {
            return;
        }
        let Some((medium, mut replay, summary)) = self.history.open else {
            return;
        };
        if replay.progress() != Progress::Reading || self.ride.active().riding() {
            return;
        }
        let recorder = self.ride.recorder(medium);
        if !recorder.exportable() {
            // A failed journal stays unreadable until VANA restarts.
            if matches!(recorder.status(), Status::Error | Status::NeedsInit) {
                replay.abandon();
                self.history.open = Some((medium, replay, summary));
                self.display.redraw();
            }
            return;
        }
        let budget = match medium {
            Medium::Internal => INTERNAL_BUDGET,
            Medium::Bulk => BULK_BUDGET,
        };
        let profile = self.preferences.profile;
        self.ride
            .replay(medium, store, bulk, &mut replay, budget, &profile);
        self.history.open = Some((medium, replay, summary));
        if replay.progress() != Progress::Reading {
            self.display.redraw();
        }
    }

    /// The ride list, its cards made on demand.
    pub(super) fn history_canvas(&self) -> crate::screens::canvas::Canvas {
        let count = self.history_count();
        history::prepare(&List {
            count,
            cursor: self.history.cursor,
            row: |index| self.history_row(index, count),
        })
    }

    fn history_row(&self, index: usize, count: usize) -> Row {
        let Some((_, summary)) = self.listed(index) else {
            return Row::default();
        };
        let summary = &summary;
        let mut row = Row {
            title: title(summary, count.saturating_sub(index)),
            ..Row::default()
        };
        let mut time = Text::<12>::new();
        crate::screens::ride::elapsed(&mut time, summary.active_ms / 1000);
        let _ = row.detail.push_str(time.as_str());
        if let Some(distance) = summary.distance_mm {
            let mut km = Text::<12>::new();
            crate::screens::ride::kilometres(&mut km, distance);
            let _ = write!(row.detail, "  {} KM", km.as_str());
        }
        if let Some(bpm) = summary.heart_average {
            let _ = write!(row.heart, "{bpm} BPM");
        }
        row
    }

    /// The open ride on the summary's pages.
    pub(super) fn past_view(&self, view: &mut View) {
        let Some((_, replay, summary)) = &self.history.open else {
            return;
        };
        view.past_title = title(
            summary,
            self.history_count().saturating_sub(self.history.cursor),
        );
        let summary = Some(summary);
        let saving = match replay.progress() {
            Progress::Reading => Saving::Reading,
            Progress::Done => Saving::Saved,
            Progress::Missing => Saving::Unreadable,
        };
        view.totals = self.ride.totals.with_ascent_dm(replay.ascent_dm());
        view.zone_time = self.ride.zone_time;
        let active_ms = summary.map_or(0, |s| s.active_ms).max(replay.active_ms);
        view.active_secs = active_ms / 1000;
        view.after = After {
            page: self.navigation.summary_page,
            saving,
            best: self.ride.efforts.best(),
            climbs: None,
            laps: None,
            past: true,
        };
    }

    /// Home's RIDES card: the last ride's numbers, or how many are saved.
    pub(super) fn rides_line(&self) -> Text<20> {
        let distance = self
            .history
            .home_last
            .unwrap_or_else(|| self.ride.totals.distance_mm());
        if !self.recording() && distance > 0 {
            return crate::screens::home::last_ride(distance);
        }
        let saved = self.history_count();
        let mut line = Text::new();
        let _ = match saved {
            0 => line.push_str("NONE YET"),
            n => write!(line, "{n} SAVED"),
        };
        line
    }
}

/// When a ride started, or RIDE n when the clock was unknown.
fn title(summary: &Summary, number: usize) -> Text<20> {
    match summary.first_utc_ms {
        Some(utc) => history::started(utc),
        None => {
            let mut text = Text::new();
            let _ = write!(text, "RIDE {number}");
            text
        }
    }
}
