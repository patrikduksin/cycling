//! Sensor selection and physical-button navigation, independent of radio scheduling.
use device_api::ant::{
    CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity, LinkState, Snapshot,
};
use device_api::input::{Button, Input};
use firmware_shell::rendering::text::{number, text};

const AVAILABLE: usize = 1 + CHANNEL_CAPACITY;
const ITEMS: usize = AVAILABLE + DISCOVERY_CAPACITY;
const WHITE: u16 = 0xffff;
const CYAN: u16 = 0x07ff;
const MUTED: u16 = 0x9cf3;
const GREEN: u16 = 0x5fe9;
const AMBER: u16 = 0xfd20;

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
#[derive(Clone, Copy)]
struct Selected {
    identity: Identity,
    observed: Option<Snapshot>,
    pending: Option<(bool, bool)>, // disconnect, admitted
    failure: Option<&'static [u8]>,
    failed_disconnect: bool,
}
impl Selected {
    fn status(self) -> &'static [u8] {
        if let Some(failure) = self.failure {
            return failure;
        }
        if let Some((_, false)) = self.pending {
            return b"WAITING FOR RADIO";
        }
        if let Some((disconnect, _)) = self.pending {
            return if disconnect {
                b"DISCONNECTING"
            } else {
                b"CONNECTING"
            };
        }
        self.observed
            .map_or(b"DISCONNECTED", |s| connection_status(s).as_bytes())
    }
    fn retry(self) -> bool {
        self.failure.is_some()
            || (self.pending.is_none()
                && self.observed.is_none_or(|s| {
                    matches!(
                        s.link,
                        LinkState::Idle
                            | LinkState::Disconnected
                            | LinkState::TimedOut
                            | LinkState::TransportLost
                    )
                }))
    }
    fn can_disconnect(self) -> bool {
        self.observed.is_some_and(|s| {
            !matches!(
                s.link,
                LinkState::Idle | LinkState::Disconnected | LinkState::Disconnecting
            )
        })
    }
    fn color(self) -> u16 {
        if self.failure.is_some() || self.retry() {
            AMBER
        } else if self.pending.is_some()
            || self
                .observed
                .is_some_and(|s| matches!(s.link, LinkState::Connecting | LinkState::Disconnecting))
        {
            CYAN
        } else if self
            .observed
            .is_some_and(|s| s.link == LinkState::Connected && !s.stale && s.age_ms.is_some())
        {
            GREEN
        } else {
            AMBER
        }
    }
}
#[derive(Clone, Copy)]
enum View {
    Tiles,
    Detail(usize),
    Replace(Identity),
}
pub struct Menu {
    discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    present: [bool; DISCOVERY_CAPACITY],
    selected: [Option<Selected>; CHANNEL_CAPACITY],
    cursor: usize,
    view: View,
    detail_action: usize,
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
            discoveries: [None; DISCOVERY_CAPACITY],
            present: [false; DISCOVERY_CAPACITY],
            selected: [None; CHANNEL_CAPACITY],
            cursor: 0,
            view: View::Tiles,
            detail_action: 0,
            last_press: None,
            scanning: false,
            needs_navigation: false,
            message: b"",
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
    pub fn set_message(&mut self, message: &'static [u8]) {
        self.message = message;
    }
    pub fn selected_identity(&self, kind: u8) -> Option<Identity> {
        self.selected
            .iter()
            .flatten()
            .find(|s| s.identity.device_type == kind)
            .map(|s| s.identity)
    }
    pub fn request_pending(&mut self, identity: Identity) {
        let slot = self
            .selected
            .iter()
            .position(|s| s.is_some_and(|s| s.identity.device_type == identity.device_type))
            .or_else(|| self.selected.iter().position(Option::is_none));
        if let Some(slot) = slot {
            self.selected[slot] = Some(Selected {
                identity,
                observed: None,
                pending: Some((false, false)),
                failure: None,
                failed_disconnect: false,
            });
            self.cursor = 1 + slot;
        }
    }
    pub fn request_accepted(&mut self, identity: Identity) {
        if let Some(s) = self
            .selected
            .iter_mut()
            .flatten()
            .find(|s| s.identity == identity)
            && let Some((_, admitted)) = &mut s.pending
        {
            *admitted = true;
        }
    }
    pub fn request_failed(&mut self, identity: Identity, message: &'static [u8]) {
        if let Some(s) = self
            .selected
            .iter_mut()
            .flatten()
            .find(|s| s.identity == identity)
        {
            s.failed_disconnect = s.pending.is_some_and(|(disconnect, _)| disconnect);
            s.pending = None;
            s.failure = Some(message);
        }
    }
    pub fn refresh(
        &mut self,
        discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
        channels: [Option<Snapshot>; CHANNEL_CAPACITY],
        scanning: bool,
        _now: u64,
    ) {
        self.present = [false; DISCOVERY_CAPACITY];
        for discovery in discoveries.into_iter().flatten() {
            let slot = self
                .discoveries
                .iter()
                .position(|d| d.is_some_and(|d| d.identity == discovery.identity))
                .or_else(|| self.discoveries.iter().position(Option::is_none));
            if let Some(slot) = slot {
                self.discoveries[slot] = Some(discovery);
                self.present[slot] = true;
            }
        }
        for snapshot in channels.into_iter().flatten() {
            let Some(identity) = snapshot.selected else {
                continue;
            };
            let slot = self
                .selected
                .iter()
                .position(|s| s.is_some_and(|s| s.identity.device_type == identity.device_type))
                .or_else(|| self.selected.iter().position(Option::is_none));
            if let Some(slot) = slot {
                match &mut self.selected[slot] {
                    Some(selected) if selected.identity == identity => {
                        selected.observed = Some(snapshot);
                        if let Some((disconnect, true)) = selected.pending {
                            let observed = if disconnect {
                                matches!(
                                    snapshot.link,
                                    LinkState::Disconnecting
                                        | LinkState::Disconnected
                                        | LinkState::TransportLost
                                )
                            } else {
                                matches!(
                                    snapshot.link,
                                    LinkState::Connecting
                                        | LinkState::Connected
                                        | LinkState::TimedOut
                                        | LinkState::TransportLost
                                )
                            };
                            if observed {
                                selected.pending = None;
                            }
                        }
                    }
                    None => {
                        self.selected[slot] = Some(Selected {
                            identity,
                            observed: Some(snapshot),
                            pending: None,
                            failure: None,
                            failed_disconnect: false,
                        })
                    }
                    _ => {}
                }
            }
        }
        self.scanning = scanning;
        if !self.visible(self.cursor) {
            self.cursor = 0;
        }
    }
    pub fn input(&mut self, input: Input, now: u64) -> Option<Action> {
        if input == Input::Cancel {
            self.needs_navigation = true;
            self.last_press = Some(now);
            self.message = b"INPUT LOST - PRESS NEXT";
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
            Button::TopLeft => match self.view {
                View::Tiles => Some(Action::Ride),
                _ => {
                    self.view = View::Tiles;
                    self.detail_action = 0;
                    None
                }
            },
            Button::BottomLeft => {
                self.needs_navigation = false;
                match self.view {
                    View::Tiles => loop {
                        self.cursor = (self.cursor + 1) % ITEMS;
                        if self.visible(self.cursor) {
                            break;
                        }
                    },
                    View::Detail(slot)
                        if self.selected[slot].is_some_and(|s| s.pending.is_some()) =>
                    {
                        self.detail_action = 0
                    }
                    _ => self.detail_action ^= 1,
                }
                None
            }
            Button::BottomRight if self.needs_navigation => None,
            Button::BottomRight => self.activate(),
        }
    }
    fn activate(&mut self) -> Option<Action> {
        match self.view {
            View::Detail(slot) => {
                let mut selected = self.selected[slot]?;
                if selected.pending.is_some()
                    || self.detail_action == 1
                        && (!selected.retry()
                            || !selected.can_disconnect()
                            || selected.failed_disconnect)
                {
                    self.view = View::Tiles;
                    return None;
                }
                self.view = View::Tiles;
                if self.detail_action == 0 && selected.retry() && !selected.failed_disconnect {
                    self.request_pending(selected.identity);
                    Some(Action::Connect(selected.identity))
                } else {
                    selected.pending = Some((true, false));
                    selected.failure = None;
                    self.selected[slot] = Some(selected);
                    Some(Action::Disconnect(selected.identity.device_type))
                }
            }
            View::Replace(identity) => {
                self.view = View::Tiles;
                if self.detail_action == 1 {
                    return None;
                }
                self.request_pending(identity);
                Some(Action::Connect(identity))
            }
            View::Tiles if self.cursor == 0 => {
                if self.scanning {
                    return None;
                }
                self.discoveries = [None; DISCOVERY_CAPACITY];
                self.present = [false; DISCOVERY_CAPACITY];
                self.message = b"";
                Some(Action::Scan)
            }
            View::Tiles if self.cursor < AVAILABLE => {
                self.view = View::Detail(self.cursor - 1);
                self.detail_action = 0;
                None
            }
            View::Tiles => {
                let index = self.cursor - AVAILABLE;
                let identity = self.discoveries[index]?.identity;
                if !self.present[index] {
                    self.message = b"NOT SEEN - SEARCH AGAIN";
                    return None;
                }
                if self
                    .selected
                    .iter()
                    .flatten()
                    .any(|s| s.identity.device_type == identity.device_type && s.pending.is_some())
                {
                    self.message = b"WAIT FOR CURRENT REQUEST";
                    return None;
                }
                if self.selected_identity(identity.device_type).is_some() {
                    self.view = View::Replace(identity);
                    self.detail_action = 0;
                    return None;
                }
                if self.selected.iter().all(Option::is_some) {
                    self.message = b"ALL TEN SENSOR SLOTS ARE IN USE";
                    return None;
                }
                self.request_pending(identity);
                Some(Action::Connect(identity))
            }
        }
    }
    fn visible(&self, item: usize) -> bool {
        if item == 0 {
            true
        } else if item < AVAILABLE {
            self.selected[item - 1].is_some()
        } else {
            self.discoveries[item - AVAILABLE].is_some_and(|d| {
                !self
                    .selected
                    .iter()
                    .flatten()
                    .any(|s| s.identity == d.identity)
            })
        }
    }
    fn duplicate_kind(&self, identity: Identity) -> bool {
        self.discoveries
            .iter()
            .flatten()
            .any(|d| d.identity.device_type == identity.device_type && d.identity != identity)
            || self
                .selected
                .iter()
                .flatten()
                .any(|s| s.identity.device_type == identity.device_type && s.identity != identity)
    }
    fn suffix_pixel(
        &self,
        x: usize,
        y: usize,
        left: usize,
        top: usize,
        identity: Identity,
    ) -> bool {
        let same_number = |other: Identity| {
            other != identity
                && other.device_type == identity.device_type
                && other.device_number == identity.device_number
        };
        let transmission = self
            .discoveries
            .iter()
            .flatten()
            .any(|d| same_number(d.identity))
            || self
                .selected
                .iter()
                .flatten()
                .any(|s| same_number(s.identity));
        number(x, y, left, top, 1, u32::from(identity.device_number), 5)
            || transmission
                && (text(x, y, left + 30, top, 1, b".")
                    || number(
                        x,
                        y,
                        left + 36,
                        top,
                        1,
                        u32::from(identity.transmission_type),
                        3,
                    ))
    }
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        if x >= 240 || y >= 320 {
            return 0;
        }
        if text(x, y, 8, 300, 2, b"NEXT") || text(x, y, 156, 300, 2, b"SELECT") {
            return WHITE;
        }
        if text(x, y, 8, 285, 1, b"TOP LEFT: BACK") {
            return MUTED;
        }
        if text(x, y, 8, 270, 1, &self.message[..self.message.len().min(37)]) {
            return AMBER;
        }
        match self.view {
            View::Tiles => self.tiles_pixel(x, y),
            View::Detail(slot) => self.detail_pixel(x, y, self.selected[slot].unwrap()),
            View::Replace(identity) => self.replace_pixel(x, y, identity),
        }
    }
    fn tiles_pixel(&self, x: usize, y: usize) -> u16 {
        let selected_count = self.selected.iter().flatten().count();
        let rows = selected_count.div_ceil(5);
        if text(x, y, 8, 34, 1, b"MY SENSORS") {
            return MUTED;
        }
        if selected_count == 0 && text(x, y, 8, 52, 1, b"CHOOSE A SENSOR BELOW") {
            return MUTED;
        }
        let mut n = 0;
        for (slot, selected) in self.selected.iter().enumerate() {
            let Some(selected) = selected else {
                continue;
            };
            let left = 5 + n % 5 * 47;
            let top = 47 + n / 5 * 49;
            n += 1;
            let focused = self.cursor == slot + 1;
            if border(x, y, left, top, 43, 44, if focused { 2 } else { 1 }) {
                return if focused { CYAN } else { 0x3186 };
            }
            if icon(x, y, left + 10, top + 5, selected.identity.device_type) {
                return WHITE;
            }
            if text(
                x,
                y,
                left + 5,
                top + 29,
                1,
                short_kind(selected.identity.device_type),
            ) {
                return WHITE;
            }
            if (left + 33..left + 38).contains(&x) && (top + 5..top + 10).contains(&y) {
                return selected.color();
            }
        }
        let heading = 47 + rows.max(1) * 49 + 4;
        if text(
            x,
            y,
            8,
            heading,
            1,
            if self.scanning {
                b"AVAILABLE - SEARCHING..."
            } else {
                b"AVAILABLE"
            },
        ) {
            return MUTED;
        }
        let mut available = [0; DISCOVERY_CAPACITY];
        let mut count = 0;
        let mut focused = 0;
        for item in AVAILABLE..ITEMS {
            if self.visible(item) {
                if item == self.cursor {
                    focused = count;
                }
                available[count] = item;
                count += 1;
            }
        }
        let capacity = if rows > 1 { 1 } else { 2 };
        let first = focused.saturating_sub(capacity - 1);
        for (n, item) in available[first..count].iter().take(capacity).enumerate() {
            let top = heading + 15 + n * 48;
            let peer = self.discoveries[*item - AVAILABLE].unwrap();
            let focus = *item == self.cursor;
            if border(x, y, 5, top, 230, 44, if focus { 2 } else { 1 }) {
                return if focus { CYAN } else { 0x3186 };
            }
            if icon(x, y, 14, top + 10, peer.identity.device_type) {
                return WHITE;
            }
            if text(x, y, 44, top + 7, 1, kind(peer.identity.device_type)) {
                return WHITE;
            }
            if self.duplicate_kind(peer.identity)
                && self.suffix_pixel(x, y, 174, top + 7, peer.identity)
            {
                return MUTED;
            }
            if text(
                x,
                y,
                44,
                top + 24,
                1,
                if !self.present[*item - AVAILABLE] {
                    b"NOT SEEN"
                } else if focus {
                    b"SELECT TO CONNECT"
                } else if peer.identity.device_type == 40 {
                    b"VEHICLES BEHIND YOU"
                } else {
                    b"AVAILABLE TO CONNECT"
                },
            ) {
                return if focus { CYAN } else { MUTED };
            }
        }
        if count == 0
            && text(
                x,
                y,
                8,
                heading + 22,
                1,
                if self.scanning {
                    b"LOOKING FOR NEARBY SENSORS"
                } else {
                    b"NO NEW SENSORS FOUND"
                },
            )
        {
            return MUTED;
        }
        if border(x, y, 5, 239, 230, 24, if self.cursor == 0 { 2 } else { 1 }) {
            return if self.cursor == 0 { CYAN } else { 0x3186 };
        }
        if text(
            x,
            y,
            16,
            247,
            1,
            if self.scanning {
                b"SEARCHING..."
            } else {
                b"SEARCH AGAIN"
            },
        ) {
            return if self.cursor == 0 { CYAN } else { WHITE };
        }
        0
    }
    fn detail_pixel(&self, x: usize, y: usize, selected: Selected) -> u16 {
        if icon(x, y, 109, 51, selected.identity.device_type) {
            return WHITE;
        }
        if text(x, y, 12, 87, 2, kind(selected.identity.device_type)) {
            return WHITE;
        }
        if text(x, y, 12, 114, 1, selected.status()) {
            return selected.color();
        }
        if self.duplicate_kind(selected.identity)
            && (text(x, y, 12, 130, 1, b"SENSOR")
                || self.suffix_pixel(x, y, 60, 130, selected.identity))
        {
            return MUTED;
        }
        if text(x, y, 12, 145, 1, b"GREEN: LIVE DATA")
            || text(x, y, 12, 160, 1, b"BLUE: CONNECTING")
            || text(x, y, 12, 175, 1, b"AMBER: NEEDS ATTENTION")
        {
            return MUTED;
        }
        for action in 0..if selected.pending.is_some() { 1 } else { 2 } {
            let top = 203 + action * 31;
            let focused = self.detail_action == action;
            if border(x, y, 6, top, 228, 26, if focused { 2 } else { 1 }) {
                return if focused { CYAN } else { 0x3186 };
            }
            let label: &[u8] = if selected.pending.is_some() {
                b"BACK TO SENSORS"
            } else if action == 0 {
                if selected.failed_disconnect {
                    b"RETRY DISCONNECT"
                } else if selected.retry() {
                    b"RETRY CONNECTION"
                } else {
                    b"DISCONNECT"
                }
            } else if selected.retry() && selected.can_disconnect() && !selected.failed_disconnect {
                b"DISCONNECT"
            } else {
                b"BACK TO SENSORS"
            };
            if text(x, y, 16, top + 9, 1, label) {
                return WHITE;
            }
        }
        0
    }
    fn replace_pixel(&self, x: usize, y: usize, identity: Identity) -> u16 {
        if text(x, y, 12, 50, 2, b"REPLACE SENSOR?")
            || text(x, y, 12, 83, 1, kind(identity.device_type))
        {
            return WHITE;
        }
        if text(x, y, 12, 111, 1, b"THIS WILL REPLACE YOUR")
            || text(x, y, 12, 127, 1, b"CURRENT SENSOR OF THIS TYPE.")
            || text(x, y, 12, 151, 1, b"NEW SENSOR")
            || number(x, y, 90, 151, 1, u32::from(identity.device_number), 5)
        {
            return MUTED;
        }
        for action in 0..2 {
            let top = 200 + action * 34;
            if border(
                x,
                y,
                6,
                top,
                228,
                27,
                if self.detail_action == action { 2 } else { 1 },
            ) {
                return if self.detail_action == action {
                    CYAN
                } else {
                    0x3186
                };
            }
            if text(
                x,
                y,
                16,
                top + 9,
                1,
                if action == 0 {
                    b"REPLACE AND CONNECT"
                } else {
                    b"KEEP CURRENT SENSOR"
                },
            ) {
                return WHITE;
            }
        }
        0
    }
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
    if x < left || y < top || x >= left + 22 || y >= top + 20 {
        return false;
    }
    let x = x - left;
    let y = y - top;
    match kind {
        120 => {
            // Heart, filled lobes tapering to a point.
            let dx = x as i32 - 11;
            ((2..9).contains(&y)
                && ((x as i32 - 6).pow(2) + (y as i32 - 6).pow(2) <= 25
                    || (x as i32 - 15).pow(2) + (y as i32 - 6).pow(2) <= 25))
                || ((8..19).contains(&y) && dx.abs() <= 19 - y as i32)
        }
        11 => {
            (y < 11 && x >= 11 - y / 2 && x <= 16 - y / 2)
                || (y >= 9 && x >= 14 - y / 2 && x <= 19 - y / 2)
        }
        40 => {
            let dx = x as i32 - 10;
            let dy = y as i32 - 18;
            let r = dx * dx + dy * dy;
            (y <= 16 && ((45..65).contains(&r) || (180..210).contains(&r)))
                || ((9..=12).contains(&x) && y >= 16)
        }
        121..=123 => {
            let dx = x as i32 - 10;
            let dy = y as i32 - 10;
            let r = dx * dx + dy * dy;
            (65..100).contains(&r) || ((x == 10 || y == 10) && r < 90)
        }
        34 | 128 => {
            let dx = x as i32 - 10;
            let dy = y as i32 - 10;
            let r = dx * dx + dy * dy;
            (36..65).contains(&r)
                || ((x == 10 || y == 10 || x == y || x + y == 20) && (21..100).contains(&r))
        }
        17 => {
            border(x, y, 3, 3, 16, 10, 2)
                || (y >= 13 && (x == 7 || x == 14))
                || (y == 18 && (4..18).contains(&x))
        }
        35 => {
            border(x, y, 2, 5, 10, 12, 2)
                || ((14..22).contains(&x) && (y == 5 || y == 10 || y == 16))
        }
        _ => border(x, y, 3, 2, 16, 16, 2) || (9..13).contains(&x) && (6..14).contains(&y),
    }
}
pub(crate) fn connection_status(channel: Snapshot) -> &'static str {
    match channel.link {
        LinkState::Idle => "NOT CONNECTED",
        LinkState::Connecting => "CONNECTING",
        LinkState::Connected if channel.age_ms.is_none() => "WAITING FOR DATA",
        LinkState::Connected if channel.stale => "DATA LOST",
        LinkState::Connected => "CONNECTED",
        LinkState::Disconnecting => "DISCONNECTING",
        LinkState::Disconnected => "DISCONNECTED",
        LinkState::TimedOut => "CONNECT FAILED",
        LinkState::TransportLost => "RADIO UNAVAILABLE",
    }
}
fn kind(device_type: u8) -> &'static [u8] {
    match device_type {
        40 => b"RADAR",
        11 => b"POWER METER",
        120 => b"HEART RATE",
        121 => b"SPEED / CADENCE",
        122 => b"CADENCE SENSOR",
        123 => b"SPEED SENSOR",
        34 | 128 => b"SHIFTING",
        17 => b"FITNESS EQUIPMENT",
        35 => b"BIKE LIGHTS",
        _ => b"ANT SENSOR",
    }
}
fn short_kind(device_type: u8) -> &'static [u8] {
    match device_type {
        40 => b"RADAR",
        11 => b"POWER",
        120 => b"HEART",
        121 => b"SPD/C",
        122 => b"CAD",
        123 => b"SPEED",
        34 | 128 => b"GEARS",
        17 => b"FIT",
        35 => b"LIGHT",
        _ => b"ANT",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn identities_with_equal_device_numbers_have_distinct_available_suffixes() {
        let mut second = peer(10);
        second.identity.transmission_type = 2;
        let mut menu = Menu::new();
        menu.refresh(
            [
                Some(peer(10)),
                Some(second),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            [None; CHANNEL_CAPACITY],
            false,
            0,
        );
        assert!((0..7).any(|dy| {
            (170..234).any(|x| (menu.pixel(x, 122 + dy) != 0) != (menu.pixel(x, 170 + dy) != 0))
        }));
    }
    #[test]
    fn admitted_connecting_sensor_keeps_the_pending_indicator() {
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
        let mut menu = Menu::new();
        menu.request_pending(peer(10).identity);
        menu.request_accepted(peer(10).identity);
        menu.refresh([None; DISCOVERY_CAPACITY], channels.snapshots(0), false, 0);
        assert_eq!(menu.pixel(39, 53), CYAN);
    }
    #[test]
    fn rejected_unadmitted_peer_offers_back_instead_of_disconnect() {
        let mut menu = Menu::new();
        menu.request_pending(peer(10).identity);
        menu.request_failed(peer(10).identity, b"CONNECT FAILED");
        menu.input(press(Button::BottomRight), 0);
        menu.input(press(Button::BottomLeft), 400);
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
        assert_eq!(menu.input(press(Button::TopLeft), 1200), Some(Action::Ride));
    }
    #[test]
    fn pending_admission_cannot_be_overwritten_by_disconnect_or_replacement() {
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
            [None; CHANNEL_CAPACITY],
            false,
            0,
        );
        menu.request_pending(peer(10).identity);
        menu.input(press(Button::BottomRight), 0);
        assert_eq!(menu.input(press(Button::BottomRight), 400), None);
        menu.input(press(Button::BottomLeft), 800);
        assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
        assert_eq!(menu.input(press(Button::BottomRight), 1600), None);
        assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
    }
    #[test]
    fn same_type_replacement_requires_confirmation_and_retains_failed_intent() {
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
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
            channels.snapshots(0),
            false,
            0,
        );
        menu.input(press(Button::BottomLeft), 0);
        menu.input(press(Button::BottomLeft), 400);
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
        assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
        assert_eq!(
            menu.input(press(Button::BottomRight), 1200),
            Some(Action::Connect(peer(20).identity))
        );
        menu.request_failed(peer(20).identity, b"CONNECT FAILED");
        menu.refresh(
            [None; DISCOVERY_CAPACITY],
            channels.snapshots(1600),
            false,
            1600,
        );
        assert_eq!(menu.selected_identity(11), Some(peer(20).identity));
        menu.input(press(Button::BottomRight), 1600);
        assert_eq!(
            menu.input(press(Button::BottomRight), 2000),
            Some(Action::Connect(peer(20).identity))
        );
    }
    #[test]
    fn reordered_discovery_keeps_focus_and_selected_peer_is_not_duplicated() {
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
            [None; CHANNEL_CAPACITY],
            true,
            0,
        );
        menu.input(press(Button::BottomLeft), 0);
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
            [None; CHANNEL_CAPACITY],
            false,
            400,
        );
        assert_eq!(
            menu.input(press(Button::BottomRight), 400),
            Some(Action::Connect(peer(10).identity))
        );
        menu.request_failed(peer(10).identity, b"CONNECT FAILED");
        // Next skips the selected identity's former available tile.
        menu.input(press(Button::BottomLeft), 800);
        assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
        assert_eq!(
            menu.input(press(Button::BottomRight), 1600),
            Some(Action::Connect(peer(20).identity))
        );
    }
    #[test]
    fn connection_indicator_requires_observed_fresh_data_and_failure_is_recoverable() {
        let mut menu = Menu::new();
        menu.request_pending(peer(10).identity);
        assert_eq!(menu.pixel(39, 53), CYAN);
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
        let mut snapshots = channels.snapshots(0);
        let snapshot = snapshots[0].as_mut().unwrap();
        snapshot.link = LinkState::Connected;
        snapshot.stale = false;
        snapshot.age_ms = Some(0);
        // A stale observation before admission cannot acknowledge new intent.
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 0);
        assert_eq!(menu.pixel(39, 53), CYAN);
        menu.request_accepted(peer(10).identity);
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 400);
        assert_eq!(menu.pixel(39, 53), GREEN);
        snapshots[0].as_mut().unwrap().link = LinkState::TimedOut;
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 800);
        assert_eq!(menu.pixel(39, 53), AMBER);
        menu.input(press(Button::BottomRight), 800);
        assert_eq!(
            menu.input(press(Button::BottomRight), 1200),
            Some(Action::Connect(peer(10).identity))
        );
    }
    #[test]
    fn cancel_holds_and_debounce_cannot_activate_a_sensor() {
        let mut menu = Menu::new();
        menu.refresh(
            [Some(peer(10)), None, None, None, None, None, None, None],
            [None; CHANNEL_CAPACITY],
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
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(menu.input(press(Button::BottomRight), 349), None);
        menu.input(Input::Cancel, 400);
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
        // Back stays usable even after input loss.
        assert_eq!(menu.input(press(Button::TopLeft), 1200), Some(Action::Ride));
    }
    #[test]
    fn all_ten_selected_slots_are_reachable_without_overflowing_capacity() {
        let mut channels = firmware_services::ant::Channels::new();
        for kind in 1..=10 {
            channels
                .connect(
                    Identity {
                        device_type: kind,
                        device_number: u16::from(kind),
                        transmission_type: 1,
                    },
                    0,
                )
                .unwrap();
        }
        let mut menu = Menu::new();
        menu.refresh(
            [Some(peer(10)), None, None, None, None, None, None, None],
            channels.snapshots(0),
            false,
            0,
        );
        for n in 0..10 {
            menu.input(press(Button::BottomLeft), n * 400);
        }
        menu.input(press(Button::BottomRight), 4000);
        assert_eq!(
            menu.input(press(Button::BottomRight), 4400),
            Some(Action::Disconnect(10))
        );
        menu.input(press(Button::BottomLeft), 4800);
        assert_eq!(menu.input(press(Button::BottomRight), 5200), None);
    }
    #[test]
    fn rejected_disconnect_can_retry_the_disconnect() {
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
        channels.receive(device_api::ant::Event::Connected(peer(10).identity), 0);
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], channels.snapshots(0), false, 0);
        menu.input(press(Button::BottomLeft), 0);
        menu.input(press(Button::BottomRight), 400);
        assert_eq!(
            menu.input(press(Button::BottomRight), 800),
            Some(Action::Disconnect(11))
        );
        menu.request_failed(peer(10).identity, b"RADIO BUSY");
        menu.input(press(Button::BottomRight), 1200);
        assert_eq!(
            menu.input(press(Button::BottomRight), 1600),
            Some(Action::Disconnect(11))
        );
    }
    #[test]
    fn choosing_available_moves_focus_to_selected_and_back_exits_immediately() {
        let mut menu = Menu::new();
        menu.refresh(
            [Some(peer(10)), None, None, None, None, None, None, None],
            [None; CHANNEL_CAPACITY],
            true,
            0,
        );
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(
            menu.input(press(Button::BottomRight), 400),
            Some(Action::Connect(peer(10).identity))
        );
        assert_eq!(menu.input(press(Button::BottomRight), 800), None);
        assert_eq!(menu.input(press(Button::TopLeft), 1200), None);
        assert_eq!(menu.input(press(Button::TopLeft), 1600), Some(Action::Ride));
    }
}
