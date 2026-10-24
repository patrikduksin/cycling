use super::Runtime;
use super::cues::Cue;
use super::recording::{Request, RideMessage};
use crate::ride::auto::{Call, Decision};
use crate::ride::control::Action;
use crate::ride::log::{Source, Status};
use crate::screens::chrome::Tone;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;
use device_api::input::Button;

const ACT: Button = Button::BottomRight;
const BACK: Button = Button::TopLeft;

impl Runtime {
    pub(super) fn ride_press(&mut self, button: Button, held: bool, pressed_at: u64, now: u64) {
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
                let step = step(self.navigation.page, self.seen(), Event::Auto(call));
                self.apply(step, now);
            }
        }
    }

    pub(super) fn ride_serviced(&mut self, before: Status, now: u64) {
        let step = step(
            self.navigation.page,
            self.seen(),
            Event::Serviced { before },
        );
        self.apply(step, now);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Press {
        button: Button,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Go {
    OpenRide,
    BackToRide,
    EndRide,
    Summary,
    Home,
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

fn press(page: Page, seen: Seen, button: Button, held: bool) -> Step {
    let startable = matches!(seen.next, Status::Ready | Status::Saved | Status::Recovered);
    let riding = matches!(seen.ride, Status::Recording | Status::Paused);
    let saving = seen.pending || riding;
    match (page, button) {
        (Page::Ready, BACK) => go(Go::Home),
        (Page::Ready, ACT) if saving => go(Go::OpenRide),
        (Page::Ready, ACT) if startable => submit(Action::Start),
        (Page::Ready, ACT) => effect(Effect::NotReadyToast),
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
        (Page::Summary, ACT | BACK) if !held && !saving => go(Go::Home),
        _ => STAY,
    }
}

fn answered(page: Page, action: Action, accepted: bool, origin: Origin) -> Step {
    match (page, action, accepted) {
        (Page::Ready, Action::Start, true) => Step {
            go: Some(Go::OpenRide),
            note: Some(RideMessage::Clear),
            ..STAY
        },
        (Page::Ready, Action::Start, false) if matches!(origin, Origin::Rider { .. }) => {
            effect(Effect::NotReadyToast)
        }
        (Page::EndRide, Action::Finish, true) => Step {
            go: Some(Go::Summary),
            note: Some(RideMessage::Clear),
            ..STAY
        },
        (Page::EndRide, Action::Finish, false) => Step {
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
        Page::Ride | Page::EndRide | Page::Summary if saved => Some(Go::Summary),
        Page::EndRide if seen.ride != Status::Paused => Some(Go::BackToRide),
        Page::Summary if finish_failed => Some(Go::BackToRide),
        Page::Ready | Page::Summary | Page::History | Page::PastRide | Page::Phone
            if seen.ride == Status::Recording =>
        {
            Some(Go::OpenRide)
        }
        Page::Boot
        | Page::Home
        | Page::Sensors
        | Page::Ready
        | Page::Ride
        | Page::EndRide
        | Page::Summary
        | Page::Settings
        | Page::Bests
        | Page::Workouts
        | Page::Phone
        | Page::History
        | Page::PastRide
        | Page::Setup => None,
    };
    Step {
        go,
        note: (before != seen.ride).then_some(RideMessage::Clear),
        ..STAY
    }
}

impl Runtime {
    fn ride_event(&mut self, event: Event, origin: Origin, now: u64) {
        let first = step(self.navigation.page, self.seen(), event);
        self.apply(first, now);
        if let Some(action) = first.submit {
            let accepted = self.submit_ride(action, origin, now);
            let answer = Event::Answered {
                action,
                accepted,
                origin,
            };
            let second = step(self.navigation.page, self.seen(), answer);
            self.apply(second, now);
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            ride: self.ride.active().status(),
            next: self.ride.recorder(self.ride.medium()).status(),
            pending: self.ride.pending(),
            auto_paused: self.ride.auto.paused(),
            recording: self.recording(),
        }
    }

    fn apply(&mut self, step: Step, now: u64) {
        if let Some(note) = step.note {
            self.ride.message = note;
        }
        match step.go {
            None => {}
            Some(Go::OpenRide) => {
                self.recovery.clear();
                self.navigation.open_ride();
            }
            Some(Go::BackToRide) => self.navigation.page = Page::Ride,
            Some(Go::EndRide) => {
                self.ride.clear_lap_toast();
                self.navigation.page = Page::EndRide;
            }
            Some(Go::Summary) => {
                self.navigation.best_news =
                    super::bests::news(&self.ride.bests_found, &self.bests.bests);
                // Opening the summary is the finish; the save completing
                // under it is not a second one.
                if self.navigation.page != Page::Summary {
                    self.recovery.finish(now);
                }
                self.navigation.summary_page = 0;
                self.navigation.page = Page::Summary;
            }
            Some(Go::Home) => {
                self.navigation.leave_summary();
                self.navigation.ride_page = RidePage::Main;
                self.navigation.page = Page::Home;
                self.recovery.clear();
                self.leave_workout();
            }
        }
        match step.effect {
            Effect::None => {}
            Effect::TakeOverPause => self.ride.auto.manual(),
            Effect::Lap => {
                if let Some(lap) = self.ride.laps.lap(now)
                    && !self.workout_lap(now)
                {
                    self.ride.lap_closed(lap, now);
                }
            }
            Effect::NotReadyToast => self.navigation.toast("STORAGE NOT READY", Tone::Alert, now),
            Effect::Remind => self.cue(Cue::StillPaused, now),
        }
    }

    fn submit_ride(&mut self, action: Action, origin: Origin, now: u64) -> bool {
        self.ride.request = Request::Idle;
        let medium = self.ride.target(action);
        let accepted = self.ride.submit(medium, |recorder, token| {
            recorder.request(action, Source::Live, now, token)
        });
        match (origin, accepted) {
            (Origin::Rider { pressed_at }, true) => {
                self.ride.auto.manual();
                self.ride.auto_toast = None;
                if action == Action::Start {
                    self.timing.ride_press(pressed_at);
                }
            }
            (Origin::Rider { .. }, false) => {}
            (Origin::Auto(decision), true) => {
                self.ride.auto.accepted(decision);
                let toast = match decision {
                    Decision::Pause => "",
                    Decision::Resume => "AUTO RESUME",
                    Decision::Start => "AUTO START",
                };
                self.ride.auto_toast = Some((toast, now));
                self.display.redraw();
            }
            (Origin::Auto(_), false) => self.ride.auto.declined(),
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::{Error, Paused, Ready, Recording, Saved, Scanning};

    const MOVE: Button = Button::BottomLeft;
    const RIDER: Origin = Origin::Rider { pressed_at: 0 };
    const AUTO: Origin = Origin::Auto(Decision::Pause);

    fn seen(ride: Status) -> Seen {
        Seen {
            ride,
            next: ride,
            pending: false,
            auto_paused: false,
            recording: matches!(ride, Recording | Paused),
        }
    }

    fn pending(ride: Status) -> Seen {
        Seen {
            pending: true,
            recording: true,
            ..seen(ride)
        }
    }

    fn press(button: Button) -> Event {
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
            (Page::Ready, seen(Ready), press(ACT), submit(Action::Start)),
            (Page::Ready, seen(Recording), press(ACT), go(Go::OpenRide)),
            (Page::Ready, pending(Ready), press(ACT), go(Go::OpenRide)),
            (
                Page::Ready,
                seen(Scanning),
                press(ACT),
                effect(Effect::NotReadyToast),
            ),
            (Page::Ready, seen(Ready), press(BACK), go(Go::Home)),
            (Page::Ready, seen(Ready), press(MOVE), STAY),
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
            (Page::Home, seen(Ready), press(ACT), STAY),
        ]);
    }

    #[test]
    fn answers() {
        let start = Origin::Auto(Decision::Start);
        let clear = RideMessage::Clear;
        let refused = RideMessage::StorageNotReady;
        check(&[
            (
                Page::Ready,
                pending(Ready),
                answer(Action::Start, true, RIDER),
                both(Go::OpenRide, clear),
            ),
            (
                Page::Ready,
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
                Page::Ready,
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
                Page::Ready,
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
                Page::Ready,
                seen(Recording),
                since(Ready),
                changed(Go::OpenRide),
            ),
            (
                Page::History,
                seen(Recording),
                since(Recording),
                go(Go::OpenRide),
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
