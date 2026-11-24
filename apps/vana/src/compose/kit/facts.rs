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
use crate::screens::theme::LAYOUT;
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
        let space = &LAYOUT.space;
        forms(&[Form {
            width: (2 * px(at, space.gutter) + px(at, space.gap) + widest) as u16,
            height: (rows.len() * px(at, space.row)) as u16,
            shapes: rows.len() as u8,
            runs: 2 * rows.len() as u8,
        }])
    }

    fn prepare(card: Card, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: px(at, LAYOUT.radius.card),
            color: theme.raised,
        });
        let (note, label) = (at.face(LAYOUT.face.note), at.face(LAYOUT.face.label));
        let (left, right) = (rect.left + px(at, space.gutter), rect.right - px(at, space.gutter));
        let row = px(at, space.row);
        for (at_row, fact) in card.spec().rows.iter().enumerate() {
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
            let shown = value(fact, data.metric(fact.metric));
            let base = middle + label.cap as usize / 2;
            let run = out.fit(&[label], shown.as_bytes(), right - left, right, base, Align::Right, theme.text);
            let base = middle + note.cap as usize / 2;
            let room = run.left.saturating_sub(left + px(at, space.gap));
            out.fit(&[note], fact.label.as_bytes(), room, left, base, Align::Left, theme.sub);
        }
    }
}
