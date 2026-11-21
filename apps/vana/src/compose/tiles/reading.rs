//! A metric's value with its unit beside it, as the ride pages show a
//! number. Its looks run down a ladder of faces from the one its setting
//! pins, so a built-in page keeps the hierarchy its design gave it.
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, SECONDARY};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct Reading;

/// A reading shrinks down the ladder no further than the body face; the
/// caption face is a look of its own, which only a pin reaches.
const LADDER: [FaceRole; 5] = [
    FaceRole::Hero,
    FaceRole::Large,
    FaceRole::Value,
    FaceRole::Body,
    FaceRole::Caption,
];

/// The faces a reading pinned at `largest` may take, best first.
fn looks(largest: u8) -> &'static [FaceRole] {
    let largest = usize::from(largest);
    if largest == LADDER.len() - 1 {
        &LADDER[largest..]
    } else {
        &LADDER[largest..LADDER.len() - 1]
    }
}

/// Columns between a value's ink and its unit.
pub(super) const UNIT_GAP_UNITS: usize = 4;

/// What a reading or a total shows: the metric, the largest look it may
/// take, and where it sits across its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub metric: Metric,
    pub largest: u8,
    pub side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Centre,
    Right,
}

impl Shown {
    pub const fn left(metric: Metric, largest: u8) -> Self {
        Self {
            metric,
            largest,
            side: Side::Left,
        }
    }

    pub const fn on(self, side: Side) -> Self {
        Self { side, ..self }
    }
}

/// A reading's setting byte: the metric's id in the low four bits, the
/// largest look in the next three, and the top bit for the right side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inline(pub Shown);

impl Inline {
    pub const fn byte(self) -> u8 {
        let right = matches!(self.0.side, Side::Right) as u8;
        self.0.metric.id() | self.0.largest << 4 | right << 7
    }
}

impl Arg for Inline {
    fn decode(raw: u8) -> Option<Self> {
        let largest = raw >> 4 & 0x07;
        if usize::from(largest) >= LADDER.len() {
            return None;
        }
        Some(Self(Shown {
            metric: Metric::from_id(raw & 0x0f)?,
            largest,
            side: if raw >> 7 == 1 {
                Side::Right
            } else {
                Side::Left
            },
        }))
    }

    fn encode(self) -> u8 {
        self.byte()
    }

    fn label(self) -> &'static str {
        self.0.metric.name()
    }
}

/// How wide `metric` reads in `face` with its unit `unit` beside it.
pub(super) fn line_width(metric: Metric, face: &Face, unit: &str, at: &Scaled) -> u16 {
    let value = metric
        .extremes()
        .iter()
        .map(|text| face.fit_width(text.as_bytes()))
        .max()
        .unwrap_or_default();
    let unit = match unit {
        "" => 0,
        unit => at.unit(UNIT_GAP_UNITS) + at.face(FaceRole::Caption).fit_width(unit.as_bytes()),
    };
    (value + unit) as u16
}

/// `metric`'s value and `unit` on one baseline at `base`, the pair placed
/// across `left..right` by `side`. On the left and in the centre the unit
/// follows the value's ink, so a "--" keeps it as close as a number does; on
/// the right it holds the edge and the value ends a gap before it.
#[allow(clippy::too_many_arguments)]
pub(super) fn line(
    shown: Shown,
    face: &'static Face,
    unit: &str,
    (left, right): (usize, usize),
    base: usize,
    at: &Scaled,
    data: &dyn Data,
    out: &mut Canvas,
) {
    let metric = shown.metric;
    let mut text = Text::<12>::new();
    metric.write(data.metric(metric), &mut text);
    let caption = at.face(FaceRole::Caption);
    let gap = if unit.is_empty() {
        0
    } else {
        at.unit(UNIT_GAP_UNITS)
    };
    let start = match shown.side {
        Side::Left => left,
        Side::Centre => {
            let group = face.width(text.as_bytes()) + gap + caption.width(unit.as_bytes());
            left + (right - left).saturating_sub(group + SPILL) / 2
        }
        Side::Right => {
            // The unit holds the right edge, so a changing value never moves it.
            let mut value_right = right - SPILL;
            if !unit.is_empty() {
                let unit = out.fit(
                    &[caption],
                    unit.as_bytes(),
                    right - left,
                    value_right,
                    base,
                    Align::Right,
                    SECONDARY,
                );
                value_right = unit.left.saturating_sub(gap);
            }
            out.fit(
                &[face],
                text.as_bytes(),
                value_right + SPILL - left,
                value_right,
                base,
                Align::Right,
                metric.color(),
            );
            return;
        }
    };
    let value = out.fit(
        &[face],
        text.as_bytes(),
        right - start,
        start,
        base,
        Align::Left,
        metric.color(),
    );
    if !unit.is_empty() {
        let at_unit = value.left + face.ink_width(value.line.text()) + gap;
        out.fit(
            &[caption],
            unit.as_bytes(),
            right.saturating_sub(at_unit),
            at_unit,
            base,
            Align::Left,
            SECONDARY,
        );
    }
}

impl Tile for Reading {
    type Arg = Inline;
    const NAME: &'static str = "reading";

    fn reads(Inline(shown): Inline) -> Option<Reads> {
        Some(Reads::Metric(shown.metric))
    }

    fn forms(Inline(shown): Inline, at: &Scaled) -> Forms {
        let unit = shown.metric.unit();
        looks(shown.largest)
            .iter()
            .map(|&role| {
                let face = at.face(role);
                Form {
                    width: line_width(shown.metric, face, unit, at),
                    height: face.fit_height() as u16,
                    shapes: 0,
                    runs: if unit.is_empty() { 1 } else { 2 },
                }
            })
            .collect()
    }

    fn prepare(
        Inline(shown): Inline,
        placed: &Placed,
        at: &Scaled,
        data: &dyn Data,
        out: &mut Canvas,
    ) {
        let role = looks(shown.largest)[usize::from(placed.form)];
        let rect = placed.rect;
        line(
            shown,
            at.face(role),
            shown.metric.unit(),
            (usize::from(rect.left), usize::from(rect.right)),
            usize::from(rect.bottom),
            at,
            data,
            out,
        );
    }
}
