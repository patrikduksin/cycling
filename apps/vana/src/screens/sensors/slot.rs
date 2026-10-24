//! A saved sensor tile: the rider's selection and what VANA last observed
//! while connecting it automatically.
use crate::screens::workout::Text;
use crate::sensors::ant::Battery;
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
    Replacing,
    /// The radio cannot connect sensors until it recovers.
    RadioUnavailable,
    /// The radio does not support this device type.
    Unsupported,
}

impl Connection {
    pub const fn text(self) -> &'static str {
        match self {
            Self::Live => "CONNECTED",
            Self::WaitingForData => "WAITING FOR DATA",
            Self::DataLost => "DATA LOST",
            Self::Searching => "SEARCHING",
            Self::NotFound => "NOT FOUND YET",
            Self::WaitingForRadio => "WAITING FOR RADIO",
            Self::Replacing => "REPLACING",
            Self::RadioUnavailable => "RADIO UNAVAILABLE",
            Self::Unsupported => "TYPE UNSUPPORTED",
        }
    }

    pub(super) const fn status(self) -> LinkStatus {
        match self {
            Self::Live => LinkStatus::Live,
            Self::WaitingForData | Self::DataLost => LinkStatus::Check,
            Self::Searching | Self::WaitingForRadio | Self::Replacing => LinkStatus::Waiting,
            Self::NotFound => LinkStatus::Missing,
            Self::RadioUnavailable | Self::Unsupported => LinkStatus::Failed,
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
}

impl LinkStatus {
    /// Prefix used when the status line also needs the peer's identity.
    pub(super) const fn prefix(self) -> &'static [u8] {
        match self {
            Self::Live => b"LIVE / ",
            Self::Waiting => b"WAIT / ",
            Self::Missing => b"NOT FOUND / ",
            Self::Failed => b"FAILED / ",
            Self::Check => b"CHECK / ",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selected {
    pub identity: Identity,
    pub connection: Connection,
    /// What the sensor last reported about its battery, if it has.
    pub battery: Option<Battery>,
}

impl Selected {
    /// The caption for the detail page, such as "BATTERY LOW  2.7 V".
    pub(super) fn battery_text(self) -> Option<Text<24>> {
        let battery = self.battery?;
        let mut text = Text::<24>::new();
        let _ = write!(text, "BATTERY {}", battery.status.text());
        if let Some(millivolts) = battery.millivolts {
            let tenths = (millivolts + 50) / 100;
            let _ = write!(text, "  {}.{} V", tenths / 10, tenths % 10);
        }
        Some(text)
    }

    pub(super) const fn link_status(self) -> LinkStatus {
        self.connection.status()
    }

    /// Detailed wording for the focused sensor.
    pub(super) const fn status_text(self) -> &'static [u8] {
        self.connection.text().as_bytes()
    }
}
