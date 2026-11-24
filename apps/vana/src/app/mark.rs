use super::Runtime;
use super::cues::Cue;
use super::input::HOLD_GAP_MS;
use super::navigation::{Navigation, Pause, Restore, To};
use crate::compose::input::Action;
use crate::compose::input::Focus;
use crate::screens::chrome::Tone;
use crate::screens::ride::RidePage;
use crate::screens::workout::{Page, Text};
use core::fmt::Write;
use device_api::ant::Ant;

const HOLD_MS: u64 = 1_500;

impl Runtime {
    /// Every MOVE report comes here before the debounce, which drops a
    /// hold's repeats: they come faster than it lets presses through.
    pub(super) fn mark_sees_every_move_report(
        &mut self,
        held: bool,
        received: u64,
        now: u64,
        ant: &mut impl Ant,
    ) -> MoveReport {
        let focus = self.shown_focus();
        let event = Event::Move {
            held,
            received,
            view: View::of(&self.navigation, focus),
            page: self.seen_page(),
            move_hold: self.move_hold(),
        };
        self.mark_apply(step(self.marks.hold, event), now, ant)
    }

    /// The hold's first report was handled as a press: what it left shown is
    /// what the mark undoes.
    pub(super) fn mark_after_press(&mut self, now: u64, ant: &mut impl Ant) {
        let focus = self.shown_focus();
        let event = Event::Pressed(View::of(&self.navigation, focus));
        self.mark_apply(step(self.marks.hold, event), now, ant);
    }

    pub(super) fn mark_input_lost(&mut self, now: u64, ant: &mut impl Ant) {
        self.mark_apply(step(self.marks.hold, Event::Cancel), now, ant);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    pub count: u16,
    pub pressed_ms: u64,
    pub page: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoveHold {
    Mark,
    /// What the screen shown holds MOVE for, as its buttons name it.
    Screen(Action),
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

state!(Marks { hold, count, made });

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
    move_hold: MoveHold,
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
        move_hold: MoveHold,
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
    ride: RidePage,
    pause: Pause,
    /// The focus on the screen shown, such as Home's card.
    focus: Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    None,
    /// Handle the report as a press.
    Press,
    Held {
        move_hold: MoveHold,
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
                move_hold,
            },
            _,
        ) => {
            let press = Press {
                since: received,
                last: received,
                page,
                move_hold,
                before: view,
                after: view,
            };
            to(Hold::Holding(press), Effect::Press)
        }
        (Event::Move { received, view, .. }, Hold::Holding(press))
            if press.held_at(received) && received.saturating_sub(press.since) >= HOLD_MS =>
        {
            let effect = Effect::Held {
                move_hold: press.move_hold,
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
    fn mark_apply(&mut self, step: Step, now: u64, ant: &mut impl Ant) -> MoveReport {
        self.marks.hold = step.hold;
        let (move_hold, pressed_ms, page, restore) = match step.effect {
            Effect::None => return MoveReport::Held,
            Effect::Press => return MoveReport::Press,
            Effect::Held {
                move_hold,
                pressed_ms,
                page,
                restore,
            } => (move_hold, pressed_ms, page, restore),
        };
        self.navigation.go(To::Restore(Restore {
            ride: restore.ride,
            pause: restore.pause,
        }));
        self.composed_focus(restore.focus);
        match move_hold {
            MoveHold::Mark => self.mark(pressed_ms, page, now),
            MoveHold::Screen(action) => self.carry(action, true, (pressed_ms, now), ant),
        }
        self.display.redraw();
        MoveReport::Held
    }

    fn mark(&mut self, pressed_ms: u64, page: &'static str, now: u64) {
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
        self.navigation.chime(Cue::Mark);
    }

    fn move_hold(&mut self) -> MoveHold {
        self.composed_hold()
            .map_or(MoveHold::Mark, MoveHold::Screen)
    }
}

impl View {
    fn of(navigation: &Navigation, focus: Focus) -> Self {
        Self {
            page: navigation.page(),
            ride: navigation.ride_page(),
            pause: Pause {
                hidden: navigation.pause_hidden(),
                wrapped: navigation.pause_wrapped(),
            },
            focus,
        }
    }

    fn marks(self) -> bool {
        matches!(self.page, Page::Ride | Page::Home)
    }
}

/// `now` with what the press changed put back, part by part, leaving each
/// part something else changed since: a pause shows the paused screen,
/// whatever the press did.
fn undone(press: Press, now: View) -> View {
    let (before, after) = (press.before, press.after);
    if before.page != after.page || now.page != after.page {
        return now;
    }
    View {
        page: now.page,
        ride: put_back(now.ride, before.ride, after.ride),
        pause: put_back(now.pause, before.pause, after.pause),
        focus: put_back(now.focus, before.focus, after.focus),
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
        ride: RidePage::Main,
        pause: Pause {
            hidden: false,
            wrapped: false,
        },
        focus: Focus::Screen,
    };
    const NEXT: View = View {
        ride: RidePage::Climb,
        ..RIDE
    };
    const SENSORS: View = View {
        page: Page::Sensors,
        ..RIDE
    };

    fn report(held: bool, received: u64, view: View) -> Event {
        Event::Move {
            held,
            received,
            view,
            page: "ride/main",
            move_hold: MoveHold::Mark,
        }
    }

    fn holding(since: u64, last: u64) -> Hold {
        Hold::Holding(Press {
            since,
            last,
            page: "ride/main",
            move_hold: MoveHold::Mark,
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
            move_hold: MoveHold::Mark,
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
                Effect::Held {
                    move_hold: MoveHold::Mark,
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
                "a hold begun on a running step extends it in place of a mark",
                Hold::Holding(Press {
                    last: 2_400,
                    move_hold: MoveHold::Screen(Action::ExtendStep),
                    ..started
                }),
                report(true, 2_500, NEXT),
                Hold::Spent,
                Effect::Held {
                    move_hold: MoveHold::Screen(Action::ExtendStep),
                    pressed_ms: 1_000,
                    page: "ride/main",
                    restore: NEXT,
                },
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
                report(false, 1_000, SENSORS),
                Hold::Idle,
                Effect::Press,
            ),
            (
                "a repeat on a page that never marks acts and ends the hold",
                holding(1_000, 2_400),
                report(true, 2_500, SENSORS),
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
            move_hold: MoveHold::Mark,
            before,
            after,
        };
        let paused = |view| View {
            pause: Pause {
                hidden: true,
                wrapped: false,
            },
            ..view
        };
        let home = |card| View {
            page: Page::Home,
            focus: Focus::Tile(card),
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
                SENSORS,
                SENSORS,
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
