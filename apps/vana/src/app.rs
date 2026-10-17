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
mod input;
mod preferences;
mod presentation;
mod readings;
mod recording;
mod sensor_links;
#[cfg(test)]
mod tests;

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
    menu: Menu,
    discovery: sensor_links::Discovery,
    links: sensor_links::Links,
    readings: readings::Readings,
    sensors: Client,
    ride: recording::Ride,
    preferences: preferences::Preferences,
}

impl Runtime {
    pub fn new(profile: Profile) -> Self {
        Self {
            navigation: presentation::Navigation::default(),
            display: presentation::Display::default(),
            startup_status: "unknown",
            menu: Menu::new(),
            discovery: sensor_links::Discovery::default(),
            links: sensor_links::Links::default(),
            readings: readings::Readings::default(),
            sensors: Client::new(profile),
            ride: recording::Ride::default(),
            preferences: preferences::Preferences::default(),
        }
    }

    /// Supply the monotonic clock used to measure synchronous recording work.
    pub fn set_clock(&mut self, clock: fn() -> u64) {
        self.ride.clock = Some(clock);
    }

    /// Advance sensor links, take new sensor data and service the ride recorder.
    pub fn tick(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        now: u64,
        ant: &mut impl Ant,
        ble: &mut impl Ble,
        observations: Observations<'_, impl Positioning, impl InputObservation>,
    ) {
        self.service_sensor_links(ant, now);
        self.receive_ant(ant, now);
        self.receive_ble(ble);
        let sample = self.sample(now, ant, &observations);
        self.record(store, now, sample);
    }
}
