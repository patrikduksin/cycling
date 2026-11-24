use super::sensor_head::maker;
use crate::compose::data::Data;
use crate::compose::kit::facts::{Line, card_width, draw_card};
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{CalibrationNote, Connection, Menu, Part, Selected, reading, widest};
use crate::screens::theme::{LAYOUT, Tone};
use crate::screens::workout::Text;
use core::fmt::Write;

pub struct SensorFacts;

const DETAIL: [&str; 5] = ["Status", "Reading", "Battery", "ID", "Link"];
const FORGET: [&str; 2] = ["Sensor", "ID"];
const LINK: &str = "ANT+";
const NOT_GIVEN: &str = "--";

const fn labels(part: Part) -> &'static [&'static str] {
    match part {
        Part::Detail => &DETAIL,
        Part::Forget => &FORGET,
    }
}

const fn row(part: Part) -> u8 {
    match part {
        Part::Detail => LAYOUT.space.dense_row,
        Part::Forget => LAYOUT.space.row,
    }
}

fn values(part: Part, menu: &Menu, sensor: Selected) -> [(Text<24>, Tone); 5] {
    let mut values = [const { (Text::new(), Tone::Text) }; 5];
    let id = menu.identity_label(sensor.identity);
    match part {
        Part::Detail => {
            let (state, tone) = sensor.state();
            values[0] = match menu.calibration() {
                Some(note) => (text(note.text().as_str()), note.tone()),
                None => (text(state), tone),
            };
            if let Some(reading) = reading(sensor.identity.device_type) {
                let value = sensor.reading;
                let mut number = Text::<12>::new();
                reading.metric.write(value, &mut number);
                if value.is_some() {
                    let mut shown = text(number.as_str());
                    let _ = shown.push_str(reading.unit);
                    values[1] = (shown, reading.tone);
                } else {
                    values[1] = (text(NOT_GIVEN), Tone::Sub);
                }
            } else {
                values[1] = (text(NOT_GIVEN), Tone::Sub);
            }
            values[2] = match sensor.battery_word() {
                Some((word, tone)) => {
                    let mut shown = text(word);
                    if let Some(volts) = sensor.volts() {
                        let _ = write!(shown, " · {} V", volts.as_str());
                    }
                    (shown, tone)
                }
                None => (text(NOT_GIVEN), Tone::Sub),
            };
            values[3] = (text(id.as_str()), Tone::Text);
            values[4] = (text(LINK), Tone::Text);
        }
        Part::Forget => {
            values[0] = (text(maker(sensor)), Tone::Text);
            let mut shown = text(id.as_str());
            let _ = write!(shown, " · {LINK}");
            values[1] = (shown, Tone::Text);
        }
    }
    values
}

fn text(words: &str) -> Text<24> {
    let mut text = Text::new();
    let _ = text.push_str(words);
    text
}

fn widest_value(part: Part, at: &Scaled) -> usize {
    let face = at.face(LAYOUT.face.label);
    let id = widest::identity(face);
    match part {
        Part::Detail => {
            let notes = CalibrationNote::ALL.map(|note| note.text());
            let states = Connection::ALL.map(Connection::text);
            let words = widest::of(face, states.iter().map(|state| state.as_bytes()))
                .max(widest::of(face, notes.iter().map(|note| note.as_bytes())));
            let battery = face.fit_width(b"Critical \xc2\xb7 0.0 V");
            let reading = face.fit_width(b"9999 km/h");
            words.max(battery).max(reading).max(id)
        }
        Part::Forget => face
            .fit_width(b"ANT+ sensor")
            .max(id + face.width(b" \xc2\xb7 ANT+")),
    }
}

impl Tile for SensorFacts {
    type Arg = Part;
    const NAME: &'static str = "sensor facts";

    fn forms(part: Part, at: &Scaled) -> Forms {
        let note = at.face(LAYOUT.face.note);
        let labels = labels(part);
        let label = widest::of(note, labels.iter().map(|label| label.as_bytes()));
        let rows = labels.len();
        forms(&[Form {
            width: card_width(at, label, widest_value(part, at)) as u16,
            height: (rows * px(at, row(part))) as u16,
            shapes: rows as u8,
            runs: 2 * rows as u8,
        }])
    }

    fn prepare(part: Part, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let Some(sensor) = menu.shown() else {
            return;
        };
        let theme = data.theme();
        let values = values(part, menu, sensor);
        let lines = labels(part)
            .iter()
            .zip(&values)
            .map(|(label, (value, tone))| Line {
                label: label.as_bytes(),
                value: value.as_bytes(),
                color: theme.tone(*tone),
            });
        draw_card(out, at, theme, placed.rect.rect(), row(part), lines);
    }
}
