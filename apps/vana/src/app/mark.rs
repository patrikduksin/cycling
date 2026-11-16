//! The rider's mark. Holding MOVE for `HOLD_MS` on the ride pages, Ready,
//! the summary or Home writes a mark into the session trace, so whoever
//! reads the trace later starts at the moment the rider noticed something.
//! The hold's first report is an ordinary MOVE; the mark puts back what it
//! changed, so a mark leaves the ride and the screen as they were.
use super::Runtime;
use super::climb_page::Follow;
use super::cues::Cue;
use super::presentation::Navigation;
use crate::screens::chrome::Tone;
use crate::screens::ride::RidePage;
use crate::screens::workout::{Page, Text};
use core::fmt::Write;

/// How long MOVE stays down to mark, from the hold's first report.
const HOLD_MS: u64 = 1_500;

/// A mark VANA made, until `report_seen` hands it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    /// Its number in this power-on session, from 1.
    pub count: u16,
    /// When the hold's first report came.
    pub pressed_ms: u64,
    /// The page shown before that report, by its seen name.
    pub page: &'static str,
}

#[derive(Default)]
pub(super) struct Marks {
    hold: Option<Hold>,
    count: u16,
    pub(super) made: Option<Mark>,
}

/// A MOVE hold that began on a page that marks.
#[derive(Clone, Copy)]
struct Hold {
    since: u64,
    page: &'static str,
    before: View,
    marked: bool,
}

/// What MOVE changes on a page that marks, as it was before the hold.
#[derive(Clone, Copy)]
enum View {
    Ride {
        page: RidePage,
        pause_hidden: bool,
        pause_wrapped: bool,
        climb: Follow,
    },
    Ready,
    Summary(usize),
    Home(usize),
}

impl View {
    /// None on a page where a held MOVE does something of its own, or
    /// nothing marks.
    fn of(navigation: &Navigation) -> Option<Self> {
        Some(match navigation.page {
            Page::Ride => Self::Ride {
                page: navigation.ride_page,
                pause_hidden: navigation.pause_hidden,
                pause_wrapped: navigation.pause_wrapped,
                climb: navigation.climb,
            },
            Page::Ready => Self::Ready,
            Page::Summary => Self::Summary(navigation.summary_page),
            Page::Home => Self::Home(navigation.home_cursor),
            _ => return None,
        })
    }

    fn restore(self, navigation: &mut Navigation) {
        match (self, navigation.page) {
            (
                Self::Ride {
                    page,
                    pause_hidden,
                    pause_wrapped,
                    climb,
                },
                Page::Ride,
            ) => {
                navigation.ride_page = page;
                navigation.pause_hidden = pause_hidden;
                navigation.pause_wrapped = pause_wrapped;
                navigation.climb = climb;
            }
            (Self::Summary(page), Page::Summary) => navigation.summary_page = page,
            (Self::Home(cursor), Page::Home) => navigation.home_cursor = cursor,
            _ => {}
        }
    }
}

impl Runtime {
    /// A MOVE report, before any debounce. On a page that marks, the hold's
    /// first report goes on as a press, its repeats never act, and the
    /// repeat that reaches `HOLD_MS` marks. True when the report is spent.
    pub(super) fn mark_hold(&mut self, held: bool, received: u64, now: u64) -> bool {
        let Some(before) = View::of(&self.navigation) else {
            self.marks.hold = None;
            return false;
        };
        if !held {
            self.marks.hold = Some(Hold {
                since: received,
                page: self.seen_page(),
                before,
                marked: false,
            });
            return false;
        }
        if let Some(hold) = &mut self.marks.hold
            && !hold.marked
            && received.saturating_sub(hold.since) >= HOLD_MS
        {
            hold.marked = true;
            let hold = *hold;
            hold.before.restore(&mut self.navigation);
            let marks = &mut self.marks;
            marks.count = marks.count.saturating_add(1);
            marks.made = Some(Mark {
                count: marks.count,
                pressed_ms: hold.since,
                page: hold.page,
            });
            let mut text = Text::<12>::new();
            let _ = write!(text, "MARK {}", marks.count);
            self.navigation.toast(text.as_str(), Tone::Info, now);
            self.navigation.chime = Some(Cue::Mark);
            self.display.redraw();
        }
        true
    }
}
