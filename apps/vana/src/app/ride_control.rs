//! The ride screens' state machine: buttons and automatic actions to recorder
//! requests and page moves. The ride page's message, the lap and automation
//! toasts and the lap chime are `Notices`, and this file is the only writer of
//! the message.
use super::Runtime;
use super::cues::Cue;
use super::navigation::To;
use crate::ride::auto::{Call, Decision};
use crate::ride::current::Discard;
use crate::ride::laps::Lap;
use crate::ride::log::Status;
use crate::ride::recorder::Action;
use crate::screens::chrome::Tone;
use crate::screens::workout::Page;
use device_api::input::Role;
use device_api::input::Role::{Act as ACT, Back as BACK, Move as MOVE};

/// A finished lap shows in the status row this long.
const LAP_TOAST_MS: u64 = 2000;

/// How long the brief notice of an automatic action stays up.
const TOAST_MS: u64 = 3000;

/// Ride-page feedback that does not come from the recorder state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RideMessage {
    #[default]
    Clear,
    StorageNotReady,
}

#[derive(Default)]
pub(super) struct Notices {
    message: RideMessage,
    /// The last finished lap and when it closed, for the status-row toast.
    lap_toast: Option<(u64, Lap)>,
    /// A lap closed since the last chime.
    lap_chime: bool,
    /// The last automatic action and when it happened, shown briefly.
    auto_toast: Option<(&'static str, u64)>,
}

state!(Notices {
    message,
    lap_toast,
    lap_chime,
    auto_toast,
});

impl Notices {
    pub(super) fn message(&self) -> RideMessage {
        self.message
    }

    pub(super) fn lap_closed(&mut self, lap: Lap, now: u64) {
        self.lap_toast = Some((now, lap));
        self.lap_chime = true;
    }

    pub(super) fn clear_lap_toast(&mut self) {
        self.lap_toast = None;
    }

    /// The finished lap the status row shows at `now`, for `LAP_TOAST_MS`.
    pub(super) fn lap_toast(&self, now: u64) -> Option<Lap> {
        self.lap_toast
            .filter(|(at, _)| now.saturating_sub(*at) < LAP_TOAST_MS)
            .map(|(_, lap)| lap)
    }

    pub(super) fn take_lap_chime(&mut self) -> bool {
        core::mem::take(&mut self.lap_chime)
    }

    /// The notice of a recent automatic action, if any.
    pub(super) fn auto_toast(&self, now: u64) -> &'static str {
        match self.auto_toast {
            Some((text, at)) if now.saturating_sub(at) <= TOAST_MS => text,
            _ => "",
        }
    }
}

impl Runtime {
    pub(super) fn ride_press(&mut self, button: Role, held: bool, pressed_at: u64, now: u64) {
        self.ride_event(
            Event::Press { button, held },
            Origin::Rider { pressed_at },
            now,
        );
    }

    pub(super) fn ride_auto(&mut self, call: Call, now: u64) {
        match call {
            Call::Do(decision) => self.ride_event(Event::Auto(call), Origin::Auto(decision), now),
            Call::Remind => {
                let step = step(self.navigation.page(), self.seen(), Event::Auto(call));
                self.apply(step, now);
            }
        }
    }

    pub(super) fn ride_serviced(&mut self, before: Status, now: u64) {
        let step = step(
            self.navigation.page(),
            self.seen(),
            Event::Serviced { before },
        );
        self.apply(step, now);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Press {
        button: Role,
        held: bool,
    },
    Auto(Call),
    Serviced {
        before: Status,
    },
    Answered {
        action: Action,
        accepted: bool,
        origin: Origin,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Rider { pressed_at: u64 },
    Auto(Decision),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    ride: Status,
    next: Status,
    pending: bool,
    auto_paused: bool,
    recording: bool,
    /// A ride can be discarded: FILES can note it.
    can_discard: bool,
    discard: Discard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Go {
    OpenRide,
    BackToRide,
    EndRide,
    DiscardRide,
    Summary,
    Home,
    /// Home, after a discarded ride was saved.
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    None,
    TakeOverPause,
    Lap,
    NotReadyToast,
    Remind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    go: Option<Go>,
    note: Option<RideMessage>,
    effect: Effect,
    submit: Option<Action>,
}

const STAY: Step = Step {
    go: None,
    note: None,
    effect: Effect::None,
    submit: None,
};

const fn go(go: Go) -> Step {
    Step {
        go: Some(go),
        ..STAY
    }
}

const fn effect(effect: Effect) -> Step {
    Step { effect, ..STAY }
}

const fn note(note: RideMessage) -> Step {
    Step {
        note: Some(note),
        ..STAY
    }
}

const fn submit(action: Action) -> Step {
    Step {
        submit: Some(action),
        ..STAY
    }
}

fn step(page: Page, seen: Seen, event: Event) -> Step {
    match event {
        Event::Press { button, held } => press(page, seen, button, held),
        Event::Answered {
            action,
            accepted,
            origin,
        } => answered(page, action, accepted, origin),
        Event::Auto(Call::Do(decision)) => submit(match decision {
            Decision::Pause => Action::Pause,
            Decision::Resume => Action::Resume,
            Decision::Start => Action::Start,
        }),
        Event::Auto(Call::Remind) => effect(Effect::Remind),
        Event::Serviced { before } => serviced(page, seen, before),
    }
}

fn press(page: Page, seen: Seen, button: Role, held: bool) -> Step {
    let startable = matches!(seen.next, Status::Ready | Status::Saved | Status::Recovered);
    let riding = matches!(seen.ride, Status::Recording | Status::Paused);
    let saving = seen.pending || riding;
    match (page, button) {
        (Page::Home, ACT) if saving => go(Go::OpenRide),
        (Page::Home, ACT) if startable => submit(Action::Start),
        (Page::Home, ACT) => effect(Effect::NotReadyToast),
        (Page::Ride, BACK) if !seen.recording => go(Go::Home),
        (Page::Ride, _) if seen.pending => STAY,
        (Page::Ride, ACT) => match seen.ride {
            Status::Recording => submit(Action::Pause),
            Status::Paused if seen.auto_paused => effect(Effect::TakeOverPause),
            Status::Paused => submit(Action::Resume),
            _ if startable => submit(Action::Start),
            _ => STAY,
        },
        (Page::Ride, BACK) => match seen.ride {
            Status::Recording => effect(Effect::Lap),
            Status::Paused => Step {
                go: Some(Go::EndRide),
                note: Some(RideMessage::Clear),
                effect: Effect::TakeOverPause,
                submit: None,
            },
            _ => STAY,
        },
        (Page::EndRide, BACK) => Step {
            go: Some(Go::BackToRide),
            note: Some(RideMessage::Clear),
            ..STAY
        },
        (Page::EndRide, ACT) if !seen.pending && seen.ride == Status::Paused => {
            submit(Action::Finish)
        }
        (Page::EndRide, MOVE) if seen.can_discard => go(Go::DiscardRide),
        (Page::DiscardRide, _) if seen.pending => STAY,
        (Page::DiscardRide, BACK) => go(Go::EndRide),
        (Page::DiscardRide, ACT) if seen.can_discard && seen.ride == Status::Paused => {
            submit(Action::Finish)
        }
        (Page::Summary, ACT | BACK) if !held && !saving => go(Go::Home),
        _ => STAY,
    }
}

fn answered(page: Page, action: Action, accepted: bool, origin: Origin) -> Step {
    match (page, action, accepted) {
        (Page::Home, Action::Start, true) => Step {
            go: Some(Go::OpenRide),
            note: Some(RideMessage::Clear),
            ..STAY
        },
        (Page::Home, Action::Start, false) if matches!(origin, Origin::Rider { .. }) => {
            effect(Effect::NotReadyToast)
        }
        (Page::EndRide, Action::Finish, true) => Step {
            go: Some(Go::Summary),
            note: Some(RideMessage::Clear),
            ..STAY
        },
        (Page::EndRide | Page::DiscardRide, Action::Finish, false) => Step {
            go: Some(Go::BackToRide),
            note: Some(RideMessage::StorageNotReady),
            ..STAY
        },
        (_, _, true) => note(RideMessage::Clear),
        (_, Action::Pause | Action::Resume, false) if matches!(origin, Origin::Auto(_)) => STAY,
        (_, _, false) => note(RideMessage::StorageNotReady),
    }
}

fn serviced(page: Page, seen: Seen, before: Status) -> Step {
    let saved = matches!(before, Status::Recording | Status::Paused) && seen.ride == Status::Saved;
    let finish_failed = seen.ride == Status::Paused && !seen.pending;
    let go = match page {
        Page::DiscardRide if matches!(seen.discard, Discard::Saved { .. }) => Some(Go::Discarded),
        Page::DiscardRide if seen.discard == Discard::Failed => Some(Go::BackToRide),
        Page::Ride | Page::EndRide | Page::DiscardRide | Page::Summary if saved => {
            Some(Go::Summary)
        }
        Page::EndRide | Page::DiscardRide if seen.ride != Status::Paused => Some(Go::BackToRide),
        Page::Summary if finish_failed => Some(Go::BackToRide),
        Page::Summary if seen.ride == Status::Recording => Some(Go::OpenRide),
        Page::Boot
        | Page::Format
        | Page::Home
        | Page::Sensors
        | Page::Ride
        | Page::EndRide
        | Page::DiscardRide
        | Page::LoseRides
        | Page::Summary
        | Page::Settings
        | Page::Workouts => None,
    };
    Step {
        go,
        note: (before != seen.ride).then_some(RideMessage::Clear),
        ..STAY
    }
}

impl Runtime {
    fn ride_event(&mut self, event: Event, origin: Origin, now: u64) {
        let first = step(self.navigation.page(), self.seen(), event);
        self.apply(first, now);
        if let Some(action) = first.submit {
            let accepted = self.submit_ride(action, origin, now);
            let answer = Event::Answered {
                action,
                accepted,
                origin,
            };
            let second = step(self.navigation.page(), self.seen(), answer);
            self.apply(second, now);
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            ride: self.ride.active().status(),
            next: self.ride.recorder(self.ride.medium()).status(),
            pending: self.ride.pending(),
            auto_paused: self.ride.auto().paused(),
            recording: self.recorder_busy(),
            can_discard: self.can_discard(),
            discard: self.ride.discard(),
        }
    }

    fn apply(&mut self, step: Step, now: u64) {
        if let Some(note) = step.note {
            self.notices.message = note;
        }
        match step.go {
            None => {}
            Some(Go::OpenRide) => {
                self.recovery.clear();
                self.navigation.set_first_page(self.first_ride_page());
                self.navigation.go(To::Ride);
            }
            Some(Go::BackToRide) => {
                self.ride.clear_discard();
                self.navigation.go(To::Page(Page::Ride));
            }
            Some(Go::EndRide) => {
                self.notices.clear_lap_toast();
                self.navigation.go(To::Page(Page::EndRide));
            }
            Some(Go::DiscardRide) => self.navigation.go(To::Page(Page::DiscardRide)),
            Some(Go::Summary) => {
                // Opening the summary is the finish; the save completing
                // under it is not a second one.
                if self.navigation.page() != Page::Summary {
                    self.recovery.finish(now);
                }
                self.navigation.go(To::Summary);
            }
            Some(Go::Home) => self.ride_left(),
            Some(Go::Discarded) => {
                let hidden = self.ride.discard() == Discard::Saved { hidden: true };
                self.ride.clear_discard();
                self.ride_left();
                if hidden {
                    self.navigation.discarded("RIDE DISCARDED", Tone::Info, now);
                } else {
                    self.navigation.discarded("NOT DISCARDED", Tone::Warn, now);
                }
                self.display.redraw();
            }
        }
        match step.effect {
            Effect::None => {}
            Effect::TakeOverPause => self.ride.auto_manual(),
            Effect::Lap => {
                if let Some(lap) = self.ride.lap(now)
                    && !self.workout_lap(now)
                {
                    self.notices.lap_closed(lap, now);
                }
            }
            Effect::NotReadyToast => self.navigation.toast("STORAGE NOT READY", Tone::Alert, now),
            Effect::Remind => self.cue(Cue::StillPaused, now),
        }
    }

    /// Home, done with the ride.
    fn ride_left(&mut self) {
        self.navigation.go(To::Home);
        self.recovery.clear();
        self.leave_workout();
    }

    fn submit_ride(&mut self, action: Action, origin: Origin, now: u64) -> bool {
        self.ride.clear_request();
        let medium = self.ride.target(action);
        let accepted = self.format.admits_rides() && self.ride.submit(medium, action, now);
        if accepted && action == Action::Finish && self.navigation.page() == Page::DiscardRide {
            self.ride.ask_discard();
        }
        match (origin, accepted) {
            (Origin::Rider { pressed_at }, true) => {
                self.ride.auto_manual();
                self.notices.auto_toast = None;
                if action == Action::Start {
                    self.timing.ride_press(pressed_at);
                }
            }
            (Origin::Rider { .. }, false) => {}
            (Origin::Auto(decision), true) => {
                self.ride.auto_accepted(decision);
                let toast = match decision {
                    Decision::Pause => "",
                    Decision::Resume => "AUTO RESUME",
                    Decision::Start => "AUTO START",
                };
                self.notices.auto_toast = Some((toast, now));
                self.display.redraw();
            }
            (Origin::Auto(_), false) => self.ride.auto_declined(),
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::{Error, Paused, Ready, Recording, Saved, Scanning};

    const RIDER: Origin = Origin::Rider { pressed_at: 0 };
    const AUTO: Origin = Origin::Auto(Decision::Pause);

    fn seen(ride: Status) -> Seen {
        Seen {
            ride,
            next: ride,
            pending: false,
            auto_paused: false,
            recording: matches!(ride, Recording | Paused),
            can_discard: false,
            discard: Discard::Idle,
        }
    }

    /// A ride FILES can note as discarded.
    fn card(ride: Status) -> Seen {
        Seen {
            can_discard: true,
            ..seen(ride)
        }
    }

    #[test]
    fn discard_saves_the_ride_then_goes_home() {
        let asked = Seen {
            pending: true,
            discard: Discard::Asked(7),
            ..card(Paused)
        };
        let saved = |hidden| Seen {
            discard: Discard::Saved { hidden },
            ..card(Saved)
        };
        let failed = Seen {
            discard: Discard::Failed,
            ..card(Paused)
        };
        check(&[
            (
                Page::EndRide,
                card(Paused),
                press(MOVE),
                go(Go::DiscardRide),
            ),
            (Page::EndRide, seen(Paused), press(MOVE), STAY),
            (
                Page::DiscardRide,
                card(Paused),
                press(BACK),
                go(Go::EndRide),
            ),
            (Page::DiscardRide, card(Paused), press(MOVE), STAY),
            (
                Page::DiscardRide,
                card(Paused),
                press(ACT),
                submit(Action::Finish),
            ),
            (Page::DiscardRide, seen(Paused), press(ACT), STAY),
            (Page::DiscardRide, asked, press(BACK), STAY),
            (Page::DiscardRide, asked, press(ACT), STAY),
            (
                Page::DiscardRide,
                card(Paused),
                answer(Action::Finish, true, RIDER),
                note(RideMessage::Clear),
            ),
            (
                Page::DiscardRide,
                card(Paused),
                answer(Action::Finish, false, RIDER),
                both(Go::BackToRide, RideMessage::StorageNotReady),
            ),
            (
                Page::DiscardRide,
                saved(true),
                since(Paused),
                both(Go::Discarded, RideMessage::Clear),
            ),
            (
                Page::DiscardRide,
                saved(false),
                since(Paused),
                both(Go::Discarded, RideMessage::Clear),
            ),
            (Page::DiscardRide, failed, since(Paused), go(Go::BackToRide)),
            (Page::DiscardRide, asked, since(Paused), STAY),
            // The host saved the ride as any other: its summary.
            (
                Page::DiscardRide,
                card(Saved),
                since(Paused),
                both(Go::Summary, RideMessage::Clear),
            ),
            (
                Page::DiscardRide,
                card(Recording),
                since(Paused),
                both(Go::BackToRide, RideMessage::Clear),
            ),
        ]);
    }

    fn pending(ride: Status) -> Seen {
        Seen {
            pending: true,
            recording: true,
            ..seen(ride)
        }
    }

    fn press(button: Role) -> Event {
        Event::Press {
            button,
            held: false,
        }
    }

    fn answer(action: Action, accepted: bool, origin: Origin) -> Event {
        Event::Answered {
            action,
            accepted,
            origin,
        }
    }

    fn since(before: Status) -> Event {
        Event::Serviced { before }
    }

    fn both(go: Go, note: RideMessage) -> Step {
        Step {
            go: Some(go),
            note: Some(note),
            ..STAY
        }
    }

    fn check(rows: &[(Page, Seen, Event, Step)]) {
        for (page, seen, event, want) in rows {
            assert_eq!(
                step(*page, *seen, *event),
                *want,
                "{page:?} {seen:?} {event:?}"
            );
        }
    }

    #[test]
    fn presses() {
        let held = Event::Press {
            button: ACT,
            held: true,
        };
        let auto_paused = Seen {
            auto_paused: true,
            ..seen(Paused)
        };
        let ask = Step {
            effect: Effect::TakeOverPause,
            ..both(Go::EndRide, RideMessage::Clear)
        };
        check(&[
            (Page::Home, seen(Ready), press(ACT), submit(Action::Start)),
            (Page::Home, seen(Recording), press(ACT), go(Go::OpenRide)),
            (Page::Home, pending(Ready), press(ACT), go(Go::OpenRide)),
            (
                Page::Home,
                seen(Scanning),
                press(ACT),
                effect(Effect::NotReadyToast),
            ),
            (Page::Home, seen(Ready), press(BACK), STAY),
            (Page::Home, seen(Ready), press(MOVE), STAY),
            (
                Page::Ride,
                seen(Recording),
                press(ACT),
                submit(Action::Pause),
            ),
            (Page::Ride, seen(Paused), press(ACT), submit(Action::Resume)),
            (
                Page::Ride,
                auto_paused,
                press(ACT),
                effect(Effect::TakeOverPause),
            ),
            (Page::Ride, seen(Saved), press(ACT), submit(Action::Start)),
            (
                Page::Ride,
                seen(Recording),
                press(BACK),
                effect(Effect::Lap),
            ),
            (Page::Ride, seen(Paused), press(BACK), ask),
            (Page::Ride, seen(Saved), press(BACK), go(Go::Home)),
            (Page::Ride, pending(Recording), press(ACT), STAY),
            (Page::Ride, pending(Paused), press(BACK), STAY),
            (
                Page::EndRide,
                seen(Paused),
                press(ACT),
                submit(Action::Finish),
            ),
            (
                Page::EndRide,
                seen(Paused),
                press(BACK),
                both(Go::BackToRide, RideMessage::Clear),
            ),
            (Page::EndRide, seen(Paused), press(MOVE), STAY),
            (Page::EndRide, pending(Paused), press(ACT), STAY),
            (Page::EndRide, seen(Recording), press(ACT), STAY),
            (Page::Summary, seen(Saved), press(ACT), go(Go::Home)),
            (Page::Summary, seen(Saved), press(BACK), go(Go::Home)),
            (Page::Summary, seen(Saved), held, STAY),
            (Page::Summary, pending(Paused), press(ACT), STAY),
            (Page::Summary, seen(Error), press(ACT), go(Go::Home)),
        ]);
    }

    #[test]
    fn answers() {
        let start = Origin::Auto(Decision::Start);
        let clear = RideMessage::Clear;
        let refused = RideMessage::StorageNotReady;
        check(&[
            (
                Page::Home,
                pending(Ready),
                answer(Action::Start, true, RIDER),
                both(Go::OpenRide, clear),
            ),
            (
                Page::Home,
                seen(Ready),
                answer(Action::Start, false, RIDER),
                effect(Effect::NotReadyToast),
            ),
            (
                Page::EndRide,
                pending(Paused),
                answer(Action::Finish, true, RIDER),
                both(Go::Summary, clear),
            ),
            (
                Page::EndRide,
                seen(Paused),
                answer(Action::Finish, false, RIDER),
                both(Go::BackToRide, refused),
            ),
            (
                Page::Ride,
                pending(Recording),
                answer(Action::Pause, true, RIDER),
                note(clear),
            ),
            (
                Page::Ride,
                seen(Paused),
                answer(Action::Resume, false, RIDER),
                note(refused),
            ),
            (
                Page::Ride,
                seen(Recording),
                answer(Action::Pause, false, AUTO),
                STAY,
            ),
            (
                Page::Home,
                seen(Ready),
                answer(Action::Start, false, start),
                note(refused),
            ),
        ]);
    }

    #[test]
    fn automatic_decisions() {
        check(&[
            (
                Page::Ride,
                seen(Recording),
                Event::Auto(Call::Do(Decision::Pause)),
                submit(Action::Pause),
            ),
            (
                Page::Ride,
                seen(Paused),
                Event::Auto(Call::Do(Decision::Resume)),
                submit(Action::Resume),
            ),
            (
                Page::Home,
                seen(Ready),
                Event::Auto(Call::Do(Decision::Start)),
                submit(Action::Start),
            ),
            (
                Page::Ride,
                seen(Paused),
                Event::Auto(Call::Remind),
                effect(Effect::Remind),
            ),
            (
                Page::EndRide,
                seen(Paused),
                Event::Auto(Call::Remind),
                effect(Effect::Remind),
            ),
            (
                Page::Home,
                seen(Paused),
                Event::Auto(Call::Remind),
                effect(Effect::Remind),
            ),
        ]);
    }

    #[test]
    fn service() {
        let changed = |go| both(go, RideMessage::Clear);
        check(&[
            (
                Page::EndRide,
                seen(Recording),
                since(Paused),
                changed(Go::BackToRide),
            ),
            (
                Page::EndRide,
                seen(Error),
                since(Paused),
                changed(Go::BackToRide),
            ),
            (
                Page::EndRide,
                seen(Saved),
                since(Paused),
                changed(Go::Summary),
            ),
            (Page::EndRide, seen(Paused), since(Paused), STAY),
            (
                Page::Ride,
                seen(Saved),
                since(Recording),
                changed(Go::Summary),
            ),
            (
                Page::Summary,
                seen(Saved),
                since(Paused),
                changed(Go::Summary),
            ),
            (Page::Summary, seen(Saved), since(Saved), STAY),
            (Page::Summary, pending(Paused), since(Paused), STAY),
            (Page::Summary, seen(Error), since(Error), STAY),
            (
                Page::Summary,
                seen(Paused),
                since(Paused),
                go(Go::BackToRide),
            ),
            (
                Page::Home,
                seen(Recording),
                since(Ready),
                note(RideMessage::Clear),
            ),
            (Page::Ride, seen(Paused), since(Paused), STAY),
        ]);
    }
}
