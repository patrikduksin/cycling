//! VANA application lifecycle and owned runtime state.
//! Recording advances independently of presentation and terminal requests.
use crate::{
    screens::sensors::Menu,
    sensors::{ble::Client, ble_profile::Profile},
};
use device_api::{
    ant::Ant, ble_transport::Ble, input::InputObservation, positioning::Positioning,
    sensors::Snapshot,
};

mod bests;
mod calibration;
mod climb_page;
pub use crate::ride::course::stored::Finder as CourseFinder;
pub use crate::ride::course::turns::{TURNS as COURSE_TURNS, Turn};
pub use crate::ride::course::{Course, POINTS as COURSE_POINTS};
pub use course::{Crumb, MapFrame, MapWork, RideTrack, TRACK_CRUMBS, Timed};
mod commands;
mod course;
mod cues;
mod deleted;
mod facts;
mod form;
mod format;
mod history;
mod input;
pub(crate) mod intent;
#[cfg(any(test, feature = "invariants"))]
mod invariants;
mod lists;
mod lookahead;
mod mark;
mod mode;
mod motion;
mod navigation;
mod night;
mod pace;
mod pass;
pub use lookahead::{TERRAIN_ARENA_BYTES, Terrain};
pub use pace::{STANDBY_DARK_PASS_MS, WAKING_ANT_TYPE};
pub use pass::{Handles, Point, pass};
mod power;
mod preferences;
pub use preferences::{
    Record as PreferencesRecord, Stored as StoredPreferences, decode as decode_preferences,
    encode as encode_preferences,
};
mod presentation;
mod readings;
mod recording;
mod ride_control;
mod seen;
mod setup;
mod storage;
mod sync;
pub use crate::ride::current::Journals;
pub use input::HOLD_GAP_MS;
pub use seen::Seen;
pub use storage::{Health, StorageHealth, Store};
mod composition;
mod phone;
mod sensor_links;
#[cfg(test)]
mod tests;
mod timing;
mod training;
mod view;
pub use phone::{
    Closed as PhoneClosed, Mode as PhoneMode, Status as PhoneStatus, Work as PhoneWork,
};

#[cfg(any(test, feature = "invariants"))]
pub use intent::IDLE_RECORD as INTENT_IDLE_RECORD;
#[cfg(any(test, feature = "invariants"))]
pub use invariants::Broken;

pub use power::{
    AUTO_OFF_MS, AUTO_OFF_WARNING_MS, AutoOff, FAREWELL_MS, Idle, STANDBY_LIT_MS, auto_off,
};

/// Observations recorded with each ride sample, beyond the radios VANA drives.
pub struct Observations<'a, P, I> {
    pub position: &'a P,
    /// UTC from Wi-Fi time synchronization, as the device clock reads now.
    pub network_time: firmware_services::network_time::Snapshot,
    /// Battery level source.
    pub input: &'a I,
    /// Companion sensors, including barometric pressure.
    pub physical: Snapshot,
    /// The onboard sensors the device has fitted. Without a barometer,
    /// pressure is never read, so there is no grade.
    pub fitted: device_api::sensors::SensorSet,
}

pub struct Runtime {
    product: crate::Product,
    navigation: navigation::Navigation,
    phone: phone::Phone,
    display: presentation::Display,
    startup: Option<device_api::sensors::Startup>,
    menu: Menu,
    discovery: sensor_links::Discovery,
    links: sensor_links::Links,
    /// The power meter zeroing in progress; live state, never stored.
    calibration: calibration::Phase,
    readings: readings::Readings,
    sensors: Client,
    ride: crate::ride::current::Ride,
    sync: sync::Sync,
    notices: ride_control::Notices,
    /// The summary's heart-rate recovery, measured after the finish.
    recovery: crate::ride::recovery::Recovery,
    preferences: preferences::Preferences,
    bests: crate::kept::bests::Kept,
    facts: crate::kept::facts::store::Kept,
    deleted: crate::kept::deleted::Hidden,
    form: form::Kept,
    format: format::Format,
    card_probe: Option<storage::StorageHealth>,
    power: power::Power,
    timing: timing::Timing,
    training: training::Training,
    cues: cues::Cues,
    marks: mark::Marks,
    history: history::History,
    night: night::Night,
    /// The look-ahead's state lives in the block composition gives it.
    lookahead: Option<&'static mut lookahead::Terrain>,
    motion: motion::Motion,
    shown: presentation::Shown,
    chart: view::Chart,
    alerts: crate::ride::alerts::Alerts,
    track: Option<&'static mut course::RideTrack>,
    reported: seen::Reported,
    /// This pass's `tick` or `look_ahead` read or wrote storage.
    storage_worked: bool,
    micros_clock: Option<fn() -> u64>,
    latency_clock: Option<fn() -> u64>,
    composition: composition::Composition,
    layout: &'static crate::screens::Layout,
}

#[cfg(not(target_os = "none"))]
impl Runtime {
    pub fn state(&self, out: &mut firmware_services::why::Fields<'_>) {
        out.owner("runtime", self);
    }
}

#[cfg(not(target_os = "none"))]
impl firmware_services::why::State for Runtime {
    fn fields(&self, out: &mut firmware_services::why::Fields<'_>) {
        out.field("startup", &self.startup);
        out.field("card_probe", &self.card_probe);
        out.field("storage_worked", &self.storage_worked);
        out.owner("navigation", &self.navigation);
        out.owner("phone", &self.phone);
        out.owner("display", &self.display);
        out.owner("sensor_menu", &self.menu);
        out.owner("sensor_discovery", &self.discovery);
        out.owner("sensor_links", &self.links);
        out.owner("calibration", &self.calibration);
        out.owner("readings", &self.readings);
        out.owner("ble", &self.sensors);
        out.owner("ride", &self.ride);
        out.owner("sync", &self.sync);
        out.owner("notices", &self.notices);
        out.owner("recovery", &self.recovery);
        out.owner("preferences", &self.preferences);
        out.owner("bests", &self.bests);
        out.owner("facts", &self.facts);
        out.owner("deleted", &self.deleted);
        out.owner("form", &self.form);
        out.owner("format", &self.format);
        out.owner("power", &self.power);
        out.owner("timing", &self.timing);
        out.owner("training", &self.training);
        out.owner("cues", &self.cues);
        out.owner("marks", &self.marks);
        out.owner("history", &self.history);
        out.owner("night", &self.night);
        if let Some(lookahead) = &self.lookahead {
            out.owner("lookahead", &**lookahead);
        }
        out.owner("motion", &self.motion);
        out.owner("shown", &self.shown);
        out.owner("chart", &self.chart);
        out.owner("alerts", &self.alerts);
        if let Some(track) = &self.track {
            out.owner("track", &**track);
        }
        out.owner("reported", &self.reported);
        out.owner("composition", &self.composition);
    }
}

impl Runtime {
    pub fn new(
        profile: Profile,
        product: crate::Product,
        layout: &'static crate::screens::Layout,
    ) -> Self {
        Self {
            product,
            navigation: navigation::Navigation::default(),
            phone: phone::Phone::default(),
            display: presentation::Display::default(),
            startup: None,
            menu: Menu::new(layout),
            discovery: sensor_links::Discovery::default(),
            links: sensor_links::Links::default(),
            calibration: calibration::Phase::Idle,
            readings: readings::Readings::default(),
            sensors: Client::new(profile),
            ride: crate::ride::current::Ride::default(),
            sync: sync::Sync::default(),
            notices: ride_control::Notices::default(),
            recovery: crate::ride::recovery::Recovery::default(),
            preferences: preferences::Preferences::default(),
            bests: crate::kept::bests::Kept::default(),
            facts: crate::kept::facts::store::Kept::default(),
            deleted: crate::kept::deleted::Hidden::default(),
            form: form::Kept::default(),
            format: format::Format::default(),
            card_probe: None,
            power: power::Power::default(),
            timing: timing::Timing::default(),
            training: training::Training::default(),
            cues: cues::Cues::default(),
            marks: mark::Marks::default(),
            history: history::History::default(),
            night: night::Night::default(),
            lookahead: None,
            motion: motion::Motion::default(),
            shown: presentation::Shown::default(),
            chart: view::Chart::default(),
            alerts: crate::ride::alerts::Alerts::default(),
            track: None,
            reported: seen::Reported::default(),
            storage_worked: false,
            micros_clock: None,
            latency_clock: None,
            composition: composition::Composition::new(layout),
            layout,
        }
    }

    /// The rider's alert choices, from the preferences record; the defaults
    /// if the stored bytes do not decode.
    pub fn alert_settings(&self) -> crate::ride::alerts::Settings {
        crate::ride::alerts::Settings::decode(&self.preferences.options.alerts).unwrap_or_default()
    }

    /// Use these alert choices and save them once edits settle. Invalid
    /// settings are refused and the current ones kept.
    pub fn set_alert_settings(
        &mut self,
        settings: crate::ride::alerts::Settings,
        now: u64,
    ) -> bool {
        let valid = settings.valid();
        if valid {
            let mut options = self.preferences.options;
            options.alerts = settings.encode();
            self.set_options(options, now);
        }
        valid
    }

    /// Take the light look or the dark one from the next pass, as the THEME
    /// setting does, and save it once edits settle.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_light(&mut self, light: bool, now: u64) {
        let mut options = self.preferences.options;
        options.theme = if light {
            preferences::Theme::Light
        } else {
            preferences::Theme::Dark
        };
        self.set_options(options, now);
        self.display.redraw();
    }

    /// Supply the monotonic clock that times synchronous work within a pass:
    /// journal writes and erases (`RIDE STATUS write_ms`), and frames, storage
    /// and recording milestones (`RIDE TIMING`, `sdk.timing` logs). Nothing a
    /// rider sees or the journal holds reads it, so a replay need not.
    pub fn set_latency_clock(&mut self, clock: fn() -> u64) {
        self.latency_clock = Some(clock);
    }

    /// Supply a monotonic microsecond clock that times the map's fills and
    /// road extractions (`take_map_work`).
    pub fn set_micros_clock(&mut self, clock: fn() -> u64) {
        self.micros_clock = Some(clock);
    }

    /// The map's work since the last take; none without a track.
    pub fn take_map_work(&mut self) -> Option<MapWork> {
        Some(core::mem::take(&mut self.track.as_deref_mut()?.work))
    }

    /// The map's work since the track was attached.
    pub fn map_work_since_attached(&self) -> Option<MapWork> {
        Some(self.track.as_deref()?.since_attached)
    }

    /// Scan both ride journals ahead of the first `tick`, while the device
    /// is still starting, so Home opens with storage ready. Does nothing once
    /// the scans are done or while FORMAT owns storage; formatting, recovery
    /// and recording wait for `tick`.
    pub fn prepare(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
        now: u64,
    ) {
        use crate::ride::log::Status;
        if self.format.stops_own_storage() {
            return;
        }
        if self.ride.internal().status() == Status::Scanning
            || self.ride.bulk().status() == Status::Scanning
        {
            // Wrapped as `tick` wraps them, so `record` is built once.
            self.record(
                &mut pace::Touched::new(store),
                &mut pace::Touched::new(bulk),
                now,
                crate::ride::log::Sample::default(),
            );
        }
    }

    fn note_milestones(&mut self, now: u64, ant: &impl Ant, sample: &crate::ride::log::Sample) {
        use device_api::sensors::Startup;
        if matches!(
            self.startup,
            Some(Startup::Ready | Startup::AlreadyRunning | Startup::RunningDegraded)
        ) {
            self.timing.companion(now);
        }
        if sample.location_e7.is_some() {
            self.timing.gnss_fix(now);
        }
        for channel in ant.channels(now).into_iter().flatten() {
            if channel.link == device_api::ant::LinkState::Connected
                && !channel.stale
                && let Some(peer) = channel.selected
            {
                self.timing.sensor(peer.device_type, now);
            }
        }
        let ble = self.sensors.snapshot(now);
        if ble.heart_bpm.is_some() {
            self.timing.sensor(timing::BLE_HEART, now);
        }
        if ble.cadence_tenths.is_some() {
            self.timing.sensor(timing::BLE_CADENCE, now);
        }
    }

    /// The page VANA shows, for composition and scenario checks.
    pub fn page(&self) -> crate::screens::workout::Page {
        self.navigation.page()
    }

    /// Advance sensor links, take new sensor data and service both ride
    /// recorders: the internal journal in `store` and the log on `bulk`.
    pub fn tick(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
        now: u64,
        ant: &mut impl Ant,
        ble: &mut impl Ble,
        observations: Observations<'_, impl Positioning, impl InputObservation>,
    ) {
        let (store, bulk) = (
            &mut pace::Touched::new(store),
            &mut pace::Touched::new(bulk),
        );
        self.service_sensor_links(ant, now);
        self.receive_ant(ant, now);
        self.service_calibration(ant, now);
        self.receive_ble(ble);
        let sample = self.sample(now, ant, &observations);
        self.auto_control(now);
        let recording = self.ride.active().status() == crate::ride::log::Status::Recording;
        let heart = self.live_heart_bpm(now);
        self.recovery.track(now, recording, heart);
        let power_status = observations
            .input
            .snapshot(now)
            .map_or(device_api::observation::Observation::Unavailable, |s| {
                s.power
            });
        let ble = self.sensors.snapshot(now);
        let live_sensors = ble.heart_bpm.is_some()
            || ble.cadence_tenths.is_some()
            || matches!(
                self.readings.radar.snapshot(now).status,
                crate::sensors::radar::Status::Ready
                    | crate::sensors::radar::Status::ReservedThreat
            )
            || ant.channels(now).into_iter().flatten().any(|channel| {
                channel.link == device_api::ant::LinkState::Connected && !channel.stale
            });
        self.power.observe(now, &sample, live_sensors, power_status);
        let charging = match power_status {
            device_api::observation::Observation::Fresh { value, .. }
            | device_api::observation::Observation::Stale { value, .. } => Some(value == 0),
            device_api::observation::Observation::Unavailable => None,
        };
        // A stale report is no news about the discharge: skipping it lets the
        // estimate lapse after `STALE_MS` without forgetting what it learned.
        let battery_fresh = observations.input.snapshot(now).is_some_and(|s| {
            matches!(
                s.battery,
                device_api::observation::Observation::Fresh { .. }
            )
        });
        if battery_fresh || charging == Some(true) {
            self.readings
                .battery_life
                .observe(now, sample.battery_percent, charging);
        }
        if let Some(news) = self
            .readings
            .battery
            .observe(sample.battery_percent, charging)
        {
            self.readings.battery_news_ms = Some(now);
            self.navigation
                .show(news.toast(), presentation::BATTERY_TOAST_MS, now);
            if news.warning() {
                self.cue(cues::Cue::LowBattery, now);
            }
            self.display.redraw();
        }
        self.note_milestones(now, ant, &sample);
        if self.format.stops_own_storage() {
            return;
        }
        self.record(store, bulk, now, sample);
        self.replay_step(store, bulk);
        self.service_workouts(bulk);
        self.service_bests(bulk, now);
        self.service_facts(bulk, now);
        self.service_deleted(bulk, now);
        self.service_course(bulk, now);
        self.directory_step(store, bulk);
        self.service_form();
        self.observe_track(store, bulk, now);
        self.follow_road();
        self.storage_worked = store.touched || bulk.touched;
    }
}
