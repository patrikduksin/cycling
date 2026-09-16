//! VANA application lifecycle and owned runtime state.
//! Recording advances independently of presentation and terminal requests.
use crate::{
    ride::{
        log::Sample,
        recorder::{Recorder, ResultEvent},
    },
    sensors::{ble::Client, ble_profile::Profile},
};

mod commands;
mod input;
mod presentation;
mod recording;
mod sensor_requests;
#[cfg(test)]
mod tests;

pub struct Runtime {
    clock: Option<fn() -> u64>,
    page: crate::screens::workout::Page,
    home_cursor: bool,
    search_requested: bool,
    discovery_generation: u32,
    scan: crate::sensors::scan::Scan,
    sensor_requests: [Option<sensor_requests::Pending>; device_api::ant::CHANNEL_CAPACITY],
    dropped_ant: [Option<u8>; device_api::ant::CHANNEL_CAPACITY],
    page_since: u64,
    last_press: u64,
    speed: crate::ride::metrics::Speed,
    live: Sample,
    pressure_base: Option<(u32, u64)>,
    distance_mm: u64,
    metric_at: u64,
    ui_message: &'static str,
    radar: crate::sensors::radar::Radar,
    radar_alert: crate::sensors::radar::Alert,
    stop_armed: Option<u64>,
    next_display: u64,
    display_active: bool,
    menu: crate::screens::sensors::Menu,
    startup_status: &'static str,
    next_reconnect: [u64; device_api::ant::CHANNEL_CAPACITY],
    ant_epochs: [Option<(u32, u32)>; 3],
    heart: Option<(crate::sensors::ant::HeartRate, u64)>,
    power: Option<(crate::sensors::ant::Power, u64)>,
    recorder: Recorder,
    sensors: Client,
    next_token: u32,
    pending: Option<u32>,
    completion: Option<ResultEvent>,
}

impl Runtime {
    pub fn new(profile: Profile) -> Self {
        Self {
            clock: None,
            page: crate::screens::workout::Page::Boot,
            home_cursor: false,
            search_requested: false,
            discovery_generation: 0,
            scan: crate::sensors::scan::Scan::default(),
            sensor_requests: [None; device_api::ant::CHANNEL_CAPACITY],
            dropped_ant: [None; device_api::ant::CHANNEL_CAPACITY],
            page_since: 0,
            last_press: 0,
            speed: crate::ride::metrics::Speed::default(),
            live: Sample::default(),
            pressure_base: None,
            distance_mm: 0,
            metric_at: 0,
            ui_message: "READY",
            radar: crate::sensors::radar::Radar::new(),
            radar_alert: crate::sensors::radar::Alert::default(),
            stop_armed: None,
            next_display: 0,
            display_active: true,
            menu: crate::screens::sensors::Menu::new(),
            startup_status: "unknown",
            next_reconnect: [0; device_api::ant::CHANNEL_CAPACITY],
            ant_epochs: [None; 3],
            heart: None,
            power: None,
            recorder: Recorder::default(),
            sensors: Client::new(profile),
            next_token: 1,
            pending: None,
            completion: None,
        }
    }

    /// Supply the monotonic clock used to measure synchronous recording work.
    pub fn set_clock(&mut self, clock: fn() -> u64) {
        self.clock = Some(clock);
    }
}
