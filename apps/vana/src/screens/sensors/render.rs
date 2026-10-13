//! Per-pixel drawing of the prepared sensor layout on the 240 × 320 canvas.
use super::layout::{NEARBY_GRID, SELECTED_GRID};
use super::menu::{Choice, Menu, View};
use super::slot::{LinkStatus, Selected};
use super::{icons, kind};
use device_api::ant::Identity;
use firmware_shell::rendering::text::text;

pub(super) const WHITE: u16 = 0xffff;
pub(super) const CYAN: u16 = 0x07ff;
pub(super) const MUTED: u16 = 0x9cf3;
pub(super) const GREEN: u16 = 0x5fe9;
pub(super) const AMBER: u16 = 0xfd20;
pub(super) const FOCUS_FILL: u16 = 0x0945;
pub(super) const BORDER: u16 = 0x3186;
const DIVIDER: u16 = 0x39e7;

const WIDTH: usize = 240;
const HEIGHT: usize = 320;
/// Text scale 2 glyphs advance 12 pixels; scale 3 glyphs advance 18.
const GLYPH: usize = 6;
const MARGIN: usize = 12;
const FOOTER_DIVIDER_Y: usize = 290;
const FOOTER_Y: usize = 300;
const SEARCH_BUTTON: (usize, usize) = (208, 130);
const SEARCH_BUTTON_SIZE: usize = 24;
const STATUS_INDICATOR_SIZE: usize = 7;

impl LinkStatus {
    pub(super) const fn color(self) -> u16 {
        match self {
            Self::Live => GREEN,
            Self::Waiting => CYAN,
            Self::Failed | Self::Off | Self::Check => AMBER,
        }
    }

    /// A 7 × 7 glyph: a check when live, a ring while waiting, otherwise an alert.
    fn indicator(self, x: usize, y: usize) -> bool {
        match self {
            Self::Live => {
                (x <= 2 && (y == x + 3 || y == x + 2)) || (x >= 2 && (y + x == 7 || y + x == 6))
            }
            Self::Waiting => {
                ((x == 1 || x == 5) && (1..6).contains(&y))
                    || ((y == 1 || y == 5) && (1..6).contains(&x))
            }
            Self::Failed | Self::Off | Self::Check => {
                (x == 3 && (1..4).contains(&y)) || (x == 3 && y == 5)
            }
        }
    }
}

impl Menu {
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        if x >= WIDTH || y >= HEIGHT {
            return 0;
        }
        if y == FOOTER_DIVIDER_Y {
            return DIVIDER;
        }
        if y >= FOOTER_Y {
            let action = if matches!(self.view, View::Tiles) {
                self.layout.action
            } else {
                b"SELECT"
            };
            let right = WIDTH - MARGIN - action.len() * 2 * GLYPH;
            if text(x, y, MARGIN, FOOTER_Y, 2, b"NEXT") || text(x, y, right, FOOTER_Y, 2, action) {
                return WHITE;
            }
            return 0;
        }
        match self.view {
            View::Tiles => self.tiles_pixel(x, y),
            View::Detail(slot) => self.selected[slot].map_or(0, |s| self.detail_pixel(x, y, s)),
            View::Replace(identity) => self.replace_pixel(x, y, identity),
        }
    }

    fn tiles_pixel(&self, x: usize, y: usize) -> u16 {
        let feedback = &self.layout.page_feedback;
        let (heading, color): (&[u8], u16) = if feedback.is_empty() {
            (b"MY SENSORS", WHITE)
        } else {
            (feedback.as_bytes(), AMBER)
        };
        if text(x, y, MARGIN, 36, 2, heading) {
            return color;
        }
        if text(x, y, MARGIN, 138, 2, b"NEARBY") {
            return WHITE;
        }
        if self.scanning && text(x, y, 96, 138, 2, b"SEARCHING") {
            return CYAN;
        }
        if let Some(color) = self.search_button_pixel(x, y) {
            return color;
        }
        if let Some((index, left, top)) = SELECTED_GRID.tile(x, y)
            && let Some(&(slot, selected)) = self.layout.selected.get(index)
        {
            return self.selected_tile_pixel(x, y, left, top, slot, selected);
        }
        if let Some((index, left, top)) = NEARBY_GRID.tile(x, y)
            && let Some(&(cursor, identity)) = self.layout.nearby.get(index)
        {
            let focused = self.cursor == cursor;
            if border(
                x,
                y,
                left,
                top,
                NEARBY_GRID.width,
                NEARBY_GRID.height,
                stroke(focused),
            ) {
                return if focused { CYAN } else { BORDER };
            }
            if icon(x, y, left + 14, top + 6, identity.device_type) {
                return WHITE;
            }
            return fill(focused);
        }
        if self.layout.selected.is_empty() && text(x, y, MARGIN, 68, 2, b"NONE SELECTED") {
            return MUTED;
        }
        let empty: &[u8] = if self.scanning {
            b"LOOKING NEARBY..."
        } else {
            b"NO SENSORS FOUND"
        };
        if self.layout.nearby.is_empty() && text(x, y, MARGIN, 168, 2, empty) {
            return MUTED;
        }
        if y == 226 && (MARGIN..WIDTH - MARGIN).contains(&x) {
            return DIVIDER;
        }
        if text(x, y, MARGIN, 238, 3, self.layout.title) {
            return WHITE;
        }
        if text(x, y, MARGIN, 267, 2, self.layout.status.as_bytes()) {
            return self.layout.color;
        }
        0
    }

    fn search_button_pixel(&self, x: usize, y: usize) -> Option<u16> {
        let (left, top) = SEARCH_BUTTON;
        let size = SEARCH_BUTTON_SIZE;
        if !(left..left + size).contains(&x) || !(top..top + size).contains(&y) {
            return None;
        }
        let focused = self.cursor == 0;
        if focused && border(x, y, left, top, size, size, 2) {
            return Some(CYAN);
        }
        if icons::refresh_pixel(x - left, y - top) {
            return Some(if self.scanning { CYAN } else { WHITE });
        }
        Some(fill(focused))
    }

    fn selected_tile_pixel(
        &self,
        x: usize,
        y: usize,
        left: usize,
        top: usize,
        slot: usize,
        selected: Selected,
    ) -> u16 {
        let focused = self.cursor == slot + 1;
        let (width, height) = (SELECTED_GRID.width, SELECTED_GRID.height);
        if border(x, y, left, top, width, height, stroke(focused)) {
            return if focused { CYAN } else { BORDER };
        }
        if icon(x, y, left + 9, top + 6, selected.identity.device_type) {
            return WHITE;
        }
        let status = selected.link_status();
        let (indicator_left, indicator_top) = (left + 30, top + 3);
        if (indicator_left..indicator_left + STATUS_INDICATOR_SIZE).contains(&x)
            && (indicator_top..indicator_top + STATUS_INDICATOR_SIZE).contains(&y)
            && status.indicator(x - indicator_left, y - indicator_top)
        {
            return status.color();
        }
        fill(focused)
    }

    fn detail_pixel(&self, x: usize, y: usize, selected: Selected) -> u16 {
        if scaled_icon(x, y, 98, 52, selected.identity.device_type) {
            return WHITE;
        }
        if centered_title(x, y, 111, kind(selected.identity.device_type)) {
            return WHITE;
        }
        let status = selected.link_status();
        if status == LinkStatus::Live {
            if text(x, y, 60, 149, 2, b"LIVE DATA") || live_check(x, y, 38, 150) {
                return GREEN;
            }
        } else {
            let line = selected.status_text();
            let line = &line[..line.len().min(18)];
            if text(x, y, centered(line.len(), 2), 149, 2, line) {
                return status.color();
            }
        }
        let identity = self.layout.identity.as_bytes();
        let left = centered(b"SENSOR ".len() + identity.len(), 2);
        if text(x, y, left, 179, 2, b"SENSOR ") || text(x, y, left + 84, 179, 2, identity) {
            return MUTED;
        }
        let reconnect = selected.retry() && !selected.failed_disconnect;
        let primary: &[u8] = if selected.pending.is_some() {
            b"BACK"
        } else if selected.failed_disconnect {
            b"RETRY DISCONNECT"
        } else if selected.retry() {
            b"RETRY"
        } else {
            b"DISCONNECT"
        };
        if let Some(color) = self.button_pixel(x, y, 220, 34, Choice::Primary, primary) {
            return color;
        }
        if selected.pending.is_none() {
            let secondary: &[u8] = if reconnect && selected.can_disconnect() {
                b"DISCONNECT"
            } else {
                b"BACK"
            };
            if let Some(color) = self.button_pixel(x, y, 262, 28, Choice::Secondary, secondary) {
                return color;
            }
        }
        0
    }

    fn replace_pixel(&self, x: usize, y: usize, identity: Identity) -> u16 {
        if scaled_icon(x, y, 98, 43, identity.device_type) {
            return WHITE;
        }
        if centered_title(x, y, 94, kind(identity.device_type))
            || text(x, y, 30, 129, 2, b"REPLACE SENSOR")
        {
            return WHITE;
        }
        if (160..191).contains(&y) {
            // Current peer on the left half, replacement on the right.
            let index = usize::from(x >= WIDTH / 2);
            let center = [54, 180][index];
            let number = &self.layout.replacement_numbers[index];
            let transmission = &self.layout.replacement_transmissions[index];
            let scale = if transmission.is_empty() { 3 } else { 2 };
            let number_left = center - number.len() * GLYPH / 2 * scale;
            let transmission_left = center - transmission.len() * GLYPH;
            if text(x, y, number_left, 160, scale, number.as_bytes())
                || text(x, y, transmission_left, 177, 2, transmission.as_bytes())
            {
                return WHITE;
            }
        }
        if arrow(x, y) {
            return CYAN;
        }
        if text(x, y, MARGIN, 191, 2, b"CURRENT") || text(x, y, 174, 191, 2, b"NEW") {
            return MUTED;
        }
        if let Some(color) = self.button_pixel(x, y, 216, 34, Choice::Primary, b"REPLACE") {
            return color;
        }
        if let Some(color) = self.button_pixel(x, y, 258, 32, Choice::Secondary, b"KEEP CURRENT") {
            return color;
        }
        0
    }

    /// A full-width button, highlighted when it is the current choice.
    fn button_pixel(
        &self,
        x: usize,
        y: usize,
        top: usize,
        height: usize,
        choice: Choice,
        label: &[u8],
    ) -> Option<u16> {
        let width = WIDTH - 2 * MARGIN;
        if !(MARGIN..MARGIN + width).contains(&x) || !(top..top + height).contains(&y) {
            return None;
        }
        let focused = self.choice == choice;
        if border(x, y, MARGIN, top, width, height, stroke(focused)) {
            return Some(if focused { CYAN } else { BORDER });
        }
        let label_top = top + (height - 14) / 2;
        if text(x, y, centered(label.len(), 2), label_top, 2, label) {
            return Some(WHITE);
        }
        Some(fill(focused))
    }
}

/// Left edge that centers `chars` glyphs of the given scale on the canvas.
const fn centered(chars: usize, scale: usize) -> usize {
    (WIDTH - chars * GLYPH * scale) / 2
}

fn centered_title(x: usize, y: usize, top: usize, title: &[u8]) -> bool {
    text(x, y, centered(title.len(), 3), top, 3, title)
}

const fn stroke(focused: bool) -> usize {
    if focused { 2 } else { 1 }
}

const fn fill(focused: bool) -> u16 {
    if focused { FOCUS_FILL } else { 0 }
}

/// The current-to-new arrow between the two replacement identities.
fn arrow(x: usize, y: usize) -> bool {
    ((103..130).contains(&x) && (171..173).contains(&y))
        || ((121..131).contains(&x) && ((y as isize - 172).unsigned_abs() == 130 - x))
}

fn live_check(x: usize, y: usize, left: usize, top: usize) -> bool {
    if !(left..left + 11).contains(&x) || !(top..top + 12).contains(&y) {
        return false;
    }
    let x = x - left;
    let y = y - top;
    ((1..=4).contains(&x) && (y == x + 4 || y + 1 == x + 4))
        || ((4..=10).contains(&x) && (x + y == 12 || x + y == 11))
}

fn scaled_icon(x: usize, y: usize, left: usize, top: usize, kind: u8) -> bool {
    x >= left && y >= top && icons::pixel(kind, (x - left) / 2, (y - top) / 2)
}

fn border(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    stroke: usize,
) -> bool {
    x >= left
        && x < left + width
        && y >= top
        && y < top + height
        && (x < left + stroke
            || x >= left + width - stroke
            || y < top + stroke
            || y >= top + height - stroke)
}

fn icon(x: usize, y: usize, left: usize, top: usize, kind: u8) -> bool {
    x >= left && y >= top && icons::pixel(kind, x - left, y - top)
}
