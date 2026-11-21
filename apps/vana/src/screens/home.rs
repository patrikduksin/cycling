//! Home: one big RIDE card, the sensors at a glance, TRAIN (workouts) and
//! settings side by side, and the saved rides with the last one's numbers.
//!
//! MOVE moves focus down the cards, ACT opens the focused one and a tap
//! opens the card under the finger. This is what the cards show; the Home
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
    /// The saved rides summed up, such as the last one's "5.00 KM".
    pub last: Text<20>,
}

impl Home {
    pub fn new(gps_fix: bool, glance: &[Glance], last: Text<20>) -> Self {
        Self {
            gps_fix,
            glance: glance.iter().take(ICONS).copied().collect(),
            paired: glance.len() as u8,
            live: glance.iter().filter(|g| g.live).count() as u8,
            battery_low: glance.iter().any(|g| g.live && g.battery_low()),
            status: Text::new(),
            status_color: SECONDARY,
            last,
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

/// "5.00 KM": the last ride's distance, beside RIDES, up to the 9999.9 km
/// a distance shows.
pub fn last_ride(distance_mm: u64) -> Text<20> {
    let mut km = Text::<12>::new();
    super::ride::kilometres(&mut km, distance_mm.min(9_999_900_000));
    let mut line = Text::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{} KM", km.as_str()));
    line
}

/// "WEEK 84 KM": the week's distance, beside RIDES (THIS WEEK is too wide),
/// up to 9999 km.
pub fn this_week(distance_mm: u64) -> Text<20> {
    let km = (distance_mm.saturating_add(500_000) / 1_000_000).min(9999);
    let mut line = Text::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("WEEK {km} KM"));
    line
}

/// "12 SAVED": how many rides are saved, beside RIDES, up to 99999.
pub fn saved(rides: usize) -> Text<20> {
    let mut line = Text::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{} SAVED", rides.min(99_999)));
    line
}

/// What RIDES says before the rides are counted, and with none saved.
pub const UNCOUNTED: &str = "...";
pub const NONE_SAVED: &str = "NONE YET";
