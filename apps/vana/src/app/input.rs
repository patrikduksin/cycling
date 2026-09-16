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
        if self.page == Page::Scan {
            if matches!(
                input,
                Input::Button {
                    button: Button::TopLeft,
                    code: 1
                }
            ) {
                self.scan.cancel(ant, now);
                self.menu.set_message(self.scan.message());
                self.page = Page::Sensors;
                self.next_display = 0;
                return;
            }
            self.menu.refresh(
                ant.discoveries(),
                ant.channels(now),
                self.scan.busy(ant),
                now,
            );
            if let Some(action) = self.menu.input(input, now) {
                use crate::screens::sensors::Action;
                let result = match action {
                    Action::Scan => {
                        self.scan.start(ant, now);
                        self.menu.set_message(self.scan.message());
                        self.next_display = 0;
                        return;
                    }
                    Action::Connect(peer) => {
                        let result = ant.request(device_api::ant::AntOperation::Connect(peer), now);
                        if result == Ok(device_api::ant::Admission::Accepted) {
                            for kind in &mut self.dropped_ant {
                                if *kind == Some(peer.device_type) {
                                    *kind = None;
                                }
                            }
                        }
                        result
                    }
                    Action::Disconnect(kind) => {
                        let result =
                            ant.request(device_api::ant::AntOperation::Disconnect(kind), now);
                        if result == Ok(device_api::ant::Admission::Accepted)
                            && !self.dropped_ant.contains(&Some(kind))
                        {
                            // Only selected channels can be dropped; retire entries whose slot
                            // has since been reused by another device type.
                            let channels = ant.channels(now);
                            for dropped in &mut self.dropped_ant {
                                if dropped.is_some_and(|kind| {
                                    !channels
                                        .iter()
                                        .flatten()
                                        .any(|s| s.selected.is_some_and(|p| p.device_type == kind))
                                }) {
                                    *dropped = None;
                                }
                            }
                            if let Some(slot) =
                                self.dropped_ant.iter_mut().find(|slot| slot.is_none())
                            {
                                *slot = Some(kind);
                            }
                        }
                        result
                    }
                    _ => {
                        self.page = Page::Sensors;
                        return;
                    }
                };
                use device_api::ant::{Admission, Error};
                self.menu.set_message(match result {
                    Ok(Admission::Accepted) => match action {
                        Action::Connect(_) => b"CONNECTING...",
                        Action::Disconnect(_) => b"DISCONNECTING...",
                        _ => b"PICK SENSOR",
                    },
                    Err(Error::Busy) => b"BUSY - TRY AGAIN",
                    Err(Error::Unavailable) => b"RADIO UNAVAILABLE",
                    Err(Error::Uncertain) => b"RADIO LOST - REBOOT",
                    Err(Error::UnsupportedType) => b"TYPE UNSUPPORTED",
                    _ => b"FAILED - TRY AGAIN",
                });
                if result == Ok(Admission::Accepted) {
                    match action {
                        Action::Connect(peer) => self.menu.follow_channel(peer.device_type),
                        Action::Disconnect(kind) => self.menu.follow_channel(kind),
                        _ => {}
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
                }
                _ => {}
            },
            Page::Sensors => match button {
                Button::TopLeft | Button::BottomLeft => self.page = Page::Home,
                Button::BottomRight => {
                    self.page = Page::Scan;
                }
                _ => {}
            },
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
            Page::Scan => {}
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
