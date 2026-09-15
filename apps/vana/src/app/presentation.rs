//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use core::fmt::Write;
use device_api::gps::FixState;

impl Runtime {
    fn workout_display<
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    >(
        &mut self,
        system: &mut firmware_shell::shell::Shell<D, I, P, B>,
        now: u64,
        ant: &impl device_api::ant::Ant,
        position: &impl device_api::positioning::Positioning,
    ) {
        use crate::screens::workout::Page;
        use crate::screens::workout::View;
        if self.page == Page::Boot && now >= 6500 {
            self.page = Page::Home;
        }
        if self.page == Page::Preflight && now.saturating_sub(self.page_since) >= 2500 {
            self.page = Page::Ride;
        }
        let utc = self.live.utc_ms.map(|v| v / 1000).or_else(|| {
            position.snapshot(now).and_then(|p| p.gps.utc).map(|v| {
                u64::from((v[0] - b'0') * 10 + v[1] - b'0') * 3600
                    + u64::from((v[2] - b'0') * 10 + v[3] - b'0') * 60
            })
        });
        let mut view = View::new(self.page, self.home_cursor, utc);
        view.now_ms = now;
        view.sample = self.live;
        view.active_secs = self.recorder.active_ms(now) / 1000;
        view.state = self.recorder.status();
        view.radar = self.radar.snapshot(now);
        view.stop_confirm = self
            .stop_armed
            .is_some_and(|at| now.saturating_sub(at) <= 5000);
        let gps = position.snapshot(now);
        view.gps_fix = gps.is_some_and(|p| p.gps.state == FixState::Fresh);
        let gps_status = gps.map_or("OFF", |p| {
            if p.gps.state == FixState::Fresh {
                "FIX OK"
            } else {
                "SEARCHING"
            }
        });
        match self.page {
            Page::Boot => {
                let _ = match (now / 1000) % 3 {
                    0 => write!(view.lines[0], "> COMPANION {}", self.startup_status),
                    1 => write!(
                        view.lines[0],
                        "> RADIO {:?} / GPS {}",
                        ant.availability(),
                        gps_status
                    ),
                    _ => write!(view.lines[0], "> STORAGE {}", self.recorder.status().name()),
                };
            }
            Page::Sensors | Page::Preflight => {
                let _ = view.lines[0].push_str(if self.page == Page::Preflight {
                    "> TRAIN / CONNECTION CHECK"
                } else {
                    "> CONNECTED SENSORS"
                });
                for (i, kind) in [123, 120, 11, 40].into_iter().enumerate() {
                    let channel = ant.channel(kind, now).or_else(|| {
                        if kind == 123 {
                            ant.channel(121, now)
                        } else {
                            None
                        }
                    });
                    let label = ["SPEED", "HEART", "POWER", "RADAR"][i];
                    if let Some(channel) = channel {
                        let id = channel.selected.map_or(0, |p| p.device_number);
                        let _ = write!(
                            view.lines[i + 2],
                            "{} {} {}",
                            label,
                            id,
                            if channel.link == device_api::ant::LinkState::Connected
                                && !channel.stale
                            {
                                "OK"
                            } else {
                                "WAIT"
                            }
                        );
                    } else {
                        let _ = write!(view.lines[i + 2], "{} --", label);
                    }
                }
                let _ = write!(view.lines[6], "GPS {}", gps_status);
                let _ = write!(view.lines[7], "STORAGE {}", self.recorder.status().name());
                let _ = write!(view.lines[8], "RADIO {:?}", ant.availability());
            }
            Page::Ride => {
                let _ = write!(view.lines[0], "> {}", self.recorder.status().name());
                if let Some(v) = self.live.speed_mm_s {
                    let k = v * 36 / 1000;
                    let _ = write!(view.lines[1], "{}.{} KM/H", k / 10, k % 10);
                } else {
                    let _ = view.lines[1].push_str("-- KM/H");
                }
                if let Some(v) = self.live.heart_bpm {
                    let _ = write!(view.lines[2], "{} BPM", v);
                } else {
                    let _ = view.lines[2].push_str("-- BPM");
                }
                if let Some(v) = self.live.power_watts {
                    let _ = write!(view.lines[3], "{} W", v);
                } else {
                    let _ = view.lines[3].push_str("-- W");
                }
                let seconds = self.recorder.active_ms(now) / 1000;
                let _ = write!(
                    view.lines[4],
                    "TIME {:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                );
                if let Some(v) = self.live.gradient_tenths {
                    let _ = write!(
                        view.lines[5],
                        "GRADE {}{}.{:01}%",
                        if v < 0 { "-" } else { "+" },
                        v.unsigned_abs() / 10,
                        v.unsigned_abs() % 10
                    );
                } else {
                    let _ = view.lines[5].push_str("GRADE --");
                }
                let _ = write!(
                    view.lines[6],
                    "POWER Z{}  HR Z{}",
                    self.live
                        .power_watts
                        .map_or(0, crate::ride::metrics::power_zone),
                    self.live
                        .heart_bpm
                        .map_or(0, crate::ride::metrics::heart_zone)
                );
                let _ = write!(
                    view.lines[7],
                    "GPS {} / SAVED {}",
                    gps_status,
                    self.recorder.written_samples()
                );
                if self
                    .stop_armed
                    .is_some_and(|at| now.saturating_sub(at) <= 5000)
                {
                    let _ = view.lines[8].push_str("PRESS STOP AGAIN TO SAVE");
                } else if let Some(result) = self.completion {
                    let _ = view.lines[8].push_str(if result.ok { "OK" } else { "WRITE FAILED" });
                } else if self.pending.is_some() {
                    let _ = view.lines[8].push_str("SAVING");
                } else {
                    let _ = view.lines[8].push_str(match self.recorder.status() {
                        crate::ride::log::Status::Recording => "RECORDING / 1 SECOND",
                        crate::ride::log::Status::Paused => "PAUSED / LEFT TO RESUME",
                        crate::ride::log::Status::Saved => "SAVED / TOP LEFT HOME",
                        _ => self.ui_message,
                    });
                }
                system.activity(now);
            }
            _ => {}
        }
        if self.page == Page::Scan {
            self.menu
                .refresh(ant.discoveries(), ant.channels(now), ant.scanning(), now);
        }
        view.prepare();
        system.draw_scaled(240, 320, |x, y| {
            if self.page == Page::Scan && y >= 25 {
                self.menu.pixel(x, y)
            } else {
                view.pixel(x, y)
            }
        });
    }

    pub fn alert(&mut self, sound: &mut impl device_api::sound::Sound, now: u64) {
        if self.page != crate::screens::workout::Page::Ride {
            return;
        }
        if self.radar_alert.update(self.radar.snapshot(now), now)
            && let Some(pattern) = sound
                .patterns()
                .iter()
                .filter(|p| p.nominal_ms <= 300)
                .max_by_key(|p| p.nominal_ms)
        {
            let _ = sound.play(pattern.id, now);
        }
    }

    pub fn radar(&self, now: u64) -> crate::sensors::radar::Snapshot {
        self.radar.snapshot(now)
    }

    pub fn present<
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    >(
        &mut self,
        system: &mut firmware_shell::shell::Shell<D, I, P, B>,
        now: u64,
        ant: &impl device_api::ant::Ant,
        position: &impl device_api::positioning::Positioning,
    ) {
        if system.foreground == firmware_shell::shell::Screen::Blank {
            return;
        }
        if !self.display_active {
            return;
        }
        if now < self.next_display {
            return;
        }
        self.next_display = now.saturating_add(
            if self.page == crate::screens::workout::Page::Boot
                || self.page == crate::screens::workout::Page::Ride
            {
                500
            } else {
                1000
            },
        );
        self.workout_display(system, now, ant, position);
    }

    pub fn set_startup_status(&mut self, status: &'static str) {
        if self.startup_status != status {
            self.startup_status = status;
            self.menu.set_message(match status {
                "charging" | "wake_pending" | "waiting" => b"STARTING SENSORS",
                "probing" | "pending" => b"WAITING FOR COMPANION",
                "unavailable" | "uncertain" | "timed_out" => b"STARTUP FAILED",
                _ => b"SCAN THEN PICK SENSOR",
            });
            self.next_display = 0;
        }
    }

    pub fn suspend_display(&mut self) {
        self.display_active = false;
    }
}
