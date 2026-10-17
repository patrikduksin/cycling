//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use super::recording::{Medium, Request, RideMessage};
use crate::ride::climb::Event as ClimbEvent;
use crate::ride::log::Status;
use crate::screens::chrome::CONTENT;
use crate::screens::chrome::{Gps, Toast, Tone};
use crate::screens::gesture::Gestures;
use crate::screens::home::{Home, last_ride};
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ride::{self, RidePage};
use crate::screens::sensors::MenuMessage;
use crate::screens::settings;
use crate::screens::workout::{LOW_STORAGE_S, Page, Text, View};
use crate::sensors::ant::device_type;
use core::fmt::Write;
use device_api::ant::Ant;
use device_api::display::Display as Panel;
use device_api::gps::FixState;
use device_api::input::InputSource;
use device_api::observation::Observation;
use device_api::positioning::Positioning;
use device_api::power::Power;
use device_api::sensors::Startup;
use device_api::sound::Sound;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::{Screen, Shell};

/// The brand stays this long after VANA's first frame, then Home opens. The
/// device keeps the backlight dark until VANA runs, so this is all a rider sees.
pub(super) const BRAND_MS: u64 = 400;
/// The connection check yields to the ride page after this long.
const PREFLIGHT_MS: u64 = 2500;
const ANIMATED_FRAME_MS: u64 = 500;
const STATIC_FRAME_MS: u64 = 1000;
/// Radar alerts use the longest pattern that still fits between radar pages.
const ALERT_PATTERN_MAX_MS: u16 = 300;
/// How long a confirmation or hint stays over the content.
pub(super) const TOAST_MS: u64 = 2000;
/// How long a ride without wheel speed notes that auto-pause is off.
const NO_SPEED_NOTE_MS: u64 = 8000;

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
    /// The page to return to when the climb that switched to the climb page
    /// ends; cleared when the rider changes page themselves.
    pub(super) climb_return: Option<RidePage>,
    pub(super) page_since: u64,
    pub(super) last_press: u64,
    /// When VANA drew its first frame; the brand flash counts from here.
    pub(super) first_frame: Option<u64>,
    /// Latest report of each button, to tell a held button from a new press.
    pub(super) last_report: [Option<u64>; 3],
    pub(super) gestures: Gestures,
    /// A confirmation or hint and when it expires.
    pub(super) toast: Option<(Toast, u64)>,
    /// The recorder state the last frame announced.
    announced: Status,
}

impl Navigation {
    /// Show `message` over the content for a moment, replacing any other.
    pub(super) fn toast(&mut self, message: &str, tone: Tone, now: u64) {
        self.toast = Some((Toast::new(message, tone), now.saturating_add(TOAST_MS)));
    }

    /// Announce a recorder state change, from any source, once.
    /// An automatic pause is named by its own toast for as long as it lasts.
    pub(super) fn announce(&mut self, status: Status, auto_paused: bool, now: u64) {
        let before = core::mem::replace(&mut self.announced, status);
        if before == status {
            return;
        }
        let (message, tone) = match (before, status) {
            (Status::Paused, Status::Recording) => ("RESUMED", Tone::Good),
            (_, Status::Recording) => ("RECORDING", Tone::Good),
            (_, Status::Paused) if auto_paused => return,
            (_, Status::Paused) => ("PAUSED", Tone::Warn),
            (Status::Recording | Status::Paused, Status::Saved) => ("RIDE SAVED", Tone::Good),
            (_, Status::Recovered) => ("RIDE RECOVERED", Tone::Info),
            _ => return,
        };
        self.toast(message, tone, now);
    }
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            page: Page::Boot,
            ride_page: RidePage::Main,
            home_cursor: 0,
            settings_field: 0,
            settings_editing: false,
            climb_return: None,
            page_since: 0,
            last_press: 0,
            first_frame: None,
            last_report: [None; 3],
            gestures: Gestures::default(),
            toast: None,
            announced: Status::Ready,
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
        // Once the device accepted turning off, the last frame stays.
        if system.foreground == Screen::Blank || self.turning_off() {
            return;
        }
        let Display::Active { next_frame_ms } = &mut self.display else {
            return;
        };
        if now < *next_frame_ms {
            return;
        }
        let animated = matches!(self.navigation.page, Page::Boot | Page::Ride)
            || matches!(self.power.view(), Some(PowerSheet::Warning { .. }));
        *next_frame_ms = now.saturating_add(if animated {
            ANIMATED_FRAME_MS
        } else {
            STATIC_FRAME_MS
        });
        self.workout_display(system, now, ant, position);
        // Open Home exactly when the brand flash ends, not on the next animation frame.
        if self.navigation.page == Page::Boot
            && let (Some(first), Display::Active { next_frame_ms }) =
                (self.navigation.first_frame, &mut self.display)
        {
            *next_frame_ms = (*next_frame_ms).min(first + BRAND_MS);
        }
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
            self.startup = status;
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

    /// When the frame just submitted reached the panel: the monotonic clock
    /// after submission, so slow rendering shows up in press-to-frame latency.
    fn drawn_at(&self, now: u64) -> u64 {
        self.ride.clock.map_or(now, |clock| clock()).max(now)
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
        if let Some(sheet) = self.power.view() {
            let canvas = sheet.canvas();
            system.draw_canvas(240, 320, |row| PowerSheet::paint(&canvas, row));
            self.power_drawn(!system.display_error);
            if !system.display_error {
                self.timing.frame(self.drawn_at(now));
            }
            return;
        }
        let navigation = &mut self.navigation;
        let first_frame = *navigation.first_frame.get_or_insert(now);
        if navigation.page == Page::Boot && now.saturating_sub(first_frame) >= BRAND_MS {
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
        view.active_secs = self.ride.active_ms(now) / 1000;
        view.state = self.ride.active().status();
        view.radar = self.readings.radar.snapshot(now);
        view.radar_paired = ant.channel(device_type::RADAR, now).is_some();
        view.auto_paused = self.ride.auto.paused();
        view.ride_page = navigation.ride_page;
        view.totals = self.ride.totals;
        view.lap = self.ride.laps.current();
        view.previous_lap = self.ride.laps.previous();
        if view.ride_page == RidePage::Lap {
            let laps = &self.ride.laps;
            let kept = (0..crate::ride::laps::HISTORY).rev();
            view.lap_bars = kept
                .filter_map(|back| laps.finished(back))
                .map(Into::into)
                .collect();
        }
        view.zone_time = self.ride.zone_time;
        view.profile = self.preferences.profile;
        if view.ride_page == RidePage::Climb {
            view.climbs = self.ride.climbs.clone();
        }
        let gps_fix = gps.is_some_and(|p| p.gps.state == FixState::Fresh);
        view.gps = match gps {
            None => Gps::Off,
            Some(_) if gps_fix => Gps::Fixed,
            Some(_) => Gps::Searching,
        };
        view.battery = match system.battery {
            Observation::Fresh { value, .. } | Observation::Stale { value, .. } => Some(value.0),
            Observation::Unavailable => None,
        };
        if page != Page::Boot {
            navigation.announce(view.state, self.ride.auto.paused(), now);
        }
        view.toast = self.toast(now);
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
                // Keep the screen lit while riding; a finished ride may dim and idle off.
                if self.recording() {
                    system.activity(now);
                }
            }
            Page::Home | Page::Sensors => {
                self.refresh_sensor_menu(ant, now);
                if page == Page::Home {
                    view.home = self.home(&view, gps_fix, now);
                }
            }
            Page::Settings => self.settings_lines(&mut view),
        }
        view.prepare();
        let menu = &self.menu;
        system.draw_canvas(240, 320, |row| {
            if page == Page::Sensors && row.y() >= CONTENT.top {
                menu.paint(row);
                view.paint_toast(row);
            } else {
                view.paint(row)
            }
        });
        // Only a frame the panel accepted is a milestone or answers a press;
        // a pending press waits for the next frame that reaches the panel.
        if system.display_error {
            return;
        }
        let drawn = self.drawn_at(now);
        self.timing.first_frame(drawn);
        if page == Page::Home {
            self.timing.home(drawn);
        }
        self.timing.frame(drawn);
    }

    /// Home says what is still starting, or why turning off did not happen;
    /// `true` when it is a problem.
    pub(super) fn home_status(&self, now: u64) -> (&'static str, bool) {
        if let Some(notice) = self.power.notice(now) {
            return (notice.text(), true);
        }
        let status = if self.ride.internal.status() == Status::Scanning
            || self.ride.bulk.status() == Status::Scanning
        {
            "CHECKING STORAGE"
        } else if matches!(
            self.startup,
            Startup::NotSubmitted
                | Startup::Probing
                | Startup::Pending
                | Startup::Waiting
                | Startup::WakePending
        ) {
            "STARTING SENSORS"
        } else if matches!(
            self.startup,
            Startup::Uncertain | Startup::TimedOut | Startup::Unavailable
        ) {
            return ("SENSORS UNAVAILABLE", true);
        } else {
            "READY TO RIDE"
        };
        (status, false)
    }

    fn home(&self, view: &View, gps_fix: bool, now: u64) -> Home {
        let totals = &view.totals;
        let ridden = !self.recording() && totals.distance_mm() > 0;
        let last = if ridden {
            last_ride(totals.distance_mm(), totals.moving_ms() / 1000)
        } else {
            Default::default()
        };
        let mut profile = Text::<12>::new();
        let _ = write!(profile, "FTP {} W", self.preferences.profile.ftp_watts);
        let mut home = Home::new(
            self.navigation.home_cursor,
            gps_fix,
            &self.menu.glance(),
            profile.as_bytes(),
            last,
        );
        let (status, problem) = self.home_status(now);
        home.set_status(status.as_bytes(), problem);
        home
    }

    /// What the toast band shows now: an armed end ride first, then a lap
    /// just finished, then an automatic start or resume, then the latest
    /// confirmation, then ride problems and low storage, which last until
    /// they are resolved, then what auto-pause is doing.
    pub(super) fn toast(&mut self, now: u64) -> Option<Toast> {
        if self.ride.stop_confirming(now) {
            return Some(Toast::new("PRESS AGAIN TO SAVE", Tone::Warn));
        }
        if let Some(lap) = self.ride.lap_toast(now) {
            return Some(Toast::new(ride::lap_toast(lap).as_str(), Tone::Info));
        }
        let auto = self.ride.toast(now);
        if !auto.is_empty() {
            return Some(Toast::new(auto, Tone::Info));
        }
        let navigation = &mut self.navigation;
        if navigation
            .toast
            .as_ref()
            .is_some_and(|(_, until)| now >= *until)
        {
            navigation.toast = None;
        }
        if let Some((toast, _)) = &navigation.toast {
            return Some(*toast);
        }
        if navigation.page != Page::Ride {
            return None;
        }
        let problem = match (self.ride.request, self.ride.active().status()) {
            (_, Status::Error) => "STORAGE ERROR",
            (_, Status::Full) => "STORAGE FULL",
            // A failed write always shows, except the bulk failure that moved
            // the ride to internal flash: the fallback toast names that one.
            (Request::Completed(result), _)
                if !result.ok && !(self.falling_back() && self.ride.request_on == Medium::Bulk) =>
            {
                "WRITE FAILED"
            }
            _ if matches!(self.ride.message, RideMessage::StorageNotReady) => "STORAGE NOT READY",
            (_, status) => {
                return self
                    .storage_toast(status)
                    .or_else(|| self.auto_pause_toast(status, now));
            }
        };
        Some(Toast::new(problem, Tone::Alert))
    }

    /// Recording time left, once it is under `LOW_STORAGE_S`; else rides
    /// falling back to internal flash because bulk media cannot take them;
    /// else a recovered ride.
    fn storage_toast(&self, status: Status) -> Option<Toast> {
        let label = match status {
            Status::Recording => "REC",
            Status::Paused => "PAUSED",
            Status::Ready | Status::Saved | Status::Recovered => "STORAGE",
            _ => return None,
        };
        // Room for the open ride, or for the next one wherever it will go.
        let next = self.ride.recorder(self.ride.medium());
        let left = next.status().usable().then(|| next.remaining_secs());
        if let Some(left) = left.filter(|left| *left < LOW_STORAGE_S) {
            let mut text = Text::<20>::new();
            let _ = write!(text, "{label} {}M LEFT", left / 60);
            return Some(Toast::new(text.as_str(), Tone::Warn));
        }
        if self.falling_back() {
            let text = match status {
                Status::Recording => "RECORDING INT",
                Status::Paused if self.ride.auto.paused() => "AUTO PAUSED INT",
                Status::Paused => "PAUSED INT",
                Status::Saved => "SAVED INT",
                Status::Recovered => "RECOVERED INT",
                _ => "READY INT",
            };
            return Some(Toast::new(text, Tone::Warn));
        }
        // Until the next start, the ride page names a ride kept after power loss.
        (status == Status::Recovered).then(|| Toast::new("RIDE RECOVERED", Tone::Info))
    }

    /// Rides go to internal flash because bulk media cannot take them, and
    /// the ride page shows an internal ride.
    fn falling_back(&self) -> bool {
        self.ride.fallback() && self.ride.shown() == Medium::Internal
    }

    /// A pause the device chose stays named until the ride resumes. Without
    /// wheel speed auto-pause cannot work, which the first seconds of the
    /// ride say once rather than for the whole ride.
    fn auto_pause_toast(&self, status: Status, now: u64) -> Option<Toast> {
        let text = match status {
            Status::Paused if self.ride.auto.paused() => "AUTO PAUSED",
            Status::Recording
                if self.readings.live.speed_mm_s.is_none()
                    && self.ride.active_ms(now) < NO_SPEED_NOTE_MS =>
            {
                "AUTO PAUSE OFF"
            }
            _ => return None,
        };
        Some(Toast::new(text, Tone::Info))
    }

    fn boot_lines(&self, view: &mut View, now: u64, ant: &impl Ant, gps_status: &str) {
        let line = &mut view.lines[0];
        let _ = match (now / 1000) % 3 {
            0 => write!(line, "COMPANION {}", self.startup_status),
            1 => write!(line, "RADIO {:?} / GPS {}", ant.availability(), gps_status),
            _ => write!(
                line,
                "STORAGE {} {}",
                self.ride.medium().label(),
                self.ride.active().status().name()
            ),
        };
    }

    pub(super) fn preflight_lines(
        &self,
        view: &mut View,
        now: u64,
        ant: &impl Ant,
        gps_status: &str,
    ) {
        let _ = view.lines[0].push_str("CONNECTION CHECK");
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
        // Name the medium only on boards that have bulk storage.
        let medium = match self.ride.bulk.status() {
            Status::Unsupported => "STORAGE",
            _ => self.ride.medium().label(),
        };
        // The connection check is about the ride about to start.
        let recorder = self.ride.recorder(self.ride.medium());
        let line = &mut view.lines[7];
        let _ = match recorder.status() {
            Status::Ready | Status::Saved | Status::Recovered => {
                let hours = recorder.remaining_secs() / 3600;
                if hours >= 100 {
                    write!(line, "{medium} {hours}H LEFT")
                } else {
                    let minutes = recorder.remaining_secs() / 60 % 60;
                    write!(line, "{medium} {hours}:{minutes:02} LEFT")
                }
            }
            status => write!(line, "{medium} {}", status.name()),
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
        let _ = lines[settings::TITLE].push_str("TRAINING PROFILE");
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
            Persistence::Locked => "NOT SAVED: DATA KEPT",
        });
    }
}
