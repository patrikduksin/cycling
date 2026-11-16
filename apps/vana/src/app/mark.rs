use super::Runtime;
use super::climb_page::{Follow, press_undone};
use super::cues::Cue;
use super::input::HOLD_GAP_MS;
use super::presentation::Navigation;
use crate::screens::chrome::Tone;
use crate::screens::ride::RidePage;
use crate::screens::workout::{Page, Text};
use core::fmt::Write;

const HOLD_MS: u64 = 1_500;

impl Runtime {
    /// Every MOVE report comes here before the debounce, which drops a
    /// hold's repeats: they come faster than it lets presses through.
    pub(super) fn mark_sees_every_move_report(
        &mut self,
        held: bool,
        received: u64,
        now: u64,
    ) -> MoveReport {
        let event = Event::Move {
            held,
            received,
            view: View::of(&self.navigation),
            page: self.seen_page(),
        };
        self.mark_apply(step(self.marks.hold, event), now)
    }

    /// The hold's first report was handled as a press: what it left shown is
    /// what the mark undoes.
    pub(super) fn mark_after_press(&mut self, now: u64) {
        let event = Event::Pressed(View::of(&self.navigation));
        self.mark_apply(step(self.marks.hold, event), now);
    }

    pub(super) fn mark_input_lost(&mut self, now: u64) {
        self.mark_apply(step(self.marks.hold, Event::Cancel), now);
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Hold {
    #[default]
    Idle,
    Holding(Press),
    /// The hold marked, or its first report went to something else: its
    /// repeats do nothing.
    Spent,
}

/// A MOVE press that may become a mark. It went down at `since` with
/// `before` shown on `page`, and left `after`. The hold lasts while each
/// report comes within `HOLD_GAP_MS` of the one at `last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    since: u64,
    last: u64,
    page: &'static str,
    before: View,
    after: View,
}

impl Press {
    fn held_at(self, received: u64) -> bool {
        received.saturating_sub(self.last) < HOLD_GAP_MS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Move {
        held: bool,
        received: u64,
        view: View,
        page: &'static str,
    },
    /// The press the hold began with was handled, leaving this view.
    Pressed(View),
    /// Input was lost: the hold may have ended unseen.
    Cancel,
}

/// What a MOVE press can change on a page that marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    page: Page,
    ride: (RidePage, Follow),
    /// `pause_hidden` and `pause_wrapped`.
    pause: (bool, bool),
    summary_page: usize,
    home_cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    None,
    /// Handle the report as a press.
    Press,
    /// The hold marked: show `restore` and record the press.
    Mark {
        pressed_ms: u64,
        page: &'static str,
        restore: View,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    hold: Hold,
    effect: Effect,
}

const fn to(hold: Hold, effect: Effect) -> Step {
    Step { hold, effect }
}

fn step(hold: Hold, event: Event) -> Step {
    match (event, hold) {
        (Event::Cancel, _) => to(Hold::Idle, Effect::None),
        (Event::Pressed(view), Hold::Holding(press)) => to(
            Hold::Holding(Press {
                after: view,
                ..press
            }),
            Effect::None,
        ),
        (Event::Pressed(_), Hold::Idle | Hold::Spent) => to(hold, Effect::None),
        (Event::Move { view, .. }, _) if !view.marks() => to(Hold::Idle, Effect::Press),
        (
            Event::Move {
                held: false,
                received,
                view,
                page,
            },
            _,
        ) => {
            let press = Press {
                since: received,
                last: received,
                page,
                before: view,
                after: view,
            };
            to(Hold::Holding(press), Effect::Press)
        }
        (Event::Move { received, view, .. }, Hold::Holding(press))
            if press.held_at(received) && received.saturating_sub(press.since) >= HOLD_MS =>
        {
            let effect = Effect::Mark {
                pressed_ms: press.since,
                page: press.page,
                restore: undone(press, view),
            };
            to(Hold::Spent, effect)
        }
        (Event::Move { received, .. }, Hold::Holding(press)) if press.held_at(received) => to(
            Hold::Holding(Press {
                last: received,
                ..press
            }),
            Effect::None,
        ),
        (Event::Move { .. }, Hold::Idle | Hold::Holding(_) | Hold::Spent) => {
            to(Hold::Spent, Effect::None)
        }
    }
}

impl Runtime {
    /// The only writer of `Marks`.
    fn mark_apply(&mut self, step: Step, now: u64) -> MoveReport {
        self.marks.hold = step.hold;
        let (pressed_ms, page, restore) = match step.effect {
            Effect::None => return MoveReport::Held,
            Effect::Press => return MoveReport::Press,
            Effect::Mark {
                pressed_ms,
                page,
                restore,
            } => (pressed_ms, page, restore),
        };
        let navigation = &mut self.navigation;
        (navigation.ride_page, navigation.climb) = restore.ride;
        (navigation.pause_hidden, navigation.pause_wrapped) = restore.pause;
        navigation.summary_page = restore.summary_page;
        navigation.home_cursor = restore.home_cursor;
        let marks = &mut self.marks;
        marks.count = marks.count.saturating_add(1);
        marks.made = Some(Mark {
            count: marks.count,
            pressed_ms,
            page,
        });
        let mut text = Text::<12>::new();
        let _ = write!(text, "MARK {}", marks.count);
        self.navigation.toast(text.as_str(), Tone::Info, now);
        self.navigation.chime = Some(Cue::Mark);
        self.display.redraw();
        MoveReport::Held
    }
}

impl View {
    fn of(navigation: &Navigation) -> Self {
        Self {
            page: navigation.page,
            ride: (navigation.ride_page, navigation.climb),
            pause: (navigation.pause_hidden, navigation.pause_wrapped),
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
}

/// `now` with what the press changed put back, part by part, leaving each
/// part something else changed since: a pause shows the paused screen, and a
/// climb its page, whatever the press did.
fn undone(press: Press, now: View) -> View {
    let (before, after) = (press.before, press.after);
    if before.page != after.page || now.page != after.page {
        return now;
    }
    View {
        page: now.page,
        ride: press_undone(before.ride, after.ride, now.ride),
        pause: put_back(now.pause, before.pause, after.pause),
        summary_page: put_back(now.summary_page, before.summary_page, after.summary_page),
        home_cursor: put_back(now.home_cursor, before.home_cursor, after.home_cursor),
    }
}

fn put_back<T: PartialEq>(now: T, before: T, after: T) -> T {
    if now == after { before } else { now }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIDE: View = View {
        page: Page::Ride,
        ride: (RidePage::Main, Follow::Idle),
        pause: (false, false),
        summary_page: 0,
        home_cursor: 0,
    };
    const NEXT: View = View {
        ride: (RidePage::Stats, Follow::Idle),
        ..RIDE
    };
    const SETTINGS: View = View {
        page: Page::Settings,
        ..RIDE
    };

    fn report(held: bool, received: u64, view: View) -> Event {
        Event::Move {
            held,
            received,
            view,
            page: "ride/main",
        }
    }

    fn holding(since: u64, last: u64) -> Hold {
        Hold::Holding(Press {
            since,
            last,
            page: "ride/main",
            before: RIDE,
            after: NEXT,
        })
    }

    #[test]
    fn a_hold_marks_once_at_the_threshold_from_its_first_report() {
        let started = Press {
            since: 1_000,
            last: 1_000,
            page: "ride/main",
            before: RIDE,
            after: RIDE,
        };
        let cases = [
            (
                "a first report presses and starts a hold",
                Hold::Idle,
                report(false, 1_000, RIDE),
                Hold::Holding(started),
                Effect::Press,
            ),
            (
                "a new press restarts the hold",
                holding(0, 900),
                report(false, 1_000, RIDE),
                Hold::Holding(started),
                Effect::Press,
            ),
            (
                "the press tells the hold what it left shown",
                Hold::Holding(started),
                Event::Pressed(NEXT),
                Hold::Holding(Press {
                    after: NEXT,
                    ..started
                }),
                Effect::None,
            ),
            (
                "a press after a mark changes nothing",
                Hold::Spent,
                Event::Pressed(NEXT),
                Hold::Spent,
                Effect::None,
            ),
            (
                "a repeat short of the threshold does nothing",
                holding(1_000, 2_300),
                report(true, 2_400, NEXT),
                holding(1_000, 2_400),
                Effect::None,
            ),
            (
                "the repeat at the threshold marks and puts back the page",
                holding(1_000, 2_400),
                report(true, 2_500, NEXT),
                Hold::Spent,
                Effect::Mark {
                    pressed_ms: 1_000,
                    page: "ride/main",
                    restore: RIDE,
                },
            ),
            (
                "repeats after a mark do nothing",
                Hold::Spent,
                report(true, 2_600, RIDE),
                Hold::Spent,
                Effect::None,
            ),
            (
                "a gap ends the hold",
                holding(1_000, 2_000),
                report(true, 2_000 + HOLD_GAP_MS, NEXT),
                Hold::Spent,
                Effect::None,
            ),
            (
                "a repeat of a first report the mark never saw does nothing",
                Hold::Idle,
                report(true, 2_600, RIDE),
                Hold::Spent,
                Effect::None,
            ),
            (
                "lost input ends the hold",
                holding(1_000, 2_400),
                Event::Cancel,
                Hold::Idle,
                Effect::None,
            ),
            (
                "a press on a page that never marks acts",
                Hold::Idle,
                report(false, 1_000, SETTINGS),
                Hold::Idle,
                Effect::Press,
            ),
            (
                "a repeat on a page that never marks acts and ends the hold",
                holding(1_000, 2_400),
                report(true, 2_500, SETTINGS),
                Hold::Idle,
                Effect::Press,
            ),
        ];
        for (name, hold, event, then, effect) in cases {
            assert_eq!(step(hold, event), Step { hold: then, effect }, "{name}");
        }
    }

    #[test]
    fn repeats_after_lost_input_never_mark() {
        let mut hold = holding(1_000, 1_100);
        hold = step(hold, Event::Cancel).hold;
        for received in (1_200..4_000).step_by(100) {
            let step = step(hold, report(true, received, NEXT));
            assert_eq!(step.effect, Effect::None, "at {received}");
            hold = step.hold;
        }
    }

    #[test]
    fn a_mark_undoes_its_press_and_leaves_what_moved_on() {
        let press = |before, after| Press {
            since: 0,
            last: 0,
            page: "ride",
            before,
            after,
        };
        let paused = |view| View {
            pause: (true, false),
            ..view
        };
        let home = |home_cursor| View {
            page: Page::Home,
            home_cursor,
            ..RIDE
        };
        let cases = [
            ("nothing else moved", RIDE, NEXT, NEXT, RIDE),
            (
                "a pause shows the paused screen over the page put back",
                paused(RIDE),
                paused(NEXT),
                NEXT,
                RIDE,
            ),
            (
                "another page leaves what is shown",
                RIDE,
                NEXT,
                SETTINGS,
                SETTINGS,
            ),
            (
                "the Home cursor goes back",
                home(0),
                home(1),
                home(1),
                home(0),
            ),
            (
                "a cursor moved since stays",
                home(0),
                home(1),
                home(2),
                home(2),
            ),
        ];
        for (name, before, after, now, then) in cases {
            assert_eq!(undone(press(before, after), now), then, "{name}");
        }
    }
}
