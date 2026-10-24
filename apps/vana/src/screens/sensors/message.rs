//! Page feedback shown on the sensor screen.

/// Sensor-page feedback from discovery, startup and rider requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuMessage {
    PickSensor,
    SearchPending,
    StartingScan,
    Scanning,
    StoppingScan,
    ScanCancelled,
    FinishingScan,
    ScanDone,
    ScanFailed,
    ScanUnavailable,
    RadioLost,
    RadioUnavailable,
    StartingSensors,
    WaitingForCompanion,
    StartupFailed,
    ScanThenPick,
    InputLost,
    NotSeen,
    SlotsFull,
    /// A sensor change could not be saved; saving retries automatically.
    NotSaved,
    /// Saved sensors could not be read, so changes are kept until restart only.
    SavedUnreadable,
}

impl MenuMessage {
    /// Full wording, used where the message has room of its own.
    pub const fn text(self) -> &'static str {
        match self {
            Self::PickSensor => "PICK SENSOR",
            Self::SearchPending => "SEARCH PENDING",
            Self::StartingScan => "STARTING SCAN",
            Self::Scanning => "SCANNING...",
            Self::StoppingScan => "STOPPING SCAN",
            Self::ScanCancelled => "SCAN CANCELLED",
            Self::FinishingScan => "FINISHING SCAN",
            Self::ScanDone => "SCAN DONE - PICK",
            Self::ScanFailed => "SCAN FAILED - RETRY",
            Self::ScanUnavailable => "SCAN UNAVAILABLE",
            Self::RadioLost => "RADIO LOST - REBOOT",
            Self::RadioUnavailable => "RADIO UNAVAILABLE",
            Self::StartingSensors => "STARTING SENSORS",
            Self::WaitingForCompanion => "WAITING FOR COMPANION",
            Self::StartupFailed => "STARTUP FAILED",
            Self::ScanThenPick => "SCAN THEN PICK SENSOR",
            Self::InputLost => "INPUT LOST - PRESS NEXT",
            Self::NotSeen => "NOT SEEN - SEARCH AGAIN",
            Self::SlotsFull => "ALL TEN SENSOR SLOTS ARE IN USE",
            Self::NotSaved => "NOT SAVED - RETRYING",
            Self::SavedUnreadable => "SAVED SENSORS UNREADABLE",
        }
    }

    /// Wording that fits the page feedback and focused status lines.
    pub const fn short(self) -> &'static str {
        match self {
            Self::SlotsFull => "SENSOR SLOTS FULL",
            Self::InputLost => "MOVE FOCUS FIRST",
            Self::WaitingForCompanion => "WAITING FOR RADIO",
            Self::ScanThenPick => "FIND NEARBY",
            Self::NotSaved => "NOT SAVED",
            Self::SavedUnreadable => "NOT SAVED",
            message => message.text(),
        }
    }

    /// Ordinary discovery and startup progress, as opposed to a problem the
    /// rider should notice.
    pub const fn routine(self) -> bool {
        matches!(
            self,
            Self::PickSensor
                | Self::SearchPending
                | Self::StartingScan
                | Self::Scanning
                | Self::StoppingScan
                | Self::ScanCancelled
                | Self::FinishingScan
                | Self::ScanDone
                | Self::StartingSensors
                | Self::WaitingForCompanion
                | Self::ScanThenPick
        )
    }
}

/// How the latest power meter calibration stands, as the rider is told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationNote {
    Calibrating,
    /// Asked, but a search or earlier message still holds the radio.
    WaitingForRadio,
    /// The offset the power meter reported, in its own units.
    Done(i16),
    /// The power meter refused, did not answer, or could not be asked.
    Failed,
    /// The radio refuses further messages until it restarts.
    NeedsRestart,
    NotConnected,
    WhileRiding,
}

impl CalibrationNote {
    /// The wording fits `CALIBRATED  OFFSET -32768` on one caption line.
    pub fn text(self) -> heapless::String<28> {
        use core::fmt::Write;
        let mut text = heapless::String::new();
        let _ = match self {
            Self::Calibrating => text.write_str("CALIBRATING..."),
            Self::WaitingForRadio => text.write_str("WAITING FOR RADIO"),
            Self::NeedsRestart => text.write_str("RESTART TO CALIBRATE"),
            Self::Done(offset) => write!(text, "CALIBRATED  OFFSET {offset}"),
            Self::Failed => text.write_str("CALIBRATION FAILED"),
            Self::NotConnected => text.write_str("NOT CONNECTED"),
            Self::WhileRiding => text.write_str("NOT DURING A RIDE"),
        };
        text
    }

    /// A grey line saying what to do, for the failure the rider can act on.
    pub const fn hint(self) -> Option<&'static str> {
        match self {
            Self::Failed => Some("CRANKS STILL, TRY AGAIN"),
            _ => None,
        }
    }
}
