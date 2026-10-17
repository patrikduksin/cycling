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

mod commands;
mod cues;
mod history;
mod input;
mod lookahead;
mod night;
pub use lookahead::{TERRAIN_ARENA_BYTES, Terrain};
mod power;
mod preferences;
mod presentation;
mod readings;
mod recording;
pub use recording::Medium;
mod sensor_links;
#[cfg(test)]
mod tests;
mod timing;
mod training;

pub use power::{
    AUTO_OFF_MS, AUTO_OFF_WARNING_MS, AutoOff, FAREWELL_MS, Idle, STANDBY_LIT_MS, auto_off,
};

/// Observations recorded with each ride sample, beyond the radios VANA drives.
pub struct Observations<'a, P, I> {
    pub position: &'a P,
    pub network_online: bool,
    /// Battery level source.
    pub input: &'a I,
    /// Companion sensors, including barometric pressure.
    pub physical: Snapshot,
}

pub struct Runtime {
    navigation: presentation::Navigation,
    display: presentation::Display,
    startup_status: &'static str,
    startup: device_api::sensors::Startup,
    menu: Menu,
    discovery: sensor_links::Discovery,
    links: sensor_links::Links,
    readings: readings::Readings,
    sensors: Client,
    ride: recording::Ride,
    preferences: preferences::Preferences,
    power: power::Power,
    timing: timing::Timing,
    training: training::Training,
    cues: cues::Cues,
    history: history::History,
    night: night::Night,
    /// The look-ahead's state lives in the block composition gives it.
    lookahead: Option<&'static mut lookahead::Terrain>,
}

impl Runtime {
    pub fn new(profile: Profile) -> Self {
        Self {
            navigation: presentation::Navigation::default(),
            display: presentation::Display::default(),
            startup_status: "unknown",
            startup: device_api::sensors::Startup::Unsupported,
            menu: Menu::new(),
            discovery: sensor_links::Discovery::default(),
            links: sensor_links::Links::default(),
            readings: readings::Readings::default(),
            sensors: Client::new(profile),
            ride: recording::Ride::default(),
            preferences: preferences::Preferences::default(),
            power: power::Power::default(),
            timing: timing::Timing::default(),
            training: training::Training::default(),
            cues: cues::Cues::default(),
            history: history::History::default(),
            night: night::Night::default(),
            lookahead: None,
        }
    }

    /// Supply the monotonic clock used to measure synchronous recording work.
    pub fn set_clock(&mut self, clock: fn() -> u64) {
        self.ride.clock = Some(clock);
    }

    /// Scan both ride journals ahead of the first `tick`, while the device
    /// is still starting, so Home opens with storage ready. Does nothing once
    /// the scans are done; formatting, recovery and recording wait for `tick`.
    pub fn prepare(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
        now: u64,
    ) {
        use crate::ride::log::Status;
        if self.ride.internal.status() == Status::Scanning
            || self.ride.bulk.status() == Status::Scanning
        {
            self.record(store, bulk, now, crate::ride::log::Sample::default());
        }
    }

    fn note_milestones(&mut self, now: u64, ant: &impl Ant, sample: &crate::ride::log::Sample) {
        use device_api::sensors::Startup;
        if matches!(
            self.startup,
            Startup::Ready | Startup::AlreadyRunning | Startup::RunningDegraded
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
        self.navigation.page
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
        self.service_sensor_links(ant, now);
        self.receive_ant(ant, now);
        self.receive_ble(ble);
        let sample = self.sample(now, ant, &observations);
        self.auto_control(now);
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
        if let Some(news) = self
            .readings
            .battery
            .observe(sample.battery_percent, charging)
        {
            self.navigation
                .show(news.toast(), presentation::BATTERY_TOAST_MS, now);
            if news.warning() {
                self.cue(cues::Cue::LowBattery, now);
            }
            self.display.redraw();
        }
        self.note_milestones(now, ant, &sample);
        self.record(store, bulk, now, sample);
        self.replay_step(store, bulk);
    }
}
