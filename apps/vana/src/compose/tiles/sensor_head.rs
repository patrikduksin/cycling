//! The top of a sensor's page: its icon large on a raised disc ringed in
//! the colour of its state, over its live reading big in the metric's
//! colour. FORGET? shows a trash on a red disc over the question.
use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{NAMES, Part, Selected, names, reading};
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::SPILL;

pub struct SensorHead;

pub const UNKNOWN_MAKER: &str = "ANT+ sensor";
const FORGET_NOTE: &str = "It stops reconnecting";
const DISC: usize = 68;

fn question(sensor: Selected) -> Text<32> {
    let long = names(sensor.identity.device_type).long;
    let (first, rest) = long.split_at(1);
    let first = char::from(first.as_bytes()[0].to_ascii_lowercase());
    let mut text = Text::new();
    let _ = write!(text, "Forget {first}{rest}?");
    text
}

fn forget_baselines(at: &Scaled) -> (usize, usize, usize) {
    let space = &LAYOUT.space;
    let (heading, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
    let title = px(at, space.badge) + px(at, space.gap) + heading.fit_height();
    let subtitle = title + px(at, space.gap) + note.fit_height();
    let bottom = subtitle + usize::from(note.height - note.top - note.cap);
    (title, subtitle, bottom)
}

fn reading_base(at: &Scaled) -> usize {
    let digits = at.face(LAYOUT.face.digits_mid);
    at.unit(DISC) + px(at, LAYOUT.space.gutter) + digits.fit_height()
}

fn widest_reading(at: &Scaled) -> usize {
    use crate::sensors::ant::device_type;
    let (digits, unit) = (at.face(LAYOUT.face.digits_mid), at.face(LAYOUT.face.label));
    let kinds = [
        device_type::POWER,
        device_type::HEART_RATE,
        device_type::CADENCE,
        device_type::SPEED,
    ];
    kinds
        .into_iter()
        .filter_map(reading)
        .map(|reading| {
            let extremes = reading.metric.extremes();
            let number = extremes.iter().map(|text| digits.width(text.as_bytes()));
            let tight = px(at, LAYOUT.space.tight);
            number.max().unwrap_or(0) + tight + unit.fit_width(reading.tag.as_bytes())
        })
        .max()
        .unwrap_or(0)
}

fn disc(out: &mut Canvas, (cx, cy): (usize, usize), diameter: usize, color: u16) {
    out.shape(Shape::Disc {
        cx: (4 * cx) as i32,
        cy: (4 * cy) as i32,
        radius: (2 * diameter) as i32,
        color,
    });
}

fn icon_in(out: &mut Canvas, icon: &'static Icon, tile: Rect, color: u16) {
    let ink = icon.ink(0, 0);
    out.shape(Shape::Icon {
        icon: StaticIcon(icon),
        left: (tile.left + tile.right - ink.width()) / 2 - ink.left,
        top: (tile.top + tile.bottom - ink.height()) / 2 - ink.top,
        color,
    });
}

fn detail(out: &mut Canvas, at: &Scaled, theme: &Theme, rect: Rect, sensor: Selected) {
    let middle = (rect.left + rect.right) / 2;
    let diameter = at.unit(DISC);
    let ring = px(at, LAYOUT.space.ring);
    let centre = (middle, rect.top + diameter / 2);
    let (_, state) = sensor.state();
    disc(out, centre, diameter, theme.tone(state));
    disc(out, centre, diameter - 2 * ring, theme.raised);
    let kind = Sensor::of(sensor.identity.device_type);
    let tile = Rect {
        left: middle - diameter / 2,
        top: rect.top,
        right: middle - diameter / 2 + diameter,
        bottom: rect.top + diameter,
    };
    icon_in(
        out,
        at.art.sensor(kind, Size::Large),
        tile,
        theme.tone(kind.tone()),
    );
    let Some(reading) = reading(sensor.identity.device_type) else {
        return;
    };
    let (digits, unit) = (at.face(LAYOUT.face.digits_mid), at.face(LAYOUT.face.label));
    let base = rect.top + reading_base(at);
    let mut number = Text::<12>::new();
    reading.metric.write(sensor.reading, &mut number);
    if sensor.reading.is_none() {
        let left = middle.saturating_sub(digits.width(number.as_bytes()) / 2);
        out.fit(
            &[digits],
            number.as_bytes(),
            rect.width(),
            left,
            base,
            Align::Left,
            theme.sub,
        );
        return;
    }
    let tight = px(at, LAYOUT.space.tight);
    let group = digits.width(number.as_bytes()) + tight + unit.width(reading.tag.as_bytes());
    let left = middle.saturating_sub(group / 2);
    let shown = out.fit(
        &[digits],
        number.as_bytes(),
        rect.width(),
        left,
        base,
        Align::Left,
        theme.tone(reading.tone),
    );
    let after = shown.left + digits.width(number.as_bytes()) + tight;
    out.fit(
        &[unit],
        reading.tag.as_bytes(),
        rect.right.saturating_sub(after),
        after,
        base,
        Align::Left,
        theme.sub,
    );
}

fn forget(out: &mut Canvas, at: &Scaled, theme: &Theme, rect: Rect, sensor: Selected) {
    let centre = rect.left + rect.width().saturating_sub(SPILL) / 2;
    let middle = (rect.left + rect.right) / 2;
    let badge = px(at, LAYOUT.space.badge);
    let tile = Rect {
        left: middle - badge / 2,
        top: rect.top,
        right: middle - badge / 2 + badge,
        bottom: rect.top + badge,
    };
    disc(out, (middle, rect.top + badge / 2), badge, theme.bad);
    icon_in(out, at.art.symbol(Symbol::Trash), tile, theme.on_tone);
    let (heading, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
    let (title_base, subtitle_base, _) = forget_baselines(at);
    out.fit(
        &[heading, at.face(LAYOUT.face.label)],
        question(sensor).as_bytes(),
        rect.width(),
        centre,
        rect.top + title_base,
        Align::Center,
        theme.text,
    );
    out.fit(
        &[note],
        FORGET_NOTE.as_bytes(),
        rect.width(),
        centre,
        rect.top + subtitle_base,
        Align::Center,
        theme.sub,
    );
}

impl Tile for SensorHead {
    type Arg = Part;
    const NAME: &'static str = "sensor head";

    fn forms(part: Part, at: &Scaled) -> Forms {
        let gutters = 2 * px(at, LAYOUT.space.gutter);
        let (width, height, shapes) = match part {
            Part::Detail => {
                let width = at.unit(DISC).max(widest_reading(at));
                let descent = {
                    let digits = at.face(LAYOUT.face.digits_mid);
                    usize::from(digits.height - digits.top - digits.cap)
                };
                (width, reading_base(at) + descent, 3)
            }
            Part::Forget => {
                let label = at.face(LAYOUT.face.label);
                let longest = NAMES.iter().map(|names| label.width(names.long.as_bytes()));
                let title = label.fit_width(b"Forget ?") + longest.max().unwrap_or(0);
                let note = at.face(LAYOUT.face.note).fit_width(FORGET_NOTE.as_bytes());
                (title.max(note), forget_baselines(at).2, 2)
            }
        };
        forms(&[Form {
            width: (gutters + width) as u16,
            height: height as u16,
            shapes,
            runs: 2,
        }])
    }

    fn prepare(part: Part, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(sensor) = data.sensors().and_then(|menu| menu.shown()) else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        match part {
            Part::Detail => detail(out, at, theme, rect, sensor),
            Part::Forget => forget(out, at, theme, rect, sensor),
        }
    }
}
