//! MOVE moves focus down the cards; ACT or a tap acts on a card: RIDE
//! starts the ride, the others open their page. This is what the cards show; the Home
//! screen's tree (`compose::builtin::home`) lays them out.
use super::chrome::{AMBER, SECONDARY};
use super::sensors::Glance;
use super::workout::Text;
use heapless::Vec;

/// Up to four sensors show, each an icon with its short name under it; the
/// count beside the title covers the rest.
pub const ICONS: usize = 4;

/// What Home shows, prepared once per frame.
#[derive(Clone, Default)]
pub struct Home {
    pub gps_fix: bool,
    /// The first sensors, which show as icons.
    pub glance: Vec<Glance, ICONS>,
    /// Every sensor the count covers, and whether a live one is low.
    pub paired: u8,
    pub live: u8,
    pub battery_low: bool,
    /// What is still starting, or why turning off did not happen, and its
    /// colour.
    pub status: Text<38>,
    pub status_color: u16,
}

impl Home {
    pub fn new(gps_fix: bool, glance: &[Glance]) -> Self {
        Self {
            gps_fix,
            glance: glance.iter().take(ICONS).copied().collect(),
            paired: glance.len() as u8,
            live: glance.iter().filter(|g| g.live).count() as u8,
            battery_low: glance.iter().any(|g| g.live && g.battery_low()),
            status: Text::new(),
            status_color: SECONDARY,
        }
    }

    /// The line under the start button: READY TO RIDE, what is still
    /// starting, or a problem in amber.
    pub fn set_status(&mut self, text: &[u8], problem: bool) {
        self.set_status_in(text, if problem { AMBER } else { SECONDARY });
    }

    /// The status line in a colour of its own, such as red for a critical
    /// battery.
    pub fn set_status_in(&mut self, text: &[u8], color: u16) {
        self.status = Text::new();
        let _ = self
            .status
            .push_str(core::str::from_utf8(text).unwrap_or(""));
        self.status_color = color;
    }
}

/// A sensor's name short enough to sit under its icon on Home.
pub(crate) fn short_name(kind: u8) -> &'static [u8] {
    use crate::sensors::ant::device_type;
    match kind {
        device_type::RADAR => b"RDR",
        device_type::POWER => b"PWR",
        device_type::HEART_RATE => b"HR",
        device_type::SPEED_CADENCE => b"SPD/C",
        device_type::CADENCE => b"CAD",
        device_type::SPEED => b"SPD",
        device_type::SHIFTING | device_type::SHIFTING_DI2 => b"SHIFT",
        device_type::FITNESS_EQUIPMENT => b"FIT",
        device_type::BIKE_LIGHTS => b"LIGHT",
        _ => b"ANT",
    }
}
