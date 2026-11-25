//! Physical controls, touch and workout navigation.
//!
//! Buttons keep one role on every screen (see `docs/ux-flows.md`) and go to the
//! focused control, their first responder. Touch goes to what lies under the
//! finger (`hit_test`), through the arbiter in `screens::touch`: it selects
//! and changes pages, and never starts, pauses, laps or stops a ride.
use super::Runtime;
use super::mark::MoveReport;
use super::navigation::{Report, To};
use crate::ride::log::Status;
use crate::screens::ride::RidePage;
use crate::screens::sensors::Action;
use crate::screens::touch::{Control, Hit, Owner, Pull, Swipe, TouchEvent};
use crate::screens::workout::Page;
use crate::sensors::ant::device_type;
use device_api::ant::Ant;
use device_api::input::Role;
use firmware_shell::input::Input;

const DEBOUNCE_MS: u64 = 350;
/// Held buttons repeat about every 100 ms; a longer gap means a new press.
pub const HOLD_GAP_MS: u64 = 250;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    /// An input as the device received it at `received_ms`, handled at `now`.
    /// Every input-timing decision (hold continuity, debounce, double-press
    /// windows, press-to-frame and ride-start latency) uses reception, so
    /// time the edge spent queued behind rendering or storage work neither
    /// splits a hold nor hides latency. Ride data keeps the handling time.
    pub fn input_received(&mut self, input: Input, received_ms: u64, now: u64, ant: &mut impl Ant) {
        let received = received_ms.min(now);
        // Every touch edge reaches the arbiter first, whatever owns input,
        // so a sheet or a dark screen can never leave it mid-contact. A
        // contact only completes on the screen it began on.
        let hit = match input {
            Input::Touch(point) => {
                self.compose_shown();
                let radar = ant.channel(device_type::RADAR, now).is_some();
                self.compose_shown();
                self.hit_test(point, now, radar)
            }
            _ => Hit::default(),
        };
        let owner = self.touch_owner();
        let events = self
            .navigation
            .touches_mut()
            .feed(input, received, owner, |_| hit);
        if input == Input::Cancel {
            self.mark_input_lost(now, ant);
        }
        if !self.display.is_active() {
            return;
        }
        // The companion repeats reports while a button is held. A report soon
        // after the previous one of that button continues a hold, such as the
        // power-on hold of TopLeft, rather than starting a new press.
        let held = match input {
            Input::Press {
                role: button,
                code: 1,
            } => self.navigation.reported(button, received, HOLD_GAP_MS) == Report::Held,
            _ => false,
        };
        // A power sheet covers every page and takes buttons first; touch never
        // turns the device off.
        if self.power.view().is_some() {
            if let Input::Press {
                role: button,
                code: 1,
            } = input
                && !held
                && received.saturating_sub(self.navigation.last_press()) >= DEBOUNCE_MS
                && let Some(super::composition::Pressed::Asks(action)) = self.composed_press(button)
            {
                self.navigation.pressed(received);
                self.display.redraw();
                self.carry(action, held, (received, now), ant);
            }
            return;
        }
        if matches!(input, Input::Touch(_) | Input::Release | Input::Cancel) {
            for event in events {
                self.touched(event, now, ant);
            }
            if matches!(input, Input::Touch(_) | Input::Release) {
                return;
            }
        }
        if self.sheet_open() {
            if let Input::Press {
                role: button,
                code: 1,
            } = input
            {
                self.button_press(button, held, received, now, ant);
            }
            return;
        }
        // Buttons and a Cancel reach the sensor menu, so lost input makes
        // MOVE reconfirm the focus before ACT can change a sensor.
        if self.navigation.page() == Page::Sensors {
            self.sensors_input(input, received, now, ant);
            return;
        }
        let Input::Press {
            role: button,
            code: 1,
        } = input
        else {
            return;
        };
        if button != Role::Move {
            self.button_press(button, held, received, now, ant);
            return;
        }
        match self.mark_sees_every_move_report(held, received, now, ant) {
            MoveReport::Held => {}
            MoveReport::Press => {
                self.button_press(button, held, received, now, ant);
                self.mark_after_press(now, ant);
            }
        }
    }

    fn button_press(
        &mut self,
        button: Role,
        held: bool,
        received: u64,
        now: u64,
        ant: &mut impl Ant,
    ) {
        if received.saturating_sub(self.navigation.last_press()) < DEBOUNCE_MS {
            return;
        }
        self.navigation.pressed(received);
        self.display.redraw();
        if let Some(super::composition::Pressed::Asks(action)) = self.composed_press(button) {
            self.act(action, held, received, now, ant);
        }
    }

    /// Everything that decides what a touch lands on: the page and its
    /// subview (the ride page, the sensor menu's tiles, detail or replacement
    /// page), a power sheet, and a dark screen. Any change cancels a contact in progress.
    ///
    /// Left out on purpose, because touch never acts on them: Home's cursor
    /// (targets have fixed bounds), the end-ride
    /// confirm window (touch never controls a ride) and toasts (not
    /// tappable).
    fn touch_owner(&self) -> Owner {
        let navigation = &self.navigation;
        let page = navigation.page() as u32;
        let subview = match navigation.page() {
            // The paused screen is a view of its own over the ride pages.
            Page::Ride => {
                let paused = self.ride.active().status() == Status::Paused;
                let screen = paused && !navigation.pause_hidden();
                navigation.ride_page().index() as u32 | u32::from(screen) << 3
            }
            Page::Sensors => u32::from(self.menu.touch_view()),
            Page::Boot
            | Page::Format
            | Page::Home
            | Page::EndRide
            | Page::Done
            | Page::LoseRides => 0,
            Page::Phone => self.screen_id() as u32,
        };
        let sheet = u32::from(self.power.view().is_some());
        let dark = u32::from(!self.display.is_active());
        let pulled = u32::from(self.sheet_open());
        Owner(page | subview << 4 | sheet << 12 | dark << 13 | pulled << 14)
    }

    /// What lies under a finger landing at `point`: the control a tap
    /// would act on and whether the page swipes.
    pub(super) fn hit_test(&self, point: device_api::input::Point, now: u64, radar: bool) -> Hit {
        if self.sheet_open() {
            return self.sheet_hit(point);
        }
        let (x, y) = (usize::from(point.x), usize::from(point.y));
        let navigation = &self.navigation;
        let paused_screen = self.paused_screen_shown();
        let composed = self.composed_hit(point);
        let key = self.key_at(x, y);
        let control = match navigation.page() {
            // The sensor menu takes its taps itself, as it takes its presses.
            Page::Sensors => Some(Control::Menu(point)),
            _ if composed.is_some() => composed,
            _ if key.is_some() => key,
            Page::Done => Some(Control::Key(device_api::input::Role::Act)),
            Page::Phone if self.screen_id() == crate::compose::builtin::ScreenId::PhoneDone => {
                Some(Control::Key(device_api::input::Role::Act))
            }
            Page::Ride if paused_screen => None,
            // The course sign floats over every ride page, a region of the
            // screen rather than a tile of its tree.
            Page::Ride
                if navigation.ride_page() != RidePage::Map && self.sign_at(x, y, now, radar) =>
            {
                Some(Control::Sign)
            }
            // A hold anywhere on the map stops following the course.
            Page::Ride if navigation.ride_page() == RidePage::Map => Some(Control::Map),
            // A screen with a tree acts only through its tiles.
            _ => None,
        };
        Hit {
            control,
            swipes: navigation.page() == Page::Ride,
            pull: self.pulls_down(y).then_some(Pull::Down),
        }
    }

    fn key_at(&self, x: usize, y: usize) -> Option<Control> {
        let laid = self.composition.laids().get(self.screen_id())?;
        let mut cues = laid.controls().shown(laid.focus());
        cues.find(|cue| cue.target(self.device).contains(x, y))
            .map(|cue| Control::Key(cue.role))
    }

    fn sign_at(&self, x: usize, y: usize, now: u64, radar: bool) -> bool {
        let sign = super::course::sign_of(&self.track, now);
        let page = self.navigation.ride_page();
        let chip = crate::screens::ride::main_chip(self.composition.main());
        crate::screens::sign::on_page(page, sign, radar, self.device, chip)
            .is_some_and(|placed| placed.rect().contains(x, y))
    }

    /// One event of a contact on the page it began on.
    fn touched(&mut self, event: TouchEvent, now: u64, ant: &mut impl Ant) {
        match event {
            TouchEvent::Pressed(_) | TouchEvent::Released(_) => {}
            TouchEvent::Activated(control) => self.activate(control, now, ant),
            TouchEvent::Hold(Control::Map) => {
                if let Some(action) = self.composed_hold() {
                    self.carry(action, true, (now, now), ant);
                }
            }
            TouchEvent::Hold(_) => {}
            TouchEvent::Swipe(swipe) => self.swiped(swipe),
            TouchEvent::Pull(Pull::Down) => self.open_sheet(),
            TouchEvent::Pull(Pull::Up) => self.close_sheet(),
            TouchEvent::Drag(Control::Level { left, right }, point) => {
                self.drag_level((left, right), point.x, now)
            }
            TouchEvent::Drag(..) => {}
        }
        self.display.redraw();
    }

    /// A tap lifted on `control`: it acts, and takes the focus if it has one.
    fn activate(&mut self, control: Control, now: u64, ant: &mut impl Ant) {
        match control {
            Control::Composed(action @ crate::compose::input::Action::Sheet(_), _) => {
                self.act(action, false, now, now, ant)
            }
            Control::Composed(action, focus) => {
                self.composed_focus(focus);
                self.act(action, false, now, now, ant);
            }
            Control::Key(role) => self.button_press(role, false, now, now, ant),
            Control::Menu(point) => {
                self.refresh_sensor_menu(ant, now);
                let (x, y) = (usize::from(point.x), usize::from(point.y));
                self.compose_shown();
                let laids = self.composition.laids();
                let Some(laid) = laids.get(self.menu.screen()) else {
                    return;
                };
                let action = self.menu.tap_at(laid, self.device, x, y, now);
                self.sensors_action(action, now, ant);
            }
            Control::Sign => self.navigation.go(To::RidePage(RidePage::Map)),
            Control::Map | Control::Level { .. } => {}
        }
    }

    /// A sideways swipe: the next or previous ride page.
    fn swiped(&mut self, swipe: Swipe) {
        let next = swipe == Swipe::Next;
        match self.navigation.page() {
            Page::Ride if next => self.next_ride_page(),
            Page::Ride => self.previous_ride_page(),
            _ => {}
        }
    }

    fn sensors_input(&mut self, input: Input, received: u64, now: u64, ant: &mut impl Ant) {
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.input(input, received);
        self.sensors_action(action, now, ant);
    }

    /// Show the sensor pages. With nothing saved they open on NEARBY,
    /// which searches.
    pub(super) fn open_sensors(&mut self, ant: &mut impl Ant, now: u64) {
        self.navigation.go(To::Page(Page::Sensors));
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.open();
        self.sensors_action(action, now, ant);
    }

    fn sensors_action(&mut self, action: Option<Action>, now: u64, ant: &mut impl Ant) {
        match action {
            Some(Action::Scan) => self.request_search(ant, now),
            Some(Action::StopScan) => self.cancel_search(ant, now),
            Some(action @ (Action::Connect(_) | Action::Forget(_))) => {
                self.request_sensor(action, ant, now)
            }
            Some(Action::Calibrate(meter)) => self.calibrate(meter, ant, now),
            Some(Action::Ride) => {
                self.cancel_search(ant, now);
                self.navigation.go(To::Page(Page::Home));
            }
            None => {}
        }
        self.refresh_sensor_menu(ant, now);
        if let Some(Action::Connect(peer)) = action {
            self.menu.added(peer);
        }
        self.display.redraw();
    }

    /// BottomLeft steps through the ride pages, wrapping after the last.
    /// While paused the paused screen comes first: from it to the page the
    /// rider was on, and from the last page back to it.
    pub(super) fn next_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden() {
            navigation.go(To::LeavePause {
                wrapped_to: pages.first(),
            });
            return;
        }
        match pages.next(navigation.ride_page()) {
            Some(next) => navigation.go(To::RidePage(next)),
            None if paused => navigation.go(To::PauseWrapped),
            None => navigation.go(To::RidePage(pages.first())),
        }
    }

    /// A swipe back: the mirror of `next_ride_page`.
    pub(super) fn previous_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden() {
            navigation.go(To::LeavePause {
                wrapped_to: pages.last(),
            });
            return;
        }
        if paused && navigation.ride_page() == pages.first() {
            navigation.go(To::PauseWrapped);
            return;
        }
        let page = pages.preceding(navigation.ride_page());
        navigation.go(To::RidePage(page));
    }
}
