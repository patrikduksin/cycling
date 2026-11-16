use super::Runtime;
use super::climb_page::Follow;
use super::cues::Cue;
use super::input::HOLD_GAP_MS;
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

/// What a MOVE report leaves for the rest of the input handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoveReport {
    /// Handle it as a press, then tell the mark what the press changed.
    Press,
    /// A repeat of a hold: nothing else acts on it.
    Held,
}

#[derive(Default)]
pub(super) struct Marks {
    hold: Hold,
    count: u16,
    pub(super) made: Option<Mark>,
}

#[derive(Clone, Copy, Default)]
enum Hold {
    #[default]
    Idle,
    /// MOVE went down at `since` with `before` shown, and its press left
    /// `after`. The hold lasts while each report comes within
    /// `HOLD_GAP_MS` of the one at `last`.
    Holding {
        since: u64,
        last: u64,
        page: &'static str,
        before: View,
        after: View,
    },
    /// The hold marked, or its first report went to something else: its
    /// repeats do nothing.
    Spent,
}

/// What a MOVE press can change on a page that marks.
#[derive(Clone, Copy, PartialEq, Eq)]
struct View {
    page: Page,
    ride_page: RidePage,
    pause_hidden: bool,
    pause_wrapped: bool,
    climb: Follow,
    summary_page: usize,
    home_cursor: usize,
}

impl View {
    fn of(navigation: &Navigation) -> Self {
        Self {
            page: navigation.page,
            ride_page: navigation.ride_page,
            pause_hidden: navigation.pause_hidden,
            pause_wrapped: navigation.pause_wrapped,
            climb: navigation.climb,
            summary_page: navigation.summary_page,
            home_cursor: navigation.home_cursor,
        }
    }

    fn marks(self) -> bool {
        matches!(
            self.page,
            Page::Ride | Page::Ready | Page::Summary | Page::Home
        )
    }

    /// Undo what the press from `before` to `after` changed, field by field,
    /// leaving each field something else changed since: a pause shows the
    /// paused screen, and a climb its page, whatever the press did. A climb
    /// page that came up goes back to the page from before the press.
    fn undo(before: Self, after: Self, navigation: &mut Navigation) {
        if before.page != after.page || navigation.page != after.page {
            return;
        }
        put_back(&mut navigation.ride_page, before.ride_page, after.ride_page);
        put_back(
            &mut navigation.pause_hidden,
            before.pause_hidden,
            after.pause_hidden,
        );
        put_back(
            &mut navigation.pause_wrapped,
            before.pause_wrapped,
            after.pause_wrapped,
        );
        put_back(&mut navigation.climb, before.climb, after.climb);
        if let Follow::Shown { back_to } = &mut navigation.climb {
            put_back(back_to, before.ride_page, after.ride_page);
        }
        put_back(
            &mut navigation.summary_page,
            before.summary_page,
            after.summary_page,
        );
        put_back(
            &mut navigation.home_cursor,
            before.home_cursor,
            after.home_cursor,
        );
    }
}

fn put_back<T: Copy + PartialEq>(field: &mut T, before: T, after: T) {
    if *field == after {
        *field = before;
    }
}

impl Runtime {
    /// Every MOVE report comes here before the debounce, which drops a
    /// hold's repeats: they come faster than it lets presses through.
    pub(super) fn mark_sees_every_move_report(
        &mut self,
        held: bool,
        received: u64,
        now: u64,
    ) -> MoveReport {
        let view = View::of(&self.navigation);
        if !view.marks() {
            self.marks.hold = Hold::Idle;
            return MoveReport::Press;
        }
        if !held {
            self.marks.hold = Hold::Holding {
                since: received,
                last: received,
                page: self.seen_page(),
                before: view,
                after: view,
            };
            return MoveReport::Press;
        }
        self.marks.hold = match self.marks.hold {
            Hold::Holding {
                since,
                last,
                page,
                before,
                after,
            } if received.saturating_sub(last) < HOLD_GAP_MS => {
                if received.saturating_sub(since) >= HOLD_MS {
                    self.mark(since, page, before, after, now);
                    Hold::Spent
                } else {
                    Hold::Holding {
                        since,
                        last: received,
                        page,
                        before,
                        after,
                    }
                }
            }
            Hold::Idle | Hold::Holding { .. } | Hold::Spent => Hold::Spent,
        };
        MoveReport::Held
    }

    /// The hold's first report was handled as a press: what it left shown is
    /// what the mark undoes.
    pub(super) fn mark_after_press(&mut self) {
        if let Hold::Holding { after, .. } = &mut self.marks.hold {
            *after = View::of(&self.navigation);
        }
    }

    fn mark(&mut self, since: u64, page: &'static str, before: View, after: View, now: u64) {
        View::undo(before, after, &mut self.navigation);
        let marks = &mut self.marks;
        marks.count = marks.count.saturating_add(1);
        marks.made = Some(Mark {
            count: marks.count,
            pressed_ms: since,
            page,
        });
        let mut text = Text::<12>::new();
        let _ = write!(text, "MARK {}", marks.count);
        self.navigation.toast(text.as_str(), Tone::Info, now);
        self.navigation.chime = Some(Cue::Mark);
        self.display.redraw();
    }
}
