//! Sensor selection and physical-button navigation, independent of radio scheduling.
//!
//! `menu` owns selection state and input, `layout` derives text and tile
//! placement after each change, and `render` draws that layout per pixel.
mod icons;
mod layout;
mod menu;
mod message;
mod render;
mod slot;
#[cfg(test)]
mod tests;

pub use menu::{Action, Diagnostics, Menu};
pub use message::MenuMessage;

use crate::sensors::ant::device_type;
use device_api::ant::{LinkState, Snapshot};

pub(crate) fn connection_status(channel: Snapshot) -> &'static str {
    match channel.link {
        LinkState::Idle => "NOT CONNECTED",
        LinkState::Connecting => "CONNECTING",
        LinkState::Connected if channel.age_ms.is_none() => "WAITING FOR DATA",
        LinkState::Connected if channel.stale => "DATA LOST",
        LinkState::Connected => "CONNECTED",
        LinkState::Disconnecting => "DISCONNECTING",
        LinkState::Disconnected => "DISCONNECTED",
        LinkState::TimedOut => "CONNECT FAILED",
        LinkState::TransportLost => "RADIO UNAVAILABLE",
    }
}

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
