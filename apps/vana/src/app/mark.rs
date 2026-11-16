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
    ride_page: RidePage,
    pause_hidden: bool,
    pause_wrapped: bool,
    climb: Follow,
    summary_page: usize,
    home_cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    None,
    /// Handle the report as a press.
    Press,
    /// The hold marked: undo its press and record it.
    Mark(Press),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    hold: Hold,
    effect: Effect,
}

fn step(hold: Hold, event: Event) -> Step {
    let (held, received, view, page) = match event {
        Event::Move {
            held,
            received,
            view,
            page,
        } => (held, received, view, page),
        Event::Pressed(view) => {
            let hold = match hold {
                Hold::Holding(press) => Hold::Holding(Press {
                    after: view,
                    ..press
                }),
                other => other,
            };
            return Step {
                hold,
                effect: Effect::None,
            };
        }
        Event::Cancel => {
            return Step {
                hold: Hold::Idle,
                effect: Effect::None,
            };
        }
    };
    if !view.marks() {
        return Step {
            hold: Hold::Idle,
            effect: Effect::Press,
        };
    }
    if !held {
        let press = Press {
            since: received,
            last: received,
            page,
            before: view,
            after: view,
        };
        return Step {
            hold: Hold::Holding(press),
            effect: Effect::Press,
        };
    }
    match hold {
        Hold::Holding(press) if received.saturating_sub(press.last) < HOLD_GAP_MS => {
            if received.saturating_sub(press.since) >= HOLD_MS {
                Step {
                    hold: Hold::Spent,
                    effect: Effect::Mark(press),
                }
            } else {
                Step {
                    hold: Hold::Holding(Press {
                        last: received,
                        ..press
                    }),
                    effect: Effect::None,
                }
            }
        }
        Hold::Idle | Hold::Holding(_) | Hold::Spent => Step {
            hold: Hold::Spent,
            effect: Effect::None,
        },
    }
}

impl Runtime {
    /// The only writer of `Marks`.
    fn mark_apply(&mut self, step: Step, now: u64) -> MoveReport {
        self.marks.hold = step.hold;
        let press = match step.effect {
            Effect::None => return MoveReport::Held,
            Effect::Press => return MoveReport::Press,
            Effect::Mark(press) => press,
        };
        undo(press, &mut self.navigation);
        let marks = &mut self.marks;
        marks.count = marks.count.saturating_add(1);
        marks.made = Some(Mark {
            count: marks.count,
            pressed_ms: press.since,
            page: press.page,
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

    /// The ride page the rider chose: under a climb page that came up by
    /// itself, the page it goes back to.
    fn rider_page(self) -> RidePage {
        match self.climb {
            Follow::Shown { back_to } => back_to,
            Follow::Idle | Follow::Done => self.ride_page,
        }
    }
}

/// Undo what the press changed, part by part, leaving each part something
/// else changed since: a pause shows the paused screen, and a climb its page,
/// whatever the press did. The ride page and the climb follower are one part,
/// so a page is never put back under a follower that has moved on.
fn undo(press: Press, navigation: &mut Navigation) {
    let (before, after) = (press.before, press.after);
    if before.page != after.page || navigation.page != after.page {
        return;
    }
    let mut ride = (navigation.ride_page, navigation.climb);
    put_back(
        &mut ride,
        (before.ride_page, before.climb),
        (after.ride_page, after.climb),
    );
    (navigation.ride_page, navigation.climb) = ride;
    // A climb page that came up during the hold goes back to the page from
    // before the press, not the one the press showed.
    if !matches!(after.climb, Follow::Shown { .. })
        && navigation.climb
            == (Follow::Shown {
                back_to: after.ride_page,
            })
    {
        navigation.climb = Follow::Shown {
            back_to: before.rider_page(),
        };
    }
    let mut pause = (navigation.pause_hidden, navigation.pause_wrapped);
    put_back(
        &mut pause,
        (before.pause_hidden, before.pause_wrapped),
        (after.pause_hidden, after.pause_wrapped),
    );
    (navigation.pause_hidden, navigation.pause_wrapped) = pause;
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

fn put_back<T: Copy + PartialEq>(part: &mut T, before: T, after: T) {
    if *part == after {
        *part = before;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIDE: View = View {
        page: Page::Ride,
        ride_page: RidePage::Main,
        pause_hidden: false,
        pause_wrapped: false,
        climb: Follow::Idle,
        summary_page: 0,
        home_cursor: 0,
    };
    const NEXT: View = View {
        ride_page: RidePage::Stats,
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
                "a repeat short of the threshold does nothing",
                holding(1_000, 2_300),
                report(true, 2_400, NEXT),
                holding(1_000, 2_400),
                Effect::None,
            ),
            (
                "the repeat at the threshold marks",
                holding(1_000, 2_400),
                report(true, 2_500, NEXT),
                Hold::Spent,
                Effect::Mark(Press {
                    since: 1_000,
                    last: 2_400,
                    page: "ride/main",
                    before: RIDE,
                    after: NEXT,
                }),
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

    fn undone(before: View, after: View, now: View) -> View {
        let mut navigation = Navigation::default();
        navigation.page = now.page;
        navigation.ride_page = now.ride_page;
        navigation.pause_hidden = now.pause_hidden;
        navigation.pause_wrapped = now.pause_wrapped;
        navigation.climb = now.climb;
        navigation.summary_page = now.summary_page;
        navigation.home_cursor = now.home_cursor;
        let press = Press {
            since: 0,
            last: 0,
            page: "ride",
            before,
            after,
        };
        undo(press, &mut navigation);
        View::of(&navigation)
    }

    #[test]
    fn a_mark_undoes_its_press_and_leaves_what_moved_on() {
        let shown = |back_to| Follow::Shown { back_to };
        let ride = |ride_page, climb| View {
            ride_page,
            climb,
            ..RIDE
        };
        let on_climb = ride(RidePage::Climb, shown(RidePage::Map));
        let paged = ride(RidePage::Main, Follow::Done);
        let cases = [
            ("nothing else moved", RIDE, NEXT, NEXT, RIDE),
            (
                "the climb page that came up by itself, and where it goes back",
                on_climb,
                paged,
                paged,
                on_climb,
            ),
            (
                "a climb that ended leaves the page the press showed",
                on_climb,
                paged,
                ride(RidePage::Main, Follow::Idle),
                ride(RidePage::Main, Follow::Idle),
            ),
            (
                "a climb page that came up stays and goes back to the page from before",
                RIDE,
                NEXT,
                ride(RidePage::Climb, shown(RidePage::Stats)),
                ride(RidePage::Climb, shown(RidePage::Main)),
            ),
            (
                "a pause shows the paused screen over the page put back",
                View {
                    pause_hidden: true,
                    ..RIDE
                },
                View {
                    pause_hidden: true,
                    ..NEXT
                },
                NEXT,
                RIDE,
            ),
        ];
        for (name, before, after, now, then) in cases {
            assert_eq!(undone(before, after, now), then, "{name}");
        }
    }
}
