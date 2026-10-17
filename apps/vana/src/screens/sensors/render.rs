//! Row painting of the prepared sensor layout on the 240 × 320 canvas.
use super::layout::{NEARBY_GRID, SELECTED_GRID};
use super::menu::{Choice, Menu, View};
use super::slot::{LinkStatus, Selected};
use super::{icons, kind};
use core::ops::Range;
use device_api::ant::Identity;
use firmware_shell::rendering::row::Row;

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
            Self::Failed | Self::Missing | Self::Check => AMBER,
        }
    }

    /// A 7 × 7 glyph: a check when live, a ring while waiting, otherwise an alert.
    pub(super) fn indicator(self, x: usize, y: usize) -> bool {
        match self {
            Self::Live => {
                (x <= 2 && (y == x + 3 || y == x + 2)) || (x >= 2 && (y + x == 7 || y + x == 6))
            }
            Self::Waiting => {
                ((x == 1 || x == 5) && (1..6).contains(&y))
                    || ((y == 1 || y == 5) && (1..6).contains(&x))
            }
            Self::Failed | Self::Missing | Self::Check => {
                (x == 3 && (1..4).contains(&y)) || (x == 3 && y == 5)
            }
        }
    }
}

impl Menu {
    /// One pixel of the painted canvas; tests and previews only, as it paints a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [0u16; WIDTH];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }

    /// Paint one row of the 240 x 320 canvas; later paints cover earlier ones.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        row.fill(0);
        if y >= HEIGHT {
            return;
        }
        if y == FOOTER_DIVIDER_Y {
            row.fill(DIVIDER);
            return;
        }
        if y >= FOOTER_Y {
            let action = if matches!(self.view, View::Tiles) {
                self.layout.action
            } else {
                b"SELECT"
            };
            let right = WIDTH - MARGIN - action.len() * 2 * GLYPH;
            row.text(right, FOOTER_Y, 2, action, WHITE);
            row.text(MARGIN, FOOTER_Y, 2, b"NEXT", WHITE);
            return;
        }
        match self.view {
            View::Tiles => self.paint_tiles(row),
            View::Detail(slot) => {
                if let Some(selected) = self.selected[slot] {
                    self.paint_detail(row, selected);
                }
            }
            View::Replace(identity) => self.paint_replace(row, identity),
        }
    }

    fn paint_tiles(&self, row: &mut Row) {
        let y = row.y();
        row.text(
            MARGIN,
            267,
            2,
            self.layout.status.as_bytes(),
            self.layout.color,
        );
        row.text(MARGIN, 238, 3, self.layout.title, WHITE);
        if y == 226 {
            row.span(MARGIN..WIDTH - MARGIN, DIVIDER);
        }
        if self.layout.nearby.is_empty() {
            let empty: &[u8] = if self.scanning {
                b"LOOKING NEARBY..."
            } else {
                b"NO SENSORS FOUND"
            };
            row.text(MARGIN, 168, 2, empty, MUTED);
        }
        if self.layout.selected.is_empty() {
            row.text(MARGIN, 68, 2, b"NONE SELECTED", MUTED);
        }
        for (index, left, top) in NEARBY_GRID.tiles(y) {
            let Some(&(cursor, identity)) = self.layout.nearby.get(index) else {
                continue;
            };
            let focused = self.cursor == cursor;
            let mut tile = row.within(left..left + NEARBY_GRID.width);
            tile.fill(fill(focused));
            icon(&mut tile, left + 14, top + 6, identity.device_type);
            border(
                &mut tile,
                top,
                NEARBY_GRID.height,
                stroke(focused),
                if focused { CYAN } else { BORDER },
            );
        }
        for (index, left, top) in SELECTED_GRID.tiles(y) {
            if let Some(&slot) = self.layout.selected.get(index)
                && let Some(selected) = self.selected[slot]
            {
                self.paint_selected_tile(
                    &mut row.within(left..left + SELECTED_GRID.width),
                    top,
                    slot,
                    selected,
                );
            }
        }
        self.paint_search_button(row);
        if self.scanning {
            row.text(96, 138, 2, b"SEARCHING", CYAN);
        }
        row.text(MARGIN, 138, 2, b"NEARBY", WHITE);
        let feedback = &self.layout.page_feedback;
        let (heading, color): (&[u8], u16) = if feedback.is_empty() {
            (b"MY SENSORS", WHITE)
        } else {
            (feedback.as_bytes(), AMBER)
        };
        row.text(MARGIN, 36, 2, heading, color);
    }

    fn paint_search_button(&self, row: &mut Row) {
        let (left, top) = SEARCH_BUTTON;
        let size = SEARCH_BUTTON_SIZE;
        let y = row.y();
        if !(top..top + size).contains(&y) {
            return;
        }
        let focused = self.cursor == 0;
        let mut button = row.within(left..left + size);
        button.fill(fill(focused));
        let color = if self.scanning { CYAN } else { WHITE };
        button.bits(left, icons::refresh_row(y - top), color);
        if focused {
            border(&mut button, top, size, 2, CYAN);
        }
    }

    /// `tile` is clipped to the tile's columns.
    fn paint_selected_tile(&self, tile: &mut Row, top: usize, slot: usize, selected: Selected) {
        let left = tile.clip().start;
        let y = tile.y();
        let focused = self.cursor == slot + 1;
        tile.fill(fill(focused));
        let status = selected.link_status();
        let (indicator_left, indicator_top) = (left + 30, top + 3);
        if (indicator_top..indicator_top + STATUS_INDICATOR_SIZE).contains(&y) {
            let dy = y - indicator_top;
            tile.each(
                indicator_left..indicator_left + STATUS_INDICATOR_SIZE,
                |x| {
                    status
                        .indicator(x - indicator_left, dy)
                        .then(|| status.color())
                },
            );
        }
        icon(tile, left + 9, top + 6, selected.identity.device_type);
        border(
            tile,
            top,
            SELECTED_GRID.height,
            stroke(focused),
            if focused { CYAN } else { BORDER },
        );
    }

    fn paint_detail(&self, row: &mut Row, selected: Selected) {
        // Disconnect is the only removal; ordinary loss reconnects by itself.
        self.paint_button(row, 262, 28, Choice::Secondary, b"BACK");
        self.paint_button(row, 220, 34, Choice::Primary, b"DISCONNECT");
        let identity = self.layout.identity.as_bytes();
        let left = centered(b"SENSOR ".len() + identity.len(), 2);
        row.text(left + 84, 179, 2, identity, MUTED);
        row.text(left, 179, 2, b"SENSOR ", MUTED);
        let status = selected.link_status();
        if status == LinkStatus::Live {
            live_check(row, 38, 150);
            row.text(60, 149, 2, b"LIVE DATA", GREEN);
        } else {
            let line = selected.status_text();
            let line = &line[..line.len().min(18)];
            row.text(centered(line.len(), 2), 149, 2, line, status.color());
        }
        centered_title(row, 111, kind(selected.identity.device_type));
        scaled_icon(row, 98, 52, selected.identity.device_type);
    }

    fn paint_replace(&self, row: &mut Row, identity: Identity) {
        let y = row.y();
        self.paint_button(row, 258, 32, Choice::Secondary, b"KEEP CURRENT");
        self.paint_button(row, 216, 34, Choice::Primary, b"REPLACE");
        row.text(174, 191, 2, b"NEW", MUTED);
        row.text(MARGIN, 191, 2, b"CURRENT", MUTED);
        arrow(row);
        if (160..191).contains(&y) {
            // Current peer on the left half, replacement on the right.
            for (index, half) in [0..WIDTH / 2, WIDTH / 2..WIDTH].into_iter().enumerate() {
                let center = [54, 180][index];
                let number = &self.layout.replacement_numbers[index];
                let transmission = &self.layout.replacement_transmissions[index];
                let scale = if transmission.is_empty() { 3 } else { 2 };
                let number_left = center - number.len() * GLYPH / 2 * scale;
                let transmission_left = center - transmission.len() * GLYPH;
                let mut half = row.within(half);
                half.text(transmission_left, 177, 2, transmission.as_bytes(), WHITE);
                half.text(number_left, 160, scale, number.as_bytes(), WHITE);
            }
        }
        row.text(30, 129, 2, b"REPLACE SENSOR", WHITE);
        centered_title(row, 94, kind(identity.device_type));
        scaled_icon(row, 98, 43, identity.device_type);
    }

    /// A full-width button, highlighted when it is the current choice.
    fn paint_button(&self, row: &mut Row, top: usize, height: usize, choice: Choice, label: &[u8]) {
        if !(top..top + height).contains(&row.y()) {
            return;
        }
        let width = WIDTH - 2 * MARGIN;
        let focused = self.choice == choice;
        let mut button = row.within(MARGIN..MARGIN + width);
        button.fill(fill(focused));
        let label_top = top + (height - 14) / 2;
        button.text(centered(label.len(), 2), label_top, 2, label, WHITE);
        border(
            &mut button,
            top,
            height,
            stroke(focused),
            if focused { CYAN } else { BORDER },
        );
    }
}

/// Left edge that centers `chars` glyphs of the given scale on the canvas.
const fn centered(chars: usize, scale: usize) -> usize {
    (WIDTH - chars * GLYPH * scale) / 2
}

fn centered_title(row: &mut Row, top: usize, title: &[u8]) {
    row.text(centered(title.len(), 3), top, 3, title, WHITE);
}

const fn stroke(focused: bool) -> usize {
    if focused { 2 } else { 1 }
}

const fn fill(focused: bool) -> u16 {
    if focused { FOCUS_FILL } else { 0 }
}

/// The current-to-new arrow between the two replacement identities.
fn arrow(row: &mut Row) {
    let y = row.y();
    if (171..173).contains(&y) {
        row.span(103..130, CYAN);
    }
    let x = 130 - (y as isize - 172).unsigned_abs().min(130);
    if (121..131).contains(&x) {
        row.set(x, CYAN);
    }
}

fn live_check(row: &mut Row, left: usize, top: usize) {
    let y = row.y();
    if !(top..top + 12).contains(&y) {
        return;
    }
    let y = y - top;
    row.each(left..left + 11, |x| {
        let x = x - left;
        (((1..=4).contains(&x) && (y == x + 4 || y + 1 == x + 4))
            || ((4..=10).contains(&x) && (x + y == 12 || x + y == 11)))
            .then_some(GREEN)
    });
}

fn scaled_icon(row: &mut Row, left: usize, top: usize, kind: u8) {
    if let Some(y) = row.y().checked_sub(top) {
        row.bits_scaled(left, icons::row(kind, y / 2), 2, WHITE);
    }
}

/// The row of a `stroke`-wide frame around a tile whose columns are `tile`'s clip.
fn border(tile: &mut Row, top: usize, height: usize, stroke: usize, color: u16) {
    let Range { start: left, end } = tile.clip();
    let y = tile.y();
    if y < top || y >= top + height {
        return;
    }
    if y < top + stroke || y >= top + height - stroke {
        tile.fill(color);
    } else {
        tile.span(left..left + stroke, color);
        tile.span(end - stroke..end, color);
    }
}

fn icon(row: &mut Row, left: usize, top: usize, kind: u8) {
    if let Some(y) = row.y().checked_sub(top) {
        row.bits(left, icons::row(kind, y), WHITE);
    }
}
