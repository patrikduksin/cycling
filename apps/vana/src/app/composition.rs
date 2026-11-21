//! The composition: the main page's and the shown screen's tiles, kept
//! solved for the rider's choices and what the ride shows (a course, a
//! radar, a workout, the arrival). It lives in the runtime, which the
//! device keeps in PSRAM, so a frame only reads where the tiles go.
use super::Runtime;
use super::navigation::To;
use crate::compose::builtin::{Asks, Choices, Laid, Laids, ScreenId};
use crate::compose::input::{self, Action, Focus, Go, ListAct, OnPage, Owners};
use crate::compose::node::What;
use crate::compose::{Flags, Screens};
use crate::ride::course::nav::Sign;
use crate::screens::ride::main_flags;
use crate::screens::ride::summary::{self, After};
use crate::screens::touch::Control;
use crate::screens::workout::Page;
use device_api::ant::Ant;
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
            editing: self.editing_number(),
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

    /// Solve the screen shown for what the rider chose, keeping what the
    /// ride shows: a press or a touch may come before the frame that shows
    /// a screen just opened or an editor just entered.
    pub(super) fn compose_shown(&mut self) {
        let choices = Choices {
            main_slots: self.preferences.options.main_slots,
            editing: self.editing_number(),
        };
        let flags = self.composition.main().flags();
        let shown = self.screen_id();
        let changed = super::overrides::shown(&self.overrides);
        let laids = &mut self.composition.laids;
        laids.update(choices, flags, changed);
        laids.show(shown, self.layout, changed);
    }

    /// A press of `role` on the built-in shown, if one is.
    pub(super) fn composed_press(&mut self, role: Role) -> Option<Pressed> {
        // The number editor's MOVE lowers and ACT raises, holding repeats:
        // its own owner takes them.
        if self.editing_number() {
            return None;
        }
        let id = self.screen_id();
        // A press may come before the frame that shows a screen just opened.
        self.compose_shown();
        let cursor = self.list_cursor_at();
        let laid = self.composition.laids.get_mut(id)?;
        let mut focus = laid.focus();
        let asked = input::press(role, laid.buttons(), &laid.solved(), &mut focus, cursor);
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
        // A pad steps the number while the finger rests on it, not as a tap.
        Some(match action {
            Action::Page(OnPage::Lower) => Control::Lower,
            Action::Page(OnPage::Raise) => Control::Raise,
            _ => Control::Composed(action, focus),
        })
    }

    /// Settings shows the number editor.
    pub(super) fn editing_number(&self) -> bool {
        let navigation = &self.navigation;
        navigation.page() == Page::Settings
            && navigation.editing()
            && navigation.setting().kind() == crate::screens::settings::Kind::Number
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
        match (action, self.page_list()) {
            (Action::List(act), Some(list)) => self.list_input(list, act, held, now, ant),
            _ => self.carry(action, held, (received, now), ant),
        }
    }

    /// The page of cards built-in `id` scrolls, solved afresh when it is
    /// not the screen shown.
    #[inline(never)]
    pub(super) fn composed_page(&self, id: ScreenId) -> crate::screens::scroll::Layout {
        let laids = &self.composition.laids;
        let page = |laid: &Laid| laid.page().copied();
        let shown = laids.get(id).and_then(page);
        let main = laids.main();
        let choices = Choices {
            editing: false,
            ..main.choices()
        };
        let changed = super::overrides::shown(&self.overrides).screen(id.key());
        shown
            .or_else(|| page(&Laid::new(id, self.layout, choices, main.flags(), changed)?))
            .expect("a list screen's built-in has a scroll")
    }

    /// A tap on card `index` of the built-in shown, which focuses its list.
    pub(super) fn composed_item(&self, index: usize) -> Option<Control> {
        let id = self.screen_id();
        let tree = (id.builtin()?.tree)(&self.composition.main().choices());
        let node = tree
            .iter()
            .position(|node| matches!(node.what(), Some(What::Scroll { .. })))?;
        let action = Action::List(ListAct::Tap(index as u16));
        Some(Control::Composed(action, Focus::Tile(node as u8)))
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
            Page::Sensors => self.open_sensors(false, ant, now),
            Page::Workouts => {
                self.navigation.go(To::Page(Page::Workouts));
                self.show_focus(crate::screens::touch::List::Workouts, now);
            }
            Page::Settings => self.navigation.go(To::Settings),
            Page::History => self.open_history(),
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
            (Go::Next, Page::Summary | Page::PastRide) => runtime.next_summary_page(1),
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
            (Page::Boot, OnPage::Back) => runtime.navigation.go(To::Page(runtime.first_page())),
            (Page::Setup, OnPage::Item(card)) => {
                runtime.setup_card(usize::from(card), self.now, self.ant)
            }
            (Page::Setup, OnPage::Back) => runtime.navigation.go(To::Page(Page::Home)),
            (Page::Phone, OnPage::PhoneCode) => runtime.phone_touch(),
            (Page::Phone, OnPage::PhoneRetry) if runtime.phone_ended() => runtime.enter_phone(),
            (Page::Phone, OnPage::Back) if !held => runtime.close_phone_page(),
            _ => why!("compose.unowned", act),
        }
    }

    fn list(&mut self, act: ListAct) {
        why!("compose.unowned", act);
    }
}
