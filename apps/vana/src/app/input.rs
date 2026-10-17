//! Physical controls and workout navigation.
use super::Runtime;
use super::recording::{Request, RideMessage};
use crate::ride::control::Action as RideAction;
use crate::ride::log::{Source, Status};
use crate::screens::ride::RidePage;
use crate::screens::sensors::Action;
use crate::screens::workout::Page;
use device_api::ant::Ant;
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        if !self.display.is_active() {
            return;
        }
        if self.navigation.page == Page::Sensors {
            self.sensors_input(input, now, ant);
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
            Page::Boot | Page::Sensors | Page::Preflight => {}
        }
    }

    fn sensors_input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        self.menu.refresh(
            ant.discoveries(),
            ant.channels(now),
            self.discovery.scan.busy(ant),
            now,
        );
        match self.menu.input(input, now) {
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
        self.display.redraw();
    }

    fn home_input(&mut self, button: Button, now: u64, ant: &mut impl Ant) {
        match button {
            Button::BottomLeft => self.navigation.home_cursor = !self.navigation.home_cursor,
            Button::BottomRight => {
                self.navigation.page_since = now;
                if self.navigation.home_cursor {
                    self.navigation.page = Page::Sensors;
                    self.request_search(ant, now);
                } else {
                    self.navigation.page = Page::Preflight;
                }
            }
            _ => {}
        }
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
