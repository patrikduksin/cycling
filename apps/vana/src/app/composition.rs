//! The composition: the main page's and the shown screen's tiles, kept
//! solved for the rider's choices and what the ride shows (a course, a
//! radar, a workout, the arrival). It lives in the runtime, which the
//! device keeps in PSRAM, so a frame only reads where the tiles go.
use super::Runtime;
use crate::compose::builtin::{Asks, Choices, Laid, Laids, ScreenId};
use crate::compose::input::{self, Action, Focus, Go, OnPage, Owners};
use crate::compose::{Flags, Screens};
use crate::ride::course::nav::Sign;
use crate::screens::ride::main_flags;
use crate::screens::ride::summary::{self, After};
use crate::screens::touch::Control;
use crate::screens::workout::Page;
use device_api::input::Role;
use firmware_services::why;

pub(super) struct Composition {
    laids: Laids,
}

impl Composition {
    pub(super) fn new(layout: &crate::screens::Layout) -> Self {
        Self {
            laids: Laids::new(layout, Choices::default(), Flags::NONE, &Screens::EMPTY),
        }
    }

    pub(super) fn laids(&self) -> &Laids {
        &self.laids
    }

    pub(super) fn main(&self) -> &Laid {
        self.laids.main()
    }

    pub(super) fn show(
        &mut self,
        id: ScreenId,
        layout: &crate::screens::Layout,
        changed: &Screens,
    ) {
        self.laids.show(id, layout, changed);
    }
}

#[cfg(not(target_os = "none"))]
impl firmware_services::why::State for Composition {
    fn fields(&self, out: &mut firmware_services::why::Fields<'_>) {
        out.field("main_slots", &self.main().choices().main_slots);
        out.field("main_flags", &self.main().flags());
    }
}

impl Runtime {
    /// Solve the main page and the screen shown again when what they show
    /// changed, the rider's screens among it, ahead of the frame that draws
    /// one, and note the screen shown.
    #[inline(never)]
    pub(super) fn compose(&mut self, radar: bool, now: u64) {
        let course = self.course().is_some();
        let arrived = course && super::course::sign_of(&self.track, now) == Sign::Arrived;
        let workout = self
            .training
            .player
            .as_ref()
            .is_some_and(|player| !player.done());
        let flags = main_flags(course, radar, workout, arrived);
        let choices = Choices {
            main_slots: self.preferences.options.main_slots,
        };
        let shown = self.screen_id();
        let changed = super::overrides::shown(&self.overrides);
        let laids = &mut self.composition.laids;
        if laids.update(choices, flags, changed) {
            let slots = choices.main_slots;
            why!("compose.solve", slots, flags);
        }
        laids.show(shown, self.layout, changed);
    }

    /// A press of `role` on the built-in shown, if one is.
    pub(super) fn composed_press(&mut self, role: Role) -> Option<Pressed> {
        let id = self.screen_id();
        // A press may come before the frame that shows a screen just opened.
        let changed = super::overrides::shown(&self.overrides);
        self.composition.laids.show(id, self.layout, changed);
        let laid = self.composition.laids.get_mut(id)?;
        let mut focus = laid.focus();
        let asked = input::press(role, laid.buttons(), &laid.solved(), &mut focus);
        *laid.focus_mut() = focus;
        Some(match asked {
            Some(action) => Pressed::Asks(action),
            None => Pressed::Nothing,
        })
    }

    /// The tile of the built-in shown under `point`, if one acts.
    pub(super) fn composed_hit(&self, point: device_api::input::Point) -> Option<Control> {
        let laid = self.composition.laids.get(self.screen_id())?;
        let (focus, action) = input::hit(laid.buttons(), &laid.solved(), point.x, point.y)?;
        Some(Control::Composed(action, focus))
    }

    /// What holding MOVE does on the built-in shown, when its owner would
    /// take it now: a hold that does nothing marks instead.
    pub(super) fn composed_hold(&mut self) -> Option<Action> {
        let id = self.screen_id();
        // A hold may come before the frame that shows a screen just opened.
        let changed = super::overrides::shown(&self.overrides);
        self.composition.laids.show(id, self.layout, changed);
        let laid = self.composition.laids.get(id)?;
        let action = laid.buttons().held?;
        let takes = match action {
            Action::CourseOff => self.following(),
            Action::ExtendStep => {
                self.ride.active().status() == crate::ride::log::Status::Recording
                    && self.step_extendable()
            }
            _ => false,
        };
        takes.then_some(action)
    }

    pub(super) fn composed_focus(&mut self, focus: Focus) {
        let id = self.screen_id();
        if let Some(laid) = self.composition.laids.get_mut(id) {
            *laid.focus_mut() = focus;
        }
    }

    pub(super) fn carry(&mut self, action: Action, held: bool, received: u64, now: u64) {
        input::apply(
            action,
            &mut Carried {
                runtime: self,
                held,
                received,
                now,
            },
        );
    }

    pub(super) fn paused_screen_shown(&self) -> bool {
        self.navigation.page() == crate::screens::workout::Page::Ride
            && self.ride.active().status() == crate::ride::log::Status::Paused
            && !self.navigation.pause_hidden()
    }

    pub(super) fn screen_id(&self) -> ScreenId {
        if let Some(sheet) = self.power.view() {
            return sheet.screen();
        }
        let navigation = &self.navigation;
        ScreenId::of(
            navigation.page(),
            navigation.ride_page(),
            self.paused_screen_shown(),
            self.ftp_sheet().is_some(),
            self.asks(),
            self.summary_shown(),
        )
    }

    fn asks(&self) -> Asks {
        match self.navigation.page() {
            Page::Bests => self.bests.mode().asks(),
            Page::Form => self.form.mode.asks(),
            Page::Format => self.format.shows().asks(),
            Page::DiscardRide if self.ride.discard() != crate::ride::current::Discard::Idle => {
                Asks::Wait
            }
            _ => Asks::Own,
        }
    }

    /// The summary page shown, from what decides which screen it is.
    fn summary_shown(&self) -> summary::Shown {
        let after = match self.navigation.page() {
            Page::Summary => After {
                saving: self.saving(),
                offer: self.summary_offer(),
                ..After::default()
            },
            Page::PastRide => After {
                saving: self.past_saving(),
                past: true,
                delete: self.deletable(),
                ..After::default()
            },
            _ => return summary::Shown::Message,
        };
        summary::Shown::of(&After {
            page: self.navigation.summary_page(),
            elevation: self.has_elevation(),
            ..after
        })
    }
}

/// What a press on a built-in asked for.
pub(super) enum Pressed {
    Nothing,
    Asks(Action),
}

/// The runtime as the owners an action reaches, for one press.
struct Carried<'r> {
    runtime: &'r mut Runtime,
    held: bool,
    received: u64,
    now: u64,
}

impl Owners for Carried<'_> {
    fn go(&mut self, go: Go) {
        let runtime = &mut *self.runtime;
        match (go, runtime.navigation.page()) {
            (Go::Next, Page::Ride) => {
                runtime.timing.press(self.received);
                runtime.next_ride_page();
            }
            (Go::Next, Page::Summary | Page::PastRide) => runtime.next_summary_page(1),
            (Go::Open(page), _) => runtime.navigation.go(super::navigation::To::Page(page)),
            (Go::Next | Go::Map | Go::TurnOff, _) => why!("compose.unowned", go),
        }
    }

    fn ride(&mut self, role: Role) {
        let runtime = &mut *self.runtime;
        runtime.timing.press(self.received);
        runtime.ride_press(role, self.held, self.received, self.now);
    }

    fn extend_step(&mut self) {
        self.runtime.extend_step(self.now);
    }

    fn course_off(&mut self) {
        self.runtime.stop_following_from_map(self.now);
    }

    fn page(&mut self, act: OnPage) {
        let (runtime, held) = (&mut *self.runtime, self.held);
        // A power sheet covers every page, the FTP sheet whatever page it
        // shows over.
        if runtime.power.view().is_some() {
            if let OnPage::Answer(yes) = act {
                runtime.answer_power(yes);
            }
            return;
        }
        if runtime.ftp_sheet().is_some() {
            if let OnPage::Answer(yes) = act {
                runtime.answer_ftp(yes, self.now);
            }
            return;
        }
        match (runtime.navigation.page(), act) {
            (Page::Bests, _) => runtime.bests_input(act, held),
            (Page::Form, _) => runtime.form_input(act, held),
            (Page::Format, OnPage::Answer(yes)) => runtime.format_input(yes, held, self.received),
            (Page::LoseRides, OnPage::Answer(yes)) if !held => runtime.lose_rides_input(yes),
            (Page::Summary, OnPage::Answer(take)) => {
                let role = if take { Role::Act } else { Role::Back };
                if !held {
                    runtime.answer_ftp_offer(role, self.now);
                }
                runtime.ride_press(role, held, self.received, self.now);
            }
            (Page::PastRide, OnPage::Send | OnPage::Delete) => {
                runtime.past_ride_input(Role::Act, held);
            }
            (Page::PastRide, OnPage::Back) => runtime.past_ride_input(Role::Back, held),
            _ => why!("compose.unowned", act),
        }
    }
}
