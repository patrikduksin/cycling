//! Power or heart rate as the climb page shows them under the grade, one
//! a column: the caption, then the value with its zone on a chip.
use crate::compose::data::{Data, Effort, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, BLACK, Rect, WHITE, zone_color};
use crate::screens::ride::whole;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct ClimbEffort;

const VALUE_UNITS: usize = 36;
const CHIP_GAP_UNITS: usize = 4;
const CHIP_UNITS: usize = 22;
/// Rows from the caption's baseline to the value's capitals.
const UNDER_CAPTION_UNITS: usize = 8;
/// How far the chip reaches above the value's capitals and below its
/// baseline.
const CHIP_ABOVE_UNITS: usize = 2;
const CHIP_BELOW_UNITS: usize = 3;

const fn caption(effort: Effort) -> &'static [u8] {
    match effort {
        Effort::Power => b"POWER 3S",
        Effort::Heart => b"HEART",
    }
}

const fn metric(effort: Effort) -> Metric {
    match effort {
        Effort::Power => Metric::Power,
        Effort::Heart => Metric::Heart,
    }
}

impl Tile for ClimbEffort {
    type Arg = Effort;
    const NAME: &'static str = "climb effort";

    /// Both efforts take one width, so two side by side split a row evenly.
    fn forms(_: Effort, at: &Scaled) -> Forms {
        let label = at.face(FaceRole::Caption);
        let value = at.face(FaceRole::Body);
        let row = at.unit(VALUE_UNITS + CHIP_GAP_UNITS + CHIP_UNITS);
        let captions =
            [Effort::Power, Effort::Heart].map(|effort| label.fit_width(caption(effort)));
        forms(&[Form {
            width: captions.into_iter().fold(row, usize::max) as u16,
            height: (label.fit_height()
                + at.unit(UNDER_CAPTION_UNITS)
                + value.cap as usize
                + at.unit(CHIP_BELOW_UNITS)) as u16,
            shapes: 1,
            runs: 3,
        }])
    }

    fn reads(effort: Effort) -> Option<Reads> {
        Some(Reads::Metric(metric(effort)))
    }

    fn prepare(effort: Effort, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let label = at.face(FaceRole::Caption);
        let face = at.face(FaceRole::Body);
        let caption_base = rect.top + label.fit_height();
        out.caption(
            at,
            caption(effort),
            rect.width(),
            rect.left,
            caption_base,
            Align::Left,
        );
        let base = caption_base + at.unit(UNDER_CAPTION_UNITS) + face.cap as usize;
        let chip_left = rect.left + at.unit(VALUE_UNITS + CHIP_GAP_UNITS);
        let chip = Rect {
            left: chip_left,
            top: base - face.cap as usize - at.unit(CHIP_ABOVE_UNITS),
            right: chip_left + at.unit(CHIP_UNITS),
            bottom: base + at.unit(CHIP_BELOW_UNITS),
        };
        let zone = data.zone(effort).unwrap_or(0);
        out.shape(Shape::Bar {
            rect: chip,
            radius: at.unit(4),
            color: zone_color(zone),
        });
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
        out.fit(
            &[label],
            name.as_bytes(),
            chip.width(),
            chip.right - chip.width() / 2,
            base,
            Align::Center,
            BLACK,
        );
        let mut text = Text::<12>::new();
        whole(&mut text, data.metric(metric(effort)));
        let right = chip.left - at.unit(CHIP_GAP_UNITS);
        out.fit(
            &[face, label],
            text.as_bytes(),
            at.unit(VALUE_UNITS),
            right,
            base,
            Align::Right,
            WHITE,
        );
    }
}
