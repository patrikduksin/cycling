//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use super::cues::Cue;
use super::night::NIGHT_PERCENT;
use super::recording::{Medium, Request, RideMessage};
use crate::clock::Zone;
use crate::daylight::Sun;
use crate::ride::alerts::{Alert, Cue as AlertSound};
use crate::ride::climb::Event as ClimbEvent;
use crate::ride::log::Status;
use crate::screens::chrome::CONTENT;
use crate::screens::chrome::{Battery, CHARGE_STEPS, Gps, Toast, Tone};
use crate::screens::gesture::Gestures;
use crate::screens::home::Home;
use crate::screens::look;
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ready::{Check, Level, Line as ReadyLine, Readiness};
use crate::screens::ride::summary::{After, Saving, SummaryPage};
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
const ANIMATED_FRAME_MS: u64 = 500;
/// Motion (the brand reveal, the farewell) draws at this cadence: a frame
/// takes about 25 ms on the C606, so this leaves the loop free between them.
pub(super) const MOTION_FRAME_MS: u64 = 40;
const STATIC_FRAME_MS: u64 = 1000;
/// How long a confirmation or hint stays over the content.
pub(super) const TOAST_MS: u64 = 2000;
/// Battery news stays a little longer than a confirmation.
pub(super) const BATTERY_TOAST_MS: u64 = 4000;
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
    /// The focused card on a first-run setup page.
    pub(super) setup_cursor: usize,
    /// The Settings editor was opened from setup and returns there.
    pub(super) setup_return: bool,
    /// The sensor menu was opened from setup and returns there.
    pub(super) sensors_return: bool,
    /// Connect phone shows the join code (0) or the page code (1).
    pub(super) phone_step: u8,
    /// The screen already moved to the page code when a phone joined.
    pub(super) phone_advanced: bool,
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
    /// When the ride was last paused, for the paused screen's timer.
    pub(super) paused_since: Option<u64>,
    /// The rider paged from the paused screen to the ride pages.
    pub(super) pause_hidden: bool,
    /// The ride page a ride opens on.
    pub(super) first_page: RidePage,
    /// The paused screen came back by paging past either end of the ride
    /// pages, so MOVE continues at the first page and a swipe back at the
    /// last, rather than at the page the rider left.
    pub(super) pause_wrapped: bool,
    /// The summary page shown after a ride.
    pub(super) summary_page: usize,
    /// A sound the next `alert` plays.
    pub(super) chime: Option<Cue>,
    /// The personal bests the ride set, announced when its summary opens.
    pub(super) best_news: Option<Text<24>>,
    /// The bests toast on screen, so leaving the summary can take it away.
    best_shown: Option<Text<24>>,
}

impl Navigation {
    /// Show `message` over the content for a moment, replacing any other.
    pub(super) fn toast(&mut self, message: &str, tone: Tone, now: u64) {
        self.toast = Some((Toast::new(message, tone), now.saturating_add(TOAST_MS)));
    }

    /// Show `toast` for `ms`, replacing any other.
    pub(super) fn show(&mut self, toast: Toast, ms: u64, now: u64) {
        self.toast = Some((toast, now.saturating_add(ms)));
    }

    pub(super) fn announce(&mut self, status: Status, now: u64) {
        let before = core::mem::replace(&mut self.announced, status);
        if before == status {
            return;
        }
        let riding = matches!(before, Status::Recording | Status::Paused);
        let (message, tone) = match (before, status) {
            (Status::Paused, Status::Recording) => {
                self.paused_since = None;
                self.chime = Some(Cue::Resume);
                ("RESUMED", Tone::Good)
            }
            (_, Status::Recording) => {
                self.paused_since = None;
                self.best_news = None;
                self.chime = Some(Cue::Start);
                ("RECORDING", Tone::Good)
            }
            // The paused screen says it, whoever paused.
            (_, Status::Paused) => {
                self.paused_since = Some(now);
                self.pause_hidden = false;
                self.pause_wrapped = false;
                self.chime = Some(Cue::Pause);
                return;
            }
            (_, Status::Saved) if riding && self.page == Page::Summary => {
                // New bests take the saved chime's place, with a toast.
                let news = self.best_news.take();
                self.chime = Some(if news.is_some() {
                    Cue::Best
                } else {
                    Cue::Saved
                });
                if let Some(news) = news {
                    self.toast(news.as_str(), Tone::Good, now);
                    self.best_shown = Some(news);
                }
                return;
            }
            (_, Status::Saved) if riding => ("RIDE SAVED", Tone::Good),
            (_, Status::Recovered) => ("RIDE RECOVERED", Tone::Info),
            _ => return,
        };
        self.toast(message, tone, now);
    }

    /// The toast of new bests belongs to the summary: leaving it takes that
    /// toast away, and only that one.
    pub(super) fn leave_summary(&mut self) {
        let Some(shown) = self.best_shown.take() else {
            return;
        };
        if self
            .toast
            .as_ref()
            .is_some_and(|(toast, _)| toast.text() == Toast::new(shown.as_str(), Tone::Good).text())
        {
            self.toast = None;
        }
    }

    /// Show the ride screen's first page.
    pub(super) fn open_ride(&mut self) {
        self.page = Page::Ride;
        self.ride_page_chosen(self.first_page);
        self.pause_hidden = false;
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
            setup_cursor: 0,
            setup_return: false,
            sensors_return: false,
            phone_step: 0,
            phone_advanced: false,
            climb_return: None,
            page_since: 0,
            last_press: 0,
            first_frame: None,
            last_report: [None; 3],
            gestures: Gestures::default(),
            toast: None,
            announced: Status::Ready,
            paused_since: None,
            pause_hidden: false,
            first_page: RidePage::Main,
            pause_wrapped: false,
            summary_page: 0,
            chime: None,
            best_news: None,
            best_shown: None,
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
    /// Draw the brand's first frame in the rider's look, before the runtime
    /// presents. Not inlined: the view is about 10 KB, and in the caller's
    /// poll frame it would sit under every later frame's preparation (#167).
    #[inline(never)]
    pub fn draw_boot<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &self,
        system: &mut Shell<D, I, P, B>,
    ) {
        let view = View::new(Page::Boot, 0, None);
        let light = self.light();
        system.draw_canvas(240, 320, |row| {
            view.paint(row);
            if light {
                look::apply(row.pixels_mut());
            }
        });
    }

    pub fn present<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) {
        self.follow_daylight(system, now, position);
        // Once the device accepted turning off, the last frame stays.
        if system.foreground == Screen::Blank || self.turning_off() {
            return;
        }
        let visible = self.motion_visible();
        let Display::Active { next_frame_ms } = &mut self.display else {
            return;
        };
        // A new sample starts its glide on the next motion frame rather than
        // waiting for the page's usual pace.
        if let Some(due) = self.motion.due(
            visible,
            &shown_sample(&self.readings),
            &radar_reports(&self.readings, now),
            MOTION_FRAME_MS,
        ) {
            *next_frame_ms = (*next_frame_ms).min(due);
        }
        if now < *next_frame_ms {
            return;
        }
        let sheet = self.power.view();
        // A reveal frame that could still be drawing when the brand time
        // ends would make Home late; skip it and draw Home on time.
        if self.navigation.page == Page::Boot
            && sheet.is_none()
            && let Some(first) = self.navigation.first_frame
        {
            let home_at = first + BRAND_MS;
            if now < home_at && now + MOTION_FRAME_MS > home_at {
                *next_frame_ms = home_at;
                return;
            }
        }
        let motion = matches!(sheet, Some(PowerSheet::TurningOff { .. }))
            || (sheet.is_none() && self.navigation.page == Page::Boot);
        let animated = matches!(
            self.navigation.page,
            Page::Ride | Page::Ready | Page::EndRide | Page::Summary
        ) || sheet.is_some_and(PowerSheet::animated)
            || self.readings.battery.charging();
        *next_frame_ms = now.saturating_add(if motion {
            MOTION_FRAME_MS
        } else if animated {
            ANIMATED_FRAME_MS
        } else {
            STATIC_FRAME_MS
        });
        self.refresh_chart();
        self.workout_display(system, now, ant, position);
        let visible = self.motion_visible();
        if self.navigation.page == Page::Ride
            && let Display::Active { next_frame_ms } = &mut self.display
        {
            // Readings gliding to their latest sample draw as motion does.
            if self.motion.moving(visible, now) {
                *next_frame_ms = (*next_frame_ms).min(now + MOTION_FRAME_MS);
            }
            // Motion frames shift the frame phase, so the ride clock asks for
            // a frame when its next second turns rather than up to a frame late.
            let active_ms = self.ride.active_ms(now);
            if self.ride.active_ms(now + 1000) > active_ms {
                *next_frame_ms = (*next_frame_ms).min(now + 1000 - active_ms % 1000);
            }
        }
        // Open Home exactly when the brand flash ends, not on the next animation frame.
        if self.navigation.page == Page::Boot
            && let (Some(first), Display::Active { next_frame_ms }) =
                (self.navigation.first_frame, &mut self.display)
        {
            *next_frame_ms = (*next_frame_ms).min(first + BRAND_MS);
        }
    }

    /// Sound the cues and alarms due: the latest ride moment's cue, a closed
    /// lap, a vehicle closing from behind while on the ride screen, which
    /// wins over any cue, and the rider's alerts (`ride::alerts`), each with
    /// a toast on the ride pages. A cue waits for the buzzer to be free, for
    /// up to a second. Call once per loop.
    pub fn alert(&mut self, sound: &mut impl Sound, now: u64) {
        if core::mem::take(&mut self.ride.lap_chime) {
            self.cue(Cue::Lap, now);
        }
        if let Some(cue) = self.navigation.chime.take() {
            self.cue(cue, now);
        }
        // A workout's step cue says more than the lap or ride chime it echoes.
        self.workout_cue();
        let radar = self.readings.radar.snapshot(now);
        let settings = self.alert_settings();
        let alert = self.alerts.update(
            now,
            &settings,
            self.ride.active().status(),
            self.ride.active_ms(now),
            &self.readings.live,
            radar,
        );
        let sunset = self.alerts.sunset(
            now,
            self.preferences.options.sunset_alert,
            self.ride.active().status(),
            self.navigation.page == Page::Ride,
            self.night.sun(now),
        );
        if self.navigation.page == Page::Ride {
            if self.readings.radar_alert.update(radar, now) {
                self.cue(Cue::RadarApproach, now);
            }
            if let Some(alert) = alert.or(sunset) {
                self.ride_alert(alert, now);
            }
        }
        self.cues.play(sound, now);
    }

    /// Show `alert` as a toast and raise its cue.
    fn ride_alert(&mut self, alert: Alert, now: u64) {
        let tone = match alert {
            Alert::Fuel => Tone::Info,
            Alert::RadarClear => Tone::Good,
            Alert::SunsetSoon | Alert::SunsetNow => Tone::Warn,
            _ => Tone::Warn,
        };
        self.navigation
            .show(Toast::new(alert.text().as_str(), tone), TOAST_MS, now);
        let cue = match alert.cue() {
            Some(AlertSound::Band) => Cue::BandAlert,
            Some(AlertSound::Fuel) => Cue::Fuel,
            Some(AlertSound::RadarClear) => Cue::RadarClear,
            Some(AlertSound::Sunset) => Cue::BandAlert,
            None => return,
        };
        self.cue(cue, now);
    }

    /// Raise `cue`, unless the rider muted cues and it is no safety alarm.
    pub(super) fn cue(&mut self, cue: Cue, now: u64) {
        self.cues.raise(cue, !self.preferences.options.sounds, now);
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
        self.power.animate(now);
        if let Some(sheet) = self.power.view() {
            let canvas = sheet.canvas();
            let light = self.light();
            system.draw_canvas(240, 320, |row| {
                PowerSheet::paint(&canvas, row);
                if light {
                    look::apply(row.pixels_mut());
                }
            });
            self.power_drawn(!system.display_error);
            if !system.display_error {
                self.timing.frame(self.drawn_at(now));
            }
            return;
        }
        let first_page = self.first_page();
        let navigation = &mut self.navigation;
        // A return to setup lasts only while its editor or sensor menu does.
        if !(navigation.page == Page::Settings && navigation.settings_editing) {
            navigation.setup_return = false;
        }
        if navigation.page != Page::Sensors {
            navigation.sensors_return = false;
        }
        let first_frame = *navigation.first_frame.get_or_insert(now);
        if navigation.page == Page::Boot && now.saturating_sub(first_frame) >= BRAND_MS {
            navigation.page = first_page;
        }
        // A ride opens on its first page: the workout page during a workout.
        let first = self.first_ride_page();
        let navigation = &mut self.navigation;
        navigation.first_page = first;
        let status = self.ride.active().status();
        if navigation.page != Page::Boot {
            navigation.announce(status, now);
        }
        // Over the ride's own news: its workout could not be read.
        if core::mem::take(&mut self.training.missing) {
            self.navigation
                .toast("WORKOUT MISSING", crate::screens::chrome::Tone::Warn, now);
        }
        if status == Status::Recording {
            self.history_ride_started();
        }
        let pages = self.workout_pages();
        let shown = match self.navigation.page {
            Page::Ride => self.shown_workout(&self.preferences.profile),
            _ => None,
        };
        let ftp_offer = self.ftp_sheet();
        let picker = (self.navigation.page == Page::Workouts).then(|| self.picker());
        let navigation = &mut self.navigation;
        // The workout page goes with the workout.
        if navigation.ride_page == RidePage::Workout && !pages {
            navigation.ride_page_chosen(RidePage::Main);
        }
        let page = navigation.page;
        let gps = position.snapshot(now);
        let zone = self.preferences.options.zone;
        let local = match self.readings.live.utc_ms {
            Some(ms) => Some(zone.local(ms / 1000)),
            None => gps.and_then(|p| p.gps.utc).map(|v| {
                zone.local_of_day(
                    u64::from((v[0] - b'0') * 10 + v[1] - b'0') * 3600
                        + u64::from((v[2] - b'0') * 10 + v[3] - b'0') * 60,
                )
            }),
        };
        let cursor = match page {
            Page::Settings => navigation.settings_field,
            Page::Setup => navigation.setup_cursor,
            _ => navigation.home_cursor,
        };
        let mut view = View::new(page, cursor, local);
        view.editing = page == Page::Settings && navigation.settings_editing;
        view.now_ms = now;
        view.active_secs = self.ride.active_ms(now) / 1000;
        view.state = self.ride.active().status();
        let mut radar = radar_reports(&self.readings, now);
        let mut sample = shown_sample(&self.readings);
        self.motion.show(&mut sample, &mut radar, now);
        view.power_3s = sample.power_watts;
        view.sample = crate::ride::log::Sample {
            power_watts: self.readings.live.power_watts,
            ..sample
        };
        view.radar = radar.snapshot;
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
        if page == Page::Ride {
            view.training = shown;
            if view.ride_page == RidePage::Workout
                && let Some(player) = &self.ride.workout
            {
                view.chart = crate::screens::ride::workout_chart(player, &view.profile);
            }
        }
        // The sheet shows wherever it takes the buttons, as after a test
        // started from the terminal while Home is open.
        view.ftp_offer = ftp_offer;
        if let Some(picker) = picker {
            view.picker = picker;
        }
        if view.ride_page == RidePage::Climb {
            view.climbs = self.ride.climbs.clone();
        }
        if view.ride_page == RidePage::Back {
            view.back = super::back::view_of(&self.back);
            view.sunset = light(
                self.night.sun(now).map(|(sun, _)| sun),
                self.preferences.options.zone,
            );
        }
        let gps_fix = gps.is_some_and(|p| p.gps.state == FixState::Fresh);
        view.gps = match gps {
            None => Gps::Off,
            Some(_) if gps_fix => Gps::Fixed,
            Some(_) => Gps::Searching,
        };
        view.battery = match system.battery {
            Observation::Fresh { value, .. } | Observation::Stale { value, .. } => Some(Battery {
                percent: value.0,
                charging: self.readings.battery.charging(),
                phase: (now / ANIMATED_FRAME_MS % u64::from(CHARGE_STEPS)) as u8,
            }),
            Observation::Unavailable => None,
        };
        view.paused_secs = navigation
            .paused_since
            .map_or(0, |since| now.saturating_sub(since) / 1000);
        view.pause_screen =
            page == Page::Ride && view.state == Status::Paused && !navigation.pause_hidden;
        view.toast = self.toast(now);
        match page {
            Page::Boot => {
                let shown = now.saturating_sub(first_frame).min(BRAND_MS);
                view.reveal = (shown * u64::from(crate::screens::brand::DONE) / BRAND_MS) as u16;
            }
            Page::Ready => {
                self.refresh_sensor_menu(ant, now);
                view.readiness = self.readiness(view.gps, view.battery);
            }
            Page::Summary => {
                view.after = self.after();
                self.show_elevation(&mut view);
            }
            Page::Bests => {
                view.bests = self.bests.bests;
                view.bests_mode = self.bests.mode();
            }
            Page::PastRide => self.past_view(&mut view),
            Page::Phone => {
                view.phone_ride = self.history.phone.is_some();
                // The codes stay readable while a phone connects.
                system.activity(now);
                self.phone_view(&mut view);
            }
            Page::History => view.set_canvas(self.history_canvas()),
            Page::EndRide => {}
            Page::Ride => {
                // Keep the screen lit while riding; a finished ride may dim and idle off.
                if self.recording() {
                    system.activity(now);
                }
            }
            Page::Home | Page::Sensors => {
                self.refresh_sensor_menu(ant, now);
                if page == Page::Home {
                    view.home = self.home(gps_fix, now);
                }
            }
            Page::Settings => self.settings_lines(&mut view),
            Page::Workouts => {}
            Page::Setup => self.setup_lines(&mut view),
        }
        if view.ride_page == RidePage::Climb {
            // Borrowed from the look-ahead's own block, not copied.
            view.forecast = super::lookahead::forecast_of(&self.lookahead);
        }
        let light = self.light();
        view.light = light;
        if view.ride_page == RidePage::Map {
            // Borrowed from the breadcrumb, not copied.
            view.track = super::back::track_of(&self.back);
        }
        view.prepare();
        let menu = &self.menu;
        // The FTP sheet covers the Sensors menu too, as it takes the buttons.
        let sheet = view.ftp_offer;
        system.draw_canvas(240, 320, |row| {
            if page == Page::Sensors && sheet.is_none() && row.y() >= CONTENT.top {
                menu.paint(row);
                view.paint_toast(row);
            } else {
                view.paint(row)
            }
            if light {
                look::apply(row.pixels_mut());
            }
        });
        // Only a frame the panel accepted is a milestone or answers a press;
        // a pending press waits for the next frame that reaches the panel.
        if system.display_error {
            return;
        }
        let drawn = self.drawn_at(now);
        self.timing.first_frame(drawn);
        // Setup on a first start is as ready as Home.
        if matches!(page, Page::Home | Page::Setup) {
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

    fn home(&self, gps_fix: bool, now: u64) -> Home {
        let last = self.rides_line();
        let mut home = Home::new(
            self.navigation.home_cursor,
            gps_fix,
            &self.menu.glance(),
            last,
        );
        let (status, problem) = self.home_status(now);
        match self.chosen_name() {
            // A workout waiting to be ridden names itself on the ride card.
            Some(name) if !problem && status == "READY TO RIDE" => {
                home.set_status(name.as_bytes(), false)
            }
            _ => home.set_status(status.as_bytes(), problem),
        }
        home
    }

    /// What the toast band shows now: a lap just finished first, then an automatic start or resume, then the latest
    /// confirmation, then ride problems and low storage, which last until
    /// they are resolved, then what auto-pause is doing.
    pub(super) fn toast(&mut self, now: u64) -> Option<Toast> {
        if let Some(lap) = self.ride.lap_toast(now) {
            return Some(Toast::new(ride::lap_toast(lap).as_str(), Tone::Info));
        }
        let auto = self.ride.auto_toast(now);
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

    /// A pause the device chose stays named on the ride pages until the ride
    /// resumes; the paused screen names it itself. Without wheel or GPS speed
    /// auto-pause cannot work, which the first seconds of the ride say once
    /// rather than for the whole ride.
    fn auto_pause_toast(&self, status: Status, now: u64) -> Option<Toast> {
        let text = match status {
            Status::Paused if self.ride.auto.paused() && self.navigation.pause_hidden => {
                "AUTO PAUSED"
            }
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

    /// The elevation chart, lent to the view while the summary page that shows
    /// it is open.
    #[inline(never)]
    pub(super) fn show_elevation<'a>(&'a self, view: &mut View<'a>) {
        if crate::screens::ride::summary::shows_elevation(view) {
            view.elevation_chart = Some(&self.ride.chart);
        }
    }

    /// The summary has an elevation page: the ride, live or read back, charted
    /// altitude.
    pub(super) fn has_elevation(&self) -> bool {
        self.ride.trace.len() >= 2
    }

    /// Chart the ride's trace again when it changed, as the summary page that
    /// shows it opens.
    fn refresh_chart(&mut self) {
        let showing = matches!(self.navigation.page, Page::Summary | Page::PastRide)
            && SummaryPage::at(self.navigation.summary_page, self.has_elevation())
                == SummaryPage::Elevation;
        if showing && self.ride.trace.take_changed() {
            self.ride.chart.refresh(&self.ride.trace);
        }
    }

    /// What the summary shows beyond the totals.
    fn after(&self) -> After {
        let status = self.ride.active().status();
        let saving = match status {
            _ if self.ride.pending() => Saving::Writing,
            Status::Recording | Status::Paused => Saving::Writing,
            Status::Error => Saving::Failed,
            _ => Saving::Saved,
        };
        After {
            page: self.navigation.summary_page,
            saving,
            best: self.ride.efforts.best(),
            climbs: Some(self.ride.climbs_done),
            // The open lap counts: a ride without a lap press is one lap.
            laps: Some(self.ride.laps.count().saturating_add(1)),
            past: false,
            elevation: self.has_elevation(),
            found: self.ride.bests_found,
            offer: self.summary_offer(),
        }
    }

    /// The Ready checks: GPS, sensors, battery and room to record, each in
    /// plain words with what to do when something needs the rider.
    pub(super) fn readiness(&self, gps: Gps, battery: Option<Battery>) -> Readiness {
        let mut readiness = Readiness::default();
        if let Some(name) = self.chosen_name() {
            let _ = readiness.riding.push_str(name.as_str());
        }
        let checks = &mut readiness.checks;
        checks[ReadyLine::Gps as usize] = match gps {
            Gps::Fixed => Check::new(Level::Good, "GPS LOCKED", ""),
            Gps::Searching => Check::new(Level::Wait, "FINDING GPS", "YOU CAN START ANYWAY"),
            Gps::Off => Check::new(Level::Off, "NO GPS", "RECORDS WITHOUT A MAP"),
        };
        checks[ReadyLine::Sensors as usize] = self.sensors_check();
        checks[ReadyLine::Battery as usize] = match battery {
            None => Check::new(Level::Off, "BATTERY", ""),
            Some(Battery {
                percent,
                charging: true,
                ..
            }) => {
                let mut title = Text::<24>::new();
                let _ = write!(title, "CHARGING {percent}%");
                Check::new(Level::Good, title.as_str(), "")
            }
            Some(Battery { percent, .. }) => {
                let mut title = Text::<24>::new();
                let _ = write!(title, "BATTERY {percent}%");
                let (level, hint) = match percent {
                    30.. => (Level::Good, ""),
                    15..30 => (Level::Wait, "CHARGE AFTER THE RIDE"),
                    _ => (Level::Problem, "CHARGE BEFORE RIDING"),
                };
                Check::new(level, title.as_str(), hint)
            }
        };
        checks[ReadyLine::Storage as usize] = self.storage_check();
        readiness
    }

    fn sensors_check(&self) -> Check {
        if matches!(
            self.startup,
            Startup::Uncertain | Startup::TimedOut | Startup::Unavailable
        ) {
            return Check::new(Level::Wait, "SENSORS UNAVAILABLE", "TURN VANA OFF AND ON");
        }
        let glance = self.menu.glance();
        if glance.is_empty() {
            return Check::new(Level::Off, "NO SENSORS", "TAP HERE TO PAIR ONE");
        }
        let live = glance.iter().filter(|g| g.live).count();
        let mut title = Text::<24>::new();
        // Short names, two spaces apart: at most four fit a line.
        let mut names = Text::<19>::new();
        let pick = |want_live: bool| glance.iter().filter(move |g| g.live == want_live);
        if live == glance.len() {
            let plural = if live == 1 { "" } else { "S" };
            let _ = write!(title, "{live} SENSOR{plural} LIVE");
            for (index, g) in pick(true).enumerate() {
                let separator = if index == 0 { "" } else { "  " };
                let name = core::str::from_utf8(crate::screens::home::short_name(g.kind));
                let _ = write!(names, "{separator}{}", name.unwrap_or(""));
            }
            return Check::new(Level::Good, title.as_str(), names.as_str());
        }
        // Name the sensors still waking; pedalling or wearing them helps.
        for (index, g) in pick(false).enumerate() {
            let separator = if index == 0 { "" } else { "  " };
            let name = core::str::from_utf8(crate::screens::home::short_name(g.kind));
            let _ = write!(names, "{separator}{}", name.unwrap_or(""));
        }
        Check::new(Level::Wait, "SENSORS WAKING UP", names.as_str())
    }

    /// Room for the ride about to start, wherever it will go.
    fn storage_check(&self) -> Check {
        let medium = self.ride.medium();
        let recorder = self.ride.recorder(medium);
        let mut title = Text::<24>::new();
        match recorder.status() {
            Status::Scanning | Status::Formatting | Status::Clearing => {
                Check::new(Level::Wait, "CHECKING STORAGE", "")
            }
            Status::Recording | Status::Paused => Check::new(Level::Good, "RECORDING", ""),
            status @ (Status::Ready | Status::Saved | Status::Recovered) => {
                let left = recorder.remaining_secs();
                if left == 0 {
                    return Check::new(Level::Problem, "STORAGE FULL", "EXPORT OLD RIDES");
                }
                // Seconds left still record; the ride counts them down.
                if left < 60 {
                    return Check::new(Level::Wait, "UNDER 1 MIN OF ROOM", "EXPORT OLD RIDES");
                }
                if left < LOW_STORAGE_S {
                    let _ = write!(title, "ROOM FOR {} MIN", left / 60);
                    return Check::new(Level::Wait, title.as_str(), "EXPORT OLD RIDES");
                }
                let _ = write!(title, "ROOM FOR {} H", left / 3600);
                if self.ride.fallback() {
                    return Check::new(Level::Wait, title.as_str(), "ON INTERNAL MEMORY");
                }
                // A ride cut short by power loss was kept; say so once here.
                if status == Status::Recovered {
                    return Check::new(Level::Good, title.as_str(), "LAST RIDE RECOVERED");
                }
                Check::new(Level::Good, title.as_str(), "")
            }
            Status::Full => Check::new(Level::Problem, "STORAGE FULL", "EXPORT OLD RIDES"),
            _ => Check::new(Level::Problem, "STORAGE PROBLEM", "TURN VANA OFF AND ON"),
        }
    }

    fn settings_lines(&self, view: &mut View) {
        use super::preferences::Persistence;
        use crate::screens::settings::{Kind, NOTES, Row, SECTIONS, STATUS, VALUES};
        let cursor = view.cursor;
        let row = Row::at(cursor);
        let editor = [row];
        let shown: &[Row] = if view.editing && row.kind() == Kind::Number {
            &editor
        } else {
            SECTIONS[settings::section(cursor).0].1
        };
        let (values, notes) = view.lines[VALUES..].split_at_mut(NOTES - VALUES);
        for ((row, value), note) in shown.iter().zip(values).zip(notes) {
            self.setting_line(*row, value, note);
        }
        let _ = view.lines[STATUS].push_str(match self.preferences.persistence() {
            Persistence::Saved => "SAVED",
            Persistence::Pending => "SAVING",
            Persistence::Failed => "SAVE FAILED, RETRYING",
            Persistence::Locked => "NOT SAVED: DATA KEPT",
        });
    }

    /// A Settings row's value and the note beside it.
    fn setting_line(&self, row: settings::Row, value: &mut Text<38>, note: &mut Text<38>) {
        use crate::screens::settings::Row;
        let options = self.preferences.options;
        let profile = self.preferences.profile;
        let display = self.preferences.display;
        let on = |on: bool| if on { "ON" } else { "OFF" };
        let _ = match row {
            Row::Ftp => {
                // Threshold zones: where FTP and LTHR put the rider's limit.
                let (low, high) = profile.power_range(4);
                let _ = write!(note, "Z4 {}-{} W", low, high - 1);
                write!(value, "{}", profile.ftp_watts)
            }
            Row::Threshold => {
                let (low, _) = profile.heart_range(5);
                let _ = write!(note, "Z5 FROM {} BPM", low);
                write!(value, "{}", profile.lthr_bpm)
            }
            Row::Brightness => {
                if self.night.is_night() && display.brightness > NIGHT_PERCENT {
                    let _ = write!(note, "NIGHT {NIGHT_PERCENT}%");
                }
                write!(value, "{}%", display.brightness)
            }
            Row::Daylight => {
                use super::preferences::Daylight;
                if options.daylight == Daylight::Auto {
                    let now = if self.night.is_night() {
                        "NIGHT"
                    } else {
                        "DAY"
                    };
                    let _ = write!(note, "{now} NOW");
                }
                write!(
                    value,
                    "{}",
                    match options.daylight {
                        Daylight::Auto => "AUTO",
                        Daylight::Day => "DAY",
                        Daylight::Night => "NIGHT",
                    }
                )
            }
            Row::Theme => {
                use super::preferences::Theme;
                if options.theme == Theme::Auto {
                    let now = if self.night.is_night() {
                        "DARK"
                    } else {
                        "LIGHT"
                    };
                    let _ = write!(note, "{now} NOW");
                }
                write!(
                    value,
                    "{}",
                    match options.theme {
                        Theme::Dark => "DARK",
                        Theme::Light => "LIGHT",
                        Theme::Auto => "AUTO",
                    }
                )
            }
            Row::TimeZone => match options.zone {
                Zone::Chile => {
                    if let Some(ms) = self.readings.live.utc_ms {
                        let offset = options.zone.offset_s((ms / 1000) as i64);
                        let _ = utc_offset(note, offset);
                        let _ = write!(note, " NOW");
                    }
                    write!(value, "CHILE")
                }
                Zone::Fixed(_) => {
                    let _ = write!(note, "ALL YEAR");
                    utc_offset(value, options.zone.offset_s(0))
                }
            },
            Row::AutoDim => match display.dim_secs {
                0 => write!(value, "NEVER"),
                secs @ ..60 => write!(value, "{secs} S"),
                secs => write!(value, "{} MIN", secs / 60),
            },
            Row::AutoPause => {
                let _ = write!(note, "WHEN STOPPED");
                write!(value, "{}", on(options.auto_pause))
            }
            Row::AutoLap => match options.auto_lap_km {
                0 => write!(value, "OFF"),
                km => write!(value, "EVERY {km} KM"),
            },
            Row::Sounds => {
                if !options.sounds {
                    let _ = write!(note, "RADAR STAYS ON");
                }
                write!(value, "{}", on(options.sounds))
            }
            Row::RadarAlerts => {
                let _ = write!(note, "APPROACH ALARM STAYS ON");
                write!(value, "{}", on(self.alert_settings().radar))
            }
            Row::SunsetAlert => {
                let _ = write!(note, "30 MIN BEFORE AND AT");
                write!(value, "{}", on(options.sunset_alert))
            }
            Row::FuelAlert => match self.alert_settings().fuel_every_min {
                0 => write!(value, "OFF"),
                min => write!(value, "EVERY {min} MIN"),
            },
            Row::CadenceLow => limit(value, self.alert_settings().cadence_low_rpm.into(), "RPM"),
            Row::HeartHigh => limit(value, self.alert_settings().heart_high_bpm.into(), "BPM"),
            Row::HeartLow => limit(value, self.alert_settings().heart_low_bpm.into(), "BPM"),
            Row::PowerHigh => limit(value, self.alert_settings().power_high_w, "W"),
            Row::PowerLow => limit(value, self.alert_settings().power_low_w, "W"),
            Row::Sensors => match self.preferences.sensors.peers().count() {
                0 => write!(value, "NONE SAVED"),
                count => write!(value, "{count} SAVED"),
            },
            Row::Phone => write!(value, "SEND RIDES"),
            Row::Bests => value.push_str(self.bests.summary().as_str()),
            Row::Storage => {
                if self.ride.bulk.status() != Status::Unsupported {
                    let _ = write!(note, "{}", self.ride.medium().label());
                }
                let recorder = self.ride.recorder(self.ride.medium());
                match recorder.status() {
                    Status::Ready | Status::Saved | Status::Recovered => {
                        let secs = recorder.remaining_secs();
                        let hours = secs / 3600;
                        if hours >= 100 {
                            write!(value, "{hours}H LEFT")
                        } else {
                            write!(value, "{hours}:{:02} LEFT", secs / 60 % 60)
                        }
                    }
                    status => write!(value, "{}", status.name()),
                }
            }
            Row::Firmware => write!(value, "VANA {}", env!("CARGO_PKG_VERSION")),
        };
    }
}

impl Runtime {
    /// What the next frame shows of gliding values: none of them on other
    /// pages or the paused screen, readings only on the main ride page.
    fn motion_visible(&self) -> super::motion::Visible {
        let navigation = &self.navigation;
        let riding = navigation.page == Page::Ride
            && !(self.ride.active().status() == Status::Paused && !navigation.pause_hidden);
        super::motion::Visible {
            readings: riding && navigation.ride_page == RidePage::Main,
            radar: riding,
        }
    }
}

/// The readings the ride page shows, to glide: power is its 3 s average.
fn shown_sample(readings: &super::readings::Readings) -> crate::ride::log::Sample {
    crate::ride::log::Sample {
        power_watts: readings.power_3s_watts,
        ..readings.live
    }
}

/// Radar targets with the times their reports arrived, for gliding markers.
fn radar_reports(readings: &super::readings::Readings, now: u64) -> super::motion::Reports {
    super::motion::Reports {
        snapshot: readings.radar.snapshot(now),
        received_ms: readings.radar.received_ms(),
    }
}

/// An alert limit's value: "OFF", or the limit in its unit.
fn limit(value: &mut Text<38>, limit: u16, unit: &str) -> core::fmt::Result {
    match limit {
        0 => write!(value, "OFF"),
        limit => write!(value, "{limit} {unit}"),
    }
}

/// The back page's light caption: sunset on the `zone` clock, or dark.
fn light(sun: Option<Sun>, zone: Zone) -> ride::Light {
    match sun {
        Some(Sun::Up { sets_at }) => {
            ride::Light::SetsAt(zone.local(sets_at.max(0) as u64).of_day() as u32)
        }
        Some(Sun::Down) => ride::Light::Dark,
        _ => ride::Light::Unknown,
    }
}

/// "UTC-3", "UTC+5:30" or "UTC", from seconds east of UTC.
fn utc_offset(text: &mut Text<38>, offset_s: i64) -> core::fmt::Result {
    let sign = if offset_s < 0 { '-' } else { '+' };
    let minutes = offset_s.unsigned_abs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => write!(text, "UTC"),
        (hours, 0) => write!(text, "UTC{sign}{hours}"),
        (hours, minutes) => write!(text, "UTC{sign}{hours}:{minutes:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daylight::sun_state;
    use firmware_services::gps::unix_seconds;

    /// The sun sets near 17:42 in a Santiago winter and 20:55 in summer.
    #[test]
    fn sunset_reads_on_the_clock_of_the_season() {
        let sets = |date: &[u8; 6]| {
            let noon = unix_seconds(*date, *b"160000").unwrap();
            match light(Some(sun_state(noon, -33.45, -70.66)), Zone::Chile) {
                ride::Light::SetsAt(local) => (local / 3600, local / 60 % 60),
                other => panic!("{other:?}"),
            }
        };
        let (hour, minute) = sets(b"210626");
        assert_eq!(hour, 17, "June");
        assert!((38..=46).contains(&minute), "June 17:{minute}");
        let (hour, minute) = sets(b"211226");
        assert_eq!(hour, 20, "December 20:{minute}");
        assert_eq!(light(Some(Sun::Down), Zone::Chile), ride::Light::Dark);
        assert_eq!(light(None, Zone::Chile), ride::Light::Unknown);
    }
}
