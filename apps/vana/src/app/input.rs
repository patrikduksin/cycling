//! Physical controls and workout navigation.
use super::Runtime;
use super::recording::{Request, RideMessage};
use crate::ride::control::Action as RideAction;
use crate::ride::log::{Source, Status};
use crate::ride::zones::TrainingProfile;
use crate::screens::ride::RidePage;
use crate::screens::sensors::Action;
use crate::screens::settings;
use crate::screens::workout::{HOME, Page};
use device_api::ant::Ant;
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        // A lift ends the touch wherever it lands, so a finger still down
        // while a button changes page cannot swallow the next tap.
        if matches!(input, Input::Release | Input::Cancel) {
            self.navigation.touching = false;
        }
        if !self.display.is_active() {
            return;
        }
        if self.navigation.page == Page::Sensors {
            self.sensors_input(input, now, ant);
            return;
        }
        if self.navigation.page == Page::Settings && self.settings_touch(input, now) {
            self.display.redraw();
            return;
        }
        let Input::Button { button, code: 1 } = input else {
            return;
        };
        if now.saturating_sub(self.navigation.last_press) < DEBOUNCE_MS {
            return;
        }
        self.navigation.last_press = now;
        self.display.redraw();
        match self.navigation.page {
            Page::Home => self.home_input(button, now, ant),
            Page::Ride => self.ride_input(button, now),
            Page::Settings => self.settings_input(button, now),
            Page::Boot | Page::Sensors | Page::Preflight => {}
        }
    }

    fn sensors_input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.input(input, now);
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

    fn home_input(&mut self, button: Button, now: u64, ant: &mut impl Ant) {
        match button {
            Button::BottomLeft => {
                self.navigation.home_cursor = (self.navigation.home_cursor + 1) % HOME.len()
            }
            Button::BottomRight => {
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
            _ => {}
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

    /// A tap on a field edits it; taps on the pads lower or raise it. Touch
    /// acts once on contact and again only after release. Returns whether the
    /// input was touch.
    fn settings_touch(&mut self, input: Input, now: u64) -> bool {
        let point = match input {
            Input::Touch(point) => point,
            Input::Release | Input::Cancel => {
                self.navigation.touching = false;
                return true;
            }
            Input::Button { .. } => return false,
        };
        if core::mem::replace(&mut self.navigation.touching, true) {
            return true;
        }
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
        true
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

    fn ride_input(&mut self, button: Button, now: u64) {
        if button == Button::TopLeft {
            self.next_ride_page();
            return;
        }
        if self.ride.pending() {
            return;
        }
        let status = self.ride.recorder.status();
        let action = match button {
            Button::BottomLeft => match status {
                Status::Recording => RideAction::Pause,
                Status::Paused => RideAction::Resume,
                Status::Ready | Status::Saved | Status::Recovered => RideAction::Start,
                _ => return,
            },
            Button::BottomRight if matches!(status, Status::Recording | Status::Paused) => {
                if !self.ride.stop_confirming(now) {
                    self.ride.arm_stop(now);
                    return;
                }
                RideAction::Finish
            }
            _ => return,
        };
        self.ride.request = Request::Idle;
        if self
            .ride
            .submit(|recorder, token| recorder.request(action, Source::Live, now, token))
        {
            self.ride.message = RideMessage::Ready;
            self.ride.disarm_stop();
        } else {
            self.ride.message = RideMessage::StorageNotReady;
        }
    }

    /// TopLeft steps through the ride pages. Past the last one it wraps to the
    /// first while a ride is open, and leaves for Home otherwise.
    fn next_ride_page(&mut self) {
        let open = self.recording();
        let navigation = &mut self.navigation;
        match navigation.ride_page.next() {
            Some(next) => navigation.ride_page = next,
            None => {
                navigation.ride_page = RidePage::Main;
                if !open {
                    navigation.page = Page::Home;
                }
            }
        }
    }
}
