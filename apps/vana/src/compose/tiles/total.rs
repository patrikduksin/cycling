//! A ride total under its caption, as the arrival screen lists them.
use super::reading::{Shown, Side, line, line_width};
use crate::compose::data::Data;
use crate::compose::data::Metric;
use crate::compose::solve::Placed;
use crate::compose::tile::Arg;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct Total;

/// A setting pins one of the first four; the body face is the last resort.
const LADDER: [FaceRole; 5] = [
    FaceRole::Total60,
    FaceRole::Large,
    FaceRole::Value,
    FaceRole::Total30,
    FaceRole::Body,
];

const CAPTION: FaceRole = FaceRole::Total11;

/// Rows from the caption's baseline to the value's capitals.
const UNDER_CAPTION_UNITS: usize = 7;

/// A total's setting byte: the metric's id in the low four bits, the
/// largest look in the next two, and the side in the top two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Captioned(pub Shown);

impl Captioned {
    pub const fn byte(self) -> u8 {
        let side = match self.0.side {
            Side::Left => 0,
            Side::Centre => 1,
            Side::Right => 2,
        };
        self.0.metric.id() | self.0.largest << 4 | side << 6
    }
}

impl Arg for Captioned {
    fn decode(raw: u8) -> Option<Self> {
        let side = match raw >> 6 {
            0 => Side::Left,
            1 => Side::Centre,
            2 => Side::Right,
            _ => return None,
        };
        Some(Self(Shown {
            metric: Metric::from_id(raw & 0x0f)?,
            largest: raw >> 4 & 0x03,
            side,
        }))
    }

    fn encode(self) -> u8 {
        self.byte()
    }

    fn label(self) -> &'static str {
        self.0.metric.name()
    }
}

impl Tile for Total {
    type Arg = Captioned;
    const NAME: &'static str = "total";

    fn forms(Captioned(shown): Captioned, at: &Scaled) -> Forms {
        let metric = shown.metric;
        let caption = at.face(CAPTION);
        let over = caption.fit_height() as u16 + at.unit(UNDER_CAPTION_UNITS) as u16;
        let named = caption.fit_width(metric.caption().as_bytes()) as u16;
        LADDER[usize::from(shown.largest)..]
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let unit = metric.captioned_unit();
                Form {
                    width: line_width(metric, face, unit, at).max(named),
                    height: over + u16::from(face.cap),
                    shapes: 0,
                    runs: if unit.is_empty() { 2 } else { 3 },
                }
            })
            .collect()
    }

    fn prepare(
        Captioned(shown): Captioned,
        placed: &Placed,
        at: &Scaled,
        data: &dyn Data,
        out: &mut Canvas,
    ) {
        let metric = shown.metric;
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let caption = at.face(CAPTION);
        let (x, align) = match shown.side {
            Side::Left => (left, Align::Left),
            Side::Centre => ((left + right - SPILL) / 2, Align::Center),
            Side::Right => (right - SPILL, Align::Right),
        };
        out.fit(
            &[caption],
            metric.caption().as_bytes(),
            right - left,
            x,
            usize::from(rect.top) + usize::from(caption.fit_height() as u16),
            align,
            crate::screens::chrome::SECONDARY,
        );
        let role = LADDER[usize::from(shown.largest + placed.form)];
        line(
            shown,
            at.face(role),
            metric.captioned_unit(),
            (left, right),
            usize::from(rect.bottom),
            at,
            data,
            out,
        );
    }
}
