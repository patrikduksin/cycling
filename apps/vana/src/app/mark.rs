use super::Runtime;
use super::climb_page::Follow;
use super::cues::Cue;
use super::presentation::Navigation;
use crate::screens::chrome::Tone;
use crate::screens::ride::RidePage;
use crate::screens::workout::{Page, Text};
use core::fmt::Write;

const HOLD_MS: u64 = 1_500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    pub count: u16,
    pub pressed_ms: u64,
    pub page: &'static str,
}

#[derive(Default)]
pub(super) struct Marks {
    hold: Option<Hold>,
    count: u16,
    pub(super) made: Option<Mark>,
}

#[derive(Clone, Copy)]
struct Hold {
    since: u64,
    page: &'static str,
    before: View,
    marked: bool,
}

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
