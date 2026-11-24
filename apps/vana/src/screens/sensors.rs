//! Sensor selection and physical-button navigation, independent of radio scheduling.
mod menu;
mod message;
mod shown;
mod slot;
#[cfg(test)]
pub(super) mod tests;

pub use menu::{Action, Diagnostics, GLANCES, Glance, Menu, Page, SETTLE_MS};
pub use message::{CalibrationNote, MenuMessage};
pub use shown::{List, Part, Reading, Row, Search, reading, widest};
pub use slot::{Connection, Selected};

use crate::sensors::ant::device_type;

/// What a sensor of one ANT device type is called: in a list's row, and
/// at the top of its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Names {
    pub short: &'static str,
    pub long: &'static str,
}

const fn named(short: &'static str, long: &'static str) -> Names {
    Names { short, long }
}

pub const NAMES: [Names; 10] = [
    named("Radar", "Radar"),
    named("Power", "Power meter"),
    named("Heart rate", "Heart rate"),
    named("Speed/cad", "Speed/cadence"),
    named("Cadence", "Cadence sensor"),
    named("Speed", "Speed sensor"),
    named("Shifting", "Shifting"),
    named("Trainer", "Trainer"),
    named("Lights", "Bike lights"),
    named("Sensor", "Sensor"),
];

pub const fn names(kind: u8) -> Names {
    NAMES[match kind {
        device_type::RADAR => 0,
        device_type::POWER => 1,
        device_type::HEART_RATE => 2,
        device_type::SPEED_CADENCE => 3,
        device_type::CADENCE => 4,
        device_type::SPEED => 5,
        device_type::SHIFTING | device_type::SHIFTING_DI2 => 6,
        device_type::FITNESS_EQUIPMENT => 7,
        device_type::BIKE_LIGHTS => 8,
        _ => 9,
    }]
}
