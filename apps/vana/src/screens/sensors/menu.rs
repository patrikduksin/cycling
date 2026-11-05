//! Sensor pages, their focus and physical-button navigation.
use super::layout::{Layout, Target};
use super::slot::{LinkStatus, Selected};
use super::{CalibrationNote, MenuMessage};
use crate::screens::canvas::Canvas;
use crate::screens::cue::{Corner, Cue};
use crate::sensors::ant::{BatteryStatus, device_type};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Scan,
    /// NEARBY closed, so nothing needs the search any more.
    StopScan,
    Connect(Identity),
    Disconnect(u8),
    /// Zero the saved power meter.
    Calibrate,
    Ride,
}

/// A selected sensor as Home summarises it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Glance {
    pub kind: u8,
    pub live: bool,
    /// The status colour the sensor rows use.
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
/// The sensor page on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Sensors,
    Nearby,
    Detail,
    Forget,
    Replace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostics {
    pub page: Page,
    /// The focused sensor, saved or nearby; `None` on the ADD SENSOR row or
    /// an empty list.
    pub focus: Option<Identity>,
    pub last_press: Option<u64>,
    pub needs_nav: bool,
    pub message: Option<MenuMessage>,
    pub notice: Option<MenuMessage>,
}

/// A row of SENSORS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mine {
    /// The saved sensor in this slot.
    Saved(usize),
    /// The ADD SENSOR row, the last, which opens NEARBY.
    Add,
}

/// A page and what it focuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum View {
    /// The saved sensors, then ADD SENSOR.
    Mine(Mine),
    /// NEARBY: sensors not saved yet, by discovery slot; `None` while there
    /// are none.
    Nearby(Option<usize>),
    /// A saved sensor's page, by slot.
    Detail(usize),
    /// FORGET SENSOR? for a saved sensor, by slot.
    Forget(usize),
    /// REPLACE SENSOR: a nearby sensor of a type already saved.
    Replace(Identity),
}

/// The sensor pages: what they show and focus, and where that places their
/// rows and cues after every change.
pub struct Menu {
    pub(super) state: State,
    pub(super) layout: Layout,
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

impl Menu {
    pub const fn new() -> Self {
        Self {
            state: State::new(),
            layout: Layout::new(),
        }
    }

    pub fn diagnostics(&self) -> Diagnostics {
        self.state.diagnostics()
    }

    pub fn set_message(&mut self, message: MenuMessage) {
        self.state.message = Some(message);
        self.prepare_layout();
    }

    /// Show or clear a lasting save problem; discovery messages never replace it.
    pub fn set_notice(&mut self, notice: Option<MenuMessage>) {
        self.state.notice = notice;
        self.prepare_layout();
    }

    pub fn set_calibration(&mut self, calibration: Option<CalibrationNote>) {
        if self.state.calibration != calibration {
            self.state.calibration = calibration;
            self.prepare_layout();
        }
    }

    /// Open the sensor pages: SENSORS on its first row, or straight to
    /// NEARBY, which searches, when nothing is saved yet.
    pub fn open(&mut self) -> Option<Action> {
        let action = self.state.open();
        self.prepare_layout();
        action
    }

    /// Begin a genuinely new bounded search: the nearby list starts empty.
    pub fn search_started(&mut self) {
        let state = &mut self.state;
        state.discoveries = [None; DISCOVERY_CAPACITY];
        state.present = [false; DISCOVERY_CAPACITY];
        state.settle();
        self.prepare_layout();
    }

    pub fn selected_identity(&self, kind: u8) -> Option<Identity> {
        self.state.selected_identity(kind)
    }

    /// Show SENSORS with the saved sensor of this device type focused, such
    /// as one just connected.
    pub fn focus_type(&mut self, kind: u8) {
        let state = &mut self.state;
        if let Some(slot) = state
            .selected
            .iter()
            .position(|s| s.is_some_and(|s| s.identity.device_type == kind))
        {
            state.view = View::Mine(Mine::Saved(slot));
            self.prepare_layout();
        }
    }

    /// Show the saved sensors in their slots, and nearby discoveries.
    pub fn refresh(
        &mut self,
        discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
        selected: [Option<Selected>; CHANNEL_CAPACITY],
        scanning: bool,
    ) {
        self.state.refresh(discoveries, selected, scanning);
        self.prepare_layout();
    }

    /// Selected sensors in slot order, for Home.
    pub fn glance(&self) -> heapless::Vec<Glance, GLANCES> {
        self.state
            .selected_peers()
            .map(|s| {
                let battery = s.battery.map(|battery| battery.status);
                Glance::of(s.identity.device_type, s.link_status(), battery)
            })
            .collect()
    }

    /// A tap focuses what is under the finger and activates it, as MOVE to it
    /// then ACT would; a tap on a cue acts as its button. A tap names its
    /// target, so it also confirms focus after lost input.
    pub fn tap(&mut self, x: usize, y: usize, now: u64) -> Option<Action> {
        let target = self.layout.target(x, y)?;
        let action = self.state.tap(target, now);
        self.prepare_layout();
        action
    }

    /// Which page touch lands on. A contact that began on one never acts on
    /// another.
    pub fn touch_view(&self) -> u8 {
        match self.state.view {
            View::Mine(_) => 0,
            View::Nearby(_) => 1,
            View::Detail(slot) => 2 + slot.min(0x3f) as u8,
            View::Forget(slot) => 0x80 + slot.min(0x3f) as u8,
            View::Replace(_) => 0xff,
        }
    }

    /// Draw the shown page for a frame. Only the frame keeps the canvas: it
    /// is too large to keep in the runtime, so each frame draws it afresh.
    #[inline(never)]
    pub fn canvas(&self) -> Canvas {
        let mut canvas = Canvas::EMPTY;
        self.layout.draw(&self.state, &mut canvas);
        canvas
    }

    /// The button cues painted over the shown page.
    pub fn cues(&self) -> &'static [Cue] {
        self.layout.cues
    }

    pub fn input(&mut self, input: Input, now: u64) -> Option<Action> {
        let action = self.state.input(input, now);
        self.prepare_layout();
        action
    }

    pub(super) fn prepare_layout(&mut self) {
        self.layout.prepare(&self.state);
    }

    /// NEARBY over the sensors already found, for runtime tests whose
    /// radio fixture would lose them to the search opening it starts.
    #[cfg(test)]
    pub(crate) fn show_found(&mut self) {
        self.state.view = View::Nearby(None);
        self.state.settle();
        self.prepare_layout();
    }
}

/// What the sensor pages show and focus.
pub(super) struct State {
    pub(super) discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    /// Whether each retained discovery was reported by the latest refresh.
    pub(super) present: [bool; DISCOVERY_CAPACITY],
    pub(super) selected: [Option<Selected>; CHANNEL_CAPACITY],
    pub(super) view: View,
    /// When a detail page last opened: ACT waits a moment after it.
    opened: u64,
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

/// ACT does nothing this long after a detail page opens, so the second of
/// two quick presses never zeroes a meter or asks to forget a sensor.
pub(super) const SETTLE_MS: u64 = 1_000;

/// A button, or the cue beside it, as the page acts on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Press {
    Back,
    Move,
    Act,
}

impl State {
    const fn new() -> Self {
        Self {
            discoveries: [None; DISCOVERY_CAPACITY],
            present: [false; DISCOVERY_CAPACITY],
            selected: [None; CHANNEL_CAPACITY],
            view: View::Mine(Mine::Add),
            opened: 0,
            last_press: None,
            scanning: false,
            needs_navigation: false,
            message: None,
            notice: None,
            calibration: None,
        }
    }

    fn diagnostics(&self) -> Diagnostics {
        let saved = |slot: usize| self.selected[slot].map(|s| s.identity);
        let (page, focus) = match self.view {
            View::Mine(Mine::Saved(slot)) => (Page::Sensors, saved(slot)),
            View::Mine(Mine::Add) => (Page::Sensors, None),
            View::Nearby(index) => (
                Page::Nearby,
                index.and_then(|i| self.discoveries[i]).map(|d| d.identity),
            ),
            View::Detail(slot) => (Page::Detail, saved(slot)),
            View::Forget(slot) => (Page::Forget, saved(slot)),
            View::Replace(identity) => (Page::Replace, Some(identity)),
        };
        Diagnostics {
            page,
            focus,
            last_press: self.last_press,
            needs_nav: self.needs_navigation,
            message: self.message,
            notice: self.notice,
        }
    }

    fn open(&mut self) -> Option<Action> {
        if self.selected_peers().next().is_none() {
            return self.open_nearby();
        }
        let first = self.rows().next().unwrap_or(Mine::Add);
        self.view = View::Mine(first);
        None
    }

    pub(super) fn selected_identity(&self, kind: u8) -> Option<Identity> {
        self.selected_peers()
            .find(|s| s.identity.device_type == kind)
            .map(|s| s.identity)
    }

    fn refresh(
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
        self.scanning = scanning;
        self.settle();
    }

    /// Keep the focus on something shown: where a sensor went, the next row
    /// takes the focus.
    fn settle(&mut self) {
        self.view = match self.view {
            View::Detail(slot) | View::Forget(slot) | View::Mine(Mine::Saved(slot))
                if self.selected[slot].is_none() =>
            {
                View::Mine(self.row_from(slot))
            }
            View::Nearby(focus) if focus.is_none_or(|index| !self.listed(index)) => {
                View::Nearby(self.nearby().next())
            }
            view => view,
        };
    }

    /// The first row at or after `slot`.
    fn row_from(&self, slot: usize) -> Mine {
        self.rows()
            .find(|row| matches!(row, Mine::Saved(s) if *s >= slot))
            .unwrap_or(Mine::Add)
    }

    fn tap(&mut self, target: Target, now: u64) -> Option<Action> {
        self.confirm_focus();
        match target {
            Target::Mine(row) => self.view = View::Mine(row),
            Target::Nearby(index) => self.view = View::Nearby(Some(index)),
            Target::Refresh => return self.search_again(),
            Target::Cue(Corner::TopLeft) => return self.press(Press::Back, now),
            Target::Cue(Corner::BottomLeft) => return self.press(Press::Move, now),
            Target::Cue(Corner::BottomRight) => {}
        }
        self.press(Press::Act, now)
    }

    fn input(&mut self, input: Input, now: u64) -> Option<Action> {
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
            Button::TopLeft => self.press(Press::Back, now),
            Button::BottomLeft => {
                self.confirm_focus();
                self.press(Press::Move, now)
            }
            // After lost input, the rider must confirm focus before activating.
            Button::BottomRight if self.needs_navigation => None,
            Button::BottomRight => self.press(Press::Act, now),
        }
    }

    /// MOVE or a tap shows the rider where the focus is.
    fn confirm_focus(&mut self) {
        self.needs_navigation = false;
        if self.message == Some(MenuMessage::InputLost) {
            self.message = None;
        }
    }

    fn press(&mut self, press: Press, now: u64) -> Option<Action> {
        let settled = now.saturating_sub(self.opened) >= SETTLE_MS;
        match (self.view, press) {
            (View::Mine(_), Press::Back) => Some(Action::Ride),
            (View::Mine(row), Press::Move) => {
                self.view = View::Mine(self.next_row(row));
                None
            }
            (View::Mine(Mine::Saved(slot)), Press::Act) => {
                self.view = View::Detail(slot);
                self.opened = now;
                None
            }
            (View::Mine(Mine::Add), Press::Act) => self.open_nearby(),
            // With nothing saved, SENSORS would show only ADD SENSOR.
            (View::Nearby(_), Press::Back) if self.selected_peers().next().is_none() => {
                Some(Action::Ride)
            }
            (View::Nearby(_), Press::Back) => {
                self.view = View::Mine(Mine::Add);
                Some(Action::StopScan)
            }
            (View::Nearby(focus), Press::Move) => {
                self.view = View::Nearby(self.next_nearby(focus));
                None
            }
            (View::Nearby(Some(index)), Press::Act) => self.choose_nearby(index),
            (View::Nearby(None), Press::Act) => self.search_again(),
            (View::Detail(slot), Press::Back) => {
                self.view = View::Mine(Mine::Saved(slot));
                None
            }
            (View::Detail(slot), Press::Act) if settled && self.calibratable(slot) => {
                Some(Action::Calibrate)
            }
            (View::Detail(slot), Press::Move) if self.calibratable(slot) => {
                self.view = View::Forget(slot);
                None
            }
            (View::Detail(slot), Press::Act) if settled && !self.calibratable(slot) => {
                self.view = View::Forget(slot);
                None
            }
            (View::Forget(slot), Press::Back) => {
                self.view = View::Detail(slot);
                self.opened = now;
                None
            }
            (View::Forget(slot), Press::Act) => {
                let kind = self.selected[slot]?.identity.device_type;
                self.view = View::Mine(Mine::Saved(slot));
                Some(Action::Disconnect(kind))
            }
            (View::Replace(identity), Press::Back) => {
                self.view = View::Nearby(self.index_of(identity));
                None
            }
            (View::Replace(identity), Press::Act) => {
                self.view = View::Nearby(self.index_of(identity));
                Some(Action::Connect(identity))
            }
            (View::Detail(_) | View::Forget(_) | View::Replace(_), _) => None,
        }
    }

    /// Open NEARBY, the only page that searches. It asks for a fresh
    /// search even while one stops, which the runtime then waits for.
    fn open_nearby(&mut self) -> Option<Action> {
        self.view = View::Nearby(None);
        self.settle();
        self.message = None;
        Some(Action::Scan)
    }

    fn search_again(&mut self) -> Option<Action> {
        if self.scanning {
            return None;
        }
        self.message = None;
        Some(Action::Scan)
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

    /// The rows of SENSORS, top to bottom.
    pub(super) fn rows(&self) -> impl Iterator<Item = Mine> + '_ {
        let saved = self.selected.iter().enumerate();
        saved
            .filter(|(_, selected)| selected.is_some())
            .map(|(slot, _)| Mine::Saved(slot))
            .chain([Mine::Add])
    }

    fn next_row(&self, row: Mine) -> Mine {
        let mut after = self.rows().skip_while(|r| *r != row).skip(1);
        after
            .next()
            .or_else(|| self.rows().next())
            .unwrap_or(Mine::Add)
    }

    /// Discovery slots NEARBY lists: nearby sensors not saved already.
    pub(super) fn nearby(&self) -> impl Iterator<Item = usize> + '_ {
        (0..DISCOVERY_CAPACITY).filter(|&index| self.listed(index))
    }

    fn listed(&self, index: usize) -> bool {
        self.discoveries[index]
            .is_some_and(|d| !self.selected_peers().any(|s| s.identity == d.identity))
    }

    fn next_nearby(&self, focus: Option<usize>) -> Option<usize> {
        let after = focus.and_then(|focus| self.nearby().find(|&index| index > focus));
        after.or_else(|| self.nearby().next())
    }

    fn index_of(&self, identity: Identity) -> Option<usize> {
        self.nearby()
            .find(|&index| self.discoveries[index].is_some_and(|d| d.identity == identity))
            .or_else(|| self.nearby().next())
    }

    pub(super) fn selected_peers(&self) -> impl Iterator<Item = &Selected> {
        self.selected.iter().flatten()
    }
}
