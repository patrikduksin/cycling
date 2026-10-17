//! Ride history: the saved rides of both journals as a list, and one of them
//! read back onto the summary's pages.
//!
//! The list comes from the summaries each journal already keeps (the last
//! four per medium), referenced rather than copied. Opening a ride replays
//! its journal records a few slots per tick, read-only, and only while that
//! journal is idle, so recording is never held up and nothing persisted
//! changes. The replay adds up into the ride sums the live ride uses: rides
//! are reachable only with no ride open, and a new ride clears them anyway.
//! Home keeps the last ride's line from before the first replay.
use super::Runtime;
use super::recording::Medium;
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
    /// Newest first: each ride's journal and its summary's index there.
    order: heapless::Vec<(Medium, u8), { history::CAPACITY }>,
    pub(super) cursor: usize,
    open: Option<(Medium, Replay)>,
    /// The ride the phone screen was opened for.
    pub(super) phone: Option<(Medium, u32)>,
    /// Home's last-ride distance, kept from before a replay took over the
    /// ride sums; cleared when a new ride starts.
    pub(super) home_last: Option<u64>,
}

impl Runtime {
    fn saved(&self, medium: Medium, index: u8) -> Option<&Summary> {
        self.ride.recorder(medium).summary(usize::from(index))
    }

    fn listed(&self, index: usize) -> Option<&Summary> {
        let (medium, at) = *self.history.order.get(index)?;
        self.saved(medium, at)
    }

    /// Show the saved rides, newest first, with the newest focused.
    pub(super) fn open_history(&mut self) {
        // Each journal's own order is certain (oldest first), so the two are
        // merged newest first, keeping each journal's order; start times
        // decide between journals where both are known.
        let newest = |medium: Medium| {
            let recorder = self.ride.recorder(medium);
            let listed = !matches!(recorder.status(), Status::Scanning | Status::NeedsInit);
            (0..crate::ride::log::HISTORY_CAPACITY as u8)
                .rev()
                .filter(move |&index| listed && recorder.summary(usize::from(index)).is_some())
                .map(move |index| (medium, index))
                .peekable()
        };
        let (mut internal, mut bulk) = (newest(Medium::Internal), newest(Medium::Bulk));
        let started = |&(medium, index): &(Medium, u8)| {
            self.saved(medium, index).and_then(|s| s.first_utc_ms)
        };
        let mut order: heapless::Vec<(Medium, u8), { history::CAPACITY }> = heapless::Vec::new();
        loop {
            let next = match (internal.peek(), bulk.peek()) {
                (Some(a), Some(b)) => match (started(a), started(b)) {
                    (Some(a), Some(b)) if b > a => bulk.next(),
                    _ => internal.next(),
                },
                (Some(_), None) => internal.next(),
                (None, Some(_)) => bulk.next(),
                (None, None) => break,
            };
            if let Some(next) = next {
                let _ = order.push(next);
            }
        }
        self.history.order = order;
        self.history.cursor = 0;
        self.history.open = None;
        self.navigation.page = Page::History;
    }

    pub(super) fn history_count(&self) -> usize {
        self.history.order.len()
    }

    /// MOVE focuses the next ride, ACT opens it, BACK goes Home.
    pub(super) fn history_input(&mut self, button: Button) {
        let count = self.history_count();
        match button {
            Button::BottomLeft if count > 0 => {
                self.history.cursor = (self.history.cursor + 1) % count;
            }
            Button::BottomRight if count > 0 => self.open_past_ride(self.history.cursor),
            Button::TopLeft => self.navigation.page = Page::Home,
            _ => {}
        }
    }

    pub(super) fn open_past_ride(&mut self, index: usize) {
        let Some(&(medium, at)) = self.history.order.get(index) else {
            return;
        };
        let Some(ride_id) = self.saved(medium, at).map(|s| s.ride_id) else {
            return;
        };
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
        self.history.open = Some((medium, Replay::new(ride_id, upper)));
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
                let Some((medium, _)) = self.history.open else {
                    return;
                };
                let Some(ride_id) = self.listed(self.history.cursor).map(|s| s.ride_id) else {
                    return;
                };
                self.history.phone = Some((medium, ride_id));
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
        let Some((medium, mut replay)) = self.history.open else {
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
                self.history.open = Some((medium, replay));
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
        self.history.open = Some((medium, replay));
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
        let Some(summary) = self.listed(index) else {
            return Row::default();
        };
        let mut row = Row {
            title: title(summary, count - index),
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
        let Some((_, replay)) = &self.history.open else {
            return;
        };
        let summary = self.listed(self.history.cursor);
        if let Some(summary) = summary {
            view.past_title = title(summary, self.history_count() - self.history.cursor);
        }
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
        let saved: usize = [Medium::Internal, Medium::Bulk]
            .iter()
            .map(|&m| usize::from(self.ride.recorder(m).summary_count()))
            .sum();
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
