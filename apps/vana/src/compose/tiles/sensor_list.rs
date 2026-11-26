use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{List, Menu, Row, Selected, names, reading, widest};
use crate::screens::theme::{LAYOUT, Theme, Tone};
use crate::screens::workout::Text;
use core::fmt::Write;
use device_api::ant::Identity;
use firmware_shell::rendering::font::Face;

pub struct SensorList;

pub const SHOWN: usize = 5;

fn height(row: Row, at: &Scaled) -> usize {
    let space = &LAYOUT.space;
    px(at, if row.short() { space.row } else { space.item })
}

/// The rows `list` shows in `rect`, each with its index and its box.
pub fn placed(
    menu: &Menu,
    list: List,
    rect: Rect,
    at: &Scaled,
) -> heapless::Vec<(usize, Row, Rect), SHOWN> {
    let rect = Rect {
        top: rect.top + px(at, LAYOUT.space.rule),
        ..rect
    };
    let tall = |index: usize| menu.rows(list).nth(index).map_or(0, |row| height(row, at));
    let focus = menu.focus(list).unwrap_or(0);
    let mut first = focus;
    let mut used = tall(focus);
    while first > 0 && focus - first + 1 < SHOWN && used + tall(first - 1) <= rect.height() {
        first -= 1;
        used += tall(first);
    }
    let mut shown = heapless::Vec::new();
    let mut top = rect.top;
    for (index, row) in menu.rows(list).enumerate().skip(first) {
        let bottom = top + height(row, at);
        if bottom > rect.bottom
            || shown
                .push((
                    index,
                    row,
                    Rect {
                        top,
                        bottom,
                        ..rect
                    },
                ))
                .is_err()
        {
            break;
        }
        top = bottom;
    }
    shown
}

pub(super) fn rule_under(out: &mut Canvas, at: &Scaled, theme: &Theme, rect: Rect) {
    out.shape(Shape::Bar {
        rect: Rect {
            top: rect.bottom - px(at, LAYOUT.space.rule),
            ..rect
        },
        radius: 0,
        color: theme.rule,
    });
}

pub(super) fn icon_at(out: &mut Canvas, icon: &'static Icon, left: usize, rect: Rect, color: u16) {
    let top = rect.top + (rect.height() - usize::from(icon.size)) / 2;
    out.shape(Shape::Icon {
        icon: StaticIcon(icon),
        left,
        top,
        color,
    });
}

fn right_edge(rect: Rect, at: &Scaled) -> usize {
    rect.right - px(at, LAYOUT.space.gutter) - px(at, LAYOUT.space.ring)
}

fn baselines(rect: Rect, name: &Face, under: &Face, at: &Scaled) -> (usize, usize) {
    let gap = px(at, LAYOUT.space.gap);
    let block = usize::from(name.cap) + gap + usize::from(under.cap);
    let first = rect.top + (rect.height() - block) / 2 + usize::from(name.cap);
    (first, first + gap + usize::from(under.cap))
}

fn state_line(sensor: Selected, new: bool, fits: impl Fn(&[u8]) -> bool) -> (Text<32>, Tone) {
    let mut line = Text::new();
    if let Some((word, warning)) = sensor.battery_warning().filter(|_| sensor.live()) {
        let _ = write!(line, "Battery {word}");
        if !fits(line.as_bytes()) {
            line = Text::new();
            let _ = write!(line, "Batt. {word}");
        }
        return (line, warning);
    }
    let (state, tone) = sensor.state();
    let volts = sensor.volts().filter(|_| sensor.live());
    for (new, volts) in [(new, volts), (new, None), (false, None)] {
        line = Text::new();
        if new {
            let _ = line.push_str("New · ");
        }
        let _ = line.push_str(state);
        if let Some(volts) = volts {
            let _ = write!(line, " · {} V", volts.as_str());
        }
        if fits(line.as_bytes()) {
            break;
        }
    }
    (line, tone)
}

struct Draw<'d> {
    at: &'d Scaled,
    theme: &'static Theme,
    menu: &'d Menu,
}

impl Draw<'_> {
    fn sensor_icon(&self, out: &mut Canvas, kind: u8, rect: Rect) -> usize {
        let sensor = Sensor::of(kind);
        let icon = self.at.art.sensor(sensor, Size::Small);
        let left = rect.left + px(self.at, LAYOUT.space.gutter);
        icon_at(out, icon, left, rect, self.theme.tone(sensor.tone()));
        left + usize::from(icon.size) + px(self.at, LAYOUT.space.pad)
    }

    fn saved(&self, out: &mut Canvas, sensor: Selected, new: bool, rect: Rect) {
        let at = self.at;
        let face = &LAYOUT.face;
        let (label, note, tag, digits) = (
            self.at.face(face.label),
            self.at.face(face.note),
            self.at.face(face.tag),
            self.at.face(face.digits_small),
        );
        let text = self.sensor_icon(out, sensor.identity.device_type, rect);
        let right = right_edge(rect, at);
        let (name_base, under_base) = baselines(rect, label, note, at);
        let mut end = right;
        if let Some(reading) = reading(sensor.identity.device_type) {
            let mut value = Text::<12>::new();
            let shown = sensor.reading;
            reading.metric.write(shown, &mut value);
            let color = if shown.is_some() {
                self.theme.tone(reading.tone)
            } else {
                self.theme.sub
            };
            let base = rect.top + (rect.height() + usize::from(digits.cap)) / 2;
            let mut number_right = right;
            if shown.is_some() {
                let unit = out.fit(
                    &[tag],
                    reading.tag.as_bytes(),
                    right - text,
                    right,
                    base,
                    Align::Right,
                    self.theme.sub,
                );
                number_right = unit.left.saturating_sub(px(at, LAYOUT.space.tight));
            }
            let shown = out.fit(
                &[digits],
                value.as_bytes(),
                number_right - text,
                number_right,
                base,
                Align::Right,
                color,
            );
            end = shown.left.saturating_sub(px(at, LAYOUT.space.gap));
        }
        let name = self.menu.row_name(sensor);
        let room = end.saturating_sub(text);
        out.fit(
            &[label, note],
            name.as_bytes(),
            room,
            text,
            name_base,
            Align::Left,
            self.theme.text,
        );
        let (line, tone) = state_line(sensor, new, |line| note.fit_width(line) <= room);
        out.fit(
            &[note],
            line.as_bytes(),
            room,
            text,
            under_base,
            Align::Left,
            self.theme.tone(tone),
        );
    }

    fn add(&self, out: &mut Canvas, rect: Rect) {
        let at = self.at;
        let plus = at.art.plus;
        let left = rect.left + px(at, LAYOUT.space.gutter);
        let good = self.theme.good;
        icon_at(out, plus, left, rect, good);
        let text = left + usize::from(plus.size) + px(at, LAYOUT.space.pad);
        let label = self.at.face(LAYOUT.face.label);
        let base = rect.top + (rect.height() + usize::from(label.cap)) / 2;
        let room = rect.right.saturating_sub(text);
        out.fit(&[label], b"Add sensor", room, text, base, Align::Left, good);
    }

    fn added(&self, out: &mut Canvas, sensor: Selected, rect: Rect) {
        let at = self.at;
        let (label, note) = (
            self.at.face(LAYOUT.face.label),
            self.at.face(LAYOUT.face.note),
        );
        let text = self.sensor_icon(out, sensor.identity.device_type, rect);
        let mut right = right_edge(rect, at);
        let (state, tone) = sensor.state();
        if sensor.live() {
            let check = at.art.symbol(Symbol::CheckDisc);
            let left = right - usize::from(check.size);
            icon_at(out, check, left, rect, self.theme.good);
            right = left - px(at, LAYOUT.space.gap);
        }
        let middle = |face: &Face| rect.top + (rect.height() + usize::from(face.cap)) / 2;
        let shown = out.fit(
            &[note],
            state.as_bytes(),
            right - text,
            right,
            middle(note),
            Align::Right,
            self.theme.tone(tone),
        );
        let name = self.menu.row_name(sensor);
        let room = shown.left.saturating_sub(text + px(at, LAYOUT.space.gap));
        out.fit(
            &[label, note],
            name.as_bytes(),
            room,
            text,
            middle(label),
            Align::Left,
            self.theme.text,
        );
    }

    fn found(&self, out: &mut Canvas, identity: Identity, present: bool, bars: u8, rect: Rect) {
        let at = self.at;
        let (label, note) = (
            self.at.face(LAYOUT.face.label),
            self.at.face(LAYOUT.face.note),
        );
        let text = self.sensor_icon(out, identity.device_type, rect);
        let all = at.art.symbol(Symbol::Bars);
        let left = right_edge(rect, at) - usize::from(all.size);
        let lit = match bars {
            0 | 1 => Symbol::Bars1,
            2 => Symbol::Bars2,
            3 => Symbol::Bars3,
            _ => Symbol::Bars,
        };
        if lit != Symbol::Bars {
            icon_at(out, all, left, rect, self.theme.off);
        }
        icon_at(out, at.art.symbol(lit), left, rect, self.theme.text);
        let room = left.saturating_sub(text + px(at, LAYOUT.space.gap));
        let (name_base, under_base) = baselines(rect, label, note, at);
        let name = names(identity.device_type).short;
        out.fit(
            &[label, note],
            name.as_bytes(),
            room,
            text,
            name_base,
            Align::Left,
            self.theme.text,
        );
        let mut line = Text::<24>::new();
        if present {
            let id = self.menu.identity_label(identity);
            let _ = write!(line, "ID {} · ANT+", id.as_str());
        } else {
            let _ = line.push_str("Not seen");
        }
        out.fit(
            &[note, self.at.face(LAYOUT.face.tag)],
            line.as_bytes(),
            room,
            text,
            under_base,
            Align::Left,
            self.theme.sub,
        );
    }

    fn ring(&self, out: &mut Canvas, rect: Rect) {
        let (radius, ring) = (
            px(self.at, LAYOUT.radius.card),
            px(self.at, LAYOUT.space.ring),
        );
        out.shape(Shape::Panel {
            rect,
            radius,
            color: self.theme.focus,
        });
        out.shape(Shape::Panel {
            rect: rect.inset(ring),
            radius: radius - ring,
            color: self.theme.bg,
        });
    }
}

fn widest_saved(at: &Scaled) -> usize {
    use crate::sensors::ant::device_type;
    let face = &LAYOUT.face;
    let (label, tag, digits) = (
        at.face(face.label),
        at.face(face.tag),
        at.face(face.digits_small),
    );
    let kinds = [
        device_type::RADAR,
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::SPEED_CADENCE,
        device_type::CADENCE,
        device_type::SPEED,
        device_type::SHIFTING,
        device_type::FITNESS_EQUIPMENT,
        device_type::BIKE_LIGHTS,
        0,
    ];
    kinds
        .into_iter()
        .map(|kind| {
            let name = label.fit_width(names(kind).short.as_bytes());
            let value = reading(kind).map_or(0, |reading| {
                let extremes = reading.metric.extremes();
                let number = extremes.iter().map(|text| digits.width(text.as_bytes()));
                let unit = tag.fit_width(reading.tag.as_bytes());
                px(at, LAYOUT.space.gap)
                    + number.max().unwrap_or(0)
                    + px(at, LAYOUT.space.tight)
                    + unit
            });
            name + value
        })
        .max()
        .unwrap_or(0)
}

impl Tile for SensorList {
    type Arg = List;
    const NAME: &'static str = "sensor list";

    fn forms(list: List, at: &Scaled) -> Forms {
        let face = &LAYOUT.face;
        let space = &LAYOUT.space;
        let (label, note) = (at.face(face.label), at.face(face.note));
        let icon = usize::from(at.art.sensor(Sensor::Generic, Size::Small).size);
        let row = match list {
            List::Sensors => widest_saved(at),
            List::Nearby => {
                let names = crate::screens::sensors::NAMES.map(|names| names.short.as_bytes());
                let name = widest::of(label, names);
                let tag = at.face(face.tag);
                let line = note
                    .fit_width(b"ID 65535 \xc2\xb7 ANT+")
                    .max(tag.fit_width(b"ID  \xc2\xb7 ANT+") + widest::identity(tag));
                let bars = usize::from(at.art.symbol(Symbol::Bars).size);
                name.max(line) + px(at, space.gap) + bars
            }
        };
        let width = 2 * px(at, space.gutter) + px(at, space.pad) + px(at, space.ring) + icon + row;
        let (shapes, runs) = match list {
            List::Sensors => (2 * SHOWN + 2, 4 * SHOWN),
            List::Nearby => (4 * SHOWN + 2, 2 * SHOWN),
        };
        forms(&[Form {
            width: width as u16,
            height: (px(at, space.rule) + px(at, space.item)) as u16,
            shapes: shapes as u8,
            runs: runs as u8,
        }])
    }

    fn prepare(list: List, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        out.shape(Shape::Bar {
            rect: Rect {
                bottom: rect.top + px(at, LAYOUT.space.rule),
                ..rect
            },
            radius: 0,
            color: theme.rule,
        });
        let draw = Draw { at, theme, menu };
        let focus = menu.focus(list);
        for (index, row, area) in self::placed(menu, list, rect, at) {
            if focus == Some(index) {
                draw.ring(out, area);
            } else if row != Row::Add {
                rule_under(out, at, theme, area);
            }
            match row {
                Row::Saved { sensor, new } => draw.saved(out, sensor, new, area),
                Row::Add => draw.add(out, area),
                Row::Added(sensor) => draw.added(out, sensor, area),
                Row::Found {
                    identity,
                    present,
                    bars,
                } => draw.found(out, identity, present, bars, area),
            }
        }
    }
}
