//! Sensor selection and physical-button navigation, independent of radio scheduling.
//!
//! `menu` owns selection state and input, `layout` derives text and tile
//! placement after each change, and `render` paints that layout row by row.
mod icons;
mod layout;
mod menu;
mod message;
mod render;
mod slot;
#[cfg(test)]
mod tests;

pub(crate) use icons::row as icon_row;
pub use menu::{Action, Diagnostics, GLANCES, Glance, Menu};
pub use message::MenuMessage;

pub use slot::{Connection, Selected};

use crate::sensors::ant::device_type;

fn kind(kind: u8) -> &'static [u8] {
    match kind {
        device_type::RADAR => b"RADAR",
        device_type::POWER => b"POWER METER",
        device_type::HEART_RATE => b"HEART RATE",
        device_type::SPEED_CADENCE => b"SPEED/CAD",
        device_type::CADENCE => b"CADENCE",
        device_type::SPEED => b"SPEED SENSOR",
        device_type::SHIFTING | device_type::SHIFTING_DI2 => b"SHIFTING",
        device_type::FITNESS_EQUIPMENT => b"FITNESS",
        device_type::BIKE_LIGHTS => b"BIKE LIGHTS",
        _ => b"ANT SENSOR",
    }
}
