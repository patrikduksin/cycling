//! What the sensor pages show, as their tiles read it, and what a finger
//! lands on.
use super::menu::{Action, Menu, Mine, View, bars};
use super::slot::Selected;
use super::{CalibrationNote, MenuMessage, names};
use crate::compose::builtin::{Laid, ScreenId};
use crate::compose::data::Metric;
use crate::compose::registry::{self, ITEM, NAV_HEADER, SENSOR_LIST, SENSOR_SEARCH};
use crate::compose::tile::Arg;
use crate::compose::tiles::sensor_list;
use crate::screens::Device;
use crate::screens::theme::Tone;
use crate::screens::workout::Text;
use crate::sensors::ant::device_type;
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
    Key(Role),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum List {
    Sensors = 0,
    Nearby = 1,
}

impl Arg for List {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Sensors),
            1 => Some(Self::Nearby),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sensors => "SENSORS",
            Self::Nearby => "NEARBY",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Part {
    Detail = 0,
    Forget = 1,
}

impl Arg for Part {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Detail),
            1 => Some(Self::Forget),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Detail => "DETAIL",
            Self::Forget => "FORGET",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Row {
    /// A saved sensor; `new` when the search just before added it.
    Saved { sensor: Selected, new: bool },
    /// SENSORS' last row, which opens NEARBY.
    Add,
    /// A sensor this search added, folded into a short row.
    Added(Selected),
    Found {
        identity: Identity,
        present: bool,
        bars: u8,
    },
}

impl Row {
    pub const fn short(self) -> bool {
        matches!(self, Self::Added(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    pub metric: Metric,
    /// Over the number on a row.
    pub tag: &'static str,
    /// After the number on the page, its space included.
    pub unit: &'static str,
    pub tone: Tone,
}

const fn read(metric: Metric, tag: &'static str, unit: &'static str, tone: Tone) -> Reading {
    Reading {
        metric,
        tag,
        unit,
        tone,
    }
}

pub const fn reading(kind: u8) -> Option<Reading> {
    Some(match kind {
        device_type::POWER => read(Metric::Power, "W", " W", Tone::Power),
        device_type::HEART_RATE => read(Metric::Heart, "BPM", " bpm", Tone::Heart),
        device_type::SPEED_CADENCE | device_type::CADENCE => {
            read(Metric::Cadence, "RPM", " rpm", Tone::Cadence)
        }
        device_type::SPEED => read(Metric::Speed, "KM/H", " km/h", Tone::Speed),
        _ => return None,
    })
}

#[derive(Clone)]
pub struct Search {
    pub text: Text<32>,
    pub tone: Tone,
    pub added: usize,
    /// The search runs with nothing added and nothing wrong.
    pub wake: bool,
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
        }
    }

    /// The rows of `list`, top to bottom. NEARBY lists what this search
    /// added first, then what it found, strongest first.
    pub fn rows(&self, list: List) -> impl Iterator<Item = Row> + '_ {
        let state = &self.state;
        let mine = (list == List::Sensors).then(|| {
            state.rows().filter_map(|row| match row {
                Mine::Saved(slot) => state.selected[slot].map(|sensor| Row::Saved {
                    sensor,
                    new: state.added.contains(&sensor.identity),
                }),
                Mine::Add => Some(Row::Add),
            })
        });
        let nearby = list == List::Nearby;
        let added = nearby.then(|| self.just_added().map(Row::Added));
        let found = nearby.then(|| {
            state.nearby().filter_map(|slot| {
                let found = state.discoveries[slot]?;
                Some(Row::Found {
                    identity: found.identity,
                    present: state.present[slot],
                    bars: bars(found.rssi),
                })
            })
        });
        let mine = mine.into_iter().flatten();
        mine.chain(added.into_iter().flatten())
            .chain(found.into_iter().flatten())
    }

    fn just_added(&self) -> impl Iterator<Item = Selected> + '_ {
        let state = &self.state;
        state
            .added
            .iter()
            .filter_map(|peer| state.selected[state.slot_of(*peer)?])
    }

    /// The row of `list` the keys act on, while it shows.
    pub fn focus(&self, list: List) -> Option<usize> {
        let state = &self.state;
        match (list, state.view) {
            (List::Sensors, View::Mine(row)) => state.rows().position(|r| r == row),
            (List::Nearby, View::Nearby(Some(slot))) => {
                let found = state.nearby().position(|index| index == slot)?;
                Some(self.just_added().count() + found)
            }
            _ => None,
        }
    }

    pub fn title(&self, list: List) -> (&'static str, Tone) {
        let state = &self.state;
        let trouble = state.notice.or(state.message.filter(|m| m.on_sensors()));
        match (list, trouble) {
            (List::Sensors, Some(problem)) => (problem.short(), Tone::Warn),
            (List::Sensors, None) => ("Sensors", Tone::Text),
            (List::Nearby, _) => ("Nearby", Tone::Text),
        }
    }

    pub fn search(&self) -> Search {
        let state = &self.state;
        let added = self.just_added().count();
        let message = state.notice.or(state.message);
        let full = state.full() && message.is_none_or(MenuMessage::routine);
        let (text, tone) = match message {
            _ if full => (MenuMessage::SlotsFull.short(), Tone::Warn),
            Some(message) if !message.routine() => (message.short(), Tone::Warn),
            Some(MenuMessage::ScanDone) if state.nearby().next().is_none() => {
                ("NO SENSORS FOUND", Tone::Sub)
            }
            Some(MenuMessage::Added | MenuMessage::Scanning) | None if state.scanning => {
                ("SEARCHING", Tone::Info)
            }
            Some(MenuMessage::Added) | None => ("TAP TO SEARCH", Tone::Sub),
            Some(message) => (message.short(), Tone::Info),
        };
        let mut line = Text::new();
        let _ = line.push_str(text);
        Search {
            wake: added == 0 && text == "SEARCHING",
            text: line,
            tone,
            added,
        }
    }

    pub fn hint(&self) -> &'static str {
        if self.state.full() {
            "BACK, THEN FORGET ONE"
        } else if self.state.added.is_empty() {
            "TAP TO ADD ONE OR MORE"
        } else {
            "BACK WHEN DONE"
        }
    }

    /// The saved sensor a detail page or FORGET? shows.
    pub fn shown(&self) -> Option<Selected> {
        match self.state.view {
            View::Detail(slot) | View::Forget(slot) => self.state.selected[slot],
            _ => None,
        }
    }

    /// How the power meter's latest calibration stands, on its page.
    pub fn calibration(&self) -> Option<CalibrationNote> {
        match self.state.view {
            View::Detail(slot) if self.state.calibratable(slot) => {
                let (meter, note) = self.state.calibration?;
                (self.state.selected[slot]?.identity == meter).then_some(note)
            }
            _ => None,
        }
    }

    pub fn row_name(&self, sensor: Selected) -> Text<24> {
        let kind = sensor.identity.device_type;
        let twin = self
            .state
            .selected_peers()
            .any(|other| other.identity != sensor.identity && other.identity.device_type == kind);
        let mut name = Text::new();
        match sensor.product.maker() {
            Some(maker) if twin => {
                let _ = name.push_str(maker);
            }
            _ if twin => {
                let label = self.identity_label(sensor.identity);
                let _ = write!(name, "{} {}", names(kind).short, label.as_str());
            }
            _ => {
                let _ = name.push_str(names(kind).short);
            }
        }
        name
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

    fn target(&self, laid: &Laid, device: &Device, x: usize, y: usize) -> Option<Target> {
        let mut cues = laid.controls().shown(laid.focus());
        if let Some(cue) = cues.find(|cue| cue.target(device).contains(x, y)) {
            return Some(Target::Key(cue.role));
        }
        let under = |component| laid.box_of(component).is_some_and(|b| b.contains(x, y));
        if under(NAV_HEADER) {
            return Some(Target::Key(Role::Back));
        }
        if under(SENSOR_SEARCH) {
            return Some(Target::Refresh);
        }
        let mut tiles = laid.tiles();
        let (component, arg, placed) =
            tiles.find(|(_, _, placed)| placed.rect.rect().contains(x, y))?;
        match component {
            ITEM => {
                let action = registry::tile(ITEM).action(arg)?;
                let buttons = laid.buttons();
                let roles = [
                    (Role::Move, buttons.move_),
                    (Role::Act, buttons.act),
                    (Role::Back, buttons.back),
                ];
                let (role, _) = roles
                    .into_iter()
                    .find(|(_, button)| button.is_some_and(|button| button.action == action))?;
                Some(Target::Key(role))
            }
            SENSOR_LIST => {
                let list = List::decode(arg)?;
                let rows = sensor_list::placed(self, list, placed.rect.rect(), &device.scaled);
                let (index, _, _) = rows.into_iter().find(|(_, _, rect)| rect.contains(x, y))?;
                let state = &self.state;
                match list {
                    List::Sensors => state.rows().nth(index).map(Target::Mine),
                    List::Nearby => {
                        let found = index.checked_sub(self.just_added().count())?;
                        state.nearby().nth(found).map(Target::Nearby)
                    }
                }
            }
            _ => None,
        }
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
    use super::super::{MenuMessage, NAMES};
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

    pub fn long_name(face: &Face) -> usize {
        of(face, NAMES.map(|names| names.long.as_bytes()))
    }

    pub fn message(face: &Face) -> usize {
        of(
            face,
            MenuMessage::ALL.map(|message| message.short().as_bytes()),
        )
    }

    /// A device number with its transmission type, such as "65535.255".
    pub fn identity(face: &Face) -> usize {
        face.fit_width(b".") + digits(face, 8)
    }
}
