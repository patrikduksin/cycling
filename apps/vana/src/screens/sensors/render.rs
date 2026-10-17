//! Row painting of the prepared sensor layout on the 240 × 320 canvas.
//!
//! Each row paints only the items whose rows it crosses; the per-pixel
//! original stays as the test reference the painted rows must match.
use super::layout::{
    DETAIL_BUTTONS, NEARBY_GRID, REPLACE_BUTTONS, SEARCH_BUTTON, SEARCH_BUTTON_SIZE, SELECTED_GRID,
};
use super::menu::{Choice, Menu, View};
use super::slot::{LinkStatus, Selected};
use super::{icons, kind};
use crate::screens::chrome::{
    self, Align, BLACK, BODY, CAPTION, CARD, Label, Line, RAISED, Rect, Row, VALUE, blend, card,
    disc, paint_rounded,
};
use device_api::ant::Identity;
use firmware_shell::rendering::font::Face;

pub(super) const WHITE: u16 = chrome::WHITE;
/// Waiting and information.
pub(super) const CYAN: u16 = chrome::ACCENT;
pub(super) const MUTED: u16 = chrome::SECONDARY;
pub(super) const GREEN: u16 = chrome::GREEN;
pub(super) const AMBER: u16 = chrome::AMBER;

const WIDTH: usize = chrome::WIDTH;
const HEIGHT: usize = chrome::HEIGHT;
const MARGIN: usize = 12;
const STATUS_INDICATOR_SIZE: usize = 7;
const TILE_RADIUS: usize = 9;
pub(super) const HEADING_TOP: usize = 30;
pub(super) const NEARBY_TOP: usize = 138;
/// SEARCHING follows the NEARBY heading on its line, a word space after it.
const SEARCHING_LEFT: usize = MARGIN + CAPTION.width(b"NEARBY") + 6;
pub(super) const TITLE_TOP: usize = 236;
pub(super) const STATUS_TOP: usize = 262;

impl LinkStatus {
    pub(super) const fn color(self) -> u16 {
        match self {
            Self::Live => GREEN,
            Self::Waiting => CYAN,
            Self::Failed | Self::Missing | Self::Check => AMBER,
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
            Self::Failed | Self::Missing | Self::Check => {
                (x == 3 && (1..4).contains(&y)) || (x == 3 && y == 5)
            }
        }
    }
}

/// `text` in `face` with its top at `top`, placed by `align` at `x`, over
/// `background`; none outside its ink.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn write(
    x: usize,
    y: usize,
    face: &'static Face,
    text: &[u8],
    at: usize,
    top: usize,
    align: Align,
    color: u16,
    background: u16,
) -> Option<u16> {
    if y < top || y >= top + usize::from(face.height) || text.is_empty() {
        return None;
    }
    Label::new(Line::<24>::new(face, text), at, top, align, color).pixel(x, y, background)
}

/// Paint `text` as `write` draws it, over what the row already holds.
fn paint_text(
    row: &mut Row,
    face: &'static Face,
    text: &[u8],
    at: usize,
    top: usize,
    align: Align,
    color: u16,
) {
    let y = row.y();
    if y < top || y >= top + usize::from(face.height) || text.is_empty() {
        return;
    }
    let label = Label::new(Line::<24>::new(face, text), at, top, align, color);
    #[cfg(test)]
    PAINTED.with(|painted| painted.borrow_mut().push(label));
    label.paint(row);
}

#[cfg(test)]
std::thread_local! {
    /// Every label `paint_text` drew, for the layout sweep.
    pub(super) static PAINTED: core::cell::RefCell<std::vec::Vec<Label<24>>> =
        const { core::cell::RefCell::new(std::vec::Vec::new()) };
}

/// A 22 × 20 sensor glyph with its top-left corner at (`left`, `top`),
/// each dot `scale` pixels square.
fn paint_icon(row: &mut Row, left: usize, top: usize, scale: usize, kind: u8) {
    let y = row.y();
    if y >= top {
        row.bits_scaled(left, icons::row(kind, (y - top) / scale), scale, WHITE);
    }
}

impl Menu {
    /// One pixel of the painted page; tests only, as it paints a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [BLACK; WIDTH];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(BLACK)
    }

    /// Paint one row of the shown view. Items are painted from the least to
    /// the most important, so where they meet the important one shows.
    pub fn paint(&self, row: &mut Row) {
        row.fill(BLACK);
        if row.y() >= HEIGHT {
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
        let left = Align::Left;
        let status = self.layout.status.as_bytes();
        paint_text(
            row,
            &CAPTION,
            status,
            MARGIN,
            STATUS_TOP,
            left,
            self.layout.color,
        );
        paint_text(
            row,
            &BODY,
            self.layout.title,
            MARGIN,
            TITLE_TOP,
            left,
            WHITE,
        );
        if self.layout.nearby.is_empty() {
            let empty: &[u8] = if self.scanning {
                b"LOOKING NEARBY..."
            } else {
                b"NO SENSORS FOUND"
            };
            let top = NEARBY_GRID.top + 10;
            paint_text(row, &CAPTION, empty, MARGIN, top, left, MUTED);
        }
        if self.layout.selected.is_empty() {
            let top = SELECTED_GRID.top + 10;
            paint_text(row, &CAPTION, b"NONE SELECTED", MARGIN, top, left, MUTED);
        }
        for (index, tile_left, top) in NEARBY_GRID.tiles(y) {
            let Some(&(cursor, identity)) = self.layout.nearby.get(index) else {
                continue;
            };
            let rect = tile(tile_left, top, NEARBY_GRID.width, NEARBY_GRID.height);
            let focused = self.cursor == cursor;
            row.span(rect.left..rect.right, BLACK);
            let edge = if focused { 3 } else { 0 };
            paint_rounded(row, rect, TILE_RADIUS, edge, |x, y| {
                surface(rect, focused, x, y)
            });
            paint_icon(row, tile_left + 14, top + 6, 1, identity.device_type);
        }
        for (index, tile_left, top) in SELECTED_GRID.tiles(y) {
            let Some(&slot) = self.layout.selected.get(index) else {
                continue;
            };
            let Some(selected) = self.selected[slot] else {
                continue;
            };
            let rect = tile(tile_left, top, SELECTED_GRID.width, SELECTED_GRID.height);
            let focused = self.cursor == slot + 1;
            row.span(rect.left..rect.right, BLACK);
            let edge = if focused { 3 } else { 0 };
            paint_rounded(row, rect, TILE_RADIUS, edge, |x, y| {
                surface(rect, focused, x, y)
            });
            let status = selected.link_status();
            let (indicator_left, indicator_top) = (tile_left + 30, top + 3);
            if (indicator_top..indicator_top + STATUS_INDICATOR_SIZE).contains(&y) {
                let iy = y - indicator_top;
                row.each(
                    indicator_left..indicator_left + STATUS_INDICATOR_SIZE,
                    |x| {
                        status
                            .indicator(x - indicator_left, iy)
                            .then(|| status.color())
                    },
                );
            }
            paint_icon(
                row,
                tile_left + 9,
                top + 6,
                1,
                selected.identity.device_type,
            );
        }
        let (button_left, button_top) = SEARCH_BUTTON;
        let size = SEARCH_BUTTON_SIZE;
        if (button_top - 1..button_top + size + 1).contains(&y) {
            row.each(button_left - 1..button_left + size + 1, |x| {
                self.search_button_pixel(x, y)
            });
        }
        if self.scanning {
            paint_text(
                row,
                &CAPTION,
                b"SEARCHING",
                SEARCHING_LEFT,
                NEARBY_TOP,
                left,
                CYAN,
            );
        }
        paint_text(row, &CAPTION, b"NEARBY", MARGIN, NEARBY_TOP, left, MUTED);
        let feedback = &self.layout.page_feedback;
        let (heading, color): (&[u8], u16) = if feedback.is_empty() {
            (b"MY SENSORS", WHITE)
        } else {
            (feedback.as_bytes(), AMBER)
        };
        paint_text(row, &BODY, heading, MARGIN, HEADING_TOP, left, color);
    }

    fn paint_detail(&self, row: &mut Row, selected: Selected) {
        // Disconnect is the only removal; ordinary loss reconnects by itself.
        self.paint_button(row, DETAIL_BUTTONS[1], Choice::Secondary, b"BACK");
        self.paint_button(row, DETAIL_BUTTONS[0], Choice::Primary, b"DISCONNECT");
        let center = Align::Center;
        let mid = WIDTH / 2;
        let (identity, len) = self.identity_line();
        paint_text(row, &CAPTION, &identity[..len], mid, 162, center, MUTED);
        let status = selected.link_status();
        let line = Self::status_line(selected);
        paint_text(row, &CAPTION, line, mid, 140, center, status.color());
        let name = kind(selected.identity.device_type);
        paint_text(row, &BODY, name, mid, 108, center, WHITE);
        paint_icon(row, 98, 52, 2, selected.identity.device_type);
    }

    fn paint_replace(&self, row: &mut Row, identity: Identity) {
        let y = row.y();
        self.paint_button(row, REPLACE_BUTTONS[1], Choice::Secondary, b"KEEP CURRENT");
        self.paint_button(row, REPLACE_BUTTONS[0], Choice::Primary, b"REPLACE");
        if (155..157).contains(&y) {
            row.span(103..130, CYAN);
        }
        // The head's two strokes meet at (130, 156).
        let dy = (y as isize - 156).unsigned_abs();
        if dy < 10 {
            row.set(130 - dy, CYAN);
        }
        if (140..200).contains(&y) {
            // Current peer on the left half, replacement on the right.
            let center = Align::Center;
            for (index, half) in [0..WIDTH / 2, WIDTH / 2..WIDTH].into_iter().enumerate() {
                let mut half = row.within(half);
                let center_x = [60, 180][index];
                let label: &[u8] = if index == 0 { b"CURRENT" } else { b"NEW" };
                paint_text(&mut half, &CAPTION, label, center_x, 188, center, MUTED);
                let transmission = self.layout.replacement_transmissions[index].as_bytes();
                paint_text(
                    &mut half,
                    &CAPTION,
                    transmission,
                    center_x,
                    172,
                    center,
                    WHITE,
                );
                let number = self.layout.replacement_numbers[index].as_bytes();
                paint_text(&mut half, &VALUE, number, center_x, 140, center, WHITE);
            }
        }
        let center = Align::Center;
        let mid = WIDTH / 2;
        paint_text(row, &CAPTION, b"REPLACE SENSOR", mid, 120, center, MUTED);
        paint_text(
            row,
            &BODY,
            kind(identity.device_type),
            mid,
            92,
            center,
            WHITE,
        );
        paint_icon(row, 98, 43, 2, identity.device_type);
    }

    /// A full-width pill, filled with the accent when it is the current choice.
    fn paint_button(
        &self,
        row: &mut Row,
        (top, height): (usize, usize),
        choice: Choice,
        label: &[u8],
    ) {
        let y = row.y();
        if !(top..top + height).contains(&y) {
            return;
        }
        let rect = tile(MARGIN, top, WIDTH - 2 * MARGIN, height);
        let fill = if self.choice == choice { CYAN } else { RAISED };
        paint_rounded(row, rect, height / 2, 0, |x, y| {
            match chrome::rounded(rect, height / 2, x, y) {
                0 => None,
                coverage => Some(blend(fill, BLACK, coverage)),
            }
        });
        let text_top = top + height / 2 - usize::from(BODY.top) - usize::from(BODY.cap) / 2;
        paint_text(row, &BODY, label, WIDTH / 2, text_top, Align::Center, WHITE);
    }

    /// "SENSOR " and the focused identity, cut to fit.
    fn identity_line(&self) -> ([u8; 24], usize) {
        let mut identity = [0_u8; 24];
        let text = b"SENSOR ";
        identity[..text.len()].copy_from_slice(text);
        let number = self.layout.identity.as_bytes();
        let len = text.len() + number.len().min(24 - text.len());
        identity[text.len()..len].copy_from_slice(&number[..len - text.len()]);
        (identity, len)
    }

    fn status_line(selected: Selected) -> &'static [u8] {
        if selected.link_status() == LinkStatus::Live {
            b"LIVE DATA"
        } else {
            let line = selected.status_text();
            &line[..line.len().min(22)]
        }
    }

    /// The per-pixel original of `paint`, kept as the reference it must match.
    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
        if x >= WIDTH || y >= HEIGHT {
            return 0;
        }
        match self.view {
            View::Tiles => self.tiles_pixel(x, y),
            View::Detail(slot) => self.selected[slot].map_or(0, |s| self.detail_pixel(x, y, s)),
            View::Replace(identity) => self.replace_pixel(x, y, identity),
        }
    }

    #[cfg(test)]
    fn tiles_pixel(&self, x: usize, y: usize) -> u16 {
        let feedback = &self.layout.page_feedback;
        let (heading, color): (&[u8], u16) = if feedback.is_empty() {
            (b"MY SENSORS", WHITE)
        } else {
            (feedback.as_bytes(), AMBER)
        };
        let left = Align::Left;
        if let Some(c) = write(
            x,
            y,
            &BODY,
            heading,
            MARGIN,
            HEADING_TOP,
            left,
            color,
            BLACK,
        ) {
            return c;
        }
        if let Some(c) = write(
            x, y, &CAPTION, b"NEARBY", MARGIN, NEARBY_TOP, left, MUTED, BLACK,
        ) {
            return c;
        }
        if self.scanning
            && let Some(c) = write(
                x,
                y,
                &CAPTION,
                b"SEARCHING",
                SEARCHING_LEFT,
                NEARBY_TOP,
                left,
                CYAN,
                BLACK,
            )
        {
            return c;
        }
        if let Some(color) = self.search_button_pixel(x, y) {
            return color;
        }
        if let Some((index, left, top)) = SELECTED_GRID.tile(x, y)
            && let Some(&slot) = self.layout.selected.get(index)
            && let Some(selected) = self.selected[slot]
        {
            return self.selected_tile_pixel(x, y, left, top, slot, selected);
        }
        if let Some((index, left, top)) = NEARBY_GRID.tile(x, y)
            && let Some(&(cursor, identity)) = self.layout.nearby.get(index)
        {
            let rect = tile(left, top, NEARBY_GRID.width, NEARBY_GRID.height);
            let Some(fill) = surface(rect, self.cursor == cursor, x, y) else {
                return BLACK;
            };
            if icon(x, y, left + 14, top + 6, identity.device_type) {
                return WHITE;
            }
            return fill;
        }
        let empty_top = SELECTED_GRID.top + 10;
        if self.layout.selected.is_empty()
            && let Some(c) = write(
                x,
                y,
                &CAPTION,
                b"NONE SELECTED",
                MARGIN,
                empty_top,
                left,
                MUTED,
                BLACK,
            )
        {
            return c;
        }
        let empty: &[u8] = if self.scanning {
            b"LOOKING NEARBY..."
        } else {
            b"NO SENSORS FOUND"
        };
        if self.layout.nearby.is_empty()
            && let Some(c) = write(
                x,
                y,
                &CAPTION,
                empty,
                MARGIN,
                NEARBY_GRID.top + 10,
                left,
                MUTED,
                BLACK,
            )
        {
            return c;
        }
        if let Some(c) = write(
            x,
            y,
            &BODY,
            self.layout.title,
            MARGIN,
            TITLE_TOP,
            left,
            WHITE,
            BLACK,
        ) {
            return c;
        }
        let status = self.layout.status.as_bytes();
        let color = self.layout.color;
        write(
            x, y, &CAPTION, status, MARGIN, STATUS_TOP, left, color, BLACK,
        )
        .unwrap_or(BLACK)
    }

    fn search_button_pixel(&self, x: usize, y: usize) -> Option<u16> {
        let (left, top) = SEARCH_BUTTON;
        let size = SEARCH_BUTTON_SIZE;
        let rect = tile(left - 1, top - 1, size + 2, size + 2);
        if !rect.contains(x, y) {
            return None;
        }
        let (cx, cy) = (4 * (left + size / 2) as i32, 4 * (top + size / 2) as i32);
        let focused = self.cursor == 0;
        let ring = disc(x, y, cx, cy, 4 * 13);
        let inner = disc(x, y, cx, cy, 4 * 11);
        let edge = blend(if focused { CYAN } else { CARD }, BLACK, ring);
        let fill = blend(if focused { RAISED } else { CARD }, edge, inner);
        if (left..left + size).contains(&x)
            && (top..top + size).contains(&y)
            && icons::refresh_row(y - top) & (1 << (x - left)) != 0
        {
            return Some(if self.scanning { CYAN } else { WHITE });
        }
        Some(fill)
    }

    #[cfg(test)]
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
        let rect = tile(left, top, SELECTED_GRID.width, SELECTED_GRID.height);
        let Some(fill) = surface(rect, focused, x, y) else {
            return BLACK;
        };
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
        fill
    }

    #[cfg(test)]
    fn detail_pixel(&self, x: usize, y: usize, selected: Selected) -> u16 {
        if scaled_icon(x, y, 98, 52, selected.identity.device_type) {
            return WHITE;
        }
        let center = Align::Center;
        let mid = WIDTH / 2;
        let name = kind(selected.identity.device_type);
        if let Some(c) = write(x, y, &BODY, name, mid, 108, center, WHITE, BLACK) {
            return c;
        }
        let status = selected.link_status();
        let line: &[u8] = if status == LinkStatus::Live {
            b"LIVE DATA"
        } else {
            let line = selected.status_text();
            &line[..line.len().min(22)]
        };
        if let Some(c) = write(
            x,
            y,
            &CAPTION,
            line,
            mid,
            140,
            center,
            status.color(),
            BLACK,
        ) {
            return c;
        }
        let mut identity = [0_u8; 24];
        let text = b"SENSOR ";
        identity[..text.len()].copy_from_slice(text);
        let number = self.layout.identity.as_bytes();
        let len = text.len() + number.len().min(24 - text.len());
        identity[text.len()..len].copy_from_slice(&number[..len - text.len()]);
        if let Some(c) = write(
            x,
            y,
            &CAPTION,
            &identity[..len],
            mid,
            162,
            center,
            MUTED,
            BLACK,
        ) {
            return c;
        }
        // Disconnect is the only removal; ordinary loss reconnects by itself.
        if let Some(color) =
            self.button_pixel(x, y, DETAIL_BUTTONS[0], Choice::Primary, b"DISCONNECT")
        {
            return color;
        }
        if let Some(color) = self.button_pixel(x, y, DETAIL_BUTTONS[1], Choice::Secondary, b"BACK")
        {
            return color;
        }
        BLACK
    }

    #[cfg(test)]
    fn replace_pixel(&self, x: usize, y: usize, identity: Identity) -> u16 {
        if scaled_icon(x, y, 98, 43, identity.device_type) {
            return WHITE;
        }
        let center = Align::Center;
        let mid = WIDTH / 2;
        let name = kind(identity.device_type);
        if let Some(c) = write(x, y, &BODY, name, mid, 92, center, WHITE, BLACK) {
            return c;
        }
        if let Some(c) = write(
            x,
            y,
            &CAPTION,
            b"REPLACE SENSOR",
            mid,
            120,
            center,
            MUTED,
            BLACK,
        ) {
            return c;
        }
        if (140..200).contains(&y) {
            // Current peer on the left half, replacement on the right.
            let index = usize::from(x >= WIDTH / 2);
            let center_x = [60, 180][index];
            let number = self.layout.replacement_numbers[index].as_bytes();
            let transmission = self.layout.replacement_transmissions[index].as_bytes();
            if let Some(c) = write(x, y, &VALUE, number, center_x, 140, center, WHITE, BLACK)
                .or_else(|| {
                    write(
                        x,
                        y,
                        &CAPTION,
                        transmission,
                        center_x,
                        172,
                        center,
                        WHITE,
                        BLACK,
                    )
                })
                .or_else(|| {
                    let label: &[u8] = if index == 0 { b"CURRENT" } else { b"NEW" };
                    write(x, y, &CAPTION, label, center_x, 188, center, MUTED, BLACK)
                })
            {
                return c;
            }
        }
        if arrow(x, y) {
            return CYAN;
        }
        if let Some(color) =
            self.button_pixel(x, y, REPLACE_BUTTONS[0], Choice::Primary, b"REPLACE")
        {
            return color;
        }
        if let Some(color) =
            self.button_pixel(x, y, REPLACE_BUTTONS[1], Choice::Secondary, b"KEEP CURRENT")
        {
            return color;
        }
        BLACK
    }

    /// A full-width pill, filled with the accent when it is the current choice.
    #[cfg(test)]
    fn button_pixel(
        &self,
        x: usize,
        y: usize,
        (top, height): (usize, usize),
        choice: Choice,
        label: &[u8],
    ) -> Option<u16> {
        let rect = tile(MARGIN, top, WIDTH - 2 * MARGIN, height);
        let coverage = chrome::rounded(rect, height / 2, x, y);
        if coverage == 0 {
            return None;
        }
        let fill = if self.choice == choice { CYAN } else { RAISED };
        let base = blend(fill, BLACK, coverage);
        let text_top = top + height / 2 - usize::from(BODY.top) - usize::from(BODY.cap) / 2;
        let mid = WIDTH / 2;
        Some(
            write(
                x,
                y,
                &BODY,
                label,
                mid,
                text_top,
                Align::Center,
                WHITE,
                base,
            )
            .unwrap_or(base),
        )
    }
}

const fn tile(left: usize, top: usize, width: usize, height: usize) -> Rect {
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// A rounded tile: raised and ringed with the accent when focused.
fn surface(rect: Rect, focused: bool, x: usize, y: usize) -> Option<u16> {
    if focused {
        card(rect, TILE_RADIUS, x, y, RAISED, Some(CYAN))
    } else {
        card(rect, TILE_RADIUS, x, y, CARD, None)
    }
}

/// The current-to-new arrow between the two replacement identities.
#[cfg(test)]
fn arrow(x: usize, y: usize) -> bool {
    ((103..130).contains(&x) && (155..157).contains(&y))
        || ((121..131).contains(&x) && ((y as isize - 156).unsigned_abs() == 130 - x))
}

#[cfg(test)]
fn scaled_icon(x: usize, y: usize, left: usize, top: usize, kind: u8) -> bool {
    x >= left && y >= top && icons::pixel(kind, (x - left) / 2, (y - top) / 2)
}

#[cfg(test)]
fn icon(x: usize, y: usize, left: usize, top: usize, kind: u8) -> bool {
    x >= left && y >= top && icons::pixel(kind, x - left, y - top)
}
