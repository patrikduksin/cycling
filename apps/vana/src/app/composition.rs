//! The composition: the main page's and the shown screen's tiles, kept
//! solved for what the ride shows (a course, a
//! radar, a workout, the arrival). It lives in the runtime, which the
//! device keeps in PSRAM, so a frame only reads where the tiles go.
use super::Runtime;
use super::navigation::To;
use crate::compose::builtin::{Asks, Laid, Laids, ScreenId};
use crate::compose::input::{self, Action, Focus, Go, ListAct, OnPage, Owners, Plan, SheetAct};
use crate::compose::{Flags, Screens};
use crate::ride::course::nav::Sign;
use crate::screens::ride::main_flags;
use crate::screens::touch::Control;
use crate::screens::workout::Page;
use device_api::ant::Ant;
use device_api::input::Role;
use firmware_services::why;

pub(super) struct Composition {
    laids: Laids,
}

impl Composition {
    pub(super) fn new(device: &crate::screens::Device) -> Self {
        Self {
            laids: Laids::new(device, Flags::NONE, &Screens::EMPTY),
        }
    }

    pub(super) fn laids(&self) -> &Laids {
        &self.laids
    }

    pub(super) fn main(&self) -> &Laid {
        self.laids.main()
    }

    pub(super) fn laids_mut(&mut self) -> &mut Laids {
        &mut self.laids
    }

    pub(super) fn show(
        &mut self,
        id: ScreenId,
        device: &crate::screens::Device,
        changed: &Screens,
    ) {
        self.laids.show(id, device, changed);
    }
}

#[cfg(not(target_os = "none"))]
impl firmware_services::why::State for Composition {
    fn fields(&self, out: &mut firmware_services::why::Fields<'_>) {
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
        let shown = self.screen_id();
        let changed = super::overrides::shown(&self.overrides);
        let sheet = self.sheet_facts(now);
        let laids = &mut self.composition.laids;
        if laids.update(flags, changed) {
            why!("compose.solve", flags);
        }
        laids.show(shown, self.device, changed);
        if let Some(facts) = sheet {
            laids.set_sheet_facts(facts);
        }
    }

    /// Solve the screen shown, keeping what the ride shows: a press or a
    /// touch may come before the frame that shows a screen just opened.
    pub(super) fn compose_shown(&mut self) {
        let flags = self.composition.main().flags();
        let shown = self.screen_id();
        let changed = super::overrides::shown(&self.overrides);
        let laids = &mut self.composition.laids;
        laids.update(flags, changed);
        laids.show(shown, self.device, changed);
    }

    pub(super) fn composed_press(&mut self, role: Role) -> Option<Pressed> {
        let id = if self.sheet_open() {
            ScreenId::ControlSheet
        } else {
            self.screen_id()
        };
        // A press may come before the frame that shows a screen just opened.
        self.compose_shown();
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

    /// Carry out what a press or a tap on a built-in asked for: a list's
    /// owner takes its items, which may open sensors over the radio.
    pub(super) fn act(
        &mut self,
        action: Action,
        held: bool,
        received: u64,
        now: u64,
        ant: &mut impl device_api::ant::Ant,
    ) {
        self.carry(action, held, (received, now), ant);
    }

    /// What holding MOVE does on the built-in shown, when its owner would
    /// take it now: a hold that does nothing marks instead.
    pub(super) fn composed_hold(&mut self) -> Option<Action> {
        self.compose_shown();
        let laid = self.composition.laids.get(self.screen_id())?;
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

    /// The focus on the built-in shown, if one is: a screen just opened
    /// has its kept focus back before anything reads it.
    pub(super) fn shown_focus(&mut self) -> Focus {
        self.compose_shown();
        let laid = self.composition.laids.get(self.screen_id());
        laid.map_or(Focus::default(), Laid::focus)
    }

    /// Which stop of the built-in shown has the focus, from 0 at the first,
    /// as `RIDE STATUS` reports Home's card.
    pub(super) fn focus_index(&self) -> usize {
        let Some(laid) = self.composition.laids.get(self.screen_id()) else {
            return 0;
        };
        let solved = laid.solved();
        let stops: heapless::Vec<u8, { crate::compose::solve::MAX_LEAVES }> =
            solved.focus_order().map(|placed| placed.node).collect();
        match input::focused(laid.buttons(), &solved, laid.focus()) {
            Some(input::Stop::Tile(placed)) => {
                let index = stops.iter().position(|&node| node == placed.node);
                index.map_or(0, |index| index + usize::from(laid.buttons().act.is_some()))
            }
            _ => 0,
        }
    }

    /// Which stop of the built-in shown starts a free ride, counted as
    /// `focus_index` counts: Home's Ride or "Just ride".
    pub(super) fn free_ride_stop(&self) -> Option<usize> {
        let laid = self.composition.laids.get(self.screen_id())?;
        let nodes = laid.screen().tree().nodes();
        let starts = |placed: &crate::compose::solve::Placed| {
            matches!(
                nodes[usize::from(placed.node)].what(),
                Some(crate::compose::node::What::Leaf {
                    component: crate::compose::registry::START,
                    ..
                })
            )
        };
        let index = laid.solved().focus_order().position(starts)?;
        Some(index + usize::from(laid.buttons().act.is_some()))
    }

    /// How many stops MOVE walks the focus through on the built-in shown,
    /// the screen's own stop included, so a harness can walk Home to RIDE.
    pub(super) fn focus_stops(&self) -> usize {
        let Some(laid) = self.composition.laids.get(self.screen_id()) else {
            return 0;
        };
        let buttons = laid.buttons();
        if buttons.move_.is_some() {
            return usize::from(buttons.act.is_some());
        }
        laid.solved().focus_order().count() + usize::from(buttons.act.is_some())
    }

    /// Put the focus on stop `index` of the built-in shown, past the screen's
    /// own stop; nothing on a screen without a tree.
    #[cfg(test)]
    pub(super) fn focus_stop(&mut self, index: usize) {
        self.compose_shown();
        let id = self.screen_id();
        if let Some(laid) = self.composition.laids.get_mut(id) {
            let node = laid.solved().focus_order().nth(index).expect("a stop").node;
            *laid.focus_mut() = Focus::Tile(node);
        }
    }

    /// The box of stop `index` of the built-in shown.
    #[cfg(test)]
    pub(super) fn stop_rect(&mut self, index: usize) -> crate::screens::chrome::Rect {
        self.compose_shown();
        let id = self.screen_id();
        let laid = self.composition.laids.get(id).expect("a built-in shows");
        laid.solved()
            .focus_order()
            .nth(index)
            .expect("a stop")
            .rect
            .rect()
    }

    pub(super) fn composed_focus(&mut self, focus: Focus) {
        let id = self.screen_id();
        if let Some(laid) = self.composition.laids.get_mut(id) {
            *laid.focus_mut() = focus;
        }
    }

    pub(super) fn carry(
        &mut self,
        action: Action,
        held: bool,
        (received, now): (u64, u64),
        ant: &mut impl Ant,
    ) {
        input::apply(
            action,
            &mut Carried {
                runtime: self,
                held,
                received,
                now,
                ant,
            },
        );
    }

    /// Open `page` the way its own opener does: a list shows its focused
    /// card, the sensor menu refreshes.
    fn open(&mut self, page: Page, ant: &mut impl Ant, now: u64) {
        match page {
            Page::Sensors => self.open_sensors(ant, now),
            page => self.navigation.go(To::Page(page)),
        }
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
        match ScreenId::of(
            navigation.page(),
            navigation.ride_page(),
            self.paused_screen_shown(),
            self.asks(),
        ) {
            ScreenId::Sensors => self.menu.screen(),
            id => id,
        }
    }

    fn asks(&self) -> Asks {
        match self.navigation.page() {
            Page::Format => self.format.shows().asks(),
            Page::Home if self.training.today.is_some() => Asks::Plan,
            _ => Asks::Own,
        }
    }
}

/// What a press on a built-in asked for.
pub(super) enum Pressed {
    Nothing,
    Asks(Action),
}

/// The runtime as the owners an action reaches, for one press.
struct Carried<'r, A> {
    runtime: &'r mut Runtime,
    held: bool,
    received: u64,
    now: u64,
    ant: &'r mut A,
}

impl<A: Ant> Owners for Carried<'_, A> {
    fn go(&mut self, go: Go) {
        let runtime = &mut *self.runtime;
        match (go, runtime.navigation.page()) {
            (Go::Next, Page::Ride) => {
                runtime.timing.press(self.received);
                runtime.next_ride_page();
            }
            (Go::Open(page), _) => runtime.open(page, self.ant, self.now),
            // A held BACK is the power-on hold still going.
            (Go::TurnOff, _) if !self.held => runtime.open_power_sheet(self.received),
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
        if runtime.power.view().is_some() {
            if let OnPage::Answer(yes) = act {
                runtime.answer_power(yes);
            }
            return;
        }
        match (runtime.navigation.page(), act) {
            (Page::Format, OnPage::Answer(yes)) => runtime.format_input(yes, held, self.received),
            (Page::LoseRides, OnPage::Answer(yes)) if !held => runtime.lose_rides_input(yes),
            (Page::Boot, OnPage::Back) => runtime.navigation.go(To::Page(runtime.first_page())),
            _ => why!("compose.unowned", act),
        }
    }

    fn list(&mut self, act: ListAct) {
        why!("compose.unowned", act);
    }

    fn sheet(&mut self, act: SheetAct) {
        self.runtime.sheet_act(act, self.ant, self.now);
    }

    fn start(&mut self, plan: Plan) {
        let runtime = &mut *self.runtime;
        runtime.timing.press(self.received);
        match plan {
            Plan::Today => runtime.ride_workout(self.held, self.received, self.now),
            Plan::Free => runtime.ride_press(Role::Act, self.held, self.received, self.now),
        }
    }
}
