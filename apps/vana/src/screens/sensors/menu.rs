//! Sensor selection state and physical-button navigation.
use super::layout::{
    DETAIL_BUTTONS, Layout, NEARBY_GRID, POWER_DETAIL_BUTTONS, REPLACE_BUTTONS, SEARCH_BUTTON,
    SEARCH_BUTTON_SIZE, SELECTED_GRID,
};
use super::slot::{LinkStatus, Selected};
use super::{CalibrationNote, MenuMessage};
use crate::sensors::ant::{BatteryStatus, device_type};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
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
    /// Zero the saved power meter.
    Calibrate,
    Ride,
}

/// A selected sensor as Home summarises it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Glance {
    pub kind: u8,
    pub live: bool,
    /// The status colour the sensor tiles use.
    pub color: u16,
    /// The battery status it last reported, if any.
    pub battery: Option<BatteryStatus>,
}

/// Saved ANT sensors and the one selected BLE sensor.
pub const GLANCES: usize = CHANNEL_CAPACITY + 1;

impl Glance {
    fn of(kind: u8, status: LinkStatus, battery: Option<BatteryStatus>) -> Self {
        let live = status == LinkStatus::Live;
        let low = battery.is_some_and(BatteryStatus::is_low);
        Self {
            kind,
            live,
            // A live sensor with a weak battery needs a look too.
            color: if live && low {
                super::render::AMBER
            } else {
                status.color()
            },
            battery,
        }
    }

    /// The BLE sensor the rider selected, live while its link is connected.
    pub fn ble(kind: u8, connected: bool, battery: Option<BatteryStatus>) -> Self {
        let status = if connected {
            LinkStatus::Live
        } else {
            LinkStatus::Waiting
        };
        Self::of(kind, status, battery)
    }

    /// A low or critical battery the rider should deal with.
    pub fn battery_low(&self) -> bool {
        self.battery.is_some_and(BatteryStatus::is_low)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostics {
    pub cursor: usize,
    pub last_press: Option<u64>,
    pub needs_nav: bool,
    pub message: Option<MenuMessage>,
    pub notice: Option<MenuMessage>,
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

/// The highlighted button on a detail or replacement page. `Calibrate` exists
/// only on a power meter's detail page, where it is the top button.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Choice {
    Primary,
    Secondary,
    Calibrate,
}

impl Choice {
    /// The next button down, wrapping; the calibrate button sits above the rest.
    const fn toggled(self, calibrate: bool) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary if calibrate => Self::Calibrate,
            Self::Secondary | Self::Calibrate => Self::Primary,
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
    /// Save trouble, kept apart from discovery progress so it stays visible
    /// for as long as the problem lasts.
    pub(super) notice: Option<MenuMessage>,
    /// How the latest power meter calibration stands, shown on its detail page.
    pub(super) calibration: Option<CalibrationNote>,
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
            notice: None,
            calibration: None,
        }
    }

    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            cursor: self.cursor,
            last_press: self.last_press,
            needs_nav: self.needs_navigation,
            message: self.message,
            notice: self.notice,
        }
    }

    pub fn set_message(&mut self, message: MenuMessage) {
        self.message = Some(message);
        self.prepare_layout();
    }

    /// Show or clear a lasting save problem; discovery messages never replace it.
    pub fn set_notice(&mut self, notice: Option<MenuMessage>) {
        self.notice = notice;
        self.prepare_layout();
    }

    pub fn set_calibration(&mut self, calibration: Option<CalibrationNote>) {
        self.calibration = calibration;
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

    /// Focus the tile holding this device type, such as a new selection.
    pub fn focus_type(&mut self, kind: u8) {
        if let Some(slot) = self
            .selected
            .iter()
            .position(|s| s.is_some_and(|s| s.identity.device_type == kind))
        {
            self.cursor = 1 + slot;
            self.prepare_layout();
        }
    }

    /// Show the saved sensors in their tile slots, and nearby discoveries.
    pub fn refresh(
        &mut self,
        discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
        selected: [Option<Selected>; CHANNEL_CAPACITY],
        scanning: bool,
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
        self.selected = selected;
        if let View::Detail(slot) = self.view
            && self.selected[slot].is_none()
        {
            self.view = View::Tiles;
            self.choice = Choice::Primary;
        }
        self.scanning = scanning;
        if !self.visible(self.cursor) {
            self.cursor = 0;
        }
        self.prepare_layout();
    }

    /// Selected sensors in tile order, for Home.
    pub fn glance(&self) -> heapless::Vec<Glance, GLANCES> {
        self.selected_peers()
            .map(|s| {
                let battery = s.battery.map(|battery| battery.status);
                Glance::of(s.identity.device_type, s.link_status(), battery)
            })
            .collect()
    }

    /// A tap focuses what is under the finger and activates it, as MOVE to it
    /// then ACT would. A tap names its target, so it also confirms focus
    /// after lost input.
    pub fn tap(&mut self, x: usize, y: usize) -> Option<Action> {
        let action = self.handle_tap(x, y);
        self.prepare_layout();
        action
    }

    fn handle_tap(&mut self, x: usize, y: usize) -> Option<Action> {
        let within = |(top, height): (usize, usize)| (top..top + height).contains(&y);
        match self.view {
            View::Tiles => {
                let (left, top) = SEARCH_BUTTON;
                let size = SEARCH_BUTTON_SIZE;
                let on_search = |margin: usize| {
                    (left.saturating_sub(margin)..left + size + margin).contains(&x)
                        && (top.saturating_sub(margin)..top + size + margin).contains(&y)
                };
                let selected = SELECTED_GRID
                    .tile(x, y)
                    .and_then(|(index, _, _)| self.layout.selected.get(index))
                    .map(|slot| 1 + slot);
                let nearby = NEARBY_GRID
                    .tile(x, y)
                    .and_then(|(index, _, _)| self.layout.nearby.get(index))
                    .map(|(cursor, _)| *cursor);
                // Shown tiles own their bounds; fingers are wider than the
                // search icon, so its margin takes only the space left over.
                let cursor = if on_search(0) {
                    0
                } else if let Some(cursor) = selected.or(nearby) {
                    cursor
                } else if on_search(8) {
                    0
                } else {
                    return None;
                };
                self.cursor = cursor;
            }
            View::Detail(slot) => {
                self.choice = if self.calibratable(slot) {
                    let [calibrate, disconnect, back] = POWER_DETAIL_BUTTONS;
                    if within(calibrate) {
                        Choice::Calibrate
                    } else if within(disconnect) {
                        Choice::Primary
                    } else if within(back) {
                        Choice::Secondary
                    } else {
                        return None;
                    }
                } else if within(DETAIL_BUTTONS[0]) {
                    Choice::Primary
                } else if within(DETAIL_BUTTONS[1]) {
                    Choice::Secondary
                } else {
                    return None;
                };
            }
            View::Replace(_) => {
                self.choice = if within(REPLACE_BUTTONS[0]) {
                    Choice::Primary
                } else if within(REPLACE_BUTTONS[1]) {
                    Choice::Secondary
                } else {
                    return None;
                };
            }
        }
        self.needs_navigation = false;
        self.activate()
    }

    /// Which subview touch lands on: the tiles, one sensor's detail page or
    /// a replacement prompt. A contact that began on one never acts on
    /// another.
    pub fn touch_view(&self) -> u8 {
        match self.view {
            View::Tiles => 0,
            View::Detail(slot) => 1 + slot.min(0x7e) as u8,
            View::Replace(_) => 0xff,
        }
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
                    View::Detail(slot) => {
                        self.choice = self.choice.toggled(self.calibratable(slot));
                    }
                    View::Replace(_) => self.choice = self.choice.toggled(false),
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
                let selected = self.selected[slot]?;
                if self.choice == Choice::Calibrate && self.calibratable(slot) {
                    return Some(Action::Calibrate);
                }
                self.view = View::Tiles;
                // The secondary button is Back.
                if self.choice == Choice::Secondary {
                    return None;
                }
                self.choice = Choice::Primary;
                self.cursor = 0;
                Some(Action::Disconnect(selected.identity.device_type))
            }
            View::Replace(identity) => {
                self.view = View::Tiles;
                if self.choice == Choice::Secondary {
                    return None;
                }
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
                    // Calibrating is harmless; disconnecting is not, so a
                    // power meter's page opens on the former.
                    self.choice = if self.calibratable(slot) {
                        Choice::Calibrate
                    } else {
                        Choice::Primary
                    };
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
        if self.selected_identity(kind).is_some() {
            self.view = View::Replace(identity);
            self.choice = Choice::Primary;
            return None;
        }
        if self.selected.iter().all(Option::is_some) {
            self.message = Some(MenuMessage::SlotsFull);
            return None;
        }
        Some(Action::Connect(identity))
    }

    /// Whether this saved sensor is a power meter, which can be zeroed.
    pub(super) fn calibratable(&self, slot: usize) -> bool {
        self.selected[slot].is_some_and(|s| s.identity.device_type == device_type::POWER)
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
}
