//! The composition: the main page's and the shown screen's tiles, kept
//! solved for the rider's choices and what the ride shows (a course, a
//! radar, a workout, the arrival). It lives in the runtime, which the
//! device keeps in PSRAM, so a frame only reads where the tiles go.
use super::Runtime;
use crate::compose::Flags;
use crate::compose::builtin::{Choices, Laid, Laids, ScreenId};
use crate::compose::input::{self, Action, Focus, Go, OnPage, Owners};
use crate::ride::course::nav::Sign;
use crate::screens::ride::main_flags;
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
            laids: Laids::new(layout, Choices::default(), Flags::NONE),
        }
    }

    pub(super) fn laids(&self) -> &Laids {
        &self.laids
    }

    pub(super) fn main(&self) -> &Laid {
        self.laids.main()
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
    /// Solve the built-ins again when what they show changed, ahead of the
    /// frame that draws one, and note the screen shown.
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
        if self.composition.laids.update(choices, flags) {
            let slots = choices.main_slots;
            why!("compose.solve", slots, flags);
        }
        let shown = self.screen_id();
        self.composition.laids.show(shown, self.layout);
    }

    /// A press of `role` on the built-in shown, if one is.
    pub(super) fn composed_press(&mut self, role: Role) -> Option<Pressed> {
        let id = self.screen_id();
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
        let navigation = &self.navigation;
        ScreenId::of(
            navigation.page(),
            navigation.ride_page(),
            self.paused_screen_shown(),
            self.ftp_sheet().is_some(),
        )
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
        why!("compose.unowned.extend_step");
    }

    fn course_off(&mut self) {
        why!("compose.unowned.course_off");
    }

    fn page(&mut self, act: OnPage) {
        why!("compose.unowned", act);
    }
}
