//! The per-pixel renderer `Menu::paint` replaced, kept as the reference its
//! rows must match exactly. Delete it when a layout changes on purpose.
use super::layout::{NEARBY_GRID, SELECTED_GRID};
use super::menu::{Choice, Menu, View};
use super::render::*;
use super::slot::{LinkStatus, Selected};
use super::{icons, kind};
use device_api::ant::Identity;
use firmware_shell::rendering::text::text;

const DIVIDER: u16 = 0x39e7;
const WIDTH: usize = 240;
const HEIGHT: usize = 320;
const GLYPH: usize = 6;
const MARGIN: usize = 12;
const FOOTER_DIVIDER_Y: usize = 290;
const FOOTER_Y: usize = 300;
const SEARCH_BUTTON: (usize, usize) = (208, 130);
const SEARCH_BUTTON_SIZE: usize = 24;
const STATUS_INDICATOR_SIZE: usize = 7;

impl Menu {
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
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
            View::Tiles => self.reference_tiles_pixel(x, y),
            View::Detail(slot) => {
                self.selected[slot].map_or(0, |s| self.reference_detail_pixel(x, y, s))
            }
            View::Replace(identity) => self.reference_replace_pixel(x, y, identity),
        }
    }

    fn reference_tiles_pixel(&self, x: usize, y: usize) -> u16 {
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
        if let Some(color) = self.reference_search_button_pixel(x, y) {
            return color;
        }
        if let Some((index, left, top)) = SELECTED_GRID.tile(x, y)
            && let Some(&slot) = self.layout.selected.get(index)
            && let Some(selected) = self.selected[slot]
        {
            return self.reference_selected_tile_pixel(x, y, left, top, slot, selected);
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

    fn reference_search_button_pixel(&self, x: usize, y: usize) -> Option<u16> {
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

    fn reference_selected_tile_pixel(
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

    fn reference_detail_pixel(&self, x: usize, y: usize, selected: Selected) -> u16 {
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
        // Disconnect is the only removal; ordinary loss reconnects by itself.
        if let Some(color) =
            self.reference_button_pixel(x, y, 220, 34, Choice::Primary, b"DISCONNECT")
        {
            return color;
        }
        if let Some(color) = self.reference_button_pixel(x, y, 262, 28, Choice::Secondary, b"BACK")
        {
            return color;
        }
        0
    }

    fn reference_replace_pixel(&self, x: usize, y: usize, identity: Identity) -> u16 {
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
        if let Some(color) = self.reference_button_pixel(x, y, 216, 34, Choice::Primary, b"REPLACE")
        {
            return color;
        }
        if let Some(color) =
            self.reference_button_pixel(x, y, 258, 32, Choice::Secondary, b"KEEP CURRENT")
        {
            return color;
        }
        0
    }

    /// A full-width button, highlighted when it is the current choice.
    fn reference_button_pixel(
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

#[cfg(test)]
mod tests {
    use super::super::{Connection, MenuMessage, Selected};
    use super::*;
    use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery};
    use device_api::input::{Button, Input};
    use firmware_shell::rendering::row::Row;

    fn assert_frame(menu: &Menu, case: &str) {
        for y in 0..330 {
            let mut pixels = [0u16; 240];
            menu.paint(&mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, menu.reference_pixel(x, y), "{case} at {x},{y}");
            }
        }
    }

    fn peer(device_type: u8, number: u16) -> Discovery {
        Discovery {
            identity: Identity {
                device_type,
                device_number: number,
                transmission_type: 1,
            },
            rssi: -40,
            seen_ms: 0,
        }
    }

    fn press(menu: &mut Menu, button: Button, now: &mut u64) {
        *now += 400;
        let _ = menu.input(Input::Button { button, code: 1 }, *now);
    }

    const CONNECTIONS: [Connection; 9] = [
        Connection::Live,
        Connection::WaitingForData,
        Connection::DataLost,
        Connection::Searching,
        Connection::NotFound,
        Connection::WaitingForRadio,
        Connection::Replacing,
        Connection::RadioUnavailable,
        Connection::Unsupported,
    ];

    #[test]
    fn empty_and_message_rows_match_the_per_pixel_reference() {
        for scanning in [false, true] {
            let mut menu = Menu::new();
            menu.refresh(
                [None; DISCOVERY_CAPACITY],
                [None; CHANNEL_CAPACITY],
                scanning,
            );
            assert_frame(&menu, "empty");
            for message in [
                MenuMessage::Scanning,
                MenuMessage::ScanDone,
                MenuMessage::ScanCancelled,
                MenuMessage::StartingScan,
                MenuMessage::SearchPending,
                MenuMessage::StoppingScan,
                MenuMessage::FinishingScan,
                MenuMessage::NotSeen,
                MenuMessage::SlotsFull,
                MenuMessage::RadioLost,
            ] {
                menu.set_message(message);
                assert_frame(&menu, "message");
            }
            menu.set_notice(Some(MenuMessage::NotSaved));
            assert_frame(&menu, "notice");
        }
    }

    #[test]
    fn full_grid_rows_match_the_per_pixel_reference_at_every_cursor() {
        let kinds = [120, 11, 123, 40, 122, 121, 34, 17, 35, 1];
        for shift in 0..CONNECTIONS.len() {
            let selected: [Option<Selected>; CHANNEL_CAPACITY] = core::array::from_fn(|n| {
                (n % 4 != 3).then(|| Selected {
                    identity: peer(kinds[n % kinds.len()], n as u16 + 1).identity,
                    connection: CONNECTIONS[(n + shift) % CONNECTIONS.len()],
                })
            });
            let nearby =
                core::array::from_fn(|n| Some(peer(kinds[n % kinds.len()], 100 + n as u16)));
            let mut menu = Menu::new();
            menu.refresh(nearby, selected, true);
            let mut now = 0;
            for step in 0..20 {
                assert_frame(&menu, "tiles");
                // Open whatever the cursor points at, look, and come back.
                press(&mut menu, Button::BottomRight, &mut now);
                assert_frame(&menu, "opened");
                press(&mut menu, Button::BottomLeft, &mut now);
                assert_frame(&menu, "toggled");
                press(&mut menu, Button::TopLeft, &mut now);
                menu.refresh(nearby, selected, step % 2 == 0);
                press(&mut menu, Button::BottomLeft, &mut now);
            }
        }
    }

    #[test]
    fn replacement_rows_match_the_per_pixel_reference() {
        for (number, transmission) in [(u16::MAX, 254), (7, 1), (420, 0)] {
            let current = Identity {
                device_type: 11,
                device_number: number,
                transmission_type: transmission,
            };
            let replacement = Identity {
                transmission_type: transmission.wrapping_add(1),
                ..current
            };
            let mut menu = Menu::new();
            let mut discovery = peer(11, number);
            discovery.identity = replacement;
            let mut selected = [None; CHANNEL_CAPACITY];
            selected[0] = Some(Selected {
                identity: current,
                connection: Connection::NotFound,
            });
            menu.refresh(
                core::array::from_fn(|n| (n == 0).then_some(discovery)),
                selected,
                false,
            );
            menu.focus_type(11);
            let mut now = 0;
            press(&mut menu, Button::BottomLeft, &mut now);
            press(&mut menu, Button::BottomRight, &mut now);
            assert_frame(&menu, "replace");
            press(&mut menu, Button::BottomLeft, &mut now);
            assert_frame(&menu, "replace toggled");
        }
    }
}
