//! Sensor selection and physical-button navigation, independent of radio scheduling.
//!
//! `menu` owns the pages, their focus and input, `shown` is what their
//! trees (`compose::builtin::sensors`) read and where a tap lands, and
//! `render` holds their colours.
mod menu;
mod message;
mod render;
mod shown;
mod slot;
#[cfg(test)]
pub(super) mod tests;

pub use menu::{Action, Diagnostics, GLANCES, Glance, Menu, Page, SETTLE_MS};
pub use message::{CalibrationNote, MenuMessage};

pub use shown::{Item, widest};
pub use slot::{Connection, Selected};

use crate::sensors::ant::device_type;

/// Every name `kind` gives.
pub const NAMES: [&[u8]; 10] = [
    b"RADAR",
    b"POWER METER",
    b"HEART RATE",
    b"SPEED/CAD",
    b"CADENCE",
    b"SPEED",
    b"SHIFTING",
    b"TRAINER",
    b"BIKE LIGHTS",
    b"ANT SENSOR",
];

/// What a sensor of this ANT device type is called.
pub fn kind(kind: u8) -> &'static [u8] {
    match kind {
        device_type::RADAR => b"RADAR",
        device_type::POWER => b"POWER METER",
        device_type::HEART_RATE => b"HEART RATE",
        device_type::SPEED_CADENCE => b"SPEED/CAD",
        device_type::CADENCE => b"CADENCE",
        device_type::SPEED => b"SPEED",
        device_type::SHIFTING | device_type::SHIFTING_DI2 => b"SHIFTING",
        device_type::FITNESS_EQUIPMENT => b"TRAINER",
        device_type::BIKE_LIGHTS => b"BIKE LIGHTS",
        _ => b"ANT SENSOR",
    }
}
