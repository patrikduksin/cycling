//! Row painting of the prepared sensor page on the 240 × 320 canvas.
//!
//! Each row paints the canvas the layout prepared, then the page's cues;
//! the canvas's per-pixel original stays the test reference painted rows
//! must match.
use super::CalibrationNote;
use super::menu::Menu;
use super::slot::LinkStatus;
use crate::screens::chrome::{self, BLACK, CONTENT, HEIGHT, Row, WIDTH};

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
    /// One pixel of the painted page; tests and previews only, as it paints
    /// a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [BLACK; WIDTH];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(BLACK)
    }

    /// Paint one row of the shown page. Pages with the status bar leave its
    /// rows black.
    pub fn paint(&self, row: &mut Row) {
        row.fill(BLACK);
        let y = row.y();
        if y >= HEIGHT || (!self.layout.full_screen && y < CONTENT.top) {
            return;
        }
        self.layout.canvas.paint(row);
        for cue in self.layout.cues {
            cue.paint(row);
        }
    }

    /// The per-pixel original of `paint`, kept as the reference it must match.
    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
        if x >= WIDTH || y >= HEIGHT || (!self.layout.full_screen && y < CONTENT.top) {
            return BLACK;
        }
        let under = self.layout.canvas.pixel(x, y, BLACK);
        self.layout
            .cues
            .iter()
            .fold(under, |under, cue| cue.pixel(x, y, under))
    }
}
