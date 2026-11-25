//! A phone-style card of facts, as iOS groups a settings list: one row a
//! fact, its label on the left and its value on the right, a rule between
//! rows.
use super::{px, specs};
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::phone::Shown;
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;

pub struct Facts;

#[derive(Debug)]
pub struct Fact {
    pub label: &'static str,
    pub value: Value,
}

#[derive(Clone, Copy, Debug)]
pub enum Value {
    Metric(Metric, &'static str),
    Phone,
}

const CONNECTED: [&str; 2] = ["Not connected", "Connected"];

impl Fact {
    pub const fn metric(&self) -> Option<Metric> {
        match self.value {
            Value::Metric(metric, _) => Some(metric),
            Value::Phone => None,
        }
    }

    fn widest(&self, face: &firmware_shell::rendering::font::Face) -> usize {
        match self.value {
            Value::Metric(metric, unit) => {
                let numbers = metric
                    .extremes()
                    .map(|number| face.fit_width(number.as_bytes()));
                numbers.into_iter().max().unwrap_or(0) + face.width(unit.as_bytes())
            }
            Value::Phone => CONNECTED
                .iter()
                .map(|words| face.fit_width(words.as_bytes()))
                .max()
                .unwrap_or(0),
        }
    }
}

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub rows: &'static [Fact],
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A card of facts. Ids: pause, end and done 64–127, sensors 128–191,
    /// phone 192–255.
    pub enum Card: Spec {
        Ride = 64 => Spec {
            name: "RIDE",
            rows: &[
                Fact { label: "Ride time", value: Value::Metric(Metric::Elapsed, "") },
                Fact { label: "Distance", value: Value::Metric(Metric::Distance, " km") },
                Fact { label: "NP", value: Value::Metric(Metric::NormalizedPower, " W") },
            ],
        },
        Phone = 192 => Spec {
            name: "PHONE",
            rows: &[Fact { label: "Status", value: Value::Phone }],
        },
    }
}

fn value(fact: &Fact, data: &dyn Data) -> Text<24> {
    let mut text = Text::new();
    match fact.value {
        Value::Metric(metric, unit) => {
            let mut number = Text::<12>::new();
            metric.write(data.metric(metric), &mut number);
            let _ = text.push_str(number.as_str());
            let _ = text.push_str(unit);
        }
        Value::Phone => {
            let connected = matches!(data.phone(), Some(Shown::Paired { connected: true }));
            let _ = text.push_str(CONNECTED[usize::from(connected)]);
        }
    }
    text
}

impl Tile for Facts {
    type Arg = Card;
    const NAME: &'static str = "facts";

    fn reads(card: Card) -> Option<Reads> {
        Some(Reads::Card(card))
    }

    fn forms(card: Card, at: &Scaled) -> Forms {
        let rows = card.spec().rows;
        let (note, label) = (at.face(LAYOUT.face.note), at.face(LAYOUT.face.label));
        let widest = rows
            .iter()
            .map(|fact| note.fit_width(fact.label.as_bytes()) + fact.widest(label))
            .max()
            .unwrap_or(0);
        forms(&[Form {
            width: card_width(at, 0, widest) as u16,
            height: (rows.len() * px(at, LAYOUT.space.row)) as u16,
            shapes: rows.len() as u8,
            runs: 2 * rows.len() as u8,
        }])
    }

    fn prepare(card: Card, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let theme = data.theme();
        let rows = card.spec().rows;
        let mut shown = [const { Text::<24>::new() }; MAX_ROWS];
        for (text, fact) in shown.iter_mut().zip(rows) {
            *text = value(fact, data);
        }
        let lines = rows.iter().zip(&shown).map(|(fact, text)| Line {
            label: fact.label.as_bytes(),
            value: text.as_bytes(),
            color: theme.text,
        });
        draw_card(out, at, theme, placed.rect.rect(), LAYOUT.space.row, lines);
    }
}

const MAX_ROWS: usize = 6;

pub struct Line<'t> {
    pub label: &'t [u8],
    pub value: &'t [u8],
    pub color: u16,
}

/// The columns a card needs for labels as wide as `labels` beside values
/// as wide as `values`, in their faces.
pub fn card_width(at: &Scaled, labels: usize, values: usize) -> usize {
    let space = &LAYOUT.space;
    2 * px(at, space.gutter) + px(at, space.gap) + labels + values
}

/// A card of `lines` over `rect`, each `row` units tall, a rule between
/// them: one shape a row, two runs.
pub fn draw_card<'t>(
    out: &mut Canvas,
    at: &Scaled,
    theme: &Theme,
    rect: Rect,
    row: u8,
    lines: impl Iterator<Item = Line<'t>>,
) {
    let space = &LAYOUT.space;
    out.shape(Shape::Panel {
        rect,
        radius: px(at, LAYOUT.radius.card),
        color: theme.raised,
    });
    let (note, label) = (at.face(LAYOUT.face.note), at.face(LAYOUT.face.label));
    let (left, right) = (
        rect.left + px(at, space.gutter),
        rect.right - px(at, space.gutter),
    );
    let row = px(at, row);
    for (at_row, line) in lines.enumerate() {
        let top = rect.top + at_row * row;
        if at_row > 0 {
            let thick = px(at, space.rule);
            out.shape(Shape::Bar {
                rect: Rect {
                    left,
                    top,
                    right: rect.right,
                    bottom: top + thick,
                },
                radius: 0,
                color: theme.rule,
            });
        }
        let middle = top + row / 2;
        let base = middle + label.cap as usize / 2;
        let run = out.fit(
            &[label],
            line.value,
            right - left,
            right,
            base,
            Align::Right,
            line.color,
        );
        let base = middle + note.cap as usize / 2;
        let room = run.left.saturating_sub(left + px(at, space.gap));
        out.fit(
            &[note],
            line.label,
            room,
            left,
            base,
            Align::Left,
            theme.sub,
        );
    }
}
