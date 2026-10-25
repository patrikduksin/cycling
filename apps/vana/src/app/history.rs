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
use crate::clock::Zone;
use crate::ride::directory::{Journal, RideIndex};
use crate::ride::log::{Status, Summary};
use crate::ride::replay::{Progress, Replay, Sums};
use crate::ride::week::Weeks;
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
    /// Home last saw the full count.
    counted: bool,
    pub(super) cursor: usize,
    /// The ride on the summary's pages: its journal, its replay and its line.
    open: Option<(Medium, Replay, Summary)>,
    /// The ride the phone screen was opened for.
    pub(super) phone: Option<(Medium, u32)>,
    /// Home's last-ride distance, kept from before a replay took over the
    /// ride sums; cleared when a new ride starts.
    pub(super) home_last: Option<u64>,
    /// The week totals as of the last time the rides or the minute changed.
    weeks: core::cell::Cell<Option<(WeeksKey, Weeks)>>,
}

/// The minute, the zone, the FTP, the ride facts kept, and each journal's
/// rides, last id and how many of them the index has read.
pub(super) type WeeksKey = (u64, Zone, u16, u32, [(usize, Option<u32>, usize); 2]);

/// Which journal's newest unlisted ride comes next: 0 INTERNAL, 1 BULK. BULK
/// only when both start times are known and BULK's is later, so undated rides
/// list INTERNAL's first.
fn newer_head(internal: Option<&Summary>, bulk: Option<&Summary>) -> Option<usize> {
    match (internal, bulk) {
        (Some(a), Some(b)) => match (a.first_utc_ms, b.first_utc_ms) {
            (Some(a), Some(b)) if b > a => Some(1),
            _ => Some(0),
        },
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (None, None) => None,
    }
}

impl History {
    /// From now on the index reads the journals in the background.
    pub(super) fn start_indexing(&mut self) {
        self.indexing = true;
    }
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
        self.ride_in(medium, index, self.split(medium))
    }

    /// The same, for a caller that already knows the journal's `split`.
    fn ride_in(&self, medium: Medium, index: usize, split: (usize, usize)) -> Option<Summary> {
        let (indexed, newer) = split;
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
            let pick = newer_head(internal.as_ref(), bulk.as_ref())?;
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

    /// Whether every saved ride is counted: always without an index, else
    /// once its scan has read both journals.
    pub(super) fn counted(&self) -> bool {
        match self.history.index.as_deref() {
            None => true,
            Some(index) => self.history.indexing && self.ride.directory_done(index),
        }
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
        // Home's RIDES card counts every saved ride, so the scan starts there
        // too once no ride is open.
        if !self.history.indexing && self.navigation.page == Page::Home && !self.recording() {
            self.history.indexing = true;
        }
        if !self.history.indexing {
            return;
        }
        let Some(index) = self.history.index.as_deref_mut() else {
            return;
        };
        let added = self.ride.directory_step(index, store, bulk);
        let page = self.navigation.page;
        let counted = self.counted();
        let flipped = counted != self.history.counted;
        let recount = page == Page::Home && flipped;
        self.history.counted = counted;
        if added && matches!(page, Page::History | Page::Form)
            || recount
            || page == Page::Form && flipped
        {
            self.display.redraw();
        }
    }

    /// MOVE focuses the next ride (held, the next page of cards), ACT opens
    /// it, BACK goes Home.
    pub(super) fn history_input(&mut self, button: Button, held: bool) {
        let count = self.history_count();
        match button {
            Button::BottomLeft if count > 0 => {
                self.history.cursor = history::scroll(count, self.history.cursor).next(held);
            }
            Button::BottomRight if count > 0 => self.open_past_ride(self.history.cursor),
            Button::TopLeft => self.navigation.page = Page::Home,
            _ => {}
        }
    }

    /// A vertical swipe focuses the ride a page on or back.
    pub(super) fn history_swipe(&mut self, on: bool) {
        let scroll = history::scroll(self.history_count(), self.history.cursor);
        self.history.cursor = if on {
            scroll.page_on()
        } else {
            scroll.page_back()
        };
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
            trace: &mut ride.trace,
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
                self.enter_phone();
            }
            _ => {}
        }
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

    /// The minute, the zone, the FTP, the facts kept and how far each
    /// journal's index has read, which together decide every total built
    /// from the saved rides.
    pub(super) fn rides_key(&self, utc_ms: u64) -> WeeksKey {
        let ids = |medium| {
            let last = self.journal_len(medium).checked_sub(1);
            (
                self.journal_len(medium),
                last.and_then(|i| self.journal_ride(medium, i))
                    .map(|r| r.ride_id),
                self.indexed(medium).map_or(0, Journal::len),
            )
        };
        (
            utc_ms / 60_000,
            self.preferences.options.zone,
            self.preferences.profile.ftp_watts,
            self.facts.revision,
            [ids(Medium::Internal), ids(Medium::Bulk)],
        )
    }

    /// Each saved ride of both journals, oldest first within a journal, with
    /// its training stress if the index has read it and it had power, at the
    /// FTP its facts kept, else the FTP now.
    pub(super) fn each_ride(&self, mut each: impl FnMut(&Summary, Option<u32>)) {
        let ftp = self.preferences.profile.ftp_watts;
        for medium in [Medium::Internal, Medium::Bulk] {
            let split = self.split(medium);
            for index in 0..self.journal_len(medium) {
                let Some(ride) = self.ride_in(medium, index, split) else {
                    continue;
                };
                let effort = if index < split.0 {
                    self.indexed(medium)
                        .and_then(|journal| journal.effort(index))
                } else {
                    None
                };
                let stress = effort.and_then(|effort| {
                    let then = self
                        .facts
                        .find(medium, ride.ride_id, ride.first_utc_ms)
                        .and_then(|facts| facts.profile);
                    effort.stress_score(then.map_or(ftp, |profile| profile.ftp_watts))
                });
                each(&ride, stress);
            }
        }
    }

    /// This week and last, from the start times, active times and distances
    /// of the saved rides of both journals, or none until the clock is known.
    /// Training stress comes from the index's rides with power, at the FTP
    /// each ride's facts kept, else the FTP now; rides the index has not read
    /// yet carry none.
    /// A ride that moved from the MMC onto internal flash is two journal
    /// rides, as the list shows it: their times and distances do not overlap,
    /// so the totals are right, and the ride count counts each part.
    pub(super) fn weeks(&self) -> Option<Weeks> {
        let utc_ms = self.readings.live.utc_ms?;
        let key = self.rides_key(utc_ms);
        if let Some((cached, weeks)) = self.history.weeks.get()
            && cached == key
        {
            return Some(weeks);
        }
        let zone = self.preferences.options.zone;
        let offset = |utc_s| zone.offset_s(utc_s);
        let mut weeks = Weeks::new(utc_ms, &offset);
        self.each_ride(|ride, stress| {
            weeks.add_stress(
                ride.first_utc_ms,
                ride.active_ms,
                ride.distance_mm,
                stress,
                &offset,
            );
        });
        self.history.weeks.set(Some((key, weeks)));
        Some(weeks)
    }

    /// The ride list, its cards made on demand.
    #[inline(never)]
    pub(super) fn history_canvas(&self) -> crate::screens::canvas::Canvas {
        let count = self.history_count();
        history::prepare(&List {
            count,
            cursor: self.history.cursor,
            row: |index| self.history_row(index, count),
            weeks: if count > 0 && self.history.cursor < history::HEADED {
                self.weeks()
            } else {
                None
            },
            reading: !self.counted(),
        })
    }

    fn history_row(&self, index: usize, count: usize) -> Row {
        let Some((_, summary)) = self.listed(index) else {
            return Row::default();
        };
        let summary = &summary;
        let mut row = Row {
            title: title(
                summary,
                count.saturating_sub(index),
                self.preferences.options.zone,
            ),
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
    pub(super) fn past_view<'a>(&'a self, view: &mut View<'a>) {
        let Some((medium, replay, summary)) = &self.history.open else {
            view.show_summary(After::default(), Text::new());
            return;
        };
        let facts = self
            .facts
            .find(*medium, summary.ride_id, summary.first_utc_ms);
        let past_title = title(
            summary,
            self.history_count().saturating_sub(self.history.cursor),
            self.preferences.options.zone,
        );
        let summary = Some(summary);
        let saving = match replay.progress() {
            Progress::Reading => Saving::Reading,
            Progress::Done => Saving::Saved,
            Progress::Missing => Saving::Unreadable,
        };
        view.totals = replay.totals(self.ride.totals);
        view.zone_time = self.ride.zone_time;
        let active_ms = summary.map_or(0, |s| s.active_ms).max(replay.active_ms);
        view.active_secs = active_ms / 1000;
        let after = After {
            page: self.navigation.summary_page,
            saving,
            best: self.ride.efforts.best(),
            climbs: facts
                .and_then(|facts| facts.climbing)
                .map(|climbing| climbing.count),
            laps: replay.laps(),
            past: true,
            elevation: self.has_elevation(),
            recovery: facts
                .and_then(|facts| facts.recovery_bpm)
                .map(crate::ride::recovery::Reading::Recovered),
            ..After::default()
        };
        view.show_summary(after, past_title);
        self.show_elevation(view);
    }

    /// Home's RIDES card: this week's distance once the index has read every
    /// ride, else the last ride's distance, or how many are saved.
    pub(super) fn rides_line(&self) -> Text<20> {
        if !self.recording()
            && self.counted()
            && let Some(weeks) = self.weeks()
            && weeks.this.distance_mm > 0
        {
            return crate::screens::home::this_week(weeks.this.distance_mm);
        }
        let distance = self
            .history
            .home_last
            .unwrap_or_else(|| self.ride.totals.distance_mm());
        if !self.recording() && distance > 0 {
            return crate::screens::home::last_ride(distance);
        }
        let mut line = Text::new();
        if !self.counted() {
            let _ = line.push_str("...");
            return line;
        }
        let saved = self.history_count();
        let _ = match saved {
            0 => line.push_str("NONE YET"),
            n => write!(line, "{n} SAVED"),
        };
        line
    }
}

/// When a ride started, or RIDE n when the clock was unknown.
fn title(summary: &Summary, number: usize, zone: Zone) -> Text<20> {
    match summary.first_utc_ms {
        Some(utc_ms) => history::started(zone.local(utc_ms / 1000)),
        None => {
            let mut text = Text::new();
            let _ = write!(text, "RIDE {number}");
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Source;

    fn started(utc_ms: Option<u64>) -> Summary {
        let mut summary = Summary::started(1, Source::Live);
        summary.first_utc_ms = utc_ms;
        summary
    }

    #[test]
    fn the_later_start_is_listed_first_and_undated_rides_list_internal_first() {
        let (early, late) = (started(Some(1_000)), started(Some(2_000)));
        let undated = started(None);
        assert_eq!(newer_head(Some(&early), Some(&late)), Some(1));
        assert_eq!(newer_head(Some(&late), Some(&early)), Some(0));
        assert_eq!(newer_head(Some(&early), Some(&early)), Some(0));
        assert_eq!(newer_head(Some(&undated), Some(&late)), Some(0));
        assert_eq!(newer_head(Some(&early), Some(&undated)), Some(0));
        assert_eq!(newer_head(Some(&undated), Some(&undated)), Some(0));
        assert_eq!(newer_head(None, Some(&undated)), Some(1));
        assert_eq!(newer_head(Some(&undated), None), Some(0));
        assert_eq!(newer_head(None, None), None);
    }

    /// Rides started either side of a change read on the clock in force at
    /// their start, and still list by UTC.
    #[test]
    fn rides_across_a_change_read_the_clock_of_their_start() {
        let at = |date: &[u8; 6], time: &[u8; 6]| {
            let utc_s = firmware_services::gps::unix_seconds(*date, *time).unwrap();
            started(Some(utc_s as u64 * 1000))
        };
        let read = |summary: &Summary, zone| title(summary, 1, zone);
        // The hour from 23:00 on Saturday 4 April 2026 repeats.
        let (first, second) = (at(b"050426", b"023000"), at(b"050426", b"033000"));
        assert_eq!(read(&first, Zone::Chile).as_str(), "APR 4  11:30");
        assert_eq!(read(&second, Zone::Chile).as_str(), "APR 4  11:30");
        assert_eq!(newer_head(Some(&first), Some(&second)), Some(1));
        // The hour after Saturday 5 September 2026 at 24:00 is skipped.
        let before = at(b"060926", b"035900");
        let after = at(b"060926", b"040000");
        assert_eq!(read(&before, Zone::Chile).as_str(), "SEP 5  11:59");
        assert_eq!(read(&after, Zone::Chile).as_str(), "SEP 6  1:00");
        assert_eq!(read(&after, Zone::utc(-4)).as_str(), "SEP 6  12:00");
        assert_eq!(read(&started(None), Zone::Chile).as_str(), "RIDE 1");
    }
}
