//! The sensor pages' colours.
use super::CalibrationNote;
use super::slot::LinkStatus;
use crate::screens::chrome;

/// Waiting and information.
pub(super) const CYAN: u16 = chrome::ACCENT;
pub(super) const GREEN: u16 = chrome::GREEN;
pub(super) const AMBER: u16 = chrome::AMBER;

impl CalibrationNote {
    /// Cyan while waiting, green once zeroed, amber when it did not happen.
    pub fn color(self) -> u16 {
        match self {
            Self::Calibrating | Self::WaitingForRadio => CYAN,
            Self::Done(_) => GREEN,
            Self::Failed | Self::NeedsRestart | Self::NotConnected | Self::WhileRiding => AMBER,
        }
    }
}

impl LinkStatus {
    pub(super) const fn color(self) -> u16 {
        match self {
            Self::Live => GREEN,
            Self::Waiting => CYAN,
            Self::Failed | Self::Missing | Self::Check => AMBER,
            Self::Idle => chrome::SECONDARY,
        }
    }
}
