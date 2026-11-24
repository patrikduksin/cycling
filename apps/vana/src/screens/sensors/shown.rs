//! What the sensor pages show, as their tiles read it, and what a finger
//! lands on.
use super::menu::{Action, Menu, Mine, View};
use super::slot::{LinkStatus, Selected};
use super::{CalibrationNote, MenuMessage, kind};
use crate::compose::builtin::{Laid, ScreenId};
use crate::compose::data::ListKind;
use crate::compose::node::What;
use crate::compose::registry::SENSOR_SEARCH;
use crate::screens::Device;
use crate::screens::chrome::{ACCENT, AMBER, GREEN, RED, SECONDARY, WHITE};
use crate::screens::scroll;
use crate::screens::workout::Text;
use crate::sensors::ant::BatteryStatus;
use core::fmt::Write;
use device_api::ant::Identity;
use device_api::input::Role;

/// What a tap lands on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Target {
    Mine(Mine),
    /// A nearby sensor, by discovery slot.
    Nearby(usize),
    Refresh,
    /// A button cue: the tap acts as its button.
    Cue(Role),
}

/// An item of a sensor list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Item {
    Saved(Selected),
    /// SENSORS' last row, which opens NEARBY.
    Add,
    /// A sensor NEARBY found, and whether the latest search still sees it.
    Found {
        identity: Identity,
        present: bool,
    },
}

impl Menu {
    /// The screen the page shown draws from.
    pub fn screen(&self) -> ScreenId {
        let state = &self.state;
        match state.view {
            View::Mine(_) => ScreenId::Sensors,
            View::Nearby(_) => ScreenId::SensorsNearby,
            View::Detail(slot) if state.calibratable(slot) => ScreenId::PowerMeter,
            View::Detail(_) => ScreenId::SensorDetail,
            View::Forget(_) => ScreenId::ForgetSensor,
            View::Replace(_) => ScreenId::ReplaceSensor,
        }
    }

    pub fn count(&self, list: ListKind) -> u16 {
        let count = match list {
            ListKind::Sensors => self.state.rows().count(),
            ListKind::Nearby => self.state.nearby().count(),
            _ => 0,
        };
        count as u16
    }

    /// The item the buttons act on, while its list shows.
    pub fn focus(&self, list: ListKind) -> Option<u16> {
        let state = &self.state;
        let index = match (list, state.view) {
            (ListKind::Sensors, View::Mine(row)) => state.rows().position(|r| r == row),
            (ListKind::Nearby, View::Nearby(Some(slot))) => {
                state.nearby().position(|index| index == slot)
            }
            _ => None,
        };
        index.map(|index| index as u16)
    }

    pub fn item(&self, list: ListKind, index: u16) -> Option<Item> {
        let state = &self.state;
        let index = usize::from(index);
        match list {
            ListKind::Sensors => match state.rows().nth(index)? {
                Mine::Saved(slot) => state.selected[slot].map(Item::Saved),
                Mine::Add => Some(Item::Add),
            },
            ListKind::Nearby => {
                let slot = state.nearby().nth(index)?;
                let found = state.discoveries[slot]?;
                Some(Item::Found {
                    identity: found.identity,
                    present: state.present[slot],
                })
            }
            _ => None,
        }
    }

    /// SENSORS' title: trouble with the buttons, the radio or saving in
    /// amber, else its name.
    pub fn title(&self) -> (&'static str, u16) {
        let state = &self.state;
        match state.notice.or(state.message.filter(|m| m.on_sensors())) {
            Some(problem) => (problem.short(), AMBER),
            None => ("SENSORS", WHITE),
        }
    }

    pub const fn scanning(&self) -> bool {
        self.state.scanning
    }

    /// NEARBY's line under its title: a problem in amber, the sensor just
    /// added in green, else how the search goes.
    pub fn search_line(&self) -> (Text<24>, u16) {
        let state = &self.state;
        let routine = if state.scanning { ACCENT } else { SECONDARY };
        let mut line = Text::new();
        let (text, color) = match state.notice.or(state.message) {
            Some(message) if !message.routine() => (message.short(), AMBER),
            Some(MenuMessage::Added(device_type)) => {
                let name = core::str::from_utf8(kind(device_type)).unwrap_or("");
                let _ = write!(line, "{name} ADDED");
                return (line, GREEN);
            }
            Some(MenuMessage::ScanDone) if state.nearby().next().is_none() => {
                ("NO SENSORS FOUND", routine)
            }
            Some(message) => (message.short(), routine),
            None if state.scanning => (MenuMessage::Scanning.short(), routine),
            None => ("FIND NEARBY", routine),
        };
        let _ = line.push_str(text);
        (line, color)
    }

    /// The saved sensor a detail page or FORGET SENSOR? shows.
    pub fn shown(&self) -> Option<Selected> {
        match self.state.view {
            View::Detail(slot) | View::Forget(slot) => self.state.selected[slot],
            _ => None,
        }
    }

    /// The sensor REPLACE SENSOR would save, and the saved one it replaces.
    pub fn replacing(&self) -> Option<(Identity, Option<Identity>)> {
        let View::Replace(new) = self.state.view else {
            return None;
        };
        Some((new, self.state.selected_identity(new.device_type)))
    }

    /// The sensor a page's icon card pictures.
    pub fn pictured(&self) -> Option<Identity> {
        let shown = self.shown().map(|selected| selected.identity);
        shown.or_else(|| self.replacing().map(|(new, _)| new))
    }

    /// How the power meter's latest calibration stands, on its page.
    pub fn calibration(&self) -> Option<CalibrationNote> {
        match self.state.view {
            View::Detail(slot) if self.state.calibratable(slot) => self.state.calibration,
            _ => None,
        }
    }

    /// The device number, with the transmission type when another known peer
    /// of the same type shares that number.
    pub fn identity_label(&self, identity: Identity) -> Text<12> {
        let mut label = Text::new();
        let _ = write!(label, "{}", identity.device_number);
        if self.shares_number(identity) {
            let _ = write!(label, ".{}", identity.transmission_type);
        }
        label
    }

    /// Another known peer of this type has the same device number.
    pub fn shares_number(&self, identity: Identity) -> bool {
        let state = &self.state;
        let discovered = state.discoveries.iter().flatten().map(|d| d.identity);
        let saved = state.selected_peers().map(|s| s.identity);
        discovered.chain(saved).any(|other| {
            other != identity
                && other.device_type == identity.device_type
                && other.device_number == identity.device_number
        })
    }

    /// How far the list `laid` draws is scrolled: just far enough that its
    /// focus shows whole.
    pub fn offset(&self, laid: &Laid) -> i32 {
        let Some((list, page)) = drawn(laid) else {
            return 0;
        };
        let focus = self.focus(list);
        focus.map_or(0, |focus| page.reveal(usize::from(focus), 0))
    }

    /// A tap at (`x`, `y`) on `laid`, the page shown on `device`'s panel.
    pub fn tap_at(
        &mut self,
        laid: &Laid,
        device: &Device,
        x: usize,
        y: usize,
        now: u64,
    ) -> Option<Action> {
        let target = self.target(laid, device, x, y)?;
        self.state.tap(target, now)
    }

    /// The cue, card or search line under (`x`, `y`).
    fn target(&self, laid: &Laid, device: &Device, x: usize, y: usize) -> Option<Target> {
        let mut cues = laid.controls().shown(laid.focus());
        if let Some(cue) = cues.find(|cue| cue.target(device).contains(x, y)) {
            return Some(Target::Cue(cue.role));
        }
        if laid
            .box_of(SENSOR_SEARCH)
            .is_some_and(|search| search.contains(x, y))
        {
            return Some(Target::Refresh);
        }
        let (list, page) = drawn(laid)?;
        let count = usize::from(self.count(list));
        let index = page.card_at(count, self.offset(laid), x, y)?;
        let state = &self.state;
        match list {
            ListKind::Nearby => state.nearby().nth(index).map(Target::Nearby),
            _ => state.rows().nth(index).map(Target::Mine),
        }
    }
}

/// The list `laid`'s tree scrolls and its page of cards: what draws is what
/// a tap reads.
fn drawn(laid: &Laid) -> Option<(ListKind, &scroll::Layout)> {
    let node = laid.scroll()?.node;
    match laid.screen().tree().nodes()[usize::from(node)].what()? {
        What::Scroll { list } => Some((list, laid.page()?)),
        _ => None,
    }
}

impl Selected {
    /// Its state, as its row and its page name it.
    pub fn link(self) -> (&'static [u8], u16) {
        let status = self.link_status();
        let text = if status == LinkStatus::Live {
            b"LIVE"
        } else {
            self.status_text()
        };
        (text, status.color())
    }

    /// Its battery on its row, only once it needs changing; its page has
    /// the voltage.
    pub fn battery_warning(self) -> Option<(&'static [u8], u16)> {
        match self.battery?.status {
            BatteryStatus::Critical => Some((b"BATTERY CRITICAL", RED)),
            BatteryStatus::Low => Some((b"BATTERY LOW", AMBER)),
            BatteryStatus::New | BatteryStatus::Good | BatteryStatus::Ok => None,
        }
    }

    pub const fn live(self) -> bool {
        matches!(self.link_status(), LinkStatus::Live)
    }
}

#[cfg(test)]
impl Menu {
    /// The page shown in a frame of its own on `device`'s panel, drawn
    /// from its tree as the runtime draws it.
    pub fn frame(&self, device: &'static Device) -> crate::screens::workout::View<'_> {
        let page = crate::screens::workout::Page::Sensors;
        let mut view = crate::screens::workout::View::new(page, 0, None);
        view.device = device;
        view.show_sensors(self);
        view.compose_main();
        view
    }
}

/// The widest words each place on the pages may show, in a face: what a
/// tile's looks are measured on, so accept refuses a box that would cut
/// them.
pub mod widest {
    use super::super::{CalibrationNote, Connection, MenuMessage, NAMES};
    use firmware_shell::rendering::font::Face;

    /// The columns the widest of `texts` covers in `face`.
    pub fn of<'t>(face: &Face, texts: impl IntoIterator<Item = &'t [u8]>) -> usize {
        texts
            .into_iter()
            .map(|text| face.fit_width(text))
            .max()
            .unwrap_or(0)
    }

    /// `digits` of the face's widest digit, without the spill.
    pub fn digits(face: &Face, digits: usize) -> usize {
        let widest = (b'0'..=b'9').map(|digit| face.width(&[digit])).max();
        digits * widest.unwrap_or(0)
    }

    pub fn name(face: &Face) -> usize {
        of(face, NAMES)
    }

    /// A saved sensor's state, or the battery warning that takes its place.
    pub fn state(face: &Face) -> usize {
        let states = Connection::ALL.map(|connection| connection.text().as_bytes());
        let warnings: [&[u8]; 3] = [b"LIVE", b"BATTERY LOW", b"BATTERY CRITICAL"];
        of(face, states).max(of(face, warnings))
    }

    /// A message's short wording, a sensor just added included.
    pub fn message(face: &Face) -> usize {
        let shorts = MenuMessage::ALL.map(|message| message.short().as_bytes());
        let added = name(face) + face.width(b" ADDED");
        of(face, shorts).max(added)
    }

    /// What SENSORS' title says: its name, or the trouble it names there.
    pub fn title(face: &Face) -> usize {
        let trouble = MenuMessage::ALL
            .into_iter()
            .filter(|message| message.on_sensors());
        let trouble = trouble.chain(MenuMessage::NOTICES);
        let name: &[u8] = b"SENSORS";
        of(
            face,
            trouble
                .map(|message| message.short().as_bytes())
                .chain([name]),
        )
    }

    /// A calibration note, any offset included.
    pub fn calibration(face: &Face) -> usize {
        let texts = CalibrationNote::ALL.map(CalibrationNote::text);
        let notes = of(face, texts.iter().map(|text| text.as_bytes()));
        let offset = face.fit_width(b"CALIBRATED OFFSET -") + digits(face, 5);
        notes.max(offset)
    }

    /// A device number with its transmission type, such as "65535.255".
    pub fn identity(face: &Face) -> usize {
        face.fit_width(b".") + digits(face, 8)
    }
}
