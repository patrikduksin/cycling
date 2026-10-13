//! Sensor selection state and physical-button navigation.
use super::MenuMessage;
use super::layout::Layout;
use super::slot::{Operation, Pending, Selected};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity, Snapshot};
use device_api::input::{Button, Input};

/// Cursor positions: the search button, then selected slots, then nearby sensors.
pub(super) const AVAILABLE: usize = 1 + CHANNEL_CAPACITY;
const ITEMS: usize = AVAILABLE + DISCOVERY_CAPACITY;
const DEBOUNCE_MS: u64 = 350;

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
    pub message: Option<MenuMessage>,
}

/// What the cursor points at.
#[derive(Clone, Copy)]
pub(super) enum Item {
    Search,
    Selected(usize),
    Nearby(usize),
}

impl Item {
    pub(super) const fn at(cursor: usize) -> Self {
        if cursor == 0 {
            Self::Search
        } else if cursor < AVAILABLE {
            Self::Selected(cursor - 1)
        } else {
            Self::Nearby(cursor - AVAILABLE)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum View {
    Tiles,
    Detail(usize),
    Replace(Identity),
}

/// The highlighted button on a detail or replacement page.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Choice {
    Primary,
    Secondary,
}

impl Choice {
    const fn toggled(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Primary,
        }
    }
}

pub struct Menu {
    pub(super) discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    /// Whether each retained discovery was reported by the latest refresh.
    pub(super) present: [bool; DISCOVERY_CAPACITY],
    pub(super) selected: [Option<Selected>; CHANNEL_CAPACITY],
    pub(super) cursor: usize,
    pub(super) layout: Layout,
    pub(super) view: View,
    pub(super) choice: Choice,
    last_press: Option<u64>,
    pub(super) scanning: bool,
    needs_navigation: bool,
    pub(super) message: Option<MenuMessage>,
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
            choice: Choice::Primary,
            last_press: None,
            scanning: false,
            needs_navigation: false,
            message: None,
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

    pub fn set_message(&mut self, message: MenuMessage) {
        self.message = Some(message);
        self.prepare_layout();
    }

    /// Begin a genuinely new bounded search without moving selected-sensor focus.
    pub fn search_started(&mut self) {
        self.discoveries = [None; DISCOVERY_CAPACITY];
        self.present = [false; DISCOVERY_CAPACITY];
        if matches!(Item::at(self.cursor), Item::Nearby(_)) {
            self.cursor = 0;
        }
        self.prepare_layout();
    }

    pub fn selected_identity(&self, kind: u8) -> Option<Identity> {
        self.selected_peers()
            .find(|s| s.identity.device_type == kind)
            .map(|s| s.identity)
    }

    pub fn request_pending(&mut self, identity: Identity) {
        if let Some(slot) = self.slot_for(identity.device_type) {
            self.selected[slot] = Some(Selected::requested(identity, Operation::Connect));
            self.cursor = 1 + slot;
        }
        self.prepare_layout();
    }

    pub fn request_accepted(&mut self, identity: Identity) {
        if let Some(pending) = self.find_mut(identity).and_then(|s| s.pending.as_mut()) {
            pending.admitted = true;
        }
        self.prepare_layout();
    }

    pub fn request_failed(&mut self, identity: Identity, message: MenuMessage) {
        if let Some(s) = self.find_mut(identity) {
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
            let Some(slot) = self.slot_for(identity.device_type) else {
                continue;
            };
            match &mut self.selected[slot] {
                Some(selected) if selected.identity == identity => selected.observe(snapshot),
                None => self.selected[slot] = Some(Selected::observed(identity, snapshot)),
                // A different peer of this type awaits the rider's replacement.
                Some(_) => {}
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
            self.message = Some(MenuMessage::InputLost);
            return None;
        }
        let Input::Button { button, code: 1 } = input else {
            return None;
        };
        if self
            .last_press
            .is_some_and(|last| now.saturating_sub(last) < DEBOUNCE_MS)
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
                    self.choice = Choice::Primary;
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
                        self.choice = Choice::Primary
                    }
                    _ => self.choice = self.choice.toggled(),
                }
                None
            }
            // After lost input, the rider must confirm focus before activating.
            Button::BottomRight if self.needs_navigation => None,
            Button::BottomRight => self.activate(),
        }
    }

    fn activate(&mut self) -> Option<Action> {
        match self.view {
            View::Detail(slot) => {
                let mut selected = self.selected[slot]?;
                self.view = View::Tiles;
                // A pending request leaves only Back.
                if selected.pending.is_some() {
                    return None;
                }
                let reconnect = selected.retry() && !selected.failed_disconnect;
                let disconnect = match self.choice {
                    Choice::Primary if reconnect => {
                        self.request_pending(selected.identity);
                        return Some(Action::Connect(selected.identity));
                    }
                    Choice::Primary => true,
                    // Otherwise the secondary button is Back.
                    Choice::Secondary => reconnect && selected.can_disconnect(),
                };
                if !disconnect {
                    return None;
                }
                selected.pending = Some(Pending {
                    operation: Operation::Disconnect,
                    admitted: false,
                });
                selected.failure = None;
                self.selected[slot] = Some(selected);
                Some(Action::Disconnect(selected.identity.device_type))
            }
            View::Replace(identity) => {
                self.view = View::Tiles;
                if self.choice == Choice::Secondary {
                    return None;
                }
                self.request_pending(identity);
                Some(Action::Connect(identity))
            }
            View::Tiles => match Item::at(self.cursor) {
                Item::Search => {
                    if self.scanning {
                        return None;
                    }
                    self.message = None;
                    Some(Action::Scan)
                }
                Item::Selected(slot) => {
                    self.view = View::Detail(slot);
                    self.choice = Choice::Primary;
                    None
                }
                Item::Nearby(index) => self.choose_nearby(index),
            },
        }
    }

    fn choose_nearby(&mut self, index: usize) -> Option<Action> {
        let identity = self.discoveries[index]?.identity;
        let kind = identity.device_type;
        if !self.present[index] {
            self.message = Some(MenuMessage::NotSeen);
            return None;
        }
        if self
            .selected_peers()
            .any(|s| s.identity.device_type == kind && s.pending.is_some())
        {
            self.message = Some(MenuMessage::WaitForCurrentRequest);
            return None;
        }
        if self.selected_identity(kind).is_some() {
            self.view = View::Replace(identity);
            self.choice = Choice::Primary;
            return None;
        }
        if self.selected.iter().all(Option::is_some) {
            self.message = Some(MenuMessage::SlotsFull);
            return None;
        }
        self.request_pending(identity);
        Some(Action::Connect(identity))
    }

    pub(super) fn visible(&self, cursor: usize) -> bool {
        match Item::at(cursor) {
            Item::Search => true,
            Item::Selected(slot) => self.selected[slot].is_some(),
            Item::Nearby(index) => self.discoveries[index]
                .is_some_and(|d| !self.selected_peers().any(|s| s.identity == d.identity)),
        }
    }

    pub(super) fn selected_peers(&self) -> impl Iterator<Item = &Selected> {
        self.selected.iter().flatten()
    }

    /// The slot holding this device type, or the first free slot.
    fn slot_for(&self, kind: u8) -> Option<usize> {
        self.selected
            .iter()
            .position(|s| s.is_some_and(|s| s.identity.device_type == kind))
            .or_else(|| self.selected.iter().position(Option::is_none))
    }

    fn find_mut(&mut self, identity: Identity) -> Option<&mut Selected> {
        self.selected
            .iter_mut()
            .flatten()
            .find(|s| s.identity == identity)
    }
}
