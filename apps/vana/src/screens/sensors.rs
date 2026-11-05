//! Sensor selection and physical-button navigation, independent of radio scheduling.
//!
//! `menu` owns the pages, their focus and input, `layout` places the shown
//! page after each change and draws it onto the frame's canvas, and
//! `render` holds its colours.
mod layout;
mod menu;
mod message;
mod render;
mod slot;
#[cfg(test)]
mod tests;

pub use menu::{Action, Diagnostics, GLANCES, Glance, Menu, Page, SETTLE_MS};
pub use message::{CalibrationNote, MenuMessage};

pub use slot::{Connection, Selected};

use crate::screens::cue::{Corner, Cue};
use crate::sensors::ant::device_type;

/// A sensor page with a back cue takes the whole panel: the status bar
/// would cut the cue.
pub fn full_screen(cues: &[Cue]) -> bool {
    cues.iter().any(|cue| cue.corner == Corner::TopLeft)
}

fn kind(kind: u8) -> &'static [u8] {
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
