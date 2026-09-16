//! Physical controls and workout navigation.
use super::Runtime;
use crate::ride::log::Source;

impl Runtime {
    pub(super) fn workout_input(
        &mut self,
        input: device_api::input::Input,
        now: u64,
        ant: &mut impl device_api::ant::Ant,
    ) {
        use crate::screens::workout::Page;
        use device_api::input::Button;
        use device_api::input::Input;
        if self.page == Page::Sensors {
            self.menu.refresh(
                ant.discoveries(),
                ant.channels(now),
                self.scan.busy(ant),
                now,
            );
            if let Some(action) = self.menu.input(input, now) {
                use crate::screens::sensors::Action;
                match action {
                    Action::Scan => {
                        self.search_requested = true;
                        self.start_sensor_search(ant, now);
                    }
                    Action::Connect(_) | Action::Disconnect(_) => {
                        self.request_sensor(action, ant, now)
                    }
                    Action::Ride => {
                        self.search_requested = false;
                        self.scan.cancel(ant, now);
                        self.page = Page::Home;
                    }
                }
            }
            self.next_display = 0;
            return;
        }
        let Input::Button { button, code: 1 } = input else {
            return;
        };
        if now.saturating_sub(self.last_press) < 350 {
            return;
        }
        self.last_press = now;
        self.next_display = 0;
        match self.page {
            Page::Boot => {}
            Page::Home => match button {
                Button::BottomLeft => self.home_cursor = !self.home_cursor,
                Button::BottomRight => {
                    self.page = if self.home_cursor {
                        Page::Sensors
                    } else {
                        Page::Preflight
                    };
                    self.page_since = now;
                    if self.page == Page::Sensors {
                        self.search_requested = true;
                        self.start_sensor_search(ant, now);
                    }
                }
                _ => {}
            },
            Page::Sensors => {}
            Page::Preflight => {}
            Page::Ride => {
                if self.pending.is_some() {
                    return;
                }
                let action = match button {
                    Button::BottomLeft => match self.recorder.status() {
                        crate::ride::log::Status::Recording => {
                            Some(crate::ride::control::Action::Pause)
                        }
                        crate::ride::log::Status::Paused => {
                            Some(crate::ride::control::Action::Resume)
                        }
                        crate::ride::log::Status::Ready
                        | crate::ride::log::Status::Saved
                        | crate::ride::log::Status::Recovered => {
                            Some(crate::ride::control::Action::Start)
                        }
                        _ => None,
                    },
                    Button::BottomRight
                        if matches!(
                            self.recorder.status(),
                            crate::ride::log::Status::Recording | crate::ride::log::Status::Paused
                        ) =>
                    {
                        if self
                            .stop_armed
                            .is_some_and(|at| now.saturating_sub(at) <= 5000)
                        {
                            Some(crate::ride::control::Action::Finish)
                        } else {
                            self.stop_armed = Some(now);
                            self.ui_message = "PRESS STOP AGAIN TO SAVE";
                            None
                        }
                    }
                    Button::TopLeft if !self.recording() => {
                        self.page = Page::Home;
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    self.completion = None;
                    let token = self.next_token;
                    if self.recorder.request(action, Source::Live, now, token) {
                        self.pending = Some(token);
                        self.next_token = self.next_token.wrapping_add(1).max(1);
                        self.ui_message = "READY";
                        self.stop_armed = None;
                    } else {
                        self.ui_message = "STORAGE NOT READY";
                    }
                }
            }
        }
    }

    pub fn input_active(&self) -> bool {
        self.display_active
    }

    pub fn input(
        &mut self,
        input: device_api::input::Input,
        now: u64,
        ant: &mut impl device_api::ant::Ant,
    ) {
        if self.display_active {
            self.workout_input(input, now, ant);
        }
    }
}
