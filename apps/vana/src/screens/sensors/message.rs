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
    Busy,
    TypeUnsupported,
    RequestFailed,
    TooManyRequests,
    InputLost,
    NotSeen,
    WaitForCurrentRequest,
    SlotsFull,
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
            Self::Busy => "BUSY - RETRY",
            Self::TypeUnsupported => "TYPE UNSUPPORTED",
            Self::RequestFailed => "REQUEST FAILED",
            Self::TooManyRequests => "TOO MANY REQUESTS",
            Self::InputLost => "INPUT LOST - PRESS NEXT",
            Self::NotSeen => "NOT SEEN - SEARCH AGAIN",
            Self::WaitForCurrentRequest => "WAIT FOR CURRENT REQUEST",
            Self::SlotsFull => "ALL TEN SENSOR SLOTS ARE IN USE",
        }
    }

    /// Wording that fits the page feedback and focused status lines.
    pub const fn short(self) -> &'static str {
        match self {
            Self::WaitForCurrentRequest => "REQUEST PENDING",
            Self::SlotsFull => "SENSOR SLOTS FULL",
            Self::InputLost => "PRESS NEXT AGAIN",
            Self::WaitingForCompanion => "WAITING FOR RADIO",
            Self::ScanThenPick => "FIND NEARBY",
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
