//! Sensor selection and physical-button navigation, independent of radio scheduling.
mod icons;

use device_api::ant::{
    CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity, LinkState, Snapshot,
};
use device_api::input::{Button, Input};
use firmware_shell::rendering::text::text;

const AVAILABLE: usize = 1 + CHANNEL_CAPACITY;
const ITEMS: usize = AVAILABLE + DISCOVERY_CAPACITY;
const WHITE: u16 = 0xffff;
const CYAN: u16 = 0x07ff;
const MUTED: u16 = 0x9cf3;
const GREEN: u16 = 0x5fe9;
const AMBER: u16 = 0xfd20;
const FOCUS_FILL: u16 = 0x0945;
const BORDER: u16 = 0x3186;
const DIVIDER: u16 = 0x39e7;

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
#[derive(Clone, Copy, Eq, PartialEq)]
enum Operation {
    Connect,
    Disconnect,
}
#[derive(Clone, Copy)]
struct Pending {
    operation: Operation,
    admitted: bool,
}
#[derive(Clone, Copy)]
struct Selected {
    identity: Identity,
    observed: Option<Snapshot>,
    pending: Option<Pending>,
    failure: Option<&'static [u8]>,
    failed_disconnect: bool,
}
impl Selected {
    fn status(self) -> &'static [u8] {
        if let Some(failure) = self.failure {
            return failure;
        }
        if self.pending.is_some_and(|pending| !pending.admitted) {
            return b"WAITING FOR RADIO";
        }
        if let Some(pending) = self.pending {
            return if pending.operation == Operation::Disconnect {
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
#[derive(Clone, Copy)]
struct Label {
    bytes: [u8; 18],
    len: usize,
}
impl Label {
    const fn new() -> Self {
        Self {
            bytes: [0; 18],
            len: 0,
        }
    }
    fn push(&mut self, value: &[u8]) {
        let count = value.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + count].copy_from_slice(&value[..count]);
        self.len += count;
    }
    fn number(&mut self, value: u16) {
        let mut digits = [0; 5];
        let mut remaining = value;
        let mut first = 4;
        loop {
            digits[first] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
            if remaining == 0 {
                break;
            }
            first -= 1;
        }
        self.push(&digits[first..]);
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
struct Layout {
    selected_slots: [usize; CHANNEL_CAPACITY],
    selected_count: usize,
    available_slots: [usize; DISCOVERY_CAPACITY],
    available_count: usize,
    title: &'static [u8],
    page_feedback: Label,
    status: Label,
    identity: Label,
    replacement_numbers: [Label; 2],
    replacement_transmissions: [Label; 2],
    show_identity: bool,
    color: u16,
    action: &'static [u8],
}
impl Layout {
    const fn new() -> Self {
        Self {
            selected_slots: [0; CHANNEL_CAPACITY],
            selected_count: 0,
            available_slots: [0; DISCOVERY_CAPACITY],
            available_count: 0,
            title: b"SCAN",
            page_feedback: Label::new(),
            status: Label::new(),
            identity: Label::new(),
            replacement_numbers: [Label::new(); 2],
            replacement_transmissions: [Label::new(); 2],
            show_identity: false,
            color: MUTED,
            action: b"SEARCH",
        }
    }
}
pub struct Menu {
    discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    present: [bool; DISCOVERY_CAPACITY],
    selected: [Option<Selected>; CHANNEL_CAPACITY],
    cursor: usize,
    layout: Layout,
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
            layout: Layout::new(),
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
        self.prepare_layout();
    }
    /// Begin a genuinely new bounded search without moving selected-sensor focus.
    pub fn search_started(&mut self) {
        self.discoveries = [None; DISCOVERY_CAPACITY];
        self.present = [false; DISCOVERY_CAPACITY];
        if self.cursor >= AVAILABLE {
            self.cursor = 0;
        }
        self.prepare_layout();
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
                pending: Some(Pending {
                    operation: Operation::Connect,
                    admitted: false,
                }),
                failure: None,
                failed_disconnect: false,
            });
            self.cursor = 1 + slot;
        }
        self.prepare_layout();
    }
    pub fn request_accepted(&mut self, identity: Identity) {
        if let Some(s) = self
            .selected
            .iter_mut()
            .flatten()
            .find(|s| s.identity == identity)
            && let Some(pending) = &mut s.pending
        {
            pending.admitted = true;
        }
        self.prepare_layout();
    }
    pub fn request_failed(&mut self, identity: Identity, message: &'static [u8]) {
        if let Some(s) = self
            .selected
            .iter_mut()
            .flatten()
            .find(|s| s.identity == identity)
        {
            s.failed_disconnect = s
                .pending
                .is_some_and(|pending| pending.operation == Operation::Disconnect);
            s.pending = None;
            s.failure = Some(message);
        }
        self.prepare_layout();
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
                        if let Some(Pending {
                            operation,
                            admitted: true,
                        }) = selected.pending
                        {
                            let observed = if operation == Operation::Disconnect {
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
        self.prepare_layout();
    }
    pub fn input(&mut self, input: Input, now: u64) -> Option<Action> {
        let action = self.handle_input(input, now);
        self.prepare_layout();
        action
    }
    fn handle_input(&mut self, input: Input, now: u64) -> Option<Action> {
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
                    selected.pending = Some(Pending {
                        operation: Operation::Disconnect,
                        admitted: false,
                    });
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
    fn prepare_layout(&mut self) {
        let mut layout = Layout::new();
        for (slot, selected) in self.selected.iter().enumerate() {
            if selected.is_some() {
                layout.selected_slots[layout.selected_count] = slot;
                layout.selected_count += 1;
            }
        }
        for item in AVAILABLE..ITEMS {
            if self.visible(item) {
                layout.available_slots[layout.available_count] = item;
                layout.available_count += 1;
            }
        }
        let focused = if self.cursor == 0 {
            None
        } else if self.cursor < AVAILABLE {
            self.selected[self.cursor - 1].map(|selected| selected.identity)
        } else {
            self.discoveries[self.cursor - AVAILABLE].map(|peer| peer.identity)
        };
        if let Some(identity) = focused {
            layout.title = kind(identity.device_type);
            layout.identity = self.identity_label(identity);
            layout.show_identity = self.duplicate_kind(identity);
            if self.cursor < AVAILABLE {
                let selected = self.selected[self.cursor - 1].unwrap();
                layout.color = selected.color();
                layout.action = b"OPEN";
                if layout.show_identity {
                    layout.status.push(if layout.color == GREEN {
                        b"LIVE / "
                    } else if layout.color == CYAN {
                        b"WAIT / "
                    } else if selected.failure.is_some()
                        || selected.observed.is_some_and(|snapshot| {
                            matches!(
                                snapshot.link,
                                LinkState::TimedOut | LinkState::TransportLost
                            )
                        })
                    {
                        b"FAILED / "
                    } else if selected.retry() {
                        b"OFF / "
                    } else {
                        b"CHECK / "
                    });
                } else {
                    layout.status.push(if layout.color == GREEN {
                        b"LIVE DATA"
                    } else {
                        selected.status()
                    });
                }
            } else {
                layout.color = MUTED;
                layout.action = if self.selected_identity(identity.device_type).is_some() {
                    b"REPLACE"
                } else {
                    b"CONNECT"
                };
                if layout.show_identity {
                    layout
                        .status
                        .push(if self.present[self.cursor - AVAILABLE] {
                            b"SENSOR / "
                        } else {
                            b"GONE / "
                        });
                } else if !self.present[self.cursor - AVAILABLE] {
                    layout.status.push(b"NOT SEEN");
                } else if identity.device_type == 40 {
                    layout.status.push(b"VEHICLES BEHIND");
                } else {
                    layout.status.push(b"READY TO CONNECT");
                }
            }
            if layout.show_identity {
                layout.status.push(layout.identity.as_bytes());
            }
        } else {
            layout.status.push(if self.scanning {
                b"SEARCHING..."
            } else {
                b"FIND NEARBY"
            });
            layout.color = if self.scanning { CYAN } else { MUTED };
        }
        if !self.message.is_empty() {
            let message = match self.message {
                b"WAIT FOR CURRENT REQUEST" => b"REQUEST PENDING".as_slice(),
                b"ALL TEN SENSOR SLOTS ARE IN USE" => b"SENSOR SLOTS FULL",
                b"INPUT LOST - PRESS NEXT" => b"PRESS NEXT AGAIN",
                b"WAITING FOR COMPANION" => b"WAITING FOR RADIO",
                b"SCAN THEN PICK SENSOR" => b"FIND NEARBY",
                message => message,
            };
            let routine = routine_scan_message(self.message);
            if !routine {
                layout.page_feedback.push(message);
            }
            // Page feedback never replaces the focused peer's state or identity.
            if self.cursor == 0 {
                layout.status = Label::new();
                layout.status.push(message);
                layout.color = if routine {
                    if self.scanning { CYAN } else { MUTED }
                } else {
                    AMBER
                };
            }
        }
        if let View::Replace(identity) = self.view {
            layout.identity = self.identity_label(identity);
            for (index, peer) in [self.selected_identity(identity.device_type), Some(identity)]
                .into_iter()
                .enumerate()
            {
                if let Some(peer) = peer {
                    layout.replacement_numbers[index].number(peer.device_number);
                    if self.identity_label(peer).as_bytes().contains(&b'.') {
                        layout.replacement_transmissions[index].push(b"T");
                        layout.replacement_transmissions[index]
                            .number(u16::from(peer.transmission_type));
                    }
                }
            }
        }
        self.layout = layout;
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
    fn identity_label(&self, identity: Identity) -> Label {
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
        let mut label = Label::new();
        label.number(identity.device_number);
        if transmission {
            label.push(b".");
            label.number(u16::from(identity.transmission_type));
        }
        label
    }
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        if x >= 240 || y >= 320 {
            return 0;
        }
        if y == 290 {
            return DIVIDER;
        }
        if y >= 300 {
            let action = if matches!(self.view, View::Tiles) {
                self.layout.action
            } else {
                b"SELECT"
            };
            if text(x, y, 12, 300, 2, b"NEXT")
                || text(x, y, 228 - action.len() * 12, 300, 2, action)
            {
                return WHITE;
            }
            return 0;
        }
        match self.view {
            View::Tiles => self.tiles_pixel(x, y),
            View::Detail(slot) => self.detail_pixel(x, y, self.selected[slot].unwrap()),
            View::Replace(identity) => self.replace_pixel(x, y, identity),
        }
    }
    fn tiles_pixel(&self, x: usize, y: usize) -> u16 {
        if text(
            x,
            y,
            12,
            36,
            2,
            if self.layout.page_feedback.len == 0 {
                b"MY SENSORS"
            } else {
                self.layout.page_feedback.as_bytes()
            },
        ) {
            return if self.layout.page_feedback.len == 0 {
                WHITE
            } else {
                AMBER
            };
        }
        if text(x, y, 12, 138, 2, b"NEARBY") {
            return WHITE;
        }
        if self.scanning && text(x, y, 96, 138, 2, b"SEARCHING") {
            return CYAN;
        }
        if (208..232).contains(&x) && (130..154).contains(&y) {
            if self.cursor == 0 && border(x, y, 208, 130, 24, 24, 2) {
                return CYAN;
            }
            if icons::refresh_pixel(x - 208, y - 130) {
                return if self.scanning { CYAN } else { WHITE };
            }
            return if self.cursor == 0 { FOCUS_FILL } else { 0 };
        }
        if x >= 8 && (56..126).contains(&y) {
            let n = (y - 56) / 38 * 5 + (x - 8) / 46;
            if n < self.layout.selected_count {
                let slot = self.layout.selected_slots[n];
                let selected = self.selected[slot].unwrap();
                let left = 8 + n % 5 * 46;
                let top = 56 + n / 5 * 38;
                if (left..left + 40).contains(&x) && (top..top + 32).contains(&y) {
                    let focused = self.cursor == slot + 1;
                    if border(x, y, left, top, 40, 32, if focused { 2 } else { 1 }) {
                        return if focused { CYAN } else { BORDER };
                    }
                    if icon(x, y, left + 9, top + 6, selected.identity.device_type) {
                        return WHITE;
                    }
                    if (left + 30..left + 37).contains(&x)
                        && (top + 3..top + 10).contains(&y)
                        && indicator(x - left - 30, y - top - 3, selected.color())
                    {
                        return selected.color();
                    }
                    return if focused { FOCUS_FILL } else { 0 };
                }
            }
        }
        if x >= 11 && (156..224).contains(&y) {
            let n = (y - 156) / 36 * 4 + (x - 11) / 56;
            if n < self.layout.available_count {
                let item = self.layout.available_slots[n];
                let identity = self.discoveries[item - AVAILABLE].unwrap().identity;
                let left = 11 + n % 4 * 56;
                let top = 156 + n / 4 * 36;
                if (left..left + 50).contains(&x) && (top..top + 32).contains(&y) {
                    let focused = self.cursor == item;
                    if border(x, y, left, top, 50, 32, if focused { 2 } else { 1 }) {
                        return if focused { CYAN } else { BORDER };
                    }
                    if icon(x, y, left + 14, top + 6, identity.device_type) {
                        return WHITE;
                    }
                    return if focused { FOCUS_FILL } else { 0 };
                }
            }
        }
        if self.layout.selected_count == 0 && text(x, y, 12, 68, 2, b"NONE SELECTED") {
            return MUTED;
        }
        if self.layout.available_count == 0
            && text(
                x,
                y,
                12,
                168,
                2,
                if self.scanning {
                    b"LOOKING NEARBY..."
                } else {
                    b"NO SENSORS FOUND"
                },
            )
        {
            return MUTED;
        }
        if y == 226 && (12..228).contains(&x) {
            return DIVIDER;
        }
        if text(x, y, 12, 238, 3, self.layout.title) {
            return WHITE;
        }
        if text(x, y, 12, 267, 2, self.layout.status.as_bytes()) {
            return self.layout.color;
        }
        0
    }
    fn detail_pixel(&self, x: usize, y: usize, selected: Selected) -> u16 {
        if scaled_icon(x, y, 98, 52, selected.identity.device_type) {
            return WHITE;
        }
        let title = kind(selected.identity.device_type);
        if text(x, y, (240 - title.len() * 18) / 2, 111, 3, title) {
            return WHITE;
        }
        if selected.color() == GREEN {
            if text(x, y, 60, 149, 2, b"LIVE DATA") || live_check(x, y, 38, 150) {
                return GREEN;
            }
        } else {
            let status = selected.status();
            if text(
                x,
                y,
                (240 - status.len().min(18) * 12) / 2,
                149,
                2,
                &status[..status.len().min(18)],
            ) {
                return selected.color();
            }
        }
        {
            let width = (7 + self.layout.identity.len) * 12;
            let left = (240 - width) / 2;
            if text(x, y, left, 179, 2, b"SENSOR ")
                || text(x, y, left + 84, 179, 2, self.layout.identity.as_bytes())
            {
                return MUTED;
            }
        }
        let first: &[u8] = if selected.pending.is_some() {
            b"BACK"
        } else if selected.failed_disconnect {
            b"RETRY DISCONNECT"
        } else if selected.retry() {
            b"RETRY"
        } else {
            b"DISCONNECT"
        };
        if let Some(color) = action_pixel(x, y, 220, 34, self.detail_action == 0, first) {
            return color;
        }
        if selected.pending.is_none() {
            let second: &[u8] =
                if selected.retry() && selected.can_disconnect() && !selected.failed_disconnect {
                    b"DISCONNECT"
                } else {
                    b"BACK"
                };
            if let Some(color) = action_pixel(x, y, 262, 28, self.detail_action == 1, second) {
                return color;
            }
        }
        0
    }
    fn replace_pixel(&self, x: usize, y: usize, identity: Identity) -> u16 {
        if scaled_icon(x, y, 98, 43, identity.device_type) {
            return WHITE;
        }
        let title = kind(identity.device_type);
        if text(x, y, (240 - title.len() * 18) / 2, 94, 3, title)
            || text(x, y, 30, 129, 2, b"REPLACE SENSOR")
        {
            return WHITE;
        }
        if (160..191).contains(&y) {
            let index = usize::from(x >= 120);
            let center = if index == 0 { 54 } else { 180 };
            let number = &self.layout.replacement_numbers[index];
            let transmission = &self.layout.replacement_transmissions[index];
            let scale = if transmission.len == 0 { 3 } else { 2 };
            if text(
                x,
                y,
                center - number.len * 3 * scale,
                160,
                scale,
                number.as_bytes(),
            ) || text(
                x,
                y,
                center - transmission.len * 6,
                177,
                2,
                transmission.as_bytes(),
            ) {
                return WHITE;
            }
        }
        if ((103..130).contains(&x) && (171..173).contains(&y))
            || ((121..131).contains(&x) && ((y as isize - 172).unsigned_abs() == 130 - x))
        {
            return CYAN;
        }
        if text(x, y, 12, 191, 2, b"CURRENT") || text(x, y, 174, 191, 2, b"NEW") {
            return MUTED;
        }
        if let Some(color) = action_pixel(x, y, 216, 34, self.detail_action == 0, b"REPLACE") {
            return color;
        }
        if let Some(color) = action_pixel(x, y, 258, 32, self.detail_action == 1, b"KEEP CURRENT") {
            return color;
        }
        0
    }
}
fn routine_scan_message(message: &[u8]) -> bool {
    matches!(
        message,
        b"PICK SENSOR"
            | b"SEARCH PENDING"
            | b"STARTING SCAN"
            | b"SCANNING..."
            | b"STOPPING SCAN"
            | b"SCAN CANCELLED"
            | b"FINISHING SCAN"
            | b"SCAN DONE - PICK"
            | b"STARTING SENSORS"
            | b"WAITING FOR COMPANION"
            | b"SCAN THEN PICK SENSOR"
    )
}
fn action_pixel(
    x: usize,
    y: usize,
    top: usize,
    height: usize,
    focused: bool,
    label: &[u8],
) -> Option<u16> {
    if !(12..228).contains(&x) || !(top..top + height).contains(&y) {
        return None;
    }
    if border(x, y, 12, top, 216, height, if focused { 2 } else { 1 }) {
        return Some(if focused { CYAN } else { BORDER });
    }
    if text(
        x,
        y,
        (240 - label.len() * 12) / 2,
        top + (height - 14) / 2,
        2,
        label,
    ) {
        return Some(WHITE);
    }
    Some(if focused { FOCUS_FILL } else { 0 })
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
fn indicator(x: usize, y: usize, color: u16) -> bool {
    if color == GREEN {
        (x <= 2 && (y == x + 3 || y == x + 2)) || (x >= 2 && (y + x == 7 || y + x == 6))
    } else if color == CYAN {
        ((x == 1 || x == 5) && (1..6).contains(&y)) || ((y == 1 || y == 5) && (1..6).contains(&x))
    } else {
        (x == 3 && (1..4).contains(&y)) || (x == 3 && y == 5)
    }
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
        121 => b"SPEED/CAD",
        122 => b"CADENCE",
        123 => b"SPEED SENSOR",
        34 | 128 => b"SHIFTING",
        17 => b"FITNESS",
        35 => b"BIKE LIGHTS",
        _ => b"ANT SENSOR",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn press(button: Button) -> Input {
        Input::Button { button, code: 1 }
    }
    fn indicator_has_color(menu: &Menu, color: u16) -> bool {
        (59..66).any(|y| (38..45).any(|x| menu.pixel(x, y) == color))
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
    fn runtime_scan_messages_preserve_focused_identity_and_connection_feedback() {
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
        let mut baseline = [0; 216 * 44];
        for y in 0..44 {
            for x in 0..216 {
                baseline[y * 216 + x] = menu.pixel(12 + x, 238 + y);
            }
        }
        for message in [
            b"SCANNING...".as_slice(),
            b"SCAN DONE - PICK",
            b"SCAN CANCELLED",
            b"STARTING SCAN",
            b"SEARCH PENDING",
            b"STOPPING SCAN",
            b"FINISHING SCAN",
        ] {
            menu.set_message(message);
            for y in 0..44 {
                for x in 0..216 {
                    assert_eq!(
                        menu.pixel(12 + x, 238 + y),
                        baseline[y * 216 + x],
                        "{message:?} at {x},{y}"
                    );
                }
            }
        }
        menu.input(press(Button::BottomRight), 400);
        menu.set_message(b"SCAN CANCELLED");
        // Pending belongs to the selected peer, not to the cancelled discovery.
        for y in 267..281 {
            for x in 12..228 {
                assert_eq!(
                    menu.pixel(x, y) == CYAN,
                    text(x, y, 12, 267, 2, b"WAIT / 10")
                );
            }
        }
        menu.set_message(b"RADIO UNAVAILABLE");
        for y in 267..281 {
            for x in 12..228 {
                assert_eq!(
                    menu.pixel(x, y) == CYAN,
                    text(x, y, 12, 267, 2, b"WAIT / 10")
                );
            }
        }
        for y in 36..50 {
            for x in 12..228 {
                assert_eq!(
                    menu.pixel(x, y) == AMBER,
                    text(x, y, 12, 36, 2, b"RADIO UNAVAILABLE")
                );
            }
        }
    }
    #[test]
    fn replacement_disambiguates_maximum_identity_without_overlapping_numbers() {
        let current = Identity {
            device_type: 11,
            device_number: u16::MAX,
            transmission_type: 254,
        };
        let replacement = Identity {
            transmission_type: 255,
            ..current
        };
        let mut menu = Menu::new();
        menu.request_pending(current);
        menu.request_failed(current, b"CONNECT FAILED");
        let mut discovery = peer(u16::MAX);
        discovery.identity = replacement;
        menu.refresh(
            [Some(discovery), None, None, None, None, None, None, None],
            [None; CHANNEL_CAPACITY],
            false,
            0,
        );
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(menu.input(press(Button::BottomRight), 400), None);
        for y in 0..320 {
            for x in 0..240 {
                let _ = menu.pixel(x, y);
            }
        }
        // Each side reserves a 14px number line and a separate transmission line.
        for y in 160..191 {
            for x in 12..100 {
                assert_eq!(
                    menu.pixel(x, y) == WHITE,
                    text(x, y, 24, 160, 2, b"65535") || text(x, y, 30, 177, 2, b"T254"),
                    "{x},{y}"
                );
            }
        }
    }
    #[test]
    fn paper_grid_shows_ten_selected_and_eight_nearby_without_scrolling() {
        let mut channels = firmware_services::ant::Channels::new();
        for kind in 1..=10 {
            channels
                .connect(
                    Identity {
                        device_type: kind,
                        ..peer(u16::from(kind)).identity
                    },
                    0,
                )
                .unwrap();
        }
        let mut menu = Menu::new();
        menu.refresh(
            core::array::from_fn(|n| Some(peer(100 + n as u16))),
            channels.snapshots(0),
            false,
            0,
        );
        assert_eq!(menu.pixel(231, 125), 0x3186);
        assert_eq!(menu.pixel(228, 223), 0x3186);
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(menu.pixel(8, 56), CYAN);
        assert_eq!(menu.pixel(10, 58), 0x0945);
    }
    #[test]
    fn new_search_reclaims_discovery_slots_and_preserves_selected_focus() {
        let mut menu = Menu::new();
        let selected = Identity {
            device_type: 40,
            ..peer(90).identity
        };
        menu.request_pending(selected);
        let old = core::array::from_fn(|index| Some(peer(index as u16 + 1)));
        menu.refresh(old, [None; CHANNEL_CAPACITY], false, 0);
        let selected_focus = menu.diagnostics().cursor;
        menu.search_started();
        menu.refresh(
            [Some(peer(99)), None, None, None, None, None, None, None],
            [None; CHANNEL_CAPACITY],
            true,
            400,
        );
        assert_eq!(menu.selected_identity(40), Some(selected));
        assert_eq!(menu.diagnostics().cursor, selected_focus);
        menu.input(press(Button::BottomLeft), 400);
        assert_eq!(
            menu.input(press(Button::BottomRight), 800),
            Some(Action::Connect(peer(99).identity))
        );
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
        menu.input(press(Button::BottomLeft), 0);
        let mut first_status = [0; 216 * 14];
        for y in 0..14 {
            for x in 0..216 {
                first_status[y * 216 + x] = menu.pixel(12 + x, 267 + y);
            }
        }
        menu.input(press(Button::BottomLeft), 400);
        assert!((0..14).any(|y| (0..216).any(|x| first_status[y * 216 + x] != menu.pixel(12 + x, 267 + y))));
    }
    #[test]
    fn admitted_connecting_sensor_keeps_the_pending_indicator() {
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
        let mut menu = Menu::new();
        menu.request_pending(peer(10).identity);
        menu.request_accepted(peer(10).identity);
        menu.refresh([None; DISCOVERY_CAPACITY], channels.snapshots(0), false, 0);
        assert!(indicator_has_color(&menu, CYAN));
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
        assert!(indicator_has_color(&menu, CYAN));
        let mut channels = firmware_services::ant::Channels::new();
        channels.connect(peer(10).identity, 0).unwrap();
        let mut snapshots = channels.snapshots(0);
        let snapshot = snapshots[0].as_mut().unwrap();
        snapshot.link = LinkState::Connected;
        snapshot.stale = false;
        snapshot.age_ms = Some(0);
        // A stale observation before admission cannot acknowledge new intent.
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 0);
        assert!(indicator_has_color(&menu, CYAN));
        menu.request_accepted(peer(10).identity);
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 400);
        assert!(indicator_has_color(&menu, GREEN));
        snapshots[0].as_mut().unwrap().link = LinkState::TimedOut;
        menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 800);
        assert!(indicator_has_color(&menu, AMBER));
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
