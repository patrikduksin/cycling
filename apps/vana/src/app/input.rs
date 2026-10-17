//! Physical controls, touch gestures and workout navigation.
//!
//! Buttons keep one role on every screen (see `docs/ui.md`). Touch selects
//! and changes pages; it never starts, pauses, laps or stops a ride.
use super::Runtime;
use super::recording::{Request, RideMessage};
use crate::ride::control::Action as RideAction;
use crate::ride::log::{Source, Status};
use crate::ride::zones::TrainingProfile;
use crate::screens::gesture::{Gesture, Owner};
use crate::screens::home::{Home, Tile};
use crate::screens::ride::RidePage;
use crate::screens::sensors::Action;
use crate::screens::settings;
use crate::screens::workout::{HOME, Page};
use device_api::ant::Ant;
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;
/// Held buttons repeat about every 100 ms; a longer gap means a new press.
const HOLD_GAP_MS: u64 = 250;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        self.input_received(input, now, now, ant);
    }

    /// `input` as the device received it at `received_ms`, handled at `now`.
    /// Every input-timing decision (hold continuity, debounce, double-press
    /// windows, press-to-frame and ride-start latency) uses reception, so
    /// time the edge spent queued behind rendering or storage work neither
    /// splits a hold nor hides latency. Ride data keeps the handling time.
    pub fn input_received(&mut self, input: Input, received_ms: u64, now: u64, ant: &mut impl Ant) {
        let received = received_ms.min(now);
        // Every touch edge reaches the recognizer first, whatever owns input,
        // so a sheet or a dark screen can never leave it mid-contact. A
        // contact only completes on the screen it began on.
        let gesture = self
            .navigation
            .gestures
            .feed(input, received, self.touch_owner());
        if !self.display.is_active() {
            return;
        }
        // The companion repeats reports while a button is held. A report soon
        // after the previous one of that button continues a hold, such as the
        // power-on hold of TopLeft, rather than starting a new press.
        let held = match input {
            Input::Button { button, code: 1 } => {
                let last = &mut self.navigation.last_report[button.index().min(2)];
                let held = last.is_some_and(|at| received.saturating_sub(at) < HOLD_GAP_MS);
                *last = Some(received);
                held
            }
            _ => false,
        };
        // A power sheet covers every page and takes buttons first; touch never
        // turns the device off.
        if self.power.view().is_some() {
            if let Input::Button { button, code: 1 } = input
                && !held
                && received.saturating_sub(self.navigation.last_press) >= DEBOUNCE_MS
                && self.power_input(button)
            {
                self.navigation.last_press = received;
                self.display.redraw();
            }
            return;
        }
        // Settings pads act on contact, not on a finished gesture.
        if self.navigation.page == Page::Settings
            && let Some(Gesture::Down(point)) = gesture
        {
            self.settings_touch(point, now);
            self.display.redraw();
            return;
        }
        if matches!(input, Input::Touch(_) | Input::Release) {
            if let Some(gesture) = gesture {
                self.gesture(gesture, now, ant);
            }
            return;
        }
        // Buttons and a Cancel reach the sensor menu, so lost input makes
        // MOVE reconfirm the focus before ACT can change a sensor.
        if self.navigation.page == Page::Sensors {
            self.sensors_input(input, received, now, ant);
            return;
        }
        let Input::Button { button, code: 1 } = input else {
            return;
        };
        if received.saturating_sub(self.navigation.last_press) < DEBOUNCE_MS {
            return;
        }
        self.navigation.last_press = received;
        self.display.redraw();
        match self.navigation.page {
            Page::Home => self.home_input(button, received, now, ant, held),
            Page::Ride => {
                self.timing.press(received);
                self.ride_input(button, received, now)
            }
            Page::Settings => self.settings_input(button, now),
            // A press during the brand flash opens Home straight away.
            Page::Boot => self.navigation.page = Page::Home,
            Page::Sensors | Page::Preflight => {}
        }
    }

    /// Everything that decides what a touch lands on: the page and its
    /// subview (the ride page, the sensor menu's tiles, detail or replacement
    /// page, Settings' edit mode with its pads), a power sheet, and a dark
    /// screen. Any change cancels a contact in progress.
    ///
    /// Left out on purpose, because touch never acts on them: Home's cursor
    /// and Settings' selected field (targets have fixed bounds), the end-ride
    /// confirm window (touch never controls a ride) and toasts (not
    /// tappable).
    fn touch_owner(&self) -> Owner {
        let navigation = &self.navigation;
        let page = navigation.page as u32;
        let subview = match navigation.page {
            Page::Ride => navigation.ride_page.index() as u32,
            Page::Sensors => u32::from(self.menu.touch_view()),
            Page::Settings => u32::from(navigation.settings_editing),
            Page::Boot | Page::Home | Page::Preflight => 0,
        };
        let sheet = u32::from(self.power.view().is_some());
        let dark = u32::from(!self.display.is_active());
        Owner(page | subview << 4 | sheet << 12 | dark << 13)
    }

    fn gesture(&mut self, gesture: Gesture, now: u64, ant: &mut impl Ant) {
        match (self.navigation.page, gesture) {
            (Page::Home, Gesture::Tap(point)) => {
                let Some(tile) = Home::tile_at(usize::from(point.x), usize::from(point.y)) else {
                    return;
                };
                self.navigation.home_cursor = match tile {
                    Tile::Ride => 0,
                    Tile::Sensors => 1,
                    Tile::Settings => 2,
                };
                self.open_home_tile(now, ant);
            }
            (Page::Sensors, Gesture::Tap(point)) => {
                self.refresh_sensor_menu(ant, now);
                let action = self.menu.tap(usize::from(point.x), usize::from(point.y));
                self.sensors_action(action, now, ant);
            }
            // Changing the page is safe while riding; ride control is not.
            (Page::Ride, Gesture::Next) => {
                let page = self.navigation.ride_page.following();
                self.navigation.ride_page_chosen(page)
            }
            (Page::Ride, Gesture::Previous) => {
                let page = self.navigation.ride_page.preceding();
                self.navigation.ride_page_chosen(page)
            }
            _ => return,
        }
        self.display.redraw();
    }

    fn sensors_input(&mut self, input: Input, received: u64, now: u64, ant: &mut impl Ant) {
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.input(input, received);
        self.sensors_action(action, now, ant);
    }

    fn sensors_action(&mut self, action: Option<Action>, now: u64, ant: &mut impl Ant) {
        match action {
            Some(Action::Scan) => self.request_search(ant, now),
            Some(action @ (Action::Connect(_) | Action::Disconnect(_))) => {
                self.request_sensor(action, ant, now)
            }
            Some(Action::Ride) => {
                self.cancel_search(ant, now);
                self.navigation.page = Page::Home;
            }
            None => {}
        }
        self.refresh_sensor_menu(ant, now);
        if let Some(Action::Connect(peer)) = action {
            self.menu.focus_type(peer.device_type);
        }
        self.display.redraw();
    }

    /// MOVE steps the focused card, ACT opens it and BACK asks to turn off.
    fn home_input(
        &mut self,
        button: Button,
        received: u64,
        now: u64,
        ant: &mut impl Ant,
        held: bool,
    ) {
        match button {
            Button::BottomLeft => {
                self.navigation.home_cursor = (self.navigation.home_cursor + 1) % HOME.len()
            }
            Button::BottomRight => self.open_home_tile(now, ant),
            // BACK on Home asks to turn off; Home is never reachable mid-ride.
            Button::TopLeft if !held => self.open_power_sheet(received),
            _ => {}
        }
    }

    fn open_home_tile(&mut self, now: u64, ant: &mut impl Ant) {
        self.navigation.page_since = now;
        match self.navigation.home_cursor {
            0 => self.navigation.page = Page::Preflight,
            1 => {
                self.navigation.page = Page::Sensors;
                self.request_search(ant, now);
            }
            _ => {
                self.navigation.page = Page::Settings;
                self.navigation.settings_field = 0;
                self.navigation.settings_editing = false;
            }
        }
    }

    /// Choosing a field: BottomLeft moves to the next, BottomRight edits it,
    /// TopLeft goes Home. Editing: BottomLeft lowers and BottomRight raises
    /// the field by `settings::STEP`, TopLeft finishes.
    fn settings_input(&mut self, button: Button, now: u64) {
        let navigation = &mut self.navigation;
        if !navigation.settings_editing {
            match button {
                Button::BottomLeft => {
                    navigation.settings_field =
                        (navigation.settings_field + 1) % settings::FIELDS.len()
                }
                Button::BottomRight => navigation.settings_editing = true,
                Button::TopLeft => navigation.page = Page::Home,
                _ => {}
            }
            return;
        }
        match button {
            Button::BottomLeft => self.adjust_setting(-i32::from(settings::STEP), now),
            Button::BottomRight => self.adjust_setting(i32::from(settings::STEP), now),
            Button::TopLeft => navigation.settings_editing = false,
            _ => {}
        }
    }

    /// A touch on a field edits it; touches on the pads lower or raise it.
    /// Each acts once, when the contact begins on Settings.
    fn settings_touch(&mut self, point: device_api::input::Point, now: u64) {
        let navigation = &mut self.navigation;
        match settings::target(
            usize::from(point.x),
            usize::from(point.y),
            navigation.settings_editing,
        ) {
            Some(settings::Target::Field(field)) => {
                navigation.settings_field = field;
                navigation.settings_editing = true;
            }
            Some(settings::Target::Lower) => self.adjust_setting(-i32::from(settings::STEP), now),
            Some(settings::Target::Raise) => self.adjust_setting(i32::from(settings::STEP), now),
            None => {}
        }
    }

    /// Move the selected field by `step`, within its range.
    fn adjust_setting(&mut self, step: i32, now: u64) {
        let field = self.navigation.settings_field;
        let mut profile = self.preferences.profile;
        let (value, range) = if field == 0 {
            (&mut profile.ftp_watts, TrainingProfile::FTP_WATTS)
        } else {
            (&mut profile.lthr_bpm, TrainingProfile::LTHR_BPM)
        };
        *value = (i32::from(*value) + step)
            .clamp(i32::from(*range.start()), i32::from(*range.end())) as u16;
        self.preferences.set(profile, now);
    }

    /// Ride buttons, with one meaning system-wide: BottomRight acts (start,
    /// pause, resume), BottomLeft moves to the next page, and TopLeft marks or
    /// backs out: a lap while recording, end ride while paused (pressed twice
    /// within the confirm window), Home when no ride is open.
    /// `received` times the press; `now` stamps the ride data it changes.
    fn ride_input(&mut self, button: Button, received: u64, now: u64) {
        if button == Button::BottomLeft {
            self.next_ride_page();
            return;
        }
        if button == Button::TopLeft && !self.recording() {
            self.navigation.ride_page = RidePage::Main;
            self.navigation.page = Page::Home;
            return;
        }
        if self.ride.pending() {
            return;
        }
        let medium = self.ride.medium();
        let status = self.ride.recorder(medium).status();
        let action = match (button, status) {
            (Button::BottomRight, Status::Recording) => RideAction::Pause,
            (Button::BottomRight, Status::Paused) => RideAction::Resume,
            (Button::BottomRight, Status::Ready | Status::Saved | Status::Recovered) => {
                RideAction::Start
            }
            (Button::TopLeft, Status::Recording) => {
                self.ride.disarm_stop();
                if let Some(lap) = self.ride.laps.lap(now) {
                    self.ride.lap_closed(lap, now);
                }
                return;
            }
            // Only a paused ride ends, and only on a confirming second press.
            (Button::TopLeft, Status::Paused) => {
                if !self.ride.stop_confirming(received) {
                    self.ride.arm_stop(received);
                    return;
                }
                RideAction::Finish
            }
            _ => return,
        };
        // ACT on an automatic pause keeps the ride paused, now by the rider.
        if action == RideAction::Resume && self.ride.auto.take_over() {
            return;
        }
        self.ride.request = Request::Idle;
        if self.ride.submit(medium, |recorder, token| {
            recorder.request(action, Source::Live, now, token)
        }) {
            self.ride.auto.manual();
            if action == RideAction::Start {
                self.timing.ride_press(received);
            }
            self.ride.message = RideMessage::Ready;
            self.ride.disarm_stop();
        } else {
            self.ride.message = RideMessage::StorageNotReady;
        }
    }

    /// BottomLeft steps through the ride pages, wrapping after the last.
    fn next_ride_page(&mut self) {
        let navigation = &mut self.navigation;
        let next = navigation.ride_page.next().unwrap_or(RidePage::Main);
        navigation.ride_page_chosen(next);
    }
}
