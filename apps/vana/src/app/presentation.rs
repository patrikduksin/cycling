//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use super::cues::Cue;
use super::ride_control::RideMessage;
use super::view::{Presented, Shows};
use crate::clock::Zone;
use crate::compose::builtin::ScreenId;
use crate::compose::registry::WORKOUT_CHART;
use crate::ride::alerts::{Alert, Cue as AlertSound};
use crate::ride::current::Request;
use crate::ride::laps::AutoLap;
use crate::ride::log::Status;
use crate::ride::medium::Medium;
use crate::ride::turn_back::Turn;
use crate::screens::chrome::{AMBER, Battery, CHARGE_STEPS, Gps, RED, Row, Toast, Tone};
use crate::screens::home::Home;
use crate::screens::look;
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ready::{Check, Level, Line as ReadyLine, Readiness};
use crate::screens::ride::summary::{After, Saving};
use crate::screens::ride::{self, RidePage};
use crate::screens::sensors::{GLANCES, Glance, MenuMessage};
use crate::screens::settings;
use crate::screens::touch::{Control, List};
use crate::screens::workout::{LOW_STORAGE_S, Page, Text, View};
use crate::sensors::ant::{BatteryStatus, device_type};
use crate::sensors::radar::Beep;
use core::fmt::Write;
use device_api::display::Display as Panel;
use device_api::gps::FixState;
use device_api::input::InputSource;
use device_api::observation::Observation;
use device_api::power::Power;
use device_api::sensors::Startup;
use device_api::sound::Sound;
use device_api::storage::OwnedFlash;
use firmware_services::why;
use firmware_shell::shell::{Picture, Shell};

/// Sensors whose battery wants changing, as Ready and Home name them. While
/// any sensor is not live it is missing first and no battery is news.
struct BatteryWarning {
    title: Text<24>,
    critical: bool,
    several: bool,
}

impl BatteryWarning {
    fn of(glance: &[crate::screens::sensors::Glance]) -> Option<Self> {
        if glance.iter().any(|g| !g.live) {
            return None;
        }
        let mut low = glance.iter().filter(|g| g.battery_low());
        let first = low.next()?;
        let count = 1 + low.count();
        let critical = glance
            .iter()
            .any(|g| g.battery == Some(BatteryStatus::Critical));
        let mut title = Text::<24>::new();
        if count == 1 {
            let name = core::str::from_utf8(crate::screens::home::short_name(first.kind));
            let state = if critical { "CRITICAL" } else { "LOW" };
            let _ = write!(title, "{} {state} BATTERY", name.unwrap_or(""));
        } else {
            let _ = write!(title, "{count} BATTERIES TO CHANGE");
        }
        Some(Self {
            title,
            critical,
            several: count > 1,
        })
    }

    fn hint(&self) -> &'static str {
        match (self.critical, self.several) {
            (false, false) => "CHANGE IT SOON",
            (false, true) => "CHANGE THEM SOON",
            (true, false) => "CHANGE IT BEFORE RIDING",
            (true, true) => "CHANGE THEM BEFORE RIDING",
        }
    }
}

/// The brand stays this long after VANA's first frame, then Home opens. The
/// device keeps the backlight dark until VANA runs, so this is all a rider sees.
pub(super) const BRAND_MS: u64 = 400;
pub(super) const ANIMATED_FRAME_MS: u64 = 500;
/// Motion (the brand reveal, the farewell) draws at this cadence: a frame
/// takes about 25 ms on the C606, so this leaves the loop free between them.
pub(super) const MOTION_FRAME_MS: u64 = 40;
pub(super) const STATIC_FRAME_MS: u64 = 1000;
/// A list following the finger or coasting draws at up to 60 frames a second.
pub(super) const SCROLL_FRAME_MS: u64 = 16;
/// How long a confirmation or hint stays over the content.
pub(super) const TOAST_MS: u64 = 2000;
/// Battery news stays a little longer than a confirmation.
pub(super) const BATTERY_TOAST_MS: u64 = 4000;
/// The time-left warning keeps this far from a battery level toast.
const BATTERY_NEWS_GAP_MS: u64 = 30_000;
/// How long a ride without wheel speed notes that auto-pause is off.
const NO_SPEED_NOTE_MS: u64 = 8000;

/// Foreground drawing. Once the terminal takes over the panel it stays suspended.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub(super) enum Display {
    Active { next_frame_ms: u64 },
    Suspended,
}

state!(Display = "display");

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
    /// presents. Not inlined: the view is several KB, and in the caller's
    /// poll frame it would sit under every later frame's preparation (#167).
    #[inline(never)]
    pub fn draw_boot<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &self,
        system: &mut Shell<D, I, P, B>,
    ) {
        let mut view = View::new(Page::Boot, 0, None);
        view.layout = self.layout;
        view.light = self.light();
        let scene = view.prepare();
        #[cfg(any(test, feature = "invariants"))]
        scene.check_layout();
        system.draw_rows(|y, pixels| scene.paint(&mut Row::new(y, pixels)));
    }

    /// Sound the cues and alarms due: the latest ride moment's cue, a closed
    /// lap, the radar's beeps for the nearest vehicle behind while on the ride
    /// screen, which win over any cue, and the rider's alerts (`ride::alerts`), each with
    /// a toast on the ride pages. A cue waits for the buzzer to be free, for
    /// up to a second. Call once per loop.
    pub fn alert(&mut self, sound: &mut impl Sound, now: u64) {
        if self.notices.take_lap_chime() {
            self.cue(Cue::Lap, now);
        }
        if let Some(cue) = self.navigation.take_chime() {
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
            self.navigation.page() == Page::Ride,
            self.night.sun(now),
        );
        let sun = self.night.sun(now);
        let leg = self.track.as_deref().and_then(|track| {
            super::course::way_home(
                track,
                self.ride.totals().avg_speed_mm_s(),
                self.ride.totals().moving_ms(),
            )
        });
        let turn = sun
            .zip(leg)
            .and_then(|((sun, unix), leg)| Turn::new(sun, unix, leg));
        let turn_back = self.alerts.turn_back(
            now,
            self.preferences.options.sunset_alert,
            self.ride.active().status(),
            self.navigation.page() == Page::Ride,
            turn,
            self.preferences.options.zone,
        );
        if self.navigation.page() == Page::Ride {
            if let Some(beep) = self.readings.radar_warning.update(radar, now) {
                self.cue(
                    match beep {
                        Beep::Appeared => Cue::RadarApproach,
                        Beep::Near => Cue::RadarNear,
                        Beep::Close => Cue::RadarClose,
                        Beep::Passing => Cue::RadarPassing,
                    },
                    now,
                );
            }
            let alert = alert.or_else(|| {
                let ride_left = self
                    .training
                    .player
                    .as_ref()
                    .and_then(|player| player.remaining_secs())
                    .map(|secs| secs.div_ceil(60));
                let after_news = self
                    .readings
                    .battery_news_ms
                    .is_some_and(|at| now.saturating_sub(at) < BATTERY_NEWS_GAP_MS);
                self.alerts.battery(
                    now,
                    self.ride.active().status(),
                    self.readings.battery_life.minutes_left(now),
                    ride_left,
                    after_news,
                )
            });
            if let Some(alert) = alert.or(sunset).or(turn_back) {
                self.ride_alert(alert, now);
            }
        } else {
            self.alerts.battery_away();
        }
        let track = &self.track;
        self.cues
            .play(sound, now, |cue| super::course::still_due(track, cue, now));
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
            Some(AlertSound::Battery) => Cue::LowBattery,
            None => return,
        };
        self.cue(cue, now);
    }

    /// Raise `cue`, unless the rider muted cues and it is no safety alarm.
    pub(super) fn cue(&mut self, cue: Cue, now: u64) {
        self.cues.raise(cue, !self.preferences.options.sounds, now);
    }

    pub fn set_startup_status(&mut self, status: Startup) {
        if self.startup != Some(status) {
            self.startup = Some(status);
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
    pub(super) fn drawn_at(&self, now: u64) -> u64 {
        self.latency_clock.map_or(now, |clock| clock()).max(now)
    }

    pub fn suspend_display(&mut self) {
        self.display = Display::Suspended;
    }

    #[inline(never)]
    pub(super) fn present<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &self,
        system: &mut Shell<D, I, P, B>,
        shows: Shows,
        now: u64,
    ) -> Presented {
        let (gps, radar_paired) = match shows {
            Shows::Sheet(sheet) => {
                let Some(laid) = self.composition.laids().get(sheet.screen()) else {
                    unreachable!("the frame showed the sheet's screen");
                };
                let (canvas, cues) = (sheet.canvas(laid), PowerSheet::cues(laid));
                #[cfg(any(test, feature = "invariants"))]
                PowerSheet::check_layout(&canvas, self.layout);
                let look = look::look(self.light());
                system.draw_rows(|y, pixels| {
                    let mut row = Row::new(y, pixels);
                    PowerSheet::paint(&canvas, &cues, self.layout, &mut row.looked(look));
                });
                return Presented::Sheet {
                    ok: !system.display_error,
                };
            }
            Shows::Page { gps, radar_paired } => (gps, radar_paired),
        };
        let (choice, course) = (self.page_choice_with_course_map(), self.course().is_some());
        let ftp_offer = self.ftp_sheet();
        let picker_offset = self.list_offset(List::Workouts, now);
        let picker = (self.navigation.page() == Page::Workouts).then(|| {
            let pressed = match self.navigation.touches().pressed() {
                Some(Control::Workout(index)) => Some(index),
                _ => None,
            };
            self.picker(self.list_offset(List::Workouts, now), pressed)
        });
        let light = self.light();
        let navigation = &self.navigation;
        let page = navigation.page();
        let zone = self.preferences.options.zone;
        let local = match self.shown.live().utc_ms {
            Some(ms) => Some(zone.local(ms / 1000)),
            None => gps.and_then(|p| p.gps.utc).map(|v| {
                zone.local_of_day(
                    u64::from((v[0] - b'0') * 10 + v[1] - b'0') * 3600
                        + u64::from((v[2] - b'0') * 10 + v[3] - b'0') * 60,
                )
            }),
        };
        let mut view = navigation.view(local);
        view.layout = self.layout;
        view.now_ms = now;
        view.active_secs = self.ride.active_ms(now) / 1000;
        view.state = self.ride.active().status();
        let sample = self.shown.sample();
        view.power_3s = sample.power_watts;
        view.sample = self.shown.live();
        view.radar_paired = radar_paired;
        view.auto_paused = self.ride.auto().paused();
        view.ride_page = navigation.ride_page();
        view.ride_pages = choice;
        view.nav = super::course::sign_of(&self.track, now);
        view.course = course;
        view.main_slots = self.preferences.options.main_slots;
        view.laids = Some(self.composition.laids());
        let readout = self.shown.readout();
        let (climbing, paused_secs) = (readout.climbing, readout.paused_secs);
        view.totals = readout.totals;
        if page == Page::Ride && view.ride_page == RidePage::Lap {
            let laps = self.ride.laps();
            let kept = (0..crate::ride::laps::HISTORY).rev();
            let bars = kept
                .filter_map(|back| laps.finished(back))
                .map(Into::into)
                .collect();
            view.show_laps(bars, readout.lap, readout.previous_lap);
        }
        view.zone_time = readout.zone_time;
        view.profile = self.preferences.profile;
        view.rider = self.preferences.options.rider;
        if page == Page::Ride {
            view.training = readout.training;
            if view.ride_page == RidePage::Workout {
                let laid = self.composition.laids().get(ScreenId::RideWorkout);
                let rect = laid.and_then(|laid| laid.box_of(WORKOUT_CHART));
                view.show_workout(match (&self.training.player, rect) {
                    (Some(player), Some(rect)) => {
                        crate::screens::ride::workout_chart(player, &view.profile, rect)
                    }
                    _ => Default::default(),
                });
            }
        }
        // The sheet shows wherever it takes the buttons, as after a test
        // started from the terminal while Home is open.
        view.ftp_offer = ftp_offer;
        if let Some(picker) = picker {
            view.scroll = picker_offset;
            view.show_picker(picker);
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
                // A full fill cannot climb, so one phase keeps the frame's
                // digest unchanged.
                phase: if value.0 >= 100 {
                    0
                } else {
                    (now / ANIMATED_FRAME_MS % u64::from(CHARGE_STEPS)) as u8
                },
            }),
            Observation::Unavailable => None,
        };
        view.paused_secs = paused_secs;
        view.pause_screen =
            page == Page::Ride && view.state == Status::Paused && !navigation.pause_hidden();
        view.toast = self.toast_at(now);
        match page {
            Page::Boot => {
                let first_frame = navigation.first_frame().unwrap_or(now);
                let shown = now.saturating_sub(first_frame).min(BRAND_MS);
                view.reveal = (shown * u64::from(crate::screens::brand::DONE) / BRAND_MS) as u16;
            }
            Page::Ready => {
                view.show_ready(self.readiness(view.gps, view.battery, now));
            }
            Page::Summary => {
                view.show_summary(self.after(now), Text::new());
                self.show_elevation(&mut view);
            }
            Page::Bests => {
                view.show_bests(self.bests.bests, self.bests.mode());
            }
            Page::Form => view.show_form(self.form.mode),
            Page::PastRide => self.past_view(&mut view),
            Page::Phone => self.phone_view(&mut view),
            Page::History => {
                let offset = self.list_offset(List::Rides, now);
                let pressed = match self.navigation.touches().pressed() {
                    Some(Control::Ride(index)) => Some(index),
                    _ => None,
                };
                view.scroll = offset;
                view.set_canvas(self.history_canvas(offset, pressed));
            }
            Page::Format => view.format = self.format.shows(),
            Page::LoseRides => view.unsaved = self.sync.count() as u8,
            Page::EndRide | Page::DiscardRide => {
                view.discard = if self.ride.discard() != crate::ride::current::Discard::Idle {
                    crate::screens::workout::Discard::Answered
                } else if self.can_discard() {
                    crate::screens::workout::Discard::Offered
                } else {
                    crate::screens::workout::Discard::Unavailable
                };
            }
            Page::Ride => {}
            Page::Home => {
                view.set_home(self.home(gps_fix, now));
            }
            Page::Sensors => {
                view.show_sensors(self.menu.canvas(), self.menu.cues());
            }
            Page::Settings => {
                view.scroll = self.list_offset(List::Settings, now);
                self.settings_lines(&mut view);
                let first = view.sections.rows(view.cursor).1;
                view.press_setting(match self.navigation.touches().pressed() {
                    Some(Control::Setting(field)) => Some((field - first) as u16),
                    _ => None,
                });
            }
            Page::Workouts => {}
            Page::Setup => self.setup_lines(&mut view),
        }
        if page == Page::Ride && view.ride_page == RidePage::Climb {
            // Borrowed from the ride and the look-ahead's own block, not copied.
            view.show_climb(crate::screens::ride::ClimbLent {
                climbs: self.ride.climbs(),
                climbing,
                trace: self.ride.trace(),
                forecast: super::lookahead::forecast_of(&self.lookahead),
                pinned: super::lookahead::pinned_of(&self.lookahead),
            });
        }
        view.light = light;
        if page == Page::Ride && view.ride_page == RidePage::Map {
            view.show_map(super::course::map_of(&self.track));
        }
        view.graphics = self.motion.graphics(self.motion_visible(), now);
        let scene = view.prepare();
        #[cfg(any(test, feature = "invariants"))]
        scene.check_layout();
        let drawn = system.draw(&scene, now);
        Presented::Page {
            page,
            drawn,
            pixels: crate::screens::ride::pixels(
                &view.graphics,
                &view.profile,
                self.motion_visible().main.then(|| self.composition.main()),
                view.layout,
            ),
        }
    }

    /// Home says what is still starting, why turning off did not happen, or
    /// that rides are not saved; `true` when it is a problem.
    pub(super) fn home_status(&self, now: u64) -> (&'static str, bool) {
        if let Some(notice) = self.power.notice(now) {
            return (notice.text(), true);
        }
        // Rides the phone has not confirmed stay named until it has.
        if self.holds_unsynced() {
            return ("NOT SAVED", true);
        }
        let status = if self.ride.internal().status() == Status::Scanning
            || self.ride.bulk().status() == Status::Scanning
        {
            "CHECKING STORAGE"
        } else if matches!(
            self.startup,
            Some(
                Startup::NotSubmitted
                    | Startup::Probing
                    | Startup::Pending
                    | Startup::Waiting
                    | Startup::WakePending
            )
        ) {
            "STARTING SENSORS"
        } else if matches!(
            self.startup,
            Some(Startup::Uncertain | Startup::TimedOut | Startup::Unavailable)
        ) {
            return ("SENSORS UNAVAILABLE", true);
        } else {
            "READY TO RIDE"
        };
        (status, false)
    }

    /// The sensors Home and Ready summarise: the saved ANT sensors, then the
    /// selected BLE sensor. A live ANT heart-rate sensor stands for the role,
    /// as the ride prefers it; with none live the rider is on the BLE strap.
    /// The ride prefers BLE cadence, so a live ANT cadence sensor (cadence-only
    /// or combined) stands for the role only while the BLE sensor is not
    /// connected. A failed radio has no strap to speak of.
    fn glance(&self, now: u64) -> heapless::Vec<Glance, GLANCES> {
        use crate::sensors::ble_profile::{Link, Profile};
        let mut glance = self.menu.glance();
        let ble = self.sensors.snapshot(now);
        let kind = match ble.profile {
            Profile::HeartRate => device_type::HEART_RATE,
            Profile::Cadence => device_type::CADENCE,
            Profile::Echo => return glance,
        };
        let ant_serves = ant_stands_for(
            kind,
            ble.link == Link::Connected,
            self.readings.crank_tenths(now).is_some(),
            &glance,
        );
        if !matches!(ble.link, Link::Off | Link::Failed) && !ant_serves {
            let battery = ble.battery.map(|battery| battery.status);
            // Cannot fail: the capacity holds every ANT slot and this one.
            let _ = glance.push(Glance::ble(kind, ble.link == Link::Connected, battery));
        }
        glance
    }

    #[inline(never)]
    fn home(&self, gps_fix: bool, now: u64) -> Home {
        let last = self.rides_line();
        let glance = self.glance(now);
        let mut home = Home::new(
            self.layout,
            self.navigation.home_cursor(),
            gps_fix,
            &glance,
            last,
        );
        let (status, problem) = self.home_status(now);
        let warning = BatteryWarning::of(&glance).filter(|_| !problem && status == "READY TO RIDE");
        match (self.chosen_name(), warning) {
            (_, Some(warning)) => {
                let color = if warning.critical { RED } else { AMBER };
                home.set_status_in(warning.title.as_bytes(), color)
            }
            // A workout waiting to be ridden names itself on the ride card.
            (Some(name), None) if !problem && status == "READY TO RIDE" => {
                home.set_status(name.as_bytes(), false)
            }
            _ => home.set_status(status.as_bytes(), problem),
        }
        home
    }

    /// What the toast band shows now: a lap just finished first, then an automatic start or resume, then the latest
    /// confirmation, then ride problems and low storage, which last until
    /// they are resolved, then what auto-pause is doing.
    pub(super) fn toast_at(&self, now: u64) -> Option<Toast> {
        if let Some(lap) = self.notices.lap_toast(now) {
            why!("toast.lap", lap = lap.number, time_ms = lap.time_ms);
            return Some(Toast::new(ride::lap_toast(lap).as_str(), Tone::Info));
        }
        let auto = self.notices.auto_toast(now);
        if !auto.is_empty() {
            why!("toast.auto", auto);
            return Some(Toast::new(auto, Tone::Info));
        }
        let navigation = &self.navigation;
        if let Some((toast, until)) = navigation.toast_shown()
            && now < until
        {
            why!("toast.confirmation", shown = shown(&toast), until);
            return Some(toast);
        }
        if navigation.page() != Page::Ride {
            why!("toast.none", page = navigation.page());
            return None;
        }
        let problem = match (self.ride.request(), self.ride.active().status()) {
            (_, Status::Error) => "STORAGE ERROR",
            (_, Status::Full) => "STORAGE FULL",
            // A failed write always shows, except the bulk failure that moved
            // the ride to internal flash: the fallback toast names that one.
            (Request::Completed(result), _)
                if !result.ok
                    && !(self.falling_back() && self.ride.request_on() == Medium::Bulk) =>
            {
                "WRITE FAILED"
            }
            _ if matches!(self.notices.message(), RideMessage::StorageNotReady) => {
                "STORAGE NOT READY"
            }
            (_, status) => {
                if let Some(toast) = self.storage_toast(status) {
                    why!("toast.storage", status, shown = shown(&toast));
                    return Some(toast);
                }
                let toast = self.auto_pause_toast(status, now);
                match &toast {
                    Some(toast) => why!("toast.auto_pause", status, shown = shown(toast)),
                    None => why!("toast.none", status),
                }
                return toast;
            }
        };
        why!("toast.problem", problem);
        Some(Toast::new(problem, Tone::Alert))
    }

    /// Recording time left, once it is under `LOW_STORAGE_S`; else rides
    /// the phone has not confirmed; else rides falling back to internal
    /// flash because bulk media cannot take them; else a recovered ride.
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
        if self.holds_unsynced() {
            return Some(Toast::new("NOT SAVED", Tone::Warn));
        }
        if self.falling_back() {
            let text = match status {
                Status::Recording => "RECORDING INT",
                Status::Paused if self.ride.auto().paused() => "AUTO PAUSED INT",
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
            Status::Paused if self.ride.auto().paused() && self.navigation.pause_hidden() => {
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
            view.set_elevation_chart(self.chart.chart());
        }
    }

    /// The summary has an elevation page: the ride, live or read back, charted
    /// altitude.
    pub(super) fn has_elevation(&self) -> bool {
        self.ride.trace().len() >= 2
    }

    /// Where the ride just finished stands.
    pub(super) fn saving(&self) -> Saving {
        match self.ride.active().status() {
            _ if self.ride.pending() => Saving::Writing,
            Status::Recording | Status::Paused => Saving::Writing,
            Status::Error => Saving::Failed,
            _ if self.shown_ride_unsynced() => Saving::Unsynced,
            _ => Saving::Saved,
        }
    }

    /// What the summary shows beyond the totals.
    pub(super) fn after(&self, now: u64) -> After {
        After {
            page: self.navigation.summary_page(),
            saving: self.saving(),
            best: self.ride.efforts().best(),
            climbs: Some(self.ride.climbing().count),
            // The open lap counts: a ride without a lap press is one lap.
            laps: Some(self.ride.laps().count().saturating_add(1)),
            past: false,
            send: false,
            delete: false,
            elevation: self.has_elevation(),
            found: self.ride.bests_found(),
            offer: self.summary_offer(),
            recovery: self.recovery.reading(now),
        }
    }

    /// The Ready checks: GPS, sensors, battery and room to record, each in
    /// plain words with what to do when something needs the rider.
    pub(super) fn readiness(&self, gps: Gps, battery: Option<Battery>, now: u64) -> Readiness {
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
        checks[ReadyLine::Sensors as usize] = self.sensors_check(now);
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
                // Time left shows when the charge needs nothing from the
                // rider, and only once the rate is trustworthy.
                let mut left = Text::<28>::new();
                if let Some(minutes) = self.readings.battery_life.minutes_left(now) {
                    let _ = write!(
                        left,
                        "ABOUT {} LEFT",
                        crate::ride::battery_life::about(minutes)
                    );
                }
                let (level, hint) = match percent {
                    30.. => (Level::Good, left.as_str()),
                    15..30 => (Level::Wait, "CHARGE AFTER THE RIDE"),
                    _ => (Level::Problem, "CHARGE BEFORE RIDING"),
                };
                Check::new(level, title.as_str(), hint)
            }
        };
        checks[ReadyLine::Storage as usize] = self.storage_check();
        readiness
    }

    fn sensors_check(&self, now: u64) -> Check {
        if matches!(
            self.startup,
            Some(Startup::Uncertain | Startup::TimedOut | Startup::Unavailable)
        ) {
            return Check::new(Level::Wait, "SENSORS UNAVAILABLE", "TURN VANA OFF AND ON");
        }
        let glance = self.glance(now);
        if glance.is_empty() {
            return Check::new(Level::Off, "NO SENSORS", "TAP HERE TO PAIR ONE");
        }
        let mut title = Text::<24>::new();
        let live = glance.iter().filter(|g| g.live).count();
        // Short names, two spaces apart: at most four fit a line.
        let mut names = Text::<19>::new();
        let pick = |want_live: bool| glance.iter().filter(move |g| g.live == want_live);
        if live == glance.len() {
            if let Some(warning) = BatteryWarning::of(&glance) {
                let level = if warning.critical {
                    Level::Problem
                } else {
                    Level::Wait
                };
                return Check::new(level, warning.title.as_str(), warning.hint());
            }
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
        const UNFORMATTED: (&str, &str) = ("STORAGE NOT FORMATTED", "OFF AND ON TO FORMAT");
        if self.format.stops_own_storage() {
            return Check::new(Level::Problem, UNFORMATTED.0, UNFORMATTED.1);
        }
        let medium = self.ride.medium();
        let recorder = self.ride.recorder(medium);
        let mut title = Text::<24>::new();
        match recorder.status() {
            Status::Scanning | Status::Formatting | Status::Clearing => {
                Check::new(Level::Wait, "CHECKING STORAGE", "")
            }
            Status::Recording | Status::Paused => Check::new(Level::Good, "RECORDING", ""),
            Status::Ready | Status::Saved | Status::Recovered
                if !self.ride.admits_start(medium) =>
            {
                let title = if self.holds_unsynced() {
                    "SYNC RIDES FIRST"
                } else {
                    "CLEAR RIDES FIRST"
                };
                Check::new(Level::Problem, title, "ON THE PHONE PAGE")
            }
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
            _ if self.storage_health().foreign() => {
                Check::new(Level::Problem, UNFORMATTED.0, UNFORMATTED.1)
            }
            _ => Check::new(Level::Problem, "STORAGE PROBLEM", "TURN VANA OFF AND ON"),
        }
    }

    fn settings_lines(&self, view: &mut View) {
        use super::preferences::Persistence;
        use crate::screens::settings::{Kind, NOTES, Row, STATUS, VALUES};
        let cursor = view.cursor;
        let row = view.sections.row(cursor);
        let editor = [row];
        let editing = view.editing && row.kind() == Kind::Number;
        let shown: &[Row] = if editing {
            &editor
        } else {
            view.sections.rows(cursor).0
        };
        let lines = view.show_settings();
        let (values, notes) = lines[VALUES..].split_at_mut(NOTES - VALUES);
        for ((row, value), note) in shown.iter().zip(values).zip(notes) {
            self.setting_line(*row, value, note);
        }
        let _ = lines[STATUS].push_str(match self.preferences.persistence() {
            Persistence::Saved => "SAVED",
            Persistence::Pending => "SAVING",
            Persistence::Failed => "SAVE FAILED, RETRYING",
            Persistence::Locked => "NOT SAVED: DATA KEPT",
        });
    }

    /// A Settings row's value and the note beside it.
    fn setting_line(&self, row: settings::Row, value: &mut Text<38>, note: &mut Text<38>) {
        use crate::ride::rider::Sex;
        use crate::screens::settings::Row;
        let options = self.preferences.options;
        let profile = self.preferences.profile;
        let display = self.preferences.display;
        let on = |on: bool| if on { "ON" } else { "OFF" };
        let _ = match row {
            Row::Ftp => {
                // Threshold zones: where FTP and LTHR put the rider's limit.
                // With the weight set the note also gives W/kg; a card too
                // narrow for both keeps the last part.
                let (low, high) = profile.power_range(4);
                let per_kg = options.rider.watts_per_kg_tenths(profile.ftp_watts.into());
                let _ = write!(note, "Z4 {}-{} W", low, high - 1);
                if let Some(tenths) = per_kg {
                    let _ = write!(note, ", {}.{} W/KG", tenths / 10, tenths % 10);
                }
                write!(value, "{}", profile.ftp_watts)
            }
            Row::Weight => match options.rider.weight_kg() {
                Some(kg) => write!(value, "{kg} KG"),
                None => {
                    let _ = write!(note, "ADD FOR W/KG");
                    write!(value, "NOT SET")
                }
            },
            Row::Age => match options.rider.age_years() {
                Some(years) => write!(value, "{years} YEARS"),
                None => {
                    let _ = write!(note, "ADD FOR CALORIES");
                    write!(value, "NOT SET")
                }
            },
            Row::Sex => match options.rider.sex() {
                Some(Sex::Male) => write!(value, "MALE"),
                Some(Sex::Female) => write!(value, "FEMALE"),
                None => {
                    let _ = write!(note, "ADD FOR CALORIES");
                    write!(value, "NOT SET")
                }
            },
            Row::Threshold => {
                let (low, _) = profile.heart_range(5);
                let _ = write!(note, "Z5 FROM {} BPM", low);
                write!(value, "{}", profile.lthr_bpm)
            }
            Row::Brightness => {
                if options.auto_brightness {
                    write!(value, "AUTO")
                } else {
                    write!(value, "{}%", display.brightness)
                }
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
            Row::Mode => {
                let policy = options.mode.policy();
                let _ = write!(note, "{}", policy.note);
                write!(value, "{}", policy.name)
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
            Row::AutoLap => match options.auto_lap {
                AutoLap::Off => write!(value, "OFF"),
                AutoLap::Every(km) => write!(value, "EVERY {km} KM"),
                AutoLap::AtStart => write!(value, "AT START"),
            },
            Row::Wheel => {
                let in_force = self.readings.speed_wheel_mm();
                let mode = if options.wheel.manual {
                    "MANUAL"
                } else {
                    "AUTO"
                };
                if in_force != u32::from(options.wheel.size_mm()) {
                    let _ = write!(note, "FROM NEXT RIDE");
                } else {
                    let _ = write!(note, "{mode}");
                }
                write!(value, "{} MM", options.wheel.size_mm())
            }
            Row::WheelMode => {
                let _ = write!(
                    note,
                    "{}",
                    if options.wheel.manual {
                        "YOUR SIZE"
                    } else {
                        "FROM GPS"
                    }
                );
                write!(
                    value,
                    "{}",
                    if options.wheel.manual {
                        "MANUAL"
                    } else {
                        "AUTO"
                    }
                )
            }
            Row::MainLeft => write!(value, "{}", options.main_slots.left.name()),
            Row::MainRight => write!(value, "{}", options.main_slots.right.name()),
            Row::PageStats | Row::PageLap | Row::PageClimb | Row::PageMap => {
                let shown = row
                    .ride_page()
                    .is_none_or(|page| options.ride_pages.shows(page));
                if row == Row::PageClimb && !shown {
                    let _ = write!(note, "NO CLIMB POP-UP");
                }
                write!(value, "{}", on(shown))
            }
            Row::Sounds => {
                if !options.sounds {
                    let _ = write!(note, "RADAR STAYS ON");
                }
                write!(value, "{}", on(options.sounds))
            }
            Row::RadarAlerts => {
                let _ = write!(note, "RADAR BEEPS STAY ON");
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
            Row::Form => value.push_str(self.form.summary().as_str()),
            Row::Storage => {
                if self.ride.bulk().status() != Status::Unsupported {
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
    /// What the next frame shows of readings and gliding graphics: none of
    /// them on other pages, the paused screen or a power sheet, the numbers
    /// and gauges only on the main ride page.
    fn motion_visible(&self) -> super::motion::Visible {
        let navigation = &self.navigation;
        let riding = navigation.page() == Page::Ride
            && !(self.ride.active().status() == Status::Paused && !navigation.pause_hidden())
            && self.power.view().is_none();
        super::motion::Visible {
            main: riding && navigation.ride_page() == RidePage::Main,
            radar: riding,
        }
    }

    pub(super) fn policy(&self) -> &'static super::mode::Policy {
        self.preferences.options.mode.policy()
    }

    pub(super) fn latch_numbers(&mut self, now: u64) {
        let (recording, second) = if self.ride.active().status() == Status::Recording {
            (true, self.ride.active_ms(now) / 1000)
        } else {
            (false, now / 1000)
        };
        let moment = Moment {
            recording,
            second,
            page: self.navigation.page(),
            ride_page: self.navigation.ride_page(),
            lap: self.ride.laps().current().number,
            climbing: self.ride.climbs().climbing().is_some(),
        };
        if self.shown.stale(self.policy().numbers, moment) {
            let ride = &self.ride;
            let readout = Readout {
                live: self.readings.live,
                power_3s_watts: self.readings.power_3s_watts,
                totals: *ride.totals(),
                zone_time: *ride.zone_time(),
                lap: ride.laps().current(),
                previous_lap: ride.laps().previous(),
                climbing: ride.climbs().climbing(),
                training: self.shown_workout(&self.preferences.profile),
                paused_secs: self.navigation.paused_since().map_or(0, |since| {
                    (now.saturating_sub(since) / 1000).min(u64::from(u32::MAX)) as u32
                }),
            };
            self.shown.set(readout, moment);
        }
    }

    /// When the readings `shown` or a glide next need a frame.
    pub(super) fn motion_due(&self, shown: &crate::ride::log::Sample, now: u64) -> Option<u64> {
        let profile = self.preferences.profile;
        let visible = self.motion_visible();
        let (main, layout) = (visible.main.then(|| self.composition.main()), self.layout);
        self.motion.due(
            visible,
            shown,
            self.policy().min_motion_frame_gap_ms,
            now,
            |graphics| crate::screens::ride::pixels(graphics, &profile, main, layout),
        )
    }
}

#[cfg(feature = "planted-reading-bug")]
const FORTY_KMH_MM_S: u32 = 11_111;

#[cfg(feature = "planted-reading-bug")]
fn dash_power_when_fast(sample: crate::ride::log::Sample) -> crate::ride::log::Sample {
    let fast = sample.speed_mm_s.is_some_and(|mm_s| mm_s > FORTY_KMH_MM_S);
    crate::ride::log::Sample {
        power_watts: sample.power_watts.filter(|_| !fast),
        ..sample
    }
}

pub(super) fn arrived_sample(readings: &super::readings::Readings) -> crate::ride::log::Sample {
    crate::ride::log::Sample {
        power_watts: readings.power_3s_watts,
        ..readings.live
    }
}

#[derive(Clone, Copy, Default)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub(super) struct Readout {
    live: crate::ride::log::Sample,
    power_3s_watts: Option<u16>,
    totals: crate::ride::totals::Totals,
    zone_time: crate::ride::zones::ZoneTime,
    lap: crate::ride::laps::Lap,
    previous_lap: Option<crate::ride::laps::Lap>,
    climbing: Option<crate::ride::climb::Summary>,
    training: Option<ride::Training>,
    paused_secs: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
struct Moment {
    recording: bool,
    second: u64,
    page: Page,
    ride_page: RidePage,
    lap: u16,
    climbing: bool,
}

#[derive(Default)]
pub(super) struct Shown {
    taken: Option<Moment>,
    readout: Readout,
}

state!(Shown { taken, readout });

impl Shown {
    fn stale(&self, numbers: super::mode::Numbers, moment: Moment) -> bool {
        numbers == super::mode::Numbers::OnArrival || self.taken != Some(moment)
    }

    fn set(&mut self, readout: Readout, moment: Moment) {
        self.readout = readout;
        self.taken = Some(moment);
    }

    pub(super) fn live(&self) -> crate::ride::log::Sample {
        self.readout.live
    }

    pub(super) fn sample(&self) -> crate::ride::log::Sample {
        let sample = crate::ride::log::Sample {
            power_watts: self.readout.power_3s_watts,
            ..self.readout.live
        };
        #[cfg(feature = "planted-reading-bug")]
        let sample = dash_power_when_fast(sample);
        sample
    }

    pub(super) fn readout(&self) -> &Readout {
        &self.readout
    }

    pub(super) fn distance_mm(&self) -> u64 {
        self.readout.totals.distance_mm()
    }
}

/// Radar targets with the times their reports arrived, for gliding markers.
pub(super) fn radar_reports(
    readings: &super::readings::Readings,
    now: u64,
) -> super::motion::Reports {
    super::motion::Reports {
        snapshot: readings.radar.snapshot(now),
        received_ms: readings.radar.received_ms(),
    }
}

fn shown(toast: &Toast) -> &str {
    core::str::from_utf8(toast.text()).unwrap_or("?")
}

/// An alert limit's value: "OFF", or the limit in its unit.
fn limit(value: &mut Text<38>, limit: u16, unit: &str) -> core::fmt::Result {
    match limit {
        0 => write!(value, "OFF"),
        limit => write!(value, "{limit} {unit}"),
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

/// A live saved ANT sensor serves the role of the BLE sensor of `kind`: heart
/// rate always, as the ride prefers it; cadence (from a cadence-only or a
/// combined sensor) only while the BLE sensor is not connected, as the ride
/// prefers that one, and once that sensor's crank has turned, so a combined
/// sensor without a cadence magnet hides nothing.
pub(super) fn ant_stands_for(
    kind: u8,
    ble_connected: bool,
    ant_cadence_turning: bool,
    glance: &[Glance],
) -> bool {
    let live = |kinds: &[u8]| glance.iter().any(|g| g.live && kinds.contains(&g.kind));
    match kind {
        device_type::HEART_RATE => live(&[kind]),
        _ => {
            !ble_connected
                && ant_cadence_turning
                && live(&[device_type::CADENCE, device_type::SPEED_CADENCE])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(
        shown: &mut Shown,
        numbers: super::super::mode::Numbers,
        (recording, second): (bool, u64),
        readout: Readout,
    ) {
        take_at(shown, numbers, at(recording, second), readout);
    }

    fn at(recording: bool, second: u64) -> Moment {
        Moment {
            recording,
            second,
            page: Page::Ride,
            ride_page: RidePage::Main,
            lap: 1,
            climbing: false,
        }
    }

    fn take_at(
        shown: &mut Shown,
        numbers: super::super::mode::Numbers,
        moment: Moment,
        readout: Readout,
    ) {
        if shown.stale(numbers, moment) {
            shown.set(readout, moment);
        }
    }

    fn readout_at(watts: u16) -> Readout {
        let mut readout = Readout::default();
        readout.live.power_watts = Some(watts);
        readout.power_3s_watts = Some(watts);
        readout.paused_secs = u32::from(watts);
        readout
    }

    #[test]
    fn numbers_on_arrival_follow_every_reading_and_every_second_hold_until_it_turns() {
        use super::super::mode::Numbers;
        let mut shown = Shown::default();
        take(&mut shown, Numbers::EverySecond, (true, 5), readout_at(200));
        take(&mut shown, Numbers::EverySecond, (true, 5), readout_at(300));
        assert_eq!(
            shown.sample().power_watts,
            Some(200),
            "held within the second"
        );
        assert_eq!(shown.live().power_watts, Some(200));
        take(&mut shown, Numbers::EverySecond, (true, 6), readout_at(400));
        assert_eq!(
            shown.sample().power_watts,
            Some(400),
            "retaken as the second turns"
        );
        take(&mut shown, Numbers::OnArrival, (true, 6), readout_at(500));
        assert_eq!(shown.sample().power_watts, Some(500));
        take(&mut shown, Numbers::EverySecond, (true, 6), readout_at(600));
        assert_eq!(
            shown.sample().power_watts,
            Some(500),
            "a mode change keeps the second"
        );
        take(
            &mut shown,
            Numbers::EverySecond,
            (false, 6),
            readout_at(700),
        );
        assert_eq!(
            shown.sample().power_watts,
            Some(700),
            "the wall clock is another clock"
        );
    }

    #[test]
    fn a_new_page_lap_or_climb_retakes_the_numbers_within_the_second() {
        use super::super::mode::Numbers;
        let mut shown = Shown::default();
        let main = at(true, 5);
        take_at(&mut shown, Numbers::EverySecond, main, readout_at(200));
        for (moment, watts) in [
            (
                Moment {
                    ride_page: RidePage::Climb,
                    ..main
                },
                300,
            ),
            (
                Moment {
                    ride_page: RidePage::Climb,
                    lap: 2,
                    ..main
                },
                400,
            ),
            (
                Moment {
                    ride_page: RidePage::Climb,
                    lap: 2,
                    climbing: true,
                    ..main
                },
                500,
            ),
        ] {
            take_at(&mut shown, Numbers::EverySecond, moment, readout_at(watts));
            assert_eq!(shown.sample().power_watts, Some(watts));
        }
    }
}
