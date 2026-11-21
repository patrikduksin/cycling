//! One best on BESTS: its caption, the value on the right, the day it was
//! set under the caption, and watts a kilo beside a power best. A best not
//! set yet shows `--` in `TERTIARY`; one set before the clock was known
//! shows no day.
use crate::compose::data::{Count, Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, GRADE, POWER, SPEED, TERTIARY};
use crate::screens::history::date;
use crate::screens::ride::summary::effort_label;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct Best;

pub const BESTS: u8 = 7;
const POWERS: u8 = 4;

/// Rows from the caption's baseline down to the day's, and from the value's.
const CAPTION_UNITS: usize = 14;
const VALUE_UNITS: usize = 3;
/// The value's column on the right, and how far left of the right edge
/// W/kg ends, clear of the widest power.
const VALUE_COLUMN_UNITS: usize = 100;
const PER_KG_UNITS: usize = 80;
const DASH_UNITS: usize = 60;
const LABEL_GAP_UNITS: usize = 4;
/// Watts a kilo in tenths shows up to 99.9, past any rider's best, and
/// over 20 minutes up to 9.9, past the world's best.
const PER_KG_END: i32 = 999;
const PER_KG_20_MIN_END: i32 = 99;
/// Jan 15 2026: twelve steps of 31 days from it land on each month once,
/// each on a two-digit day.
const MID_JANUARY: u16 = 20_468;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place(u8);

impl Place {
    pub const fn of(index: u8) -> Self {
        assert!(index < BESTS);
        Self(index)
    }

    pub const fn byte(self) -> u8 {
        self.0
    }

    fn label(self) -> Text<12> {
        let mut label = Text::new();
        let _ = label.push_str(match self.0 {
            4 => "LONGEST RIDE",
            5 => "LONGEST TIME",
            6 => "MOST ASCENT",
            power => return effort_label(usize::from(power)),
        });
        label
    }

    const fn color(self) -> u16 {
        match self.0 {
            0..POWERS => POWER,
            4 | 5 => SPEED,
            _ => GRADE,
        }
    }

    /// What the best reads as: its metric, and the unit after it.
    const fn metric(self) -> (Metric, &'static str) {
        match self.0 {
            0..POWERS => (Metric::Power, " W"),
            4 => (Metric::Distance, " KM"),
            5 => (Metric::Elapsed, ""),
            _ => (Metric::Ascent, " M"),
        }
    }

    /// The value as its metric shows it, up to the metric's end.
    fn value(self, value: Option<i32>, out: &mut Text<12>) {
        let (metric, unit) = self.metric();
        metric.write(value, out);
        let _ = out.push_str(unit);
    }

    /// The widest value the best can show in `face`.
    fn widest(self, face: &Face) -> usize {
        let (metric, unit) = self.metric();
        metric
            .extremes()
            .iter()
            .map(|value| {
                let mut text = Text::<12>::new();
                let _ = text.push_str(value.as_str());
                let _ = text.push_str(unit);
                face.fit_width(text.as_bytes())
            })
            .max()
            .unwrap_or(0)
    }

    const fn per_kg_end(self) -> i32 {
        match self.0 {
            3 => PER_KG_20_MIN_END,
            _ => PER_KG_END,
        }
    }

    fn per_kg(self, tenths: i32) -> Text<12> {
        let tenths = tenths.clamp(0, self.per_kg_end());
        let mut text = Text::new();
        let _ = write!(text, "{}.{} W/KG", tenths / 10, tenths % 10);
        text
    }

    /// The columns W/kg covers at its widest.
    fn per_kg_width(self, at: &Scaled) -> usize {
        let caption = at.face(FaceRole::Caption);
        caption.fit_width(self.per_kg(self.per_kg_end()).as_bytes())
    }

    /// The value's column: wide enough for its widest value.
    fn column(self, at: &Scaled) -> usize {
        at.unit(VALUE_COLUMN_UNITS)
            .max(self.widest(at.face(FaceRole::Body)))
    }
}

/// The widest day a best can be set on in `face`.
fn widest_day(face: &Face) -> usize {
    (0..12)
        .map(|month| face.fit_width(date(MID_JANUARY + 31 * month).as_bytes()))
        .max()
        .unwrap_or(0)
}

impl Arg for Place {
    fn decode(raw: u8) -> Option<Self> {
        (raw < BESTS).then_some(Self(raw))
    }

    fn encode(self) -> u8 {
        self.0
    }
}

impl Tile for Best {
    type Arg = Place;
    const NAME: &'static str = "best";

    /// The caption over the day on the left, the value on the right across
    /// both, and W/kg between them on the caption's line.
    fn forms(place: Place, at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let label = Form::text(caption, place.label().as_bytes());
        let gap = at.unit(LABEL_GAP_UNITS);
        let column = place.column(at);
        let value = place.widest(at.face(FaceRole::Body));
        let per_kg = if place.0 < POWERS {
            place.per_kg_width(at) + at.unit(PER_KG_UNITS)
        } else {
            0
        };
        let top = caption.ink_width(place.label().as_bytes()) + gap + value.max(per_kg);
        let bottom = widest_day(caption) + gap + column;
        forms(&[Form {
            width: top.max(bottom) as u16,
            height: label.height + at.unit(CAPTION_UNITS) as u16,
            shapes: 0,
            runs: if place.0 < POWERS { 4 } else { 3 },
        }])
    }

    fn prepare(place: Place, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let base = usize::from(rect.bottom);
        let caption_base = base.saturating_sub(at.unit(CAPTION_UNITS));
        let width = right.saturating_sub(left);
        out.caption(
            at,
            place.label().as_bytes(),
            width,
            left,
            caption_base,
            Align::Left,
        );
        let index = place.0;
        let value = data.count(Count::Best(index)).filter(|&value| value > 0);
        let body = at.face(FaceRole::Body);
        let value_base = base.saturating_sub(at.unit(VALUE_UNITS));
        let column = place.column(at).min(width);
        let Some(value) = value else {
            let dash = at.unit(DASH_UNITS).min(column);
            let end = right.saturating_sub(SPILL);
            out.fit(
                &[body],
                b"--",
                dash,
                end,
                value_base,
                Align::Right,
                TERTIARY,
            );
            return;
        };
        let mut text = Text::<12>::new();
        place.value(Some(value), &mut text);
        out.fit(
            &[body],
            text.as_bytes(),
            column,
            right.saturating_sub(SPILL),
            value_base,
            Align::Right,
            place.color(),
        );
        let per_kg_tenths = (index < POWERS).then(|| data.count(Count::BestPerKg(index)));
        if let Some(tenths) = per_kg_tenths.flatten() {
            let end = right.saturating_sub(at.unit(PER_KG_UNITS) + SPILL);
            out.caption(
                at,
                place.per_kg(tenths).as_bytes(),
                place.per_kg_width(at).min(end.saturating_sub(left)),
                end,
                caption_base,
                Align::Right,
            );
        }
        let day = data.count(Count::BestDay(index)).unwrap_or(0);
        if let Ok(day @ 1..) = u16::try_from(day) {
            let room = width.saturating_sub(column);
            out.caption(at, date(day).as_bytes(), room, left, base, Align::Left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::scaled::BASE;

    #[test]
    fn no_day_reads_wider_than_the_widest() {
        let face = BASE.face(FaceRole::Caption);
        let widest = widest_day(face);
        for day in 0..=u16::MAX {
            assert!(
                face.fit_width(date(day).as_bytes()) <= widest,
                "{}",
                date(day).as_str()
            );
        }
    }
}
