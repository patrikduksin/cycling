//! Page feedback shown on the sensor screen.
use crate::screens::theme::Tone;

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
    Added,
}

impl MenuMessage {
    pub const ALL: [Self; 22] = [
        Self::PickSensor,
        Self::SearchPending,
        Self::StartingScan,
        Self::Scanning,
        Self::StoppingScan,
        Self::ScanCancelled,
        Self::FinishingScan,
        Self::ScanDone,
        Self::ScanFailed,
        Self::ScanUnavailable,
        Self::RadioLost,
        Self::RadioUnavailable,
        Self::StartingSensors,
        Self::WaitingForCompanion,
        Self::StartupFailed,
        Self::ScanThenPick,
        Self::InputLost,
        Self::NotSeen,
        Self::SlotsFull,
        Self::NotSaved,
        Self::SavedUnreadable,
        Self::Added,
    ];

    /// The lasting save problems `Menu::set_notice` shows.
    pub const NOTICES: [Self; 2] = [Self::NotSaved, Self::SavedUnreadable];

    /// Full wording, used where the message has room of its own.
    pub const fn text(self) -> &'static str {
        match self {
            Self::PickSensor => "PICK SENSOR",
            Self::SearchPending => "SEARCH PENDING",
            Self::StartingScan => "STARTING SCAN",
            Self::Scanning => "SEARCHING...",
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
            Self::SlotsFull => "TEN SENSORS SAVED - FORGET ONE TO ADD",
            Self::NotSaved => "NOT SAVED - RETRYING",
            Self::SavedUnreadable => "SAVED SENSORS UNREADABLE",
            Self::Added => "SENSOR ADDED",
        }
    }

    /// Wording that fits the page feedback and focused status lines.
    pub const fn short(self) -> &'static str {
        match self {
            Self::SlotsFull => "FULL - FORGET ONE",
            Self::InputLost => "MOVE FOCUS FIRST",
            Self::WaitingForCompanion => "WAITING FOR RADIO",
            Self::ScanThenPick => "FIND NEARBY",
            Self::NotSaved => "NOT SAVED",
            Self::SavedUnreadable => "NOT SAVED",
            // The card the rider picked already says NOT SEEN.
            Self::NotSeen => "SEARCH AGAIN",
            message => message.text(),
        }
    }

    /// A problem SENSORS names in its title: the search belongs to ADD
    /// SENSOR, so only trouble with the buttons or the radio shows here.
    pub const fn on_sensors(self) -> bool {
        matches!(
            self,
            Self::InputLost | Self::RadioLost | Self::RadioUnavailable | Self::StartupFailed
        )
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
                | Self::Added
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
    /// Every note, `Done` standing for each offset.
    pub const ALL: [Self; 7] = [
        Self::Calibrating,
        Self::WaitingForRadio,
        Self::Done(0),
        Self::Failed,
        Self::NeedsRestart,
        Self::NotConnected,
        Self::WhileRiding,
    ];

    pub fn text(self) -> heapless::String<24> {
        use core::fmt::Write;
        let mut text = heapless::String::new();
        let _ = match self {
            Self::Calibrating => text.write_str("Zeroing"),
            Self::WaitingForRadio => text.write_str("Waiting for radio"),
            Self::NeedsRestart => text.write_str("Restart to zero"),
            Self::Done(offset) => write!(text, "Zeroed, offset {offset}"),
            Self::Failed => text.write_str("Keep cranks still"),
            Self::NotConnected => text.write_str("Not connected"),
            Self::WhileRiding => text.write_str("Not during a ride"),
        };
        text
    }

    pub const fn tone(self) -> Tone {
        match self {
            Self::Calibrating | Self::WaitingForRadio => Tone::Info,
            Self::Done(_) => Tone::Good,
            Self::Failed | Self::NeedsRestart | Self::NotConnected | Self::WhileRiding => {
                Tone::Warn
            }
        }
    }
}
