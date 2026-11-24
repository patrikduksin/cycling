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

pub(super) fn caps(text: &str) -> Text<24> {
    let mut out = Text::new();
    let mut buffer = [0; 4];
    for character in text.chars() {
        let _ = out.push_str(character.to_ascii_uppercase().encode_utf8(&mut buffer));
    }
    out
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

fn baselines(rect: Rect, name: &Face, under: &Face, at: &Scaled) -> (usize, usize) {
    let gap = px(at, LAYOUT.space.gap);
    let block = usize::from(name.cap) + gap + usize::from(under.cap);
    let first = rect.top + (rect.height() - block) / 2 + usize::from(name.cap);
    (first, first + gap + usize::from(under.cap))
}

fn state_line(sensor: Selected, new: bool) -> (Text<32>, Tone) {
    let mut line = Text::new();
    if let Some((word, warning)) = sensor.battery_warning().filter(|_| sensor.live()) {
        // Beside a live reading, LIVE goes without saying.
        let _ = write!(line, "Battery {word}");
        return (line, warning);
    }
    let (state, tone) = sensor.state();
    if new {
        let _ = line.push_str("New · ");
    }
    let _ = line.push_str(state);
    if let Some(volts) = sensor.volts().filter(|_| sensor.live()) {
        let _ = write!(line, " · {} V", volts.as_str());
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
        left + usize::from(icon.size) + px(self.at, LAYOUT.space.gutter)
    }

    fn saved(&self, out: &mut Canvas, sensor: Selected, new: bool, rect: Rect) {
        let at = self.at;
        let face = &LAYOUT.face;
        let (label, tag, digits) = (
            self.at.face(face.label),
            self.at.face(face.tag),
            self.at.face(face.digits_small),
        );
        let text = self.sensor_icon(out, sensor.identity.device_type, rect);
        let right = rect.right - px(at, LAYOUT.space.gutter);
        let (name_base, under_base) = baselines(rect, label, tag, at);
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
            let shown = out.fit(
                &[digits],
                value.as_bytes(),
                right - text,
                right,
                under_base,
                Align::Right,
                color,
            );
            out.fit(
                &[tag],
                reading.tag.as_bytes(),
                right - text,
                right,
                under_base - usize::from(digits.top + digits.cap) - px(at, LAYOUT.space.tight),
                Align::Right,
                self.theme.sub,
            );
            end = shown.left.saturating_sub(px(at, LAYOUT.space.gap));
        }
        let name = caps(self.menu.row_name(sensor).as_str());
        let room = end.saturating_sub(text);
        out.fit(
            &[label, tag],
            name.as_bytes(),
            room,
            text,
            name_base,
            Align::Left,
            self.theme.text,
        );
        let (line, tone) = state_line(sensor, new);
        out.fit(
            &[tag],
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
        let text = left + usize::from(plus.size) + px(at, LAYOUT.space.gutter);
        let label = self.at.face(LAYOUT.face.label);
        let base = rect.top + (rect.height() + usize::from(label.cap)) / 2;
        let room = rect.right.saturating_sub(text);
        out.fit(&[label], b"ADD SENSOR", room, text, base, Align::Left, good);
    }

    fn added(&self, out: &mut Canvas, sensor: Selected, rect: Rect) {
        let at = self.at;
        let (label, tag) = (
            self.at.face(LAYOUT.face.label),
            self.at.face(LAYOUT.face.tag),
        );
        let text = self.sensor_icon(out, sensor.identity.device_type, rect);
        let mut right = rect.right - px(at, LAYOUT.space.gutter);
        let (state, tone) = sensor.state();
        if sensor.live() {
            let check = at.art.symbol(Symbol::CheckDisc);
            let left = right - usize::from(check.size);
            icon_at(out, check, left, rect, self.theme.good);
            right = left - px(at, LAYOUT.space.gap);
        }
        let middle = |face: &Face| rect.top + (rect.height() + usize::from(face.cap)) / 2;
        let shown = out.fit(
            &[tag],
            state.as_bytes(),
            right - text,
            right,
            middle(tag),
            Align::Right,
            self.theme.tone(tone),
        );
        let name = caps(self.menu.row_name(sensor).as_str());
        let room = shown.left.saturating_sub(text + px(at, LAYOUT.space.gap));
        out.fit(
            &[label, tag],
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
        let (label, tag) = (
            self.at.face(LAYOUT.face.label),
            self.at.face(LAYOUT.face.tag),
        );
        let text = self.sensor_icon(out, identity.device_type, rect);
        let all = at.art.symbol(Symbol::Bars);
        let left = rect.right - px(at, LAYOUT.space.gutter) - usize::from(all.size);
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
        let (name_base, under_base) = baselines(rect, label, tag, at);
        let name = caps(names(identity.device_type).short);
        out.fit(
            &[label, tag],
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
            let _ = line.push_str("NOT SEEN");
        }
        out.fit(
            &[tag],
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

fn widest_reading(face: &Face) -> usize {
    use crate::sensors::ant::device_type;
    let kinds = [
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::CADENCE,
        device_type::SPEED,
    ];
    kinds
        .into_iter()
        .filter_map(reading)
        .flat_map(|reading| reading.metric.extremes())
        .map(|text| face.fit_width(text.as_bytes()))
        .max()
        .unwrap_or(0)
}

impl Tile for SensorList {
    type Arg = List;
    const NAME: &'static str = "sensor list";

    fn forms(list: List, at: &Scaled) -> Forms {
        let face = &LAYOUT.face;
        let space = &LAYOUT.space;
        let (label, tag) = (at.face(face.label), at.face(face.tag));
        let names = crate::screens::sensors::NAMES.map(|names| caps(names.short));
        let name = widest::of(label, names.iter().map(|name| name.as_bytes()));
        let icon = usize::from(at.art.sensor(Sensor::Generic, Size::Small).size);
        let right = match list {
            List::Sensors => widest_reading(at.face(face.digits_small)),
            List::Nearby => usize::from(at.art.symbol(Symbol::Bars).size),
        };
        let line = match list {
            List::Sensors => 0,
            List::Nearby => tag.fit_width(b"ID  \xc2\xb7 ANT+") + widest::identity(tag),
        };
        let width = 3 * px(at, space.gutter) + icon + name.max(line) + px(at, space.gap) + right;
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
