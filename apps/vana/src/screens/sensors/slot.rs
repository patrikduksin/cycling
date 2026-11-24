//! A saved sensor tile: the rider's selection and what VANA last observed
//! while connecting it automatically.
use crate::screens::theme::Tone;
use crate::screens::workout::Text;
use crate::sensors::ant::{Battery, BatteryStatus, Product};
use core::fmt::Write;
use device_api::ant::Identity;

/// Observed connection progress of a saved sensor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Connection {
    /// Connected with fresh data.
    Live,
    /// Connected; no data has arrived yet.
    WaitingForData,
    /// Connected, but data stopped arriving.
    DataLost,
    /// The radio is searching for this sensor.
    Searching,
    /// The last search found nothing; another follows automatically.
    NotFound,
    /// The radio is starting, discovering or busy; connecting follows.
    WaitingForRadio,
    /// The previous sensor of this type is closing first.
    Switching,
    /// The radio cannot connect sensors until it recovers.
    RadioUnavailable,
    /// The radio does not support this device type.
    Unsupported,
    Standby,
    NotInRange,
}

impl Connection {
    pub const ALL: [Self; 11] = [
        Self::Live,
        Self::WaitingForData,
        Self::DataLost,
        Self::Searching,
        Self::NotFound,
        Self::WaitingForRadio,
        Self::Switching,
        Self::RadioUnavailable,
        Self::Unsupported,
        Self::Standby,
        Self::NotInRange,
    ];

    pub const fn text(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::WaitingForData => "Waiting for data",
            Self::DataLost => "Data lost",
            Self::Searching => "Searching",
            Self::NotFound => "Not found yet",
            Self::WaitingForRadio => "Waiting for radio",
            Self::Switching => "Switching",
            Self::RadioUnavailable => "Radio unavailable",
            Self::Unsupported => "Type unsupported",
            Self::Standby => "In range",
            Self::NotInRange => "Not in range",
        }
    }

    pub const fn in_use(self) -> bool {
        !matches!(self, Self::Standby | Self::NotInRange)
    }

    pub(super) const fn status(self) -> LinkStatus {
        match self {
            Self::Live => LinkStatus::Live,
            Self::WaitingForData | Self::DataLost => LinkStatus::Check,
            Self::Searching | Self::WaitingForRadio | Self::Switching => LinkStatus::Waiting,
            Self::NotFound => LinkStatus::Missing,
            Self::RadioUnavailable | Self::Unsupported => LinkStatus::Failed,
            Self::Standby | Self::NotInRange => LinkStatus::Idle,
        }
    }
}

/// What the rider should conclude about a saved sensor at a glance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkStatus {
    /// Connected with fresh data.
    Live,
    /// Connecting or waiting for the radio.
    Waiting,
    /// Not found by the last search; retrying automatically.
    Missing,
    /// The radio cannot connect it.
    Failed,
    /// Connected, but data is missing or stale.
    Check,
    Idle,
}

impl LinkStatus {
    pub(super) const fn tone(self) -> Tone {
        match self {
            Self::Live => Tone::Good,
            Self::Waiting | Self::Missing | Self::Check => Tone::Warn,
            Self::Failed => Tone::Bad,
            Self::Idle => Tone::Sub,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selected {
    pub identity: Identity,
    pub connection: Connection,
    /// What the sensor last reported about its battery, if it has.
    pub battery: Option<Battery>,
    pub product: Product,
    /// What it measures itself, in its `Reading`'s unit, while it is the
    /// live sensor of its kind.
    pub reading: Option<i32>,
}

impl Selected {
    /// The battery's voltage to a tenth, such as "2.7", when it reported one.
    pub fn volts(self) -> Option<Text<8>> {
        let millivolts = self.battery?.millivolts?;
        let tenths = (millivolts + 50) / 100;
        let mut text = Text::new();
        let _ = write!(text, "{}.{}", tenths / 10, tenths % 10);
        Some(text)
    }

    pub(super) const fn link_status(self) -> LinkStatus {
        self.connection.status()
    }

    pub const fn live(self) -> bool {
        matches!(self.link_status(), LinkStatus::Live)
    }

    pub const fn state(self) -> (&'static str, Tone) {
        (self.connection.text(), self.link_status().tone())
    }

    pub const fn battery_word(self) -> Option<(&'static str, Tone)> {
        let Some(battery) = self.battery else {
            return None;
        };
        Some(match battery.status {
            BatteryStatus::New => ("New", Tone::Text),
            BatteryStatus::Good => ("Good", Tone::Text),
            BatteryStatus::Ok => ("OK", Tone::Text),
            BatteryStatus::Low => ("Low", Tone::Warn),
            BatteryStatus::Critical => ("Critical", Tone::Bad),
        })
    }

    pub const fn battery_warning(self) -> Option<(&'static str, Tone)> {
        match self.battery {
            Some(battery) if battery.status.is_low() => self.battery_word(),
            _ => None,
        }
    }
}
