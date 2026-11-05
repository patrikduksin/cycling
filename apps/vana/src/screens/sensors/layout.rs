//! Each sensor page placed once per change, with the boxes taps land on,
//! and drawn from that onto a canvas of runs, shapes and cues for each
//! frame.
use super::kind;
use super::menu::{Mine, State, View};
use super::slot::{LinkStatus, Selected};
use crate::screens::canvas::{Canvas, Run, Shape, on_baseline};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BODY, CAPTION, CARD, CONTENT, GREEN, HEIGHT, Line, RAISED, RED, Rect,
    SECONDARY, TITLE, VALUE, WHITE,
};
use crate::screens::cue::{Corner, Cue, Icon as CueIcon};
use crate::screens::icons::{ARROW, Icon, PLUS, REFRESH, Sensor, Size};
use crate::screens::workout::Text;
use crate::sensors::ant::BatteryStatus;
use core::fmt::Write;
use device_api::ant::Identity;
use firmware_shell::rendering::font::{Face, SPILL};
use heapless::Vec;

/// The screen margin; rows and cards keep 2 px less.
const MARGIN: usize = 10;
const ROW_LEFT: usize = 8;
const ROW_RIGHT: usize = 232;
const RADIUS: usize = 12;
const RING: usize = 3;
pub(super) const TITLE_BASE: usize = 62;
/// SENSORS: a row per saved sensor, then ADD SENSOR.
pub(super) const MINE_TOP: usize = 72;
pub(super) const MINE_PITCH: usize = 50;
pub(super) const ROW_HEIGHT: usize = 48;
/// NEARBY: the search line under the title, then a card per sensor.
pub(super) const SEARCH_BASE: usize = 84;
pub(super) const NEARBY_TOP: usize = 94;
pub(super) const NEARBY_PITCH: usize = 52;
/// The list scrolls so the focused row ends above this line.
pub(super) const FOCUS_BOTTOM: usize = HEIGHT - 8;
/// Text in a row starts right of its icon.
const MINE_TEXT: usize = 56;
const NEARBY_TEXT: usize = 52;
const TEXT_RIGHT: usize = ROW_RIGHT - 10;
/// The plus at a nearby sensor's right, and the refresh mark on the
/// search line.
const PLUS_LEFT: usize = 204;
const REFRESH_LEFT: usize = 206;
/// Detail, FORGET SENSOR? and REPLACE SENSOR: a card holding the sensor's
/// icon, centred.
const ICON_CARD: usize = 56;
const ICON_CARD_RADIUS: usize = 18;
const MID: usize = crate::screens::chrome::WIDTH / 2;
pub(super) const DETAIL_CARD_TOP: usize = 22;
pub(super) const NAME_BASE: usize = 106;
pub(super) const STATUS_BASE: usize = 126;
/// BATTERY and ID: a caption over the value, in two columns.
pub(super) const CELL_BASE: usize = 158;
pub(super) const VALUE_BASE: usize = 192;
pub(super) const BATTERY_LEFT: usize = 34;
pub(super) const ID_LEFT: usize = 126;
pub(super) const CALIBRATION_BASE: usize = 233;
pub(super) const HINT_BASE: usize = 249;
pub(super) const FORGET_CARD_TOP: usize = 56;
pub(super) const FORGET_BASES: [usize; 2] = [158, 192];
pub(super) const FORGET_NAME_BASE: usize = 214;
pub(super) const REPLACE_CARD_TOP: usize = 30;

const BACK: Cue = Cue::new(Corner::TopLeft, CueIcon::Back);
/// A power meter's page: ACT zeroes it, MOVE forgets it.
pub(super) const POWER_CUES: [Cue; 3] = [
    BACK,
    Cue::new(Corner::BottomLeft, CueIcon::Trash),
    Cue::new(Corner::BottomRight, CueIcon::Calibrate),
];
/// Any other sensor's page, and FORGET SENSOR?: ACT forgets.
pub(super) const FORGET_CUES: [Cue; 2] = [BACK, Cue::new(Corner::BottomRight, CueIcon::Trash)];
pub(super) const REPLACE_CUES: [Cue; 2] = [BACK, Cue::new(Corner::BottomRight, CueIcon::Check)];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Target {
    Mine(Mine),
    /// A nearby sensor, by discovery slot.
    Nearby(usize),
    Refresh,
    /// A button cue: the tap acts as its button.
    Cue(Corner),
}

/// Where the shown page's rows and cues are, worked out after every change:
/// small enough to keep, unlike the canvas, which each frame draws afresh
/// from it.
pub(super) struct Layout {
    /// How far the list has scrolled up to keep its focus in view.
    scroll: usize,
    pub(super) cues: &'static [Cue],
    /// The shown rows and cues, with the boxes taps land on.
    targets: Vec<(Rect, Target), 12>,
}

impl Layout {
    pub(super) const fn new() -> Self {
        Self {
            scroll: 0,
            cues: &[],
            targets: Vec::new(),
        }
    }

    /// The target under a point; rows and cues own their bounds. Rows under
    /// the status bar take no taps.
    pub(super) fn target(&self, x: usize, y: usize) -> Option<Target> {
        if y < CONTENT.top && !super::full_screen(self.cues) {
            return None;
        }
        self.targets
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|&(_, target)| target)
    }

    /// Place the shown page: its scroll, rows and cues.
    pub(super) fn prepare(&mut self, state: &State) {
        self.targets.clear();
        self.scroll = 0;
        self.cues = &[];
        match state.view {
            View::Mine(focus) => {
                let index = state.rows().position(|row| row == focus).unwrap_or(0);
                self.scroll_to(MINE_TOP + index * MINE_PITCH);
                for (index, row) in state.rows().enumerate() {
                    self.row(MINE_TOP + index * MINE_PITCH, Target::Mine(row));
                }
            }
            View::Nearby(focus) => {
                let index = focus
                    .and_then(|focus| state.nearby().position(|index| index == focus))
                    .unwrap_or(0);
                self.scroll_to(NEARBY_TOP + index * NEARBY_PITCH);
                if let Some(base) = self.search_base() {
                    let bounds = Rect {
                        left: REFRESH_LEFT - 14,
                        top: base - 24,
                        right: crate::screens::chrome::WIDTH,
                        bottom: base + 10,
                    };
                    let _ = self.targets.push((bounds, Target::Refresh));
                }
                for (row, index) in state.nearby().enumerate() {
                    self.row(NEARBY_TOP + row * NEARBY_PITCH, Target::Nearby(index));
                }
            }
            View::Detail(slot) if state.calibratable(slot) => self.cues = &POWER_CUES,
            View::Detail(_) | View::Forget(_) => self.cues = &FORGET_CUES,
            View::Replace(_) => self.cues = &REPLACE_CUES,
        }
        for cue in self.cues {
            let _ = self
                .targets
                .push((cue_box(cue.corner), Target::Cue(cue.corner)));
        }
    }

    /// Scroll so the row at `top` of the whole list ends above
    /// `FOCUS_BOTTOM`.
    fn scroll_to(&mut self, top: usize) {
        self.scroll = (top + ROW_HEIGHT).saturating_sub(FOCUS_BOTTOM);
    }

    /// The row at `top` of the whole list, where it shows whole on the
    /// screen or runs under the status bar.
    fn row(&mut self, top: usize, target: Target) {
        let Some(top) = top.checked_sub(self.scroll) else {
            return;
        };
        if top + ROW_HEIGHT > HEIGHT {
            return;
        }
        let rect = Rect {
            left: ROW_LEFT,
            top,
            right: ROW_RIGHT,
            bottom: top + ROW_HEIGHT,
        };
        let _ = self.targets.push((rect, target));
    }

    /// The search line's baseline on NEARBY, until it scrolls under the
    /// status bar.
    fn search_base(&self) -> Option<usize> {
        SEARCH_BASE
            .checked_sub(self.scroll)
            .filter(|&base| base >= CONTENT.top + 12)
    }

    /// Draw the page as placed: the same state and layout always draw the
    /// same canvas.
    pub(super) fn draw(&self, state: &State, canvas: &mut Canvas) {
        let mut page = Drawing { state, canvas };
        match state.view {
            View::Mine(focus) => {
                let problem = state.notice.or(state.message.filter(|m| m.on_sensors()));
                let (title, color): (&[u8], u16) = match problem {
                    Some(problem) => (problem.short().as_bytes(), AMBER),
                    None => (b"SENSORS", WHITE),
                };
                page.title(title, color, self.scroll);
                for &(rect, target) in &self.targets {
                    if let Target::Mine(row) = target {
                        page.mine(row, rect, row == focus);
                    }
                }
            }
            View::Nearby(focus) => {
                page.title(b"NEARBY", WHITE, self.scroll);
                if let Some(base) = self.search_base() {
                    page.search_line(base);
                }
                for &(rect, target) in &self.targets {
                    if let Target::Nearby(index) = target {
                        page.nearby(index, rect, focus == Some(index));
                    }
                }
            }
            View::Detail(slot) => {
                if let Some(selected) = state.selected[slot] {
                    page.detail(selected, state.calibratable(slot));
                }
            }
            View::Forget(slot) => {
                if let Some(selected) = state.selected[slot] {
                    page.forget(selected);
                }
            }
            View::Replace(identity) => page.replace(identity),
        }
    }
}

struct Drawing<'a> {
    state: &'a State,
    canvas: &'a mut Canvas,
}

impl Drawing<'_> {
    fn mine(&mut self, row: Mine, rect: Rect, focused: bool) {
        let top = rect.top;
        match row {
            Mine::Saved(slot) => {
                let Some(selected) = self.state.selected[slot] else {
                    return;
                };
                if focused {
                    self.focus_card(rect);
                }
                let sensor = Sensor::of(selected.identity.device_type);
                self.icon(
                    sensor.icon(Size::Row),
                    ROW_LEFT + 10,
                    top + 11,
                    sensor.tint(),
                );
                let name = kind(selected.identity.device_type);
                let width = TEXT_RIGHT - MINE_TEXT;
                self.canvas.fit(
                    &[&BODY, &CAPTION],
                    name,
                    width,
                    MINE_TEXT,
                    top + 23,
                    Align::Left,
                    WHITE,
                );
                let (text, color) = link(selected);
                let warning = battery_warning(selected);
                let live = selected.link_status() == LinkStatus::Live;
                let base = top + 39;
                match warning {
                    Some(warning) if live && !fits(text, warning.0, width) => {
                        self.status(MINE_TEXT, base, width, warning, None);
                    }
                    Some(warning) if live => {
                        let state = (text, warning.1);
                        self.status(MINE_TEXT, base, width, state, Some(warning));
                    }
                    _ => self.status(MINE_TEXT, base, width, (text, color), warning),
                }
            }
            Mine::Add => {
                let card = Rect {
                    top: top + 4,
                    bottom: top + 44,
                    ..rect
                };
                if focused {
                    self.focus_card(card);
                } else {
                    self.panel(card, RADIUS, CARD);
                }
                self.icon(&PLUS, ROW_LEFT + 10, top + 15, GREEN);
                self.canvas.fit(
                    &[&BODY],
                    b"ADD SENSOR",
                    TEXT_RIGHT - 48,
                    48,
                    top + 31,
                    Align::Left,
                    WHITE,
                );
            }
        }
    }

    /// NEARBY's line under its title, and the refresh mark at its end.
    fn search_line(&mut self, base: usize) {
        let (line, color) = self.state.search_line();
        self.canvas.fit(
            &[&CAPTION],
            line.as_bytes(),
            REFRESH_LEFT - 8 - MARGIN,
            MARGIN,
            base,
            Align::Left,
            color,
        );
        let mark = if self.state.scanning { ACCENT } else { WHITE };
        self.icon(&REFRESH, REFRESH_LEFT, base - 14, mark);
    }

    /// A nearby sensor's card, by discovery slot.
    fn nearby(&mut self, index: usize, rect: Rect, focused: bool) {
        let state = self.state;
        let Some(discovery) = state.discoveries[index] else {
            return;
        };
        let top = rect.top;
        if focused {
            self.focus_card(rect);
        } else {
            self.panel(rect, RADIUS, CARD);
        }
        let identity = discovery.identity;
        let icon = Sensor::of(identity.device_type).icon(Size::Small);
        self.icon(icon, ROW_LEFT + 10, top + 13, SECONDARY);
        self.icon(&PLUS, PLUS_LEFT, top + 15, GREEN);
        let width = PLUS_LEFT - 8 - NEARBY_TEXT;
        let name = kind(identity.device_type);
        self.canvas.fit(
            &[&BODY, &CAPTION],
            name,
            width,
            NEARBY_TEXT,
            top + 23,
            Align::Left,
            WHITE,
        );
        let mut caption = Text::<24>::new();
        if state.present[index] {
            let _ = write!(caption, "ID {}", state.identity_label(identity).as_str());
        } else {
            let _ = caption.push_str("NOT SEEN");
        }
        let caption = caption.as_bytes();
        self.canvas
            .caption(caption, width, NEARBY_TEXT, top + 39, Align::Left);
    }

    fn detail(&mut self, selected: Selected, calibratable: bool) {
        let state = self.state;
        let identity = selected.identity;
        self.icon_card(identity, DETAIL_CARD_TOP);
        let width = 2 * (MID - MARGIN);
        let name = kind(identity.device_type);
        let faces: [&'static Face; 2] = [&BODY, &CAPTION];
        self.canvas
            .fit(&faces, name, width, MID, NAME_BASE, Align::Center, WHITE);
        let (status, color) = link(selected);
        self.canvas.fit(
            &[&CAPTION],
            status,
            width,
            MID,
            STATUS_BASE,
            Align::Center,
            color,
        );
        let caption = |canvas: &mut Canvas, text: &[u8], left: usize| {
            canvas.caption(
                text,
                ID_LEFT - BATTERY_LEFT - 8,
                left,
                CELL_BASE,
                Align::Left,
            );
        };
        caption(self.canvas, b"BATTERY", BATTERY_LEFT);
        caption(self.canvas, b"ID", ID_LEFT);
        self.battery_value(selected);
        let number = state.identity_label(identity);
        let id_width = crate::screens::chrome::WIDTH - MARGIN - ID_LEFT;
        let faces: [&'static Face; 2] = [&VALUE, &BODY];
        let number = number.as_bytes();
        self.canvas.fit(
            &faces,
            number,
            id_width,
            ID_LEFT,
            VALUE_BASE,
            Align::Left,
            WHITE,
        );
        if let Some(note) = state.calibration.filter(|_| calibratable) {
            let text = note.text();
            self.canvas.fit(
                &[&CAPTION],
                text.as_bytes(),
                width,
                MID,
                CALIBRATION_BASE,
                Align::Center,
                note.color(),
            );
            if let Some(hint) = note.hint() {
                let hint = hint.as_bytes();
                self.canvas
                    .caption(hint, width, MID, HINT_BASE, Align::Center);
            }
        }
    }

    /// The battery cell's value: the voltage when known, else the status
    /// word, in the colour of its urgency.
    fn battery_value(&mut self, selected: Selected) {
        let Some(battery) = selected.battery else {
            let line = Line::new(&VALUE, b"--");
            self.canvas.run(on_baseline(
                line,
                BATTERY_LEFT,
                VALUE_BASE,
                Align::Left,
                WHITE,
            ));
            return;
        };
        let color = match battery.status {
            BatteryStatus::Critical => RED,
            status if status.is_low() => AMBER,
            _ => WHITE,
        };
        let Some(volts) = selected.volts() else {
            let word = battery.status.text().as_bytes();
            let line = Line::new(&BODY, word);
            self.canvas.run(on_baseline(
                line,
                BATTERY_LEFT,
                VALUE_BASE,
                Align::Left,
                color,
            ));
            return;
        };
        let line = Line::new(&VALUE, volts.as_bytes());
        let unit_left = BATTERY_LEFT + VALUE.ink_width(volts.as_bytes()) + 6;
        self.canvas.run(on_baseline(
            line,
            BATTERY_LEFT,
            VALUE_BASE,
            Align::Left,
            color,
        ));
        let unit = Line::new(&CAPTION, b"V");
        self.canvas.run(on_baseline(
            unit,
            unit_left,
            VALUE_BASE,
            Align::Left,
            SECONDARY,
        ));
    }

    fn forget(&mut self, selected: Selected) {
        let state = self.state;
        let identity = selected.identity;
        self.icon_card(identity, FORGET_CARD_TOP);
        let width = 2 * (MID - MARGIN);
        for (word, base) in [&b"FORGET"[..], b"SENSOR?"].into_iter().zip(FORGET_BASES) {
            self.canvas.fit(
                &[&TITLE, &BODY],
                word,
                width,
                MID,
                base,
                Align::Center,
                WHITE,
            );
        }
        let mut name = Text::<24>::new();
        let _ = name.push_str(core::str::from_utf8(kind(identity.device_type)).unwrap_or(""));
        let _ = write!(name, " {}", state.identity_label(identity).as_str());
        let name = name.as_bytes();
        self.canvas
            .caption(name, width, MID, FORGET_NAME_BASE, Align::Center);
    }

    /// The saved sensor on the left, the one that would replace it on the
    /// right.
    fn replace(&mut self, identity: Identity) {
        let state = self.state;
        self.icon_card(identity, REPLACE_CARD_TOP);
        let width = 2 * (MID - MARGIN);
        let name = kind(identity.device_type);
        self.canvas.fit(
            &[&BODY, &CAPTION],
            name,
            width,
            MID,
            108,
            Align::Center,
            WHITE,
        );
        self.canvas
            .caption(b"REPLACE SENSOR", width, MID, 132, Align::Center);
        self.icon(&ARROW, MID - 14, 142, ACCENT);
        let current = state.selected_identity(identity.device_type);
        let sides = [
            (current, &b"CURRENT"[..], 60),
            (Some(identity), b"NEW", 180),
        ];
        for (peer, label, center) in sides {
            self.canvas.caption(label, 100, center, 200, Align::Center);
            let Some(peer) = peer else { continue };
            let mut number = Text::<6>::new();
            let _ = write!(number, "{}", peer.device_number);
            let faces: [&'static Face; 2] = [&VALUE, &BODY];
            let number = number.as_bytes();
            self.canvas
                .fit(&faces, number, 96, center, 167, Align::Center, WHITE);
            if state.shares_number(peer) {
                let mut transmission = Text::<4>::new();
                let _ = write!(transmission, "T{}", peer.transmission_type);
                let text = transmission.as_bytes();
                self.canvas
                    .fit(&[&CAPTION], text, 96, center, 184, Align::Center, WHITE);
            }
        }
    }

    /// The page title, in `TITLE` while it fits, scrolled with the page;
    /// left out once it has gone under the status bar.
    fn title(&mut self, text: &[u8], color: u16, scroll: usize) {
        let Some(base) = TITLE_BASE.checked_sub(scroll) else {
            return;
        };
        let line = Line::fit(&[&TITLE, &BODY], text, 2 * (MID - MARGIN));
        let face = line.face();
        if base < CONTENT.top + usize::from(face.cap) {
            return;
        }
        self.canvas
            .run(on_baseline(line, MARGIN, base, Align::Left, color));
    }

    /// A state line: `first`, then `second` after a dot where it fits whole.
    fn status(
        &mut self,
        left: usize,
        base: usize,
        width: usize,
        (text, color): (&[u8], u16),
        second: Option<(&[u8], u16)>,
    ) {
        let first: Run = self
            .canvas
            .fit(&[&CAPTION], text, width, left, base, Align::Left, color);
        let Some((text, color)) =
            second.filter(|&(second, _)| fits(first.line.text(), second, width))
        else {
            return;
        };
        let end = first.left + CAPTION.ink_width(first.line.text());
        self.canvas.shape(Shape::Disc {
            cx: 4 * (end + DOT_GAP) as i32 + 2,
            cy: 4 * base as i32 - 2 * i32::from(CAPTION.cap),
            radius: 6,
            color,
        });
        let line = Line::new(&CAPTION, text);
        self.canvas.run(on_baseline(
            line,
            end + 2 * DOT_GAP,
            base,
            Align::Left,
            color,
        ));
    }

    fn focus_card(&mut self, rect: Rect) {
        self.panel(rect, RADIUS, ACCENT);
        self.panel(rect.inset(RING), RADIUS - RING, RAISED);
    }

    fn icon_card(&mut self, identity: Identity, top: usize) {
        let rect = Rect {
            left: MID - ICON_CARD / 2,
            top,
            right: MID + ICON_CARD / 2,
            bottom: top + ICON_CARD,
        };
        self.panel(rect, ICON_CARD_RADIUS, CARD);
        let sensor = Sensor::of(identity.device_type);
        let icon = sensor.icon(Size::Large);
        let inset = (ICON_CARD - usize::from(icon.size)) / 2;
        self.icon(icon, rect.left + inset, top + inset, sensor.tint());
    }

    fn panel(&mut self, rect: Rect, radius: usize, color: u16) {
        self.canvas.shape(Shape::Panel {
            rect,
            radius,
            color,
        });
    }

    fn icon(&mut self, icon: &'static Icon, left: usize, top: usize, color: u16) {
        self.canvas.shape(Shape::Icon {
            icon,
            left,
            top,
            color,
        });
    }
}

/// Columns between a state and the dot after it, and the dot and what follows.
const DOT_GAP: usize = 5;

/// Both parts of a state line fit `width`.
fn fits(first: &[u8], second: &[u8], width: usize) -> bool {
    both(first, second) + SPILL <= width
}

const fn both(first: &[u8], second: &[u8]) -> usize {
    CAPTION.ink_width(first) + 2 * DOT_GAP + CAPTION.width(second)
}

/// A saved sensor's state, as its row and detail page name it.
fn link(selected: Selected) -> (&'static [u8], u16) {
    let status = selected.link_status();
    let text = if status == LinkStatus::Live {
        b"LIVE"
    } else {
        selected.status_text()
    };
    (text, status.color())
}

/// A saved sensor's battery on its row, only once it needs changing; the
/// sensor's page has the voltage.
fn battery_warning(selected: Selected) -> Option<(&'static [u8], u16)> {
    match selected.battery?.status {
        BatteryStatus::Critical => Some((b"BATTERY CRITICAL", RED)),
        BatteryStatus::Low => Some((b"BATTERY LOW", AMBER)),
        BatteryStatus::New | BatteryStatus::Good | BatteryStatus::Ok => None,
    }
}

/// The box a cue's quarter disc fills, which its taps own.
const fn cue_box(corner: Corner) -> Rect {
    use crate::screens::chrome::WIDTH;
    use crate::screens::cue::RADIUS;
    let (left, top) = match corner {
        Corner::TopLeft => (0, 0),
        Corner::BottomLeft => (0, HEIGHT - RADIUS),
        Corner::BottomRight => (WIDTH - RADIUS, HEIGHT - RADIUS),
    };
    Rect {
        left,
        top,
        right: left + RADIUS,
        bottom: top + RADIUS,
    }
}

impl State {
    /// NEARBY's line under its title: a problem in amber, the sensor just
    /// added in green, else how the search goes.
    pub(super) fn search_line(&self) -> (Text<24>, u16) {
        use super::MenuMessage;
        let routine = if self.scanning { ACCENT } else { SECONDARY };
        let mut line = Text::new();
        let (text, color) = match self.notice.or(self.message) {
            Some(message) if !message.routine() => (message.short(), AMBER),
            Some(MenuMessage::Added(device_type)) => {
                let name = core::str::from_utf8(kind(device_type)).unwrap_or("");
                let _ = write!(line, "{name} ADDED");
                return (line, GREEN);
            }
            Some(MenuMessage::ScanDone) if self.nearby().next().is_none() => {
                ("NO SENSORS FOUND", routine)
            }
            Some(message) => (message.short(), routine),
            None if self.scanning => (MenuMessage::Scanning.short(), routine),
            None => ("FIND NEARBY", routine),
        };
        let _ = line.push_str(text);
        (line, color)
    }

    /// The device number, with the transmission type when another known peer
    /// of the same type shares that number.
    pub(super) fn identity_label(&self, identity: Identity) -> Text<12> {
        let mut label = Text::new();
        let _ = write!(label, "{}", identity.device_number);
        if self.shares_number(identity) {
            let _ = write!(label, ".{}", identity.transmission_type);
        }
        label
    }

    /// Another known peer of this type has the same device number.
    fn shares_number(&self, identity: Identity) -> bool {
        let discovered = self.discoveries.iter().flatten().map(|d| d.identity);
        let saved = self.selected_peers().map(|s| s.identity);
        discovered.chain(saved).any(|other| {
            other != identity
                && other.device_type == identity.device_type
                && other.device_number == identity.device_number
        })
    }
}
