//! A phone-style card of facts, as iOS groups a settings list: one row a
//! fact, its label on the left and its value on the right, a rule between
//! rows.
use super::{px, specs};
use crate::compose::data::Data;
use crate::compose::data::Metric;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;

pub struct Facts;

/// One row: a label in sentence case, and a metric with its unit after it.
#[derive(Debug)]
pub struct Fact {
    pub label: &'static str,
    pub metric: Metric,
    /// Written after the number, its space included: `" km"`.
    pub unit: &'static str,
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
    /// A card of facts. Ids: pause, end and done 64–127, sensors 128–191.
    pub enum Card: Spec {
        Ride = 64 => Spec {
            name: "RIDE",
            rows: &[
                Fact { label: "Ride time", metric: Metric::Elapsed, unit: "" },
                Fact { label: "Distance", metric: Metric::Distance, unit: " km" },
                Fact { label: "NP", metric: Metric::NormalizedPower, unit: " W" },
            ],
        },
    }
}

fn value(fact: &Fact, reading: Option<i32>) -> Text<24> {
    let mut number = Text::<12>::new();
    fact.metric.write(reading, &mut number);
    let mut text = Text::new();
    let _ = text.push_str(number.as_str());
    let _ = text.push_str(fact.unit);
    text
}

impl Tile for Facts {
    type Arg = Card;
    const NAME: &'static str = "facts";

    fn forms(card: Card, at: &Scaled) -> Forms {
        let rows = card.spec().rows;
        let (note, label) = (at.face(LAYOUT.face.note), at.face(LAYOUT.face.label));
        let widest = rows
            .iter()
            .map(|fact| {
                let values = fact.metric.extremes().map(|number| {
                    label.fit_width(number.as_bytes()) + label.width(fact.unit.as_bytes())
                });
                note.fit_width(fact.label.as_bytes()) + values.into_iter().max().unwrap_or(0)
            })
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
            *text = value(fact, data.metric(fact.metric));
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
