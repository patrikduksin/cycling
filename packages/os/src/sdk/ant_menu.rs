//! Physical-button ANT selection. Discovery order stays stable until an explicit scan.
use crate::ant::{Discovery, Identity, LinkState, Snapshot};
use crate::capabilities::{Button, Input};
use crate::ui_text::{number, text};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Scan,
    Connect(Identity),
    Disconnect(u8),
    Ride,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostics {
    pub cursor: usize,
    pub last_press: Option<u64>,
    pub needs_nav: bool,
    pub message: &'static [u8],
}

pub struct Menu {
    discoveries: [Option<Discovery>; 8],
    present: [bool; 8],
    channels: [Option<Snapshot>; crate::ant::CHANNEL_CAPACITY],
    cursor: usize,
    visible_rows: [usize; 5],
    visible_count: usize,
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
            channels: [None; crate::ant::CHANNEL_CAPACITY],
            cursor: 0,
            visible_rows: [0; 5],
            visible_count: 1,
            last_press: None,
            scanning: false,
            needs_navigation: false,
            message: b"SCAN THEN PICK SENSOR",
        }
    }
    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            cursor: self.cursor,
            last_press: self.last_press,
            needs_nav: self.needs_navigation,
            message: self.message,
        }
    }
    fn layout(&mut self) {
        let mut rows = [0; 13];
        let mut count = 0;
        let mut selected: usize = 0;
        for row in 0..13 {
            if self.visible(row) {
                if row == self.cursor {
                    selected = count;
                }
                rows[count] = row;
                count += 1;
            }
        }
        let capacity = self.visible_rows.len();
        let first = selected.saturating_sub(capacity - 1);
        self.visible_count = (count - first).min(capacity);
        self.visible_rows[..self.visible_count]
            .copy_from_slice(&rows[first..first + self.visible_count]);
    }
    pub fn set_message(&mut self, message: &'static [u8]) {
        self.message = message;
    }
    pub fn refresh(
        &mut self,
        discoveries: [Option<Discovery>; 8],
        channels: [Option<Snapshot>; crate::ant::CHANNEL_CAPACITY],
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
        self.layout();
    }
    pub fn input(&mut self, input: Input, now: u64) -> Option<Action> {
        let result = self.handle_input(input, now);
        self.layout();
        result
    }
    fn handle_input(&mut self, input: Input, now: u64) -> Option<Action> {
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
                    self.cursor = (self.cursor + 1) % 13;
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
                9..=12 => self.channels[self.cursor - 9]
                    .and_then(|channel| channel.selected)
                    .map(|peer| Action::Disconnect(peer.device_type)),
                _ => None,
            },
        }
    }
    fn visible(&self, row: usize) -> bool {
        match row {
            0 => true,
            1..=8 => self.discoveries[row - 1].is_some(),
            9..=12 => self.channels[row - 9].is_some_and(|s| s.selected.is_some()),
            _ => false,
        }
    }
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        if x >= 240 || y >= 320 {
            return 0;
        }
        let cyan = 0x07ff;
        let white = 0xffff;
        if (39..249).contains(&y) {
            let index = (y - 39) / 42;
            if index >= self.visible_count {
                return 0;
            }
            let row = self.visible_rows[index];
            let top = 39 + index * 42;
            let color = if row == self.cursor { cyan } else { white };
            if row == self.cursor && text(x, y, 3, top, 2, b">") {
                return cyan;
            }
            if row == 0
                && text(
                    x,
                    y,
                    22,
                    top,
                    2,
                    if self.scanning {
                        b"SCANNING..."
                    } else {
                        b"SCAN SENSORS"
                    },
                )
            {
                return color;
            }
            if (1..=8).contains(&row) {
                let peer = self.discoveries[row - 1].unwrap();
                if text(x, y, 22, top, 2, kind(peer.identity.device_type))
                    || number(x, y, 108, top, 2, u32::from(peer.identity.device_number), 5)
                    || text(
                        x,
                        y,
                        22,
                        top + 20,
                        1,
                        if self.present[row - 1] {
                            b"SELECT TO CONNECT"
                        } else {
                            b"NOT SEEN - SCAN AGAIN"
                        },
                    )
                {
                    return color;
                }
            }
            if (9..=12).contains(&row) {
                let channel = self.channels[row - 9].unwrap();
                let peer = channel.selected.unwrap();
                if text(x, y, 22, top, 2, kind(peer.device_type))
                    || text(x, y, 108, top, 2, b"DROP")
                    || text(
                        x,
                        y,
                        22,
                        top + 20,
                        1,
                        if channel.link == LinkState::Connected && !channel.stale {
                            b"CONNECTED"
                        } else {
                            b"WAITING"
                        },
                    )
                {
                    return color;
                }
            }
        }
        if text(x, y, 8, 260, 2, &self.message[..self.message.len().min(19)]) {
            return 0xffe0;
        }
        if text(x, y, 8, 300, 2, b"NEXT") || text(x, y, 156, 300, 2, b"SELECT") {
            return white;
        }
        if text(x, y, 8, 283, 1, b"TOP LEFT BACK") {
            return white;
        }
        0
    }
}
fn kind(device_type: u8) -> &'static [u8] {
    match device_type {
        40 => b"RADAR",
        11 => b"POWER",
        120 => b"HEART",
        121 | 123 => b"SPEED",
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
            Some(Action::Scan)
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
            [None; crate::ant::CHANNEL_CAPACITY],
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
            [None; crate::ant::CHANNEL_CAPACITY],
            false,
            400,
        );
        assert_eq!(
            menu.input(press(Button::BottomRight), 400),
            Some(Action::Connect(peer(10).identity))
        );
        menu.refresh(
            [Some(peer(20)), None, None, None, None, None, None, None],
            [None; crate::ant::CHANNEL_CAPACITY],
            false,
            800,
        );
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    }
    #[test]
    fn holds_and_debounce_do_not_navigate_or_select() {
        let mut menu = Menu::new();
        menu.refresh(
            [Some(peer(10)), None, None, None, None, None, None, None],
            [None; crate::ant::CHANNEL_CAPACITY],
            false,
            0,
        );
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
            Some(Action::Connect(peer(10).identity))
        );
        assert_eq!(menu.input(press(Button::TopLeft), 700), None);
        assert_eq!(menu.input(press(Button::TopLeft), 1050), Some(Action::Ride));
    }
}
