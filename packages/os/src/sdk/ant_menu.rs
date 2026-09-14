//! Physical-button ANT selection. Discovery order stays stable until an explicit scan.
use super::radar_screen::{number, text};
use crate::ant::{Discovery, Identity, LinkState, Snapshot};
use crate::capabilities::{Button, Input};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Scan,
    Connect(Identity),
    Disconnect(u8),
    Start,
    Ride,
}

pub struct Menu {
    discoveries: [Option<Discovery>; 8],
    present: [bool; 8],
    channels: [Option<Snapshot>; 3],
    cursor: usize,
    last_press: Option<u64>,
    scanning: bool,
    needs_navigation: bool,
    message: &'static [u8],
}
impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}
impl Menu {
    pub const fn new() -> Self {
        Self {
            discoveries: [None; 8],
            present: [false; 8],
            channels: [None; 3],
            cursor: 0,
            last_press: None,
            scanning: false,
            needs_navigation: false,
            message: b"SCAN THEN PICK SENSOR",
        }
    }
    pub fn set_message(&mut self, message: &'static [u8]) {
        self.message = message;
    }
    pub fn refresh(
        &mut self,
        discoveries: [Option<Discovery>; 8],
        channels: [Option<Snapshot>; 3],
        scanning: bool,
        _now: u64,
    ) {
        self.present = [false; 8];
        for discovery in discoveries.into_iter().flatten() {
            let slot = self
                .discoveries
                .iter()
                .position(|old| old.is_some_and(|old| old.identity == discovery.identity))
                .or_else(|| self.discoveries.iter().position(Option::is_none));
            if let Some(slot) = slot {
                self.discoveries[slot] = Some(discovery);
                self.present[slot] = true;
            }
        }
        self.channels = channels;
        self.scanning = scanning;
        if !self.visible(self.cursor) {
            self.cursor = 0;
        }
    }
    pub fn input(&mut self, input: Input, now: u64) -> Option<Action> {
        if input == Input::Cancel {
            self.cursor = 0;
            self.needs_navigation = true;
            self.last_press = Some(now);
            self.message = b"INPUT LOST - SELECT AGAIN";
            return None;
        }
        let Input::Button { button, code: 1 } = input else {
            return None;
        };
        if self
            .last_press
            .is_some_and(|last| now.saturating_sub(last) < 350)
        {
            return None;
        }
        self.last_press = Some(now);
        match button {
            Button::Center => None,
            Button::TopLeft => {
                if self.cursor == 0 {
                    return Some(Action::Ride);
                }
                self.cursor = 0;
                None
            }
            Button::BottomLeft => {
                self.needs_navigation = false;
                loop {
                    self.cursor = (self.cursor + 1) % 14;
                    if self.visible(self.cursor) {
                        break;
                    }
                }
                None
            }
            Button::BottomRight if self.needs_navigation => None,
            Button::BottomRight => match self.cursor {
                0 => {
                    if self.scanning {
                        self.message = b"SCAN IN PROGRESS";
                        return None;
                    }
                    self.discoveries = [None; 8];
                    self.present = [false; 8];
                    self.message = b"STARTING SCAN";
                    Some(Action::Scan)
                }
                1..=8 => {
                    let slot = self.cursor - 1;
                    if !self.present[slot] {
                        self.message = b"PEER GONE - SCAN AGAIN";
                        return None;
                    }
                    self.discoveries[slot].map(|peer| Action::Connect(peer.identity))
                }
                9..=11 => self.channels[self.cursor - 9]
                    .and_then(|channel| channel.selected)
                    .map(|peer| Action::Disconnect(peer.device_type)),
                12 => Some(Action::Start),
                13 => Some(Action::Ride),
                _ => None,
            },
        }
    }
    fn visible(&self, row: usize) -> bool {
        match row {
            0 | 12 | 13 => true,
            1..=8 => self.discoveries[row - 1].is_some(),
            9..=11 => self.channels[row - 9].is_some_and(|s| s.selected.is_some()),
            _ => false,
        }
    }
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        const WHITE: u16 = 0xffff;
        const GREEN: u16 = 0x07e0;
        const YELLOW: u16 = 0xffe0;
        if x >= 240 || y >= 320 {
            return 0;
        }
        if text(x, y, 24, 8, 2, b"ANT SENSOR MENU") {
            return WHITE;
        }
        if text(x, y, 8, 31, 1, b"PICK THE NUMBER ON YOUR SENSOR") {
            return WHITE;
        }
        let mut rows = [0usize; 14];
        let mut count = 0;
        let mut selected = 0;
        for row in 0..14 {
            if self.visible(row) {
                if row == self.cursor {
                    selected = count;
                }
                rows[count] = row;
                count += 1;
            }
        }
        let first = selected.saturating_sub(6);
        for (index, &row) in rows[first..count.min(first + 7)].iter().enumerate() {
            let top = 49 + index * 29;
            if row == self.cursor && text(x, y, 4, top, 2, b">") {
                return GREEN;
            }
            let color = if row == self.cursor { GREEN } else { WHITE };
            let label: &[u8] = match row {
                0 => {
                    if self.scanning {
                        b"SCANNING..."
                    } else {
                        b"SCAN SENSORS"
                    }
                }
                12 => b"START RIDE TEST",
                13 => b"RIDE STATUS",
                _ => b"",
            };
            if text(x, y, 20, top, 2, label) {
                return color;
            }
            if (1..=8).contains(&row) {
                let peer = self.discoveries[row - 1].unwrap();
                let age_label: &[u8] = if self.present[row - 1] {
                    b"RSSI"
                } else {
                    b"GONE"
                };
                if text(x, y, 20, top, 1, kind(peer.identity.device_type))
                    || number(x, y, 80, top, 1, u32::from(peer.identity.device_number), 5)
                    || text(x, y, 116, top, 1, b"TX")
                    || number(
                        x,
                        y,
                        134,
                        top,
                        1,
                        u32::from(peer.identity.transmission_type),
                        3,
                    )
                    || text(x, y, 164, top, 1, b"T")
                    || number(x, y, 176, top, 1, u32::from(peer.identity.device_type), 3)
                    || text(x, y, 20, top + 12, 1, age_label)
                    || (self.present[row - 1]
                        && text(
                            x,
                            y,
                            56,
                            top + 12,
                            1,
                            if peer.rssi < 0 { b"-" } else { b"" },
                        ))
                    || (self.present[row - 1]
                        && number(
                            x,
                            y,
                            62,
                            top + 12,
                            1,
                            u32::from(peer.rssi.unsigned_abs()),
                            3,
                        ))
                {
                    return if self.present[row - 1] { color } else { YELLOW };
                }
            }
            if (9..=11).contains(&row) {
                let channel = self.channels[row - 9].unwrap();
                let peer = channel.selected.unwrap();
                let status: &[u8] = match channel.link {
                    LinkState::Connecting => b"CONNECTING",
                    LinkState::Connected if channel.stale || channel.age_ms.is_none() => b"STALE",
                    LinkState::Connected => b"FRESH",
                    LinkState::Disconnecting => b"STOPPING",
                    _ => b"OFF",
                };
                if text(x, y, 20, top, 1, b"DROP")
                    || text(x, y, 50, top, 1, kind(peer.device_type))
                    || number(x, y, 110, top, 1, u32::from(peer.device_number), 5)
                    || text(x, y, 20, top + 12, 1, status)
                {
                    return color;
                }
            }
        }
        if text(x, y, 8, 261, 1, &self.message[..self.message.len().min(37)]) {
            return YELLOW;
        }
        if text(x, y, 8, 282, 1, b"BOTTOM LEFT NEXT")
            || text(x, y, 8, 294, 1, b"BOTTOM RIGHT CHOOSE")
            || text(x, y, 8, 306, 1, b"TOP LEFT BACK")
        {
            return WHITE;
        }
        0
    }
}
fn kind(device_type: u8) -> &'static [u8] {
    match device_type {
        40 => b"RADAR",
        11 => b"POWER",
        120 => b"HEART",
        _ => b"SENSOR",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_navigation_cannot_choose_until_a_new_navigation_press() {
        let mut menu = Menu::new();
        assert_eq!(menu.input(Input::Cancel, 1000), None);
        assert_eq!(menu.input(press(Button::BottomRight), 1000), None);
        assert_eq!(menu.input(press(Button::BottomRight), 2000), None);
        assert_eq!(menu.input(press(Button::BottomLeft), 2400), None);
        assert_eq!(
            menu.input(press(Button::BottomRight), 2800),
            Some(Action::Start)
        );
    }

    fn press(button: Button) -> Input {
        Input::Button { button, code: 1 }
    }
    fn peer(number: u16) -> Discovery {
        Discovery {
            identity: Identity {
                device_type: 11,
                device_number: number,
                transmission_type: 1,
            },
            rssi: -40,
            seen_ms: 0,
        }
    }
    #[test]
    fn refresh_keeps_cursor_identity_and_never_connects_automatically() {
        let mut menu = Menu::new();
        menu.refresh(
            [
                Some(peer(10)),
                Some(peer(20)),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            [None; 3],
            false,
            0,
        );
        assert_eq!(menu.input(press(Button::BottomLeft), 0), None);
        menu.refresh(
            [
                Some(peer(20)),
                Some(peer(10)),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            [None; 3],
            false,
            400,
        );
        assert_eq!(
            menu.input(press(Button::BottomRight), 400),
            Some(Action::Connect(peer(10).identity))
        );
        menu.refresh(
            [Some(peer(20)), None, None, None, None, None, None, None],
            [None; 3],
            false,
            800,
        );
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    }
    #[test]
    fn holds_and_debounce_do_not_navigate_or_select() {
        let mut menu = Menu::new();
        for code in [4, 5] {
            assert_eq!(
                menu.input(
                    Input::Button {
                        button: Button::BottomLeft,
                        code
                    },
                    0
                ),
                None
            );
        }
        assert_eq!(menu.input(press(Button::BottomLeft), 0), None);
        assert_eq!(menu.input(press(Button::BottomRight), 349), None);
        assert_eq!(
            menu.input(press(Button::BottomRight), 350),
            Some(Action::Start)
        );
        assert_eq!(menu.input(press(Button::TopLeft), 700), None);
        assert_eq!(menu.input(press(Button::TopLeft), 1050), Some(Action::Ride));
    }
}
