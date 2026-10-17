//! Text and tile placement derived after every state change, so rendering a
//! pixel never has to search the menu state.
use super::kind;
use super::menu::{AVAILABLE, Item, Menu, View};
use super::render::{AMBER, CYAN, MUTED};
use super::slot::{LinkStatus, Selected};
use crate::sensors::ant::device_type;
use core::fmt::Write;
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Identity};
use heapless::Vec;

/// A row-major grid of equally sized tiles.
pub(super) struct Grid {
    pub(super) left: usize,
    pub(super) top: usize,
    pub(super) columns: usize,
    pub(super) rows: usize,
    pub(super) pitch_x: usize,
    pub(super) pitch_y: usize,
    pub(super) width: usize,
    pub(super) height: usize,
}

impl Grid {
    /// Index, left and top of each tile whose rows include `y`.
    pub(super) fn tiles(&self, y: usize) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        let row = y
            .checked_sub(self.top)
            .map(|offset| offset / self.pitch_y)
            .filter(|&row| row < self.rows && y < self.top + row * self.pitch_y + self.height);
        row.into_iter().flat_map(move |row| {
            (0..self.columns).map(move |column| {
                (
                    row * self.columns + column,
                    self.left + column * self.pitch_x,
                    self.top + row * self.pitch_y,
                )
            })
        })
    }

    /// The tile index under a point and the tile's top-left corner.
    #[cfg(test)]
    pub(super) fn tile(&self, x: usize, y: usize) -> Option<(usize, usize, usize)> {
        if x < self.left || y < self.top {
            return None;
        }
        let (column, row) = (
            (x - self.left) / self.pitch_x,
            (y - self.top) / self.pitch_y,
        );
        if column >= self.columns || row >= self.rows {
            return None;
        }
        let left = self.left + column * self.pitch_x;
        let top = self.top + row * self.pitch_y;
        ((left..left + self.width).contains(&x) && (top..top + self.height).contains(&y))
            .then_some((row * self.columns + column, left, top))
    }
}

/// Two rows of five, room for every selectable channel.
pub(super) const SELECTED_GRID: Grid = Grid {
    left: 8,
    top: 56,
    columns: 5,
    rows: 2,
    pitch_x: 46,
    pitch_y: 38,
    width: 40,
    height: 32,
};

/// Two rows of four, room for every retained discovery.
pub(super) const NEARBY_GRID: Grid = Grid {
    left: 11,
    top: 156,
    columns: 4,
    rows: 2,
    pitch_x: 56,
    pitch_y: 36,
    width: 50,
    height: 32,
};

const LABEL_CAPACITY: usize = 18;

/// Bounded text that silently truncates at the status line's width.
#[derive(Clone)]
pub(super) struct Label(Vec<u8, LABEL_CAPACITY>);

impl Label {
    pub(super) const fn new() -> Self {
        Self(Vec::new())
    }

    pub(super) fn push(&mut self, value: &[u8]) {
        let room = LABEL_CAPACITY - self.0.len();
        let _ = self.0.extend_from_slice(&value[..value.len().min(room)]);
    }

    fn number(&mut self, value: u16) {
        let mut digits = heapless::String::<5>::new();
        let _ = write!(digits, "{value}");
        self.push(digits.as_bytes());
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(super) struct Layout {
    /// Occupied selection slots, in tile order.
    pub(super) selected: Vec<usize, CHANNEL_CAPACITY>,
    /// Visible nearby tiles in discovery order, with their cursor position.
    pub(super) nearby: Vec<(usize, Identity), DISCOVERY_CAPACITY>,
    pub(super) title: &'static [u8],
    pub(super) page_feedback: Label,
    pub(super) status: Label,
    pub(super) identity: Label,
    pub(super) replacement_numbers: [Label; 2],
    pub(super) replacement_transmissions: [Label; 2],
    pub(super) color: u16,
    pub(super) action: &'static [u8],
}

impl Layout {
    pub(super) const fn new() -> Self {
        Self {
            selected: Vec::new(),
            nearby: Vec::new(),
            title: b"SCAN",
            page_feedback: Label::new(),
            status: Label::new(),
            identity: Label::new(),
            replacement_numbers: [Label::new(), Label::new()],
            replacement_transmissions: [Label::new(), Label::new()],
            color: MUTED,
            action: b"SEARCH",
        }
    }
}

enum Focus {
    Selected(Selected),
    Nearby { identity: Identity, present: bool },
}

impl Menu {
    pub(super) fn prepare_layout(&mut self) {
        let mut layout = Layout::new();
        layout.selected = self
            .selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| selected.is_some())
            .map(|(slot, _)| slot)
            .collect();
        layout.nearby = (AVAILABLE..AVAILABLE + DISCOVERY_CAPACITY)
            .filter(|&cursor| self.visible(cursor))
            .filter_map(|cursor| Some((cursor, self.discoveries[cursor - AVAILABLE]?.identity)))
            .collect();
        match self.focus() {
            Some(focus) => self.describe_focus(&mut layout, focus),
            None => {
                layout.status.push(if self.scanning {
                    b"SEARCHING..."
                } else {
                    b"FIND NEARBY"
                });
                layout.color = self.search_color();
            }
        }
        if let Some(message) = self.message {
            let text = message.short().as_bytes();
            if !message.routine() {
                layout.page_feedback.push(text);
            }
            // Page feedback never replaces the focused peer's state or identity.
            if matches!(Item::at(self.cursor), Item::Search) {
                layout.status = Label::new();
                layout.status.push(text);
                layout.color = if message.routine() {
                    self.search_color()
                } else {
                    AMBER
                };
            }
        }
        if let Some(notice) = self.notice {
            layout.page_feedback = Label::new();
            layout.page_feedback.push(notice.short().as_bytes());
        }
        if let View::Replace(identity) = self.view {
            layout.identity = self.identity_label(identity);
            let current = self.selected_identity(identity.device_type);
            for (index, peer) in [current, Some(identity)].into_iter().enumerate() {
                let Some(peer) = peer else { continue };
                layout.replacement_numbers[index].number(peer.device_number);
                if self.identity_label(peer).as_bytes().contains(&b'.') {
                    let transmission = &mut layout.replacement_transmissions[index];
                    transmission.push(b"T");
                    transmission.number(u16::from(peer.transmission_type));
                }
            }
        }
        self.layout = layout;
    }

    fn focus(&self) -> Option<Focus> {
        match Item::at(self.cursor) {
            Item::Search => None,
            Item::Selected(slot) => self.selected[slot].map(Focus::Selected),
            Item::Nearby(index) => self.discoveries[index].map(|discovery| Focus::Nearby {
                identity: discovery.identity,
                present: self.present[index],
            }),
        }
    }

    fn describe_focus(&self, layout: &mut Layout, focus: Focus) {
        let identity = match focus {
            Focus::Selected(selected) => selected.identity,
            Focus::Nearby { identity, .. } => identity,
        };
        layout.title = kind(identity.device_type);
        layout.identity = self.identity_label(identity);
        let show_identity = self.duplicate_kind(identity);
        match focus {
            Focus::Selected(selected) => {
                let status = selected.link_status();
                layout.color = status.color();
                layout.action = b"OPEN";
                layout.status.push(if show_identity {
                    status.prefix()
                } else if status == LinkStatus::Live {
                    b"LIVE DATA"
                } else {
                    selected.status_text()
                });
            }
            Focus::Nearby { identity, present } => {
                layout.color = MUTED;
                layout.action = if self.selected_identity(identity.device_type).is_some() {
                    b"REPLACE"
                } else {
                    b"CONNECT"
                };
                layout.status.push(match (show_identity, present) {
                    (true, true) => b"SENSOR / ",
                    (true, false) => b"GONE / ",
                    (false, false) => b"NOT SEEN",
                    (false, true) if identity.device_type == device_type::RADAR => {
                        b"VEHICLES BEHIND"
                    }
                    (false, true) => b"READY TO CONNECT",
                });
            }
        }
        if show_identity {
            layout.status.push(layout.identity.as_bytes());
        }
    }

    fn search_color(&self) -> u16 {
        if self.scanning { CYAN } else { MUTED }
    }

    /// Another known peer shares this device type, so the number must be shown.
    fn duplicate_kind(&self, identity: Identity) -> bool {
        self.known_peers()
            .any(|peer| peer.device_type == identity.device_type && peer != identity)
    }

    /// The device number, with the transmission type when another known peer
    /// of the same type shares that number.
    fn identity_label(&self, identity: Identity) -> Label {
        let transmission = self.known_peers().any(|other| {
            other != identity
                && other.device_type == identity.device_type
                && other.device_number == identity.device_number
        });
        let mut label = Label::new();
        label.number(identity.device_number);
        if transmission {
            label.push(b".");
            label.number(u16::from(identity.transmission_type));
        }
        label
    }

    fn known_peers(&self) -> impl Iterator<Item = Identity> {
        let discovered = self.discoveries.iter().flatten().map(|d| d.identity);
        discovered.chain(self.selected_peers().map(|s| s.identity))
    }
}
