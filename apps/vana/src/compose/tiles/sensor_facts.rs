//! Under a sensor's reading, two chips: its battery as a glyph, when it
//! reported one, and a dot and the word for its state, or how zeroing
//! went; then its number and link in one small line. FORGET? keeps a
//! card of what it forgets.
use super::sensor_head::UNKNOWN_MAKER;
use crate::compose::data::Data;
use crate::compose::kit::facts::{Line, card_width, draw_card};
use crate::compose::kit::px;
use crate::compose::kit::top_line::{BATTERY_SHAPES, battery_glyph, battery_glyph_width};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{CalibrationNote, Connection, Menu, Part, Selected, widest};
use crate::screens::theme::{LAYOUT, Theme, Tone};
use crate::screens::workout::Text;
use crate::sensors::ant::BatteryStatus;
use core::fmt::Write;
use firmware_shell::rendering::font::SPILL;

pub struct SensorFacts;

const FORGET: [&str; 2] = ["Sensor", "ID"];
const LINK: &str = "ANT+";
const CHIP: usize = 30;
const DOT: usize = 8;

const fn fill(status: BatteryStatus) -> u8 {
    match status {
        BatteryStatus::New => 100,
        BatteryStatus::Good => 75,
        BatteryStatus::Ok => 50,
        BatteryStatus::Low => 25,
        BatteryStatus::Critical => 10,
    }
}

fn text(words: &str) -> Text<24> {
    let mut text = Text::new();
    let _ = text.push_str(words);
    text
}

fn id_line(menu: &Menu, sensor: Selected) -> Text<24> {
    let mut line = Text::new();
    let id = menu.identity_label(sensor.identity);
    let _ = write!(line, "ID {} · {LINK}", id.as_str());
    line
}

fn state(menu: &Menu, sensor: Selected) -> (Text<24>, Tone) {
    match menu.calibration() {
        Some(note) => (text(note.text().as_str()), note.tone()),
        None => {
            let (words, tone) = sensor.state();
            (text(words), tone)
        }
    }
}

fn chip(out: &mut Canvas, theme: &Theme, rect: Rect) {
    out.shape(Shape::Panel {
        rect,
        radius: rect.height() / 2,
        color: theme.raised,
    });
}

fn widest_state(at: &Scaled) -> usize {
    let note = at.face(LAYOUT.face.note);
    let notes = CalibrationNote::ALL.map(|note| note.text());
    let states = Connection::ALL.map(Connection::text);
    widest::of(note, states.iter().map(|state| state.as_bytes()))
        .max(widest::of(note, notes.iter().map(|note| note.as_bytes())))
}

fn around(at: &Scaled, inner: usize) -> usize {
    2 * px(at, LAYOUT.space.pad) + inner
}

fn state_inner(at: &Scaled, words: usize) -> usize {
    at.unit(DOT) + px(at, LAYOUT.space.gap) + words
}

fn detail(out: &mut Canvas, at: &Scaled, theme: &Theme, rect: Rect, menu: &Menu, sensor: Selected) {
    let space = &LAYOUT.space;
    let note = at.face(LAYOUT.face.note);
    let (words, tone) = state(menu, sensor);
    let color = theme.tone(tone);
    let battery = sensor.battery_word().zip(sensor.battery);
    let state_width = around(at, state_inner(at, note.width(words.as_bytes())));
    let battery_width = around(at, battery_glyph_width(at));
    let group = state_width + battery.map_or(0, |_| battery_width + px(at, space.gap));
    let middle = (rect.left + rect.right) / 2;
    let mut left = middle.saturating_sub(group / 2);
    let height = at.unit(CHIP);
    let chip_rect = |left: usize, width: usize| Rect {
        left,
        top: rect.top,
        right: left + width,
        bottom: rect.top + height,
    };
    let centre = rect.top + height / 2;
    if let Some(((_, tone), battery)) = battery {
        let area = chip_rect(left, battery_width);
        chip(out, theme, area);
        let right = area.right - px(at, space.pad);
        let fill = (fill(battery.status), theme.tone(tone));
        battery_glyph(out, at, theme, (right, centre), fill, theme.raised);
        left = area.right + px(at, space.gap);
    }
    let area = chip_rect(left, state_width);
    chip(out, theme, area);
    let dot = at.unit(DOT);
    let x = area.left + px(at, space.pad);
    out.shape(Shape::Disc {
        cx: (4 * x + 2 * dot) as i32,
        cy: (4 * centre) as i32,
        radius: (2 * dot) as i32,
        color,
    });
    let words_at = x + dot + px(at, space.gap);
    out.fit(
        &[note],
        words.as_bytes(),
        area.right.saturating_sub(words_at),
        words_at,
        centre + usize::from(note.cap) / 2,
        Align::Left,
        color,
    );
    let line = id_line(menu, sensor);
    out.fit(
        &[note],
        line.as_bytes(),
        rect.width(),
        rect.left + rect.width().saturating_sub(SPILL) / 2,
        rect.top + height + px(at, space.gutter) + note.fit_height(),
        Align::Center,
        theme.sub,
    );
}

fn forget(out: &mut Canvas, at: &Scaled, theme: &Theme, rect: Rect, menu: &Menu, sensor: Selected) {
    let maker = sensor.product.maker().unwrap_or(UNKNOWN_MAKER);
    let mut id = text(menu.identity_label(sensor.identity).as_str());
    let _ = write!(id, " · {LINK}");
    let values = [text(maker), id];
    let lines = FORGET.iter().zip(&values).map(|(label, value)| Line {
        label: label.as_bytes(),
        value: value.as_bytes(),
        color: theme.text,
    });
    draw_card(out, at, theme, rect, LAYOUT.space.row, lines);
}

impl Tile for SensorFacts {
    type Arg = Part;
    const NAME: &'static str = "sensor facts";

    fn forms(part: Part, at: &Scaled) -> Forms {
        let note = at.face(LAYOUT.face.note);
        let form = match part {
            Part::Detail => {
                let space = &LAYOUT.space;
                let chips = around(at, battery_glyph_width(at))
                    + px(at, space.gap)
                    + around(at, state_inner(at, widest_state(at)));
                let line = note.fit_width(b"ID  \xc2\xb7 ANT+") + widest::identity(note);
                let descent = usize::from(note.height - note.top - note.cap);
                Form {
                    width: (2 * px(at, space.gutter) + chips.max(line)) as u16,
                    height: (at.unit(CHIP) + px(at, space.gutter) + note.fit_height() + descent)
                        as u16,
                    shapes: 2 + BATTERY_SHAPES + 1,
                    runs: 2,
                }
            }
            Part::Forget => {
                let label = widest::of(note, FORGET.iter().map(|label| label.as_bytes()));
                let face = at.face(LAYOUT.face.label);
                let id = widest::identity(face) + face.width(b" \xc2\xb7 ANT+");
                let value = face.fit_width(UNKNOWN_MAKER.as_bytes()).max(id);
                Form {
                    width: card_width(at, label, value) as u16,
                    height: (FORGET.len() * px(at, LAYOUT.space.row)) as u16,
                    shapes: FORGET.len() as u8,
                    runs: 2 * FORGET.len() as u8,
                }
            }
        };
        forms(&[form])
    }

    fn prepare(part: Part, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let Some(sensor) = menu.shown() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        match part {
            Part::Detail => detail(out, at, theme, rect, menu, sensor),
            Part::Forget => forget(out, at, theme, rect, menu, sensor),
        }
    }
}
