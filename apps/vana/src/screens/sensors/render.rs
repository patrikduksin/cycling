//! The sensor pages' colours, and the page in a frame of its own. A frame
//! paints the page as the workout `View`'s canvas body, with its cues.
use super::CalibrationNote;
use super::menu::Menu;
use super::slot::LinkStatus;
use crate::screens::chrome;
use crate::screens::workout::{Page as Screen, View};

/// Waiting and information.
pub(super) const CYAN: u16 = chrome::ACCENT;
pub(super) const GREEN: u16 = chrome::GREEN;
pub(super) const AMBER: u16 = chrome::AMBER;

impl CalibrationNote {
    /// Cyan while waiting, green once zeroed, amber when it did not happen.
    pub(super) fn color(self) -> u16 {
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
        }
    }
}

impl Menu {
    /// The shown page in a frame of its own, as the runtime prepares it;
    /// tests and previews.
    pub fn frame(&self) -> View<'static> {
        let mut view = View::new(Screen::Sensors, 0, None);
        view.show_sensors(self.canvas(), self.cues());
        view.prepare();
        view
    }
}
