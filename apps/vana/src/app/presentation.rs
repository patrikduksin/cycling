//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use super::recording::Request;
use crate::ride::climb::Event as ClimbEvent;
use crate::ride::log::Status;
use crate::screens::ride::RidePage;
use crate::screens::sensors::MenuMessage;
use crate::screens::settings;
use crate::screens::workout::{Page, View};
use crate::sensors::ant::device_type;
use core::fmt::Write;
use device_api::ant::Ant;
use device_api::display::Display as Panel;
use device_api::gps::FixState;
use device_api::input::InputSource;
use device_api::positioning::Positioning;
use device_api::power::Power;
use device_api::sensors::Startup;
use device_api::sound::Sound;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::{Screen, Shell};

/// The boot splash yields to Home after this long.
const BOOT_MS: u64 = 6500;
/// The connection check yields to the ride page after this long.
const PREFLIGHT_MS: u64 = 2500;
const ANIMATED_FRAME_MS: u64 = 500;
const STATIC_FRAME_MS: u64 = 1000;
/// Radar alerts use the longest pattern that still fits between radar pages.
const ALERT_PATTERN_MAX_MS: u16 = 300;

pub(super) struct Navigation {
    pub(super) page: Page,
    /// The ride page shown while `page` is the ride screen.
    pub(super) ride_page: RidePage,
    /// The highlighted Home choice, an index into `workout::HOME`.
    pub(super) home_cursor: usize,
    /// The selected Settings field, an index into `settings::FIELDS`.
    pub(super) settings_field: usize,
    /// The selected Settings field is being changed.
    pub(super) settings_editing: bool,
    /// A touch is down; taps act on contact, once, until release.
    pub(super) touching: bool,
    /// The page to return to when the climb that switched to the climb page
    /// ends; cleared when the rider changes page themselves.
    pub(super) climb_return: Option<RidePage>,
    pub(super) page_since: u64,
    pub(super) last_press: u64,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            page: Page::Boot,
            ride_page: RidePage::Main,
            home_cursor: 0,
            settings_field: 0,
            settings_editing: false,
            touching: false,
            climb_return: None,
            page_since: 0,
            last_press: 0,
        }
    }
}

impl Navigation {
    /// The ride screen follows climbs: a confirmed climb shows the climb page,
    /// and its end returns to the page the rider was on, unless they changed
    /// page during the climb (`ride_page_chosen`).
    pub(super) fn follow_climb(&mut self, event: ClimbEvent) {
        if self.page != Page::Ride {
            return;
        }
        match event {
            ClimbEvent::Confirmed if self.ride_page != RidePage::Climb => {
                self.climb_return = Some(self.ride_page);
                self.ride_page = RidePage::Climb;
            }
            ClimbEvent::Confirmed => {}
            ClimbEvent::Finished(_) => {
                if let Some(previous) = self.climb_return.take() {
                    self.ride_page = previous;
                }
            }
        }
    }

    /// The rider picked a ride page: a climb no longer takes them back.
    pub(super) fn ride_page_chosen(&mut self, page: RidePage) {
        self.ride_page = page;
        self.climb_return = None;
    }
}

/// Foreground drawing. Once the terminal takes over the panel it stays suspended.
#[derive(Clone, Copy)]
pub(super) enum Display {
    Active { next_frame_ms: u64 },
    Suspended,
}

impl Default for Display {
    fn default() -> Self {
        Self::Active { next_frame_ms: 0 }
    }
}

impl Display {
    pub(super) fn is_active(self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Draw at the next opportunity rather than waiting for the frame interval.
    pub(super) fn redraw(&mut self) {
        if let Self::Active { next_frame_ms } = self {
            *next_frame_ms = 0;
        }
    }
}

impl Runtime {
    pub fn present<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) {
        if system.foreground == Screen::Blank {
            return;
        }
        let Display::Active { next_frame_ms } = &mut self.display else {
            return;
        };
        if now < *next_frame_ms {
            return;
        }
        let animated = matches!(self.navigation.page, Page::Boot | Page::Ride);
        *next_frame_ms = now.saturating_add(if animated {
            ANIMATED_FRAME_MS
        } else {
            STATIC_FRAME_MS
        });
        self.workout_display(system, now, ant, position);
    }

    pub fn alert(&mut self, sound: &mut impl Sound, now: u64) {
        if self.navigation.page != Page::Ride {
            return;
        }
        let radar = self.readings.radar.snapshot(now);
        if self.readings.radar_alert.update(radar, now)
            && let Some(pattern) = sound
                .patterns()
                .iter()
                .filter(|p| p.nominal_ms <= ALERT_PATTERN_MAX_MS)
                .max_by_key(|p| p.nominal_ms)
        {
            let _ = sound.play(pattern.id, now);
        }
    }

    pub fn set_startup_status(&mut self, status: Startup) {
        if self.startup_status != status.name() {
            self.startup_status = status.name();
            self.menu.set_message(match status {
                Startup::Charging | Startup::WakePending | Startup::Waiting => {
                    MenuMessage::StartingSensors
                }
                Startup::Probing | Startup::Pending => MenuMessage::WaitingForCompanion,
                Startup::Unavailable | Startup::Uncertain | Startup::TimedOut => {
                    MenuMessage::StartupFailed
                }
                _ => MenuMessage::ScanThenPick,
            });
            self.display.redraw();
        }
    }

    pub fn suspend_display(&mut self) {
        self.display = Display::Suspended;
    }

    fn workout_display<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) {
        let navigation = &mut self.navigation;
        if navigation.page == Page::Boot && now >= BOOT_MS {
            navigation.page = Page::Home;
        }
        if navigation.page == Page::Preflight
            && now.saturating_sub(navigation.page_since) >= PREFLIGHT_MS
        {
            navigation.page = Page::Ride;
            navigation.ride_page_chosen(RidePage::Main);
        }
        let page = navigation.page;
        let gps = position.snapshot(now);
        let utc = self.readings.live.utc_ms.map(|v| v / 1000).or_else(|| {
            gps.and_then(|p| p.gps.utc).map(|v| {
                u64::from((v[0] - b'0') * 10 + v[1] - b'0') * 3600
                    + u64::from((v[2] - b'0') * 10 + v[3] - b'0') * 60
            })
        });
        let cursor = if page == Page::Settings {
            navigation.settings_field
        } else {
            navigation.home_cursor
        };
        let mut view = View::new(page, cursor, utc);
        view.editing = page == Page::Settings && navigation.settings_editing;
        view.now_ms = now;
        view.sample = self.readings.live;
        view.active_secs = self.ride.recorder.active_ms(now) / 1000;
        view.state = self.ride.recorder.status();
        view.storage_left_s = matches!(
            view.state,
            Status::Ready | Status::Saved | Status::Recovered | Status::Recording | Status::Paused
        )
        .then(|| self.ride.recorder.remaining_secs());
        view.radar = self.readings.radar.snapshot(now);
        view.stop_confirm = self.ride.stop_confirming(now);
        view.ride_page = navigation.ride_page;
        view.totals = self.ride.totals;
        view.lap = self.ride.laps.current();
        view.previous_lap = self.ride.laps.previous();
        view.lap_toast = self.ride.lap_toast(now);
        view.zone_time = self.ride.zone_time;
        view.profile = self.preferences.profile;
        if view.ride_page == RidePage::Climb {
            view.climbs = self.ride.climbs.clone();
        }
        view.gps_fix = gps.is_some_and(|p| p.gps.state == FixState::Fresh);
        let gps_status = gps.map_or("OFF", |p| {
            if p.gps.state == FixState::Fresh {
                "FIX OK"
            } else {
                "SEARCHING"
            }
        });
        match page {
            Page::Boot => self.boot_lines(&mut view, now, ant, gps_status),
            Page::Preflight => self.preflight_lines(&mut view, now, ant, gps_status),
            Page::Ride => {
                self.ride_lines(&mut view, now, gps_status);
                system.activity(now);
            }
            Page::Home => {}
            Page::Settings => self.settings_lines(&mut view),
            Page::Sensors => self.refresh_sensor_menu(ant, now),
        }
        view.prepare();
        let menu = &self.menu;
        system.draw_canvas(240, 320, |row| {
            if page == Page::Sensors && row.y() >= 25 {
                menu.paint(row)
            } else {
                view.paint(row)
            }
        });
    }

    fn boot_lines(&self, view: &mut View, now: u64, ant: &impl Ant, gps_status: &str) {
        let line = &mut view.lines[0];
        let _ = match (now / 1000) % 3 {
            0 => write!(line, "> COMPANION {}", self.startup_status),
            1 => write!(
                line,
                "> RADIO {:?} / GPS {}",
                ant.availability(),
                gps_status
            ),
            _ => write!(line, "> STORAGE {}", self.ride.recorder.status().name()),
        };
    }

    fn preflight_lines(&self, view: &mut View, now: u64, ant: &impl Ant, gps_status: &str) {
        let _ = view.lines[0].push_str("> TRAIN / CONNECTION CHECK");
        let checks = [
            ("SPEED", device_type::SPEED),
            ("HEART", device_type::HEART_RATE),
            ("POWER", device_type::POWER),
            ("RADAR", device_type::RADAR),
        ];
        let tiles = self.sensor_tiles(ant, now);
        let saved = |kind: u8| {
            tiles
                .iter()
                .flatten()
                .find(|tile| tile.identity.device_type == kind)
        };
        for (line, (label, kind)) in view.lines[2..].iter_mut().zip(checks) {
            // A combined speed/cadence sensor also provides speed.
            let tile = saved(kind).or_else(|| {
                (kind == device_type::SPEED)
                    .then(|| saved(device_type::SPEED_CADENCE))
                    .flatten()
            });
            let _ = match tile {
                Some(tile) => write!(
                    line,
                    "{label} {} {}",
                    tile.identity.device_number,
                    tile.connection.text()
                ),
                None => write!(line, "{label} --"),
            };
        }
        let _ = write!(view.lines[6], "GPS {}", gps_status);
        let recorder = &self.ride.recorder;
        let _ = match recorder.status() {
            Status::Ready | Status::Saved | Status::Recovered => {
                let minutes = recorder.remaining_secs() / 60;
                write!(
                    view.lines[7],
                    "STORAGE {}:{:02} LEFT",
                    minutes / 60,
                    minutes % 60
                )
            }
            status => write!(view.lines[7], "STORAGE {}", status.name()),
        };
        let _ = match self.discovery.scan.overview_message() {
            Some(message) => view.lines[8].push_str(message.text()),
            None => write!(view.lines[8], "RADIO {:?}", ant.availability()),
        };
    }

    fn settings_lines(&self, view: &mut View) {
        use super::preferences::Persistence;
        let preferences = &self.preferences;
        let profile = preferences.profile;
        let lines = &mut view.lines;
        let _ = lines[settings::TITLE].push_str("> SETTINGS / TRAINING PROFILE");
        let _ = write!(lines[settings::VALUES], "{}", profile.ftp_watts);
        let _ = write!(lines[settings::VALUES + 1], "{}", profile.lthr_bpm);
        // Threshold zones: where FTP and LTHR put the rider's limit.
        let (low, high) = profile.power_range(4);
        let _ = write!(lines[settings::ANCHORS], "Z4 {}-{} W", low, high - 1);
        let (low, _) = profile.heart_range(5);
        let _ = write!(lines[settings::ANCHORS + 1], "Z5 FROM {} BPM", low);
        let _ = lines[settings::STATUS].push_str(match preferences.persistence() {
            Persistence::Saved => "SAVED",
            Persistence::Pending => "SAVING",
            Persistence::Failed => "SAVE FAILED, RETRYING",
            Persistence::Locked => "NOT SAVED - UNREADABLE DATA KEPT",
        });
    }

    fn ride_lines(&self, view: &mut View, now: u64, gps_status: &str) {
        let live = self.readings.live;
        let recorder = &self.ride.recorder;
        let lines = &mut view.lines;
        let _ = write!(lines[0], "> {}", recorder.status().name());
        let _ = match live.speed_mm_s {
            Some(v) => {
                let k = v * 36 / 1000;
                write!(lines[1], "{}.{} KM/H", k / 10, k % 10)
            }
            None => lines[1].push_str("-- KM/H"),
        };
        let _ = match live.heart_bpm {
            Some(v) => write!(lines[2], "{} BPM", v),
            None => lines[2].push_str("-- BPM"),
        };
        let _ = match live.power_watts {
            Some(v) => write!(lines[3], "{} W", v),
            None => lines[3].push_str("-- W"),
        };
        let seconds = recorder.active_ms(now) / 1000;
        let _ = write!(
            lines[4],
            "TIME {:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        );
        let _ = match live.gradient_tenths {
            Some(v) => write!(
                lines[5],
                "GRADE {}{}.{:01}%",
                if v < 0 { "-" } else { "+" },
                v.unsigned_abs() / 10,
                v.unsigned_abs() % 10
            ),
            None => lines[5].push_str("GRADE --"),
        };
        let profile = self.preferences.profile;
        let _ = write!(
            lines[6],
            "POWER Z{}  HR Z{}",
            live.power_watts.map_or(0, |w| profile.power_zone(w)),
            live.heart_bpm.map_or(0, |b| profile.heart_zone(b))
        );
        let _ = write!(
            lines[7],
            "GPS {} / SAVED {}",
            gps_status,
            recorder.written_samples()
        );
        let status = if self.ride.stop_confirming(now) {
            "PRESS AGAIN TO SAVE"
        } else {
            match self.ride.request {
                Request::Completed(result) if result.ok => "OK",
                Request::Completed(_) => "WRITE FAILED",
                Request::Pending(_) => "SAVING",
                Request::Idle => match recorder.status() {
                    Status::Recording => "RECORDING / TOP LEFT LAPS",
                    Status::Paused => "PAUSED / TOP LEFT ENDS",
                    Status::Saved => "SAVED / TOP LEFT HOME",
                    _ => self.ride.message.text(),
                },
            }
        };
        let _ = lines[8].push_str(status);
    }
}
