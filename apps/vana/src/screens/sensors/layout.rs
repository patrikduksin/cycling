use super::kind;
use super::menu::{Mine, State, View};
use super::slot::{LinkStatus, Selected};
use crate::screens::Layout as Screens;
use crate::screens::canvas::{Canvas, Run, Shape, on_baseline};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, CARD, GREEN, Line, RAISED, RED, Rect, SECONDARY, WHITE,
};
use crate::screens::cue::{Corner, Cue, Icon as CueIcon};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::Text;
use crate::sensors::ant::BatteryStatus;
use core::fmt::Write;
use device_api::ant::Identity;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};
use heapless::Vec;

/// The sensor pages on one panel. SENSORS and NEARBY are lists under the
/// status bar, measured as they lie before they scroll; every other page
/// takes the whole panel, whose spare rows part it from the edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// Both lists' title, its capitals on its bottom.
    pub title: Rect,
    /// NEARBY's search line, its capitals on its bottom, and the refresh
    /// mark at its end with the box its taps own.
    pub search: Rect,
    pub refresh: Rect,
    pub refresh_tap: Rect,
    pub mine: Mines,
    pub nearby: Nearby,
    /// The list scrolls so the focused row ends above this line.
    pub focus_bottom: usize,
    pub detail: Detail,
    pub forget: Forget,
    pub replace: Replace,
    /// A card's corner, and the focus ring's width inside it.
    pub radius: usize,
    pub ring: usize,
    /// The corner of the big card holding a sensor's icon on its pages.
    pub icon_card_radius: usize,
    /// Columns between a state and the dot after it, and the dot's radius in
    /// quarter pixels.
    pub dot_gap: usize,
    pub dot: usize,
    /// Columns between a battery's voltage and its unit.
    pub unit_gap: usize,
}

/// A list's rows. The boxes inside a row are measured from its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rows {
    /// The first row's box; each next one lies `pitch` lower.
    pub first: Rect,
    pub pitch: usize,
    /// The sensor's icon, from the box's top left corner.
    pub icon: Rect,
    /// The name over its state, capitals on their bottoms.
    pub name: Rect,
    pub state: Rect,
}

/// NEARBY: a card per sensor found, with a plus at its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Nearby {
    pub rows: Rows,
    pub plus: Rect,
}

/// SENSORS: a row per saved sensor, then ADD SENSOR in a card of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mines {
    pub rows: Rows,
    pub add_card: Rect,
    pub add_plus: Rect,
    pub add_text: Rect,
}

/// A saved sensor's page: its icon card, name and state, then BATTERY and
/// ID over their values, then a power meter's calibration note and hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Detail {
    pub card: Rect,
    pub name: Rect,
    pub status: Rect,
    /// The two cells, captions on the top row and values below.
    pub battery: Rect,
    pub id: Rect,
    pub value: Rect,
    pub calibration: Rect,
    pub hint: Rect,
}

/// FORGET SENSOR?: its icon card, the question in two words, and the
/// sensor's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Forget {
    pub card: Rect,
    pub words: [Rect; 2],
    pub name: Rect,
}

/// REPLACE SENSOR: the new sensor's card and name, the question, an arrow,
/// and the saved sensor on the left beside the new one on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Replace {
    pub card: Rect,
    pub name: Rect,
    pub question: Rect,
    pub arrow: Rect,
    pub sides: [Rect; 2],
    /// Rows of each side: the device number, its transmission type and the
    /// side's label.
    pub number: Rect,
    pub transmission: Rect,
    pub label: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [_, title, search, _] = screen.rows([Unit(24), Unit(38), Unit(22), Fr(1)], Unit(0), at);
    let [_, line, _, mark, _] =
        screen.cols([Unit(10), Fr(1), Unit(8), Unit(18), Unit(16)], Unit(0), at);
    let [_, refresh, _] = screen.rows([Unit(70), Unit(18), Fr(1)], Unit(0), at);
    let [_, tap, _] = screen.rows([Unit(60), Unit(34), Fr(1)], Unit(0), at);
    let refresh_tap = Rect {
        left: mark.left - at.unit(14),
        right: screen.right,
        ..tap
    };
    Regions {
        title: title.with_columns(text),
        search: search.with_columns(line),
        refresh: refresh.with_columns(mark),
        refresh_tap,
        mine: mines(at),
        nearby: nearby(at),
        focus_bottom: screen.bottom - at.unit(8),
        detail: detail(at, text),
        forget: forget(at, text),
        replace: replace(at, text),
        radius: at.unit(12),
        ring: at.unit(3),
        icon_card_radius: at.unit(18),
        dot_gap: at.unit(5),
        dot: at.unit(6),
        unit_gap: at.unit(6),
    }
}

const fn first_row(at: &Scaled, above: u16) -> (Rect, Rect) {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, row, _] = screen.cols([Unit(8), Fr(1), Unit(8)], Unit(0), at);
    let [_, first, _] = screen.rows([Unit(above), Unit(48), Fr(1)], Unit(0), at);
    let first = first.with_columns(row);
    let top = Rect {
        top: 0,
        bottom: first.bottom - first.top,
        ..first
    };
    (first, top)
}

const fn mines(at: &Scaled) -> Mines {
    use Track::{Fr, Unit};
    let (first, row) = first_row(at, 72);
    let [_, _, icon, text, _, _] = at.screen().cols(
        [Unit(8), Unit(10), Unit(38), Fr(1), Unit(10), Unit(8)],
        Unit(0),
        at,
    );
    let [_, icon_rows, _] = row.rows([Unit(11), Unit(26), Fr(1)], Unit(0), at);
    let [name, state, _] = row.rows([Unit(23), Unit(16), Fr(1)], Unit(0), at);
    let [_, add_card, _] = row.rows([Unit(4), Unit(40), Fr(1)], Unit(0), at);
    let [_, plus, _] = row.rows([Unit(15), Unit(18), Fr(1)], Unit(0), at);
    let [add_base, _] = row.rows([Unit(31), Fr(1)], Unit(0), at);
    let [_, add_text, _] = at.screen().cols([Unit(48), Fr(1), Unit(18)], Unit(0), at);
    Mines {
        rows: Rows {
            first,
            pitch: at.unit(50),
            icon: icon_rows.with_columns(icon),
            name: name.with_columns(text),
            state: state.with_columns(text),
        },
        add_card,
        add_plus: plus.with_columns(icon),
        add_text: add_base.with_columns(add_text),
    }
}

const fn nearby(at: &Scaled) -> Nearby {
    use Track::{Fr, Unit};
    let (first, row) = first_row(at, 94);
    let [_, _, icon, text, _, plus, _, _] = at.screen().cols(
        [
            Unit(8),
            Unit(10),
            Unit(34),
            Fr(1),
            Unit(8),
            Unit(18),
            Unit(10),
            Unit(8),
        ],
        Unit(0),
        at,
    );
    let [_, icon_rows, _] = row.rows([Unit(13), Unit(22), Fr(1)], Unit(0), at);
    let [name, state, _] = row.rows([Unit(23), Unit(16), Fr(1)], Unit(0), at);
    let [_, plus_rows, _] = row.rows([Unit(15), Unit(18), Fr(1)], Unit(0), at);
    Nearby {
        rows: Rows {
            first,
            pitch: at.unit(52),
            icon: icon_rows.with_columns(icon),
            name: name.with_columns(text),
            state: state.with_columns(text),
        },
        plus: plus_rows.with_columns(plus),
    }
}

const fn icon_card(at: &Scaled, rows: Rect) -> Rect {
    use Track::{Fr, Unit};
    let [_, card, _] = at.screen().cols([Fr(1), Unit(56), Fr(1)], Unit(0), at);
    rows.with_columns(card)
}

const fn detail(at: &Scaled, text: Rect) -> Detail {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [
        _,
        _,
        card,
        name,
        status,
        captions,
        value,
        calibration,
        hint,
        _,
        _,
    ] = screen.rows(
        [
            Fr(1),
            Unit(22),
            Unit(56),
            Unit(28),
            Unit(20),
            Unit(32),
            Unit(34),
            Unit(41),
            Unit(16),
            Unit(71),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, _, battery, _, id, _, _] = screen.cols(
        [
            Fr(1),
            Unit(34),
            Unit(84),
            Unit(8),
            Unit(104),
            Unit(10),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    Detail {
        card: icon_card(at, card),
        name: name.with_columns(text),
        status: status.with_columns(text),
        battery: captions.with_columns(battery),
        id: captions.with_columns(id),
        value,
        calibration: calibration.with_columns(text),
        hint: hint.with_columns(text),
    }
}

const fn forget(at: &Scaled, text: Rect) -> Forget {
    use Track::{Fr, Unit};
    let [_, _, card, first, second, name, _, _] = at.screen().rows(
        [
            Fr(1),
            Unit(56),
            Unit(56),
            Unit(46),
            Unit(34),
            Unit(22),
            Unit(106),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    Forget {
        card: icon_card(at, card),
        words: [first.with_columns(text), second.with_columns(text)],
        name: name.with_columns(text),
    }
}

const fn replace(at: &Scaled, text: Rect) -> Replace {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, _, card, name, question, _, arrow, _, _] = screen.rows(
        [
            Fr(1),
            Unit(30),
            Unit(56),
            Unit(22),
            Unit(24),
            Unit(10),
            Unit(28),
            Unit(150),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, _, number, transmission, label, _, _] = screen.rows(
        [
            Fr(1),
            Unit(132),
            Unit(35),
            Unit(17),
            Unit(16),
            Unit(120),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, arrow_cols, _] = screen.cols([Fr(1), Unit(28), Fr(1)], Unit(0), at);
    let [_, _, current, _, new, _, _] = screen.cols(
        [
            Fr(1),
            Unit(10),
            Unit(100),
            Unit(20),
            Unit(100),
            Unit(10),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    Replace {
        card: icon_card(at, card),
        name: name.with_columns(text),
        question: question.with_columns(text),
        arrow: arrow.with_columns(arrow_cols),
        sides: [current, new],
        number,
        transmission,
        label,
    }
}

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
    pub(super) screens: &'static Screens,
    /// How far the list has scrolled up to keep its focus in view.
    scroll: usize,
    pub(super) cues: &'static [Cue],
    /// The shown rows and cues, with the boxes taps land on.
    targets: Vec<(Rect, Target), 12>,
}

impl Layout {
    pub(super) const fn new(screens: &'static Screens) -> Self {
        Self {
            screens,
            scroll: 0,
            cues: &[],
            targets: Vec::new(),
        }
    }

    const fn regions(&self) -> &'static Regions {
        &self.screens.sensors
    }

    /// The target under a point; rows and cues own their bounds. Rows under
    /// the status bar take no taps.
    pub(super) fn target(&self, x: usize, y: usize) -> Option<Target> {
        if y < self.screens.chrome.content.top && !super::full_screen(self.cues) {
            return None;
        }
        self.targets
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|&(_, target)| target)
    }

    /// Place the shown page: its scroll, rows and cues.
    pub(super) fn prepare(&mut self, state: &State) {
        let regions = self.regions();
        self.targets.clear();
        self.scroll = 0;
        self.cues = &[];
        match state.view {
            View::Mine(focus) => {
                let rows = &regions.mine.rows;
                let index = state.rows().position(|row| row == focus).unwrap_or(0);
                self.scroll_to(rows, index);
                for (index, row) in state.rows().enumerate() {
                    self.row(rows, index, Target::Mine(row));
                }
            }
            View::Nearby(focus) => {
                let rows = &regions.nearby.rows;
                let index = focus
                    .and_then(|focus| state.nearby().position(|index| index == focus))
                    .unwrap_or(0);
                self.scroll_to(rows, index);
                if let Some(shift) = self.search_scroll() {
                    let tap = regions.refresh_tap;
                    let bounds = Rect {
                        top: tap.top - shift,
                        bottom: tap.bottom - shift,
                        ..tap
                    };
                    let _ = self.targets.push((bounds, Target::Refresh));
                }
                for (row, index) in state.nearby().enumerate() {
                    self.row(rows, row, Target::Nearby(index));
                }
            }
            View::Detail(slot) if state.calibratable(slot) => self.cues = &POWER_CUES,
            View::Detail(_) | View::Forget(_) => self.cues = &FORGET_CUES,
            View::Replace(_) => self.cues = &REPLACE_CUES,
        }
        let panel = self.screens.scaled.panel;
        for cue in self.cues {
            let _ = self
                .targets
                .push((cue_box(cue.corner, panel), Target::Cue(cue.corner)));
        }
    }

    fn scroll_to(&mut self, rows: &Rows, index: usize) {
        let bottom = rows.first.bottom + index * rows.pitch;
        self.scroll = bottom.saturating_sub(self.regions().focus_bottom);
    }

    fn row(&mut self, rows: &Rows, index: usize, target: Target) {
        let Some(top) = (rows.first.top + index * rows.pitch).checked_sub(self.scroll) else {
            return;
        };
        let bottom = top + rows.first.height();
        if bottom > self.screens.scaled.panel.height {
            return;
        }
        let rect = Rect {
            top,
            bottom,
            ..rows.first
        };
        let _ = self.targets.push((rect, target));
    }

    fn search_scroll(&self) -> Option<usize> {
        let regions = self.regions();
        let at = &self.screens.scaled;
        regions
            .search
            .bottom
            .checked_sub(self.scroll)
            .filter(|&base| base >= self.screens.chrome.content.top + at.unit(12))
            .map(|_| self.scroll)
    }

    /// Draw the page as placed: the same state and layout always draw the
    /// same canvas.
    pub(super) fn draw(&self, state: &State, canvas: &mut Canvas) {
        let mut page = Drawing {
            state,
            canvas,
            at: &self.screens.scaled,
            regions: self.regions(),
            content: self.screens.chrome.content,
        };
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
                if let Some(shift) = self.search_scroll() {
                    page.search_line(shift);
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

const fn in_row(inner: Rect, top: usize) -> Rect {
    Rect {
        top: top + inner.top,
        bottom: top + inner.bottom,
        ..inner
    }
}

struct Drawing<'a> {
    state: &'a State,
    canvas: &'a mut Canvas,
    at: &'static Scaled,
    regions: &'static Regions,
    content: Rect,
}

impl Drawing<'_> {
    fn face(&self, role: FaceRole) -> &'static Face {
        self.at.face(role)
    }

    fn mine(&mut self, row: Mine, rect: Rect, focused: bool) {
        let regions = self.regions;
        let rows = &regions.mine.rows;
        let top = rect.top;
        let (body, caption) = (self.face(FaceRole::Body), self.face(FaceRole::Caption));
        match row {
            Mine::Saved(slot) => {
                let Some(selected) = self.state.selected[slot] else {
                    return;
                };
                if focused {
                    self.focus_card(rect);
                }
                let sensor = Sensor::of(selected.identity.device_type);
                let icon = in_row(rows.icon, top);
                let art = self.at.art.sensor(sensor, Size::Row);
                self.icon(art, icon.left, icon.top, sensor.tint());
                let name = in_row(rows.name, top);
                let width = name.width();
                self.canvas.fit(
                    &[body, caption],
                    kind(selected.identity.device_type),
                    width,
                    name.left,
                    name.bottom,
                    Align::Left,
                    WHITE,
                );
                let (text, color) = link(selected);
                let warning = battery_warning(selected);
                let live = selected.link_status() == LinkStatus::Live;
                let state = in_row(rows.state, top);
                match warning {
                    Some(warning) if live && !self.fits(text, warning.0, width) => {
                        self.status(state, warning, None);
                    }
                    Some(warning) if live => {
                        self.status(state, (text, warning.1), Some(warning));
                    }
                    _ => self.status(state, (text, color), warning),
                }
            }
            Mine::Add => {
                let card = in_row(regions.mine.add_card, top);
                if focused {
                    self.focus_card(card);
                } else {
                    self.panel(card, regions.radius, CARD);
                }
                let plus = in_row(regions.mine.add_plus, top);
                self.icon(self.at.art.plus, plus.left, plus.top, GREEN);
                let text = in_row(regions.mine.add_text, top);
                self.canvas.fit(
                    &[body],
                    b"ADD SENSOR",
                    text.width(),
                    text.left,
                    text.bottom,
                    Align::Left,
                    WHITE,
                );
            }
        }
    }

    fn search_line(&mut self, shift: usize) {
        let regions = self.regions;
        let (line, color) = self.state.search_line();
        let search = regions.search;
        let caption = self.face(FaceRole::Caption);
        self.canvas.fit(
            &[caption],
            line.as_bytes(),
            search.width(),
            search.left,
            search.bottom - shift,
            Align::Left,
            color,
        );
        let mark = if self.state.scanning { ACCENT } else { WHITE };
        let refresh = regions.refresh;
        self.icon(self.at.art.refresh, refresh.left, refresh.top - shift, mark);
    }

    /// A nearby sensor's card, by discovery slot.
    fn nearby(&mut self, index: usize, rect: Rect, focused: bool) {
        let state = self.state;
        let rows = &self.regions.nearby.rows;
        let Some(discovery) = state.discoveries[index] else {
            return;
        };
        let top = rect.top;
        if focused {
            self.focus_card(rect);
        } else {
            self.panel(rect, self.regions.radius, CARD);
        }
        let identity = discovery.identity;
        let icon = in_row(rows.icon, top);
        let art = self
            .at
            .art
            .sensor(Sensor::of(identity.device_type), Size::Small);
        self.icon(art, icon.left, icon.top, SECONDARY);
        let plus = in_row(self.regions.nearby.plus, top);
        self.icon(self.at.art.plus, plus.left, plus.top, GREEN);
        let name = in_row(rows.name, top);
        let faces = [self.face(FaceRole::Body), self.face(FaceRole::Caption)];
        self.canvas.fit(
            &faces,
            kind(identity.device_type),
            name.width(),
            name.left,
            name.bottom,
            Align::Left,
            WHITE,
        );
        let mut caption = Text::<24>::new();
        if state.present[index] {
            let _ = write!(caption, "ID {}", state.identity_label(identity).as_str());
        } else {
            let _ = caption.push_str("NOT SEEN");
        }
        let line = in_row(rows.state, top);
        self.canvas.caption(
            self.at,
            caption.as_bytes(),
            line.width(),
            line.left,
            line.bottom,
            Align::Left,
        );
    }

    fn detail(&mut self, selected: Selected, calibratable: bool) {
        let state = self.state;
        let at = self.at;
        let regions = &self.regions.detail;
        let identity = selected.identity;
        self.icon_card(identity, regions.card);
        let (body, caption) = (self.face(FaceRole::Body), self.face(FaceRole::Caption));
        let centred =
            |canvas: &mut Canvas, faces: &[&'static Face], text: &[u8], row: Rect, color| {
                canvas.fit(
                    faces,
                    text,
                    row.width(),
                    row.center_x(),
                    row.bottom,
                    Align::Center,
                    color,
                );
            };
        centred(
            self.canvas,
            &[body, caption],
            kind(identity.device_type),
            regions.name,
            WHITE,
        );
        let (status, color) = link(selected);
        centred(self.canvas, &[caption], status, regions.status, color);
        for (text, cell) in [(&b"BATTERY"[..], regions.battery), (b"ID", regions.id)] {
            let width = regions.battery.width();
            self.canvas
                .caption(at, text, width, cell.left, cell.bottom, Align::Left);
        }
        self.battery_value(selected);
        let number = state.identity_label(identity);
        let value = [self.face(FaceRole::Value), body];
        let id = regions.id;
        self.canvas.fit(
            &value,
            number.as_bytes(),
            id.width(),
            id.left,
            regions.value.bottom,
            Align::Left,
            WHITE,
        );
        if let Some(note) = state.calibration.filter(|_| calibratable) {
            let text = note.text();
            let row = regions.calibration;
            centred(self.canvas, &[caption], text.as_bytes(), row, note.color());
            if let Some(hint) = note.hint() {
                let row = regions.hint;
                self.canvas.caption(
                    at,
                    hint.as_bytes(),
                    row.width(),
                    row.center_x(),
                    row.bottom,
                    Align::Center,
                );
            }
        }
    }

    /// The battery cell's value: the voltage when known, else the status
    /// word, in the colour of its urgency.
    fn battery_value(&mut self, selected: Selected) {
        let detail = &self.regions.detail;
        let (left, base) = (detail.battery.left, detail.value.bottom);
        let value = self.face(FaceRole::Value);
        let Some(battery) = selected.battery else {
            let line = Line::new(value, b"--");
            self.canvas
                .run(on_baseline(line, left, base, Align::Left, WHITE));
            return;
        };
        let color = match battery.status {
            BatteryStatus::Critical => RED,
            status if status.is_low() => AMBER,
            _ => WHITE,
        };
        let Some(volts) = selected.volts() else {
            let word = battery.status.text().as_bytes();
            let line = Line::new(self.face(FaceRole::Body), word);
            self.canvas
                .run(on_baseline(line, left, base, Align::Left, color));
            return;
        };
        let line = Line::new(value, volts.as_bytes());
        let unit_left = left + value.ink_width(volts.as_bytes()) + self.regions.unit_gap;
        self.canvas
            .run(on_baseline(line, left, base, Align::Left, color));
        let unit = Line::new(self.face(FaceRole::Caption), b"V");
        self.canvas
            .run(on_baseline(unit, unit_left, base, Align::Left, SECONDARY));
    }

    fn forget(&mut self, selected: Selected) {
        let state = self.state;
        let regions = &self.regions.forget;
        let identity = selected.identity;
        self.icon_card(identity, regions.card);
        let faces = [self.face(FaceRole::Title), self.face(FaceRole::Body)];
        for (word, row) in [&b"FORGET"[..], b"SENSOR?"].into_iter().zip(regions.words) {
            self.canvas.fit(
                &faces,
                word,
                row.width(),
                row.center_x(),
                row.bottom,
                Align::Center,
                WHITE,
            );
        }
        let mut name = Text::<24>::new();
        let _ = name.push_str(core::str::from_utf8(kind(identity.device_type)).unwrap_or(""));
        let _ = write!(name, " {}", state.identity_label(identity).as_str());
        let row = regions.name;
        self.canvas.caption(
            self.at,
            name.as_bytes(),
            row.width(),
            row.center_x(),
            row.bottom,
            Align::Center,
        );
    }

    /// The saved sensor on the left, the one that would replace it on the
    /// right.
    fn replace(&mut self, identity: Identity) {
        let state = self.state;
        let at = self.at;
        let regions = &self.regions.replace;
        self.icon_card(identity, regions.card);
        let (body, caption) = (self.face(FaceRole::Body), self.face(FaceRole::Caption));
        let name = regions.name;
        self.canvas.fit(
            &[body, caption],
            kind(identity.device_type),
            name.width(),
            name.center_x(),
            name.bottom,
            Align::Center,
            WHITE,
        );
        let question = regions.question;
        self.canvas.caption(
            at,
            b"REPLACE SENSOR",
            question.width(),
            question.center_x(),
            question.bottom,
            Align::Center,
        );
        let arrow = regions.arrow;
        self.icon(at.art.arrow, arrow.left, arrow.top, ACCENT);
        let current = state.selected_identity(identity.device_type);
        let sides = [(current, &b"CURRENT"[..]), (Some(identity), b"NEW")];
        for ((peer, label), side) in sides.into_iter().zip(regions.sides) {
            let center = side.center_x();
            let base = regions.label.bottom;
            self.canvas
                .caption(at, label, side.width(), center, base, Align::Center);
            let Some(peer) = peer else { continue };
            let mut number = Text::<6>::new();
            let _ = write!(number, "{}", peer.device_number);
            let width = side.width() - at.unit(4);
            let faces = [self.face(FaceRole::Value), body];
            let base = regions.number.bottom;
            self.canvas.fit(
                &faces,
                number.as_bytes(),
                width,
                center,
                base,
                Align::Center,
                WHITE,
            );
            if state.shares_number(peer) {
                let mut transmission = Text::<4>::new();
                let _ = write!(transmission, "T{}", peer.transmission_type);
                let base = regions.transmission.bottom;
                self.canvas.fit(
                    &[caption],
                    transmission.as_bytes(),
                    width,
                    center,
                    base,
                    Align::Center,
                    WHITE,
                );
            }
        }
    }

    fn title(&mut self, text: &[u8], color: u16, scroll: usize) {
        let title = self.regions.title;
        let Some(base) = title.bottom.checked_sub(scroll) else {
            return;
        };
        let faces = [self.face(FaceRole::Title), self.face(FaceRole::Body)];
        let line = Line::fit(&faces, text, title.width());
        let face = line.face();
        if base < self.content.top + usize::from(face.cap) {
            return;
        }
        self.canvas
            .run(on_baseline(line, title.left, base, Align::Left, color));
    }

    fn status(&mut self, row: Rect, (text, color): (&[u8], u16), second: Option<(&[u8], u16)>) {
        let caption = self.face(FaceRole::Caption);
        let (left, base, width) = (row.left, row.bottom, row.width());
        let first: Run = self
            .canvas
            .fit(&[caption], text, width, left, base, Align::Left, color);
        let Some((text, color)) =
            second.filter(|&(second, _)| self.fits(first.line.text(), second, width))
        else {
            return;
        };
        let gap = self.regions.dot_gap;
        let end = first.left + caption.ink_width(first.line.text());
        self.canvas.shape(Shape::Disc {
            cx: 4 * (end + gap) as i32 + 2,
            cy: 4 * base as i32 - 2 * i32::from(caption.cap),
            radius: self.regions.dot as i32,
            color,
        });
        let line = Line::new(caption, text);
        self.canvas
            .run(on_baseline(line, end + 2 * gap, base, Align::Left, color));
    }

    fn fits(&self, first: &[u8], second: &[u8], width: usize) -> bool {
        let caption = self.face(FaceRole::Caption);
        let both = caption.ink_width(first) + 2 * self.regions.dot_gap + caption.width(second);
        both + SPILL <= width
    }

    fn focus_card(&mut self, rect: Rect) {
        let (radius, ring) = (self.regions.radius, self.regions.ring);
        self.panel(rect, radius, ACCENT);
        self.panel(rect.inset(ring), radius - ring, RAISED);
    }

    fn icon_card(&mut self, identity: Identity, card: Rect) {
        self.panel(card, self.regions.icon_card_radius, CARD);
        let sensor = Sensor::of(identity.device_type);
        let icon = self.at.art.sensor(sensor, Size::Large);
        let inset = (card.width() - usize::from(icon.size)) / 2;
        self.icon(icon, card.left + inset, card.top + inset, sensor.tint());
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
            icon: StaticIcon(icon),
            left,
            top,
            color,
        });
    }
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

const fn cue_box(corner: Corner, panel: crate::screens::scaled::Panel) -> Rect {
    use crate::screens::cue::RADIUS;
    let (right, bottom) = (panel.width, panel.height);
    let (left, top) = match corner {
        Corner::TopLeft => (0, 0),
        Corner::BottomLeft => (0, bottom - RADIUS),
        Corner::BottomRight => (right - RADIUS, bottom - RADIUS),
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
