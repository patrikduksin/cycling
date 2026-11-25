//! Foreground scheduling, view preparation and radar alerts.
use super::Runtime;
use super::cues::Cue;
use super::ride_control::RideMessage;
use super::view::{Presented, Shows};
use crate::compose::builtin::ScreenId;
use crate::compose::registry::{CHART, TODAY};
use crate::ride::alerts::{Alert, Cue as AlertSound};
use crate::ride::current::Request;
use crate::ride::log::Status;
use crate::ride::medium::Medium;
use crate::screens::chrome::{Battery, CHARGE_STEPS, Gps, Row, Toast, Tone};
use crate::screens::home::{self, Home, Strip, Today};
use crate::screens::look;
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ride::{self, RidePage};
use crate::screens::sensors::{GLANCES, Glance, MenuMessage};
use crate::screens::workout::{LOW_STORAGE_S, Page, Text, View};
use crate::sensors::ant::device_type;
use crate::sensors::radar::Beep;
use crate::workout::shape::Shape;
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

/// The brand stays this long after VANA's first frame, then Home opens. The
/// device keeps the backlight dark until VANA runs, so this is all a rider sees.
pub(super) const BRAND_MS: u64 = 400;
pub(super) const ANIMATED_FRAME_MS: u64 = 500;
/// Motion (the brand reveal, the farewell) draws at this cadence: a frame
/// takes about 25 ms on the C606, so this leaves the loop free between them.
pub(super) const MOTION_FRAME_MS: u64 = 40;
pub(super) const STATIC_FRAME_MS: u64 = 1000;
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
        &mut self,
        system: &mut Shell<D, I, P, B>,
    ) {
        self.compose_shown();
        let mut view = View::new(Page::Boot, 0, None);
        view.device = self.device;
        view.laids = Some(self.composition.laids());
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
            if let Some(alert) = alert {
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
            _ => Tone::Warn,
        };
        self.navigation
            .show(Toast::new(alert.text().as_str(), tone), TOAST_MS, now);
        let cue = match alert.cue() {
            Some(AlertSound::Band) => Cue::BandAlert,
            Some(AlertSound::Fuel) => Cue::Fuel,
            Some(AlertSound::RadarClear) => Cue::RadarClear,
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
                PowerSheet::check_layout(&canvas, self.device);
                let look = look::look(self.light());
                system.draw_rows(|y, pixels| {
                    let mut row = Row::new(y, pixels);
                    PowerSheet::paint(&canvas, &cues, self.device, &mut row.looked(look));
                });
                return Presented::Sheet {
                    ok: !system.display_error,
                };
            }
            Shows::Page { gps, radar_paired } => (gps, radar_paired),
        };
        let course = self.course().is_some();
        let light = self.light();
        let navigation = &self.navigation;
        let page = navigation.page();
        let zone = self.preferences.options.zone;
        let local = self.shown.live().utc_ms.map(|ms| zone.local(ms / 1000));
        let mut view = navigation.view(local);
        view.device = self.device;
        view.now_ms = now;
        view.active_secs = self.ride.active_ms(now) / 1000;
        view.state = self.ride.active().status();
        let sample = self.shown.sample();
        view.power_3s = sample.power_watts;
        view.sample = self.shown.live();
        view.radar_paired = radar_paired;
        view.auto_paused = self.ride.auto().paused();
        view.ride_page = navigation.ride_page();
        view.nav = super::course::sign_of(&self.track, now);
        view.course = course;
        view.laids = Some(self.composition.laids());
        let readout = self.shown.readout();
        let (climbing, paused_secs) = (readout.climbing, readout.paused_secs);
        view.totals = readout.totals;
        view.zone_time = readout.zone_time;
        view.profile = self.preferences.profile;
        view.rider = self.preferences.options.rider;
        view.light = light;
        if page == Page::Ride {
            view.training = readout.training;
            if view.ride_page == RidePage::Workout {
                let laid = self.composition.laids().get(ScreenId::RideWorkout);
                let rect = laid.and_then(|laid| laid.box_of(CHART));
                let theme = crate::compose::data::Data::theme(&view);
                view.show_workout(match (&self.training.player, rect) {
                    (Some(player), Some(rect)) => crate::screens::ride::workout_chart(
                        player,
                        &view.profile,
                        crate::compose::kit::chart::plotted(rect, &self.device.scaled),
                        theme,
                    ),
                    _ => Default::default(),
                });
            }
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
            Page::Format => view.format = self.format.shows(),
            Page::LoseRides => view.unsaved = self.sync.count() as u8,
            Page::Ride | Page::EndRide => {}
            Page::Done => view.show_done(&self.done),
            Page::Home => {
                view.set_home(self.home(local, now));
            }
            Page::Sensors => {
                view.show_sensors(&self.menu);
            }
            Page::Phone => {
                if let Some(shown) = self.phone_shown() {
                    view.show_phone(shown);
                }
            }
        }
        if page == Page::Ride && view.ride_page == RidePage::Climb {
            // Borrowed from the ride and the look-ahead's own block, not copied.
            view.show_climb(crate::screens::ride::ClimbLent {
                climbs: self.ride.climbs(),
                climbing,
                pinned: super::lookahead::pinned_of(&self.lookahead),
            });
        }
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
                view.device,
            ),
        }
    }

    /// The sensors Home and Ready summarise: the saved ANT sensors, then the
    /// selected BLE sensor. A live ANT heart-rate sensor stands for the role,
    /// as the ride prefers it; with none live the rider is on the BLE strap.
    /// The ride prefers BLE cadence, so a live ANT cadence sensor (cadence-only
    /// or combined) stands for the role only while the BLE sensor is not
    /// connected. A failed radio has no strap to speak of.
    pub(super) fn glance(&self, now: u64) -> heapless::Vec<Glance, GLANCES> {
        use crate::sensors::ble_profile::{Link, Profile};
        let mut glance = self.menu.glance();
        let ble = self.sensors.snapshot(now);
        let kind = match ble.profile {
            Profile::HeartRate => device_type::HEART_RATE,
            Profile::Cadence => device_type::CADENCE,
            Profile::Unset => return glance,
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
    fn home(&self, local: Option<crate::clock::Local>, now: u64) -> Home {
        let today = self.training.today.as_ref();
        let card = self.composition.laids().get(ScreenId::HomeToday);
        let card = card.and_then(|laid| laid.box_of(TODAY));
        let strip = match (today, card) {
            (Some(workout), Some(card)) => {
                let rect = crate::compose::tiles::home::strip(card, &self.device.scaled);
                Strip::lay(Shape::of(workout).pieces(), rect)
            }
            _ => Strip::default(),
        };
        Home {
            local,
            sensors: self
                .glance(now)
                .iter()
                .take(home::SENSORS)
                .copied()
                .collect(),
            today: today.map(|workout| Today {
                name: workout.name,
                planned_secs: workout.planned_secs(),
            }),
            strip,
        }
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
        if let Some(notice) = self.power.notice(now)
            && navigation.page() == Page::Home
        {
            why!("toast.power", notice = notice.text());
            return Some(Toast::new(notice.text(), Tone::Alert));
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
        let (main, device) = (visible.main.then(|| self.composition.main()), self.device);
        self.motion.due(
            visible,
            shown,
            self.policy().min_motion_frame_gap_ms,
            now,
            |graphics| crate::screens::ride::pixels(graphics, &profile, main, device),
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
