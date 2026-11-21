//! Power or heart rate: the value and its unit, the zone it is in, and the
//! zone gauge under them, whose lit run glides. With a workout's target it
//! keeps a band under the gauge for the step's range.
use super::reading::{Shown, line, line_width};
use crate::compose::data::{Data, Effort as Kind, Metric, OnGauge};
use crate::compose::solve::{Placed, Rect16};
use crate::compose::tile::{Arg, Form, Forms, Landed, Tile};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect, TERTIARY, WHITE, zone_color};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct Effort;

const LADDER: [FaceRole; 2] = [FaceRole::Large, FaceRole::Value];
const ZONE: FaceRole = FaceRole::Body;
/// Columns between the unit and the zone.
const ZONE_GAP_UNITS: usize = 8;
/// Rows from the value's baseline to the gauge.
const UNDER_VALUE_UNITS: usize = 8;
const GAUGE_UNITS: usize = 5;
const SEGMENT_GAP_UNITS: usize = 3;
/// The narrowest a segment may be.
const SEGMENT_UNITS: usize = 6;
/// Rows from the gauge to the target band, and the band's own.
const UNDER_GAUGE_UNITS: usize = 2;
const BAND_UNITS: usize = 2;

/// Which effort, and whether a workout's target band sits under its gauge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gauged {
    pub effort: Kind,
    pub target: bool,
}

impl Arg for Gauged {
    fn decode(raw: u8) -> Option<Self> {
        Some(Self {
            effort: Kind::from_id(raw & 1)?,
            target: match raw >> 1 {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }

    fn encode(self) -> u8 {
        self.effort.id() | u8::from(self.target) << 1
    }

    fn label(self) -> &'static str {
        self.effort.name()
    }
}

const fn metric(effort: Kind) -> Metric {
    match effort {
        Kind::Power => Metric::Power,
        Kind::Heart => Metric::Heart,
    }
}

/// The gauge's segments across `rect`: where each starts, and its width.
struct Segments {
    left: usize,
    pitch: usize,
    width: usize,
    count: usize,
}

impl Segments {
    fn across(rect: Rect16, effort: Kind, at: &Scaled) -> Self {
        let gap = at.unit(SEGMENT_GAP_UNITS);
        let count = usize::from(effort.zones());
        let pitch = (usize::from(rect.width()) + gap) / count;
        Self {
            left: usize::from(rect.left),
            pitch,
            width: pitch - gap,
            count,
        }
    }

    /// The column `on` falls at.
    fn x(&self, on: OnGauge) -> usize {
        let within = on.within.min(on.range) as usize;
        self.left
            + usize::from(on.zone.saturating_sub(1)) * self.pitch
            + self.width * within / on.range.max(1) as usize
    }
}

/// The value's baseline, and the gauge's rows.
fn rows(placed: &Placed, at: &Scaled) -> (usize, core::ops::Range<usize>) {
    let face = at.face(LADDER[usize::from(placed.form)]);
    let base = usize::from(placed.rect.top) + usize::from(face.fit_height() as u16);
    let top = base + at.unit(UNDER_VALUE_UNITS);
    (base, top..top + at.unit(GAUGE_UNITS))
}

impl Tile for Effort {
    type Arg = Gauged;
    const NAME: &'static str = "effort";

    fn forms(gauged: Gauged, at: &Scaled) -> Forms {
        let metric = metric(gauged.effort);
        let zones = usize::from(gauged.effort.zones());
        let gauge = zones * at.unit(SEGMENT_UNITS) + (zones - 1) * at.unit(SEGMENT_GAP_UNITS);
        let zone = at.unit(ZONE_GAP_UNITS) + at.face(ZONE).fit_width(b"Z7");
        let under = at.unit(UNDER_VALUE_UNITS)
            + at.unit(GAUGE_UNITS)
            + if gauged.target {
                at.unit(UNDER_GAUGE_UNITS) + at.unit(BAND_UNITS)
            } else {
                0
            };
        LADDER
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let line = usize::from(line_width(metric, face, metric.unit(), at)) + zone;
                Form {
                    width: line.max(gauge) as u16,
                    height: face.fit_height() as u16 + under as u16,
                    shapes: (zones + usize::from(gauged.target)) as u8,
                    runs: 3,
                }
            })
            .collect()
    }

    fn prepare(gauged: Gauged, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let effort = gauged.effort;
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let (base, gauge) = rows(placed, at);
        let zone = data.zone(effort);
        let mut name = Text::<12>::new();
        let _ = match zone {
            Some(zone) => write!(name, "Z{zone}"),
            None => name.write_str("Z-"),
        };
        let chip = out.fit(
            &[at.face(ZONE)],
            name.as_bytes(),
            right - left,
            right - SPILL,
            base,
            Align::Right,
            zone_color(zone.unwrap_or(0)),
        );
        let face = at.face(LADDER[usize::from(placed.form)]);
        let end = chip.left.saturating_sub(at.unit(ZONE_GAP_UNITS)).max(left);
        let shown = Shown::left(metric(effort), 0);
        line(
            shown,
            face,
            metric(effort).unit(),
            (left, end),
            base,
            at,
            data,
            out,
        );
        let segments = Segments::across(rect, effort, at);
        let lit_to = data.gauge(effort).map(|on| (on.zone, segments.x(on)));
        for segment in 0..segments.count {
            let from = segments.left + segment * segments.pitch;
            let split = match lit_to {
                Some((zone, _)) if segment + 1 < usize::from(zone) => from + segments.width,
                Some((zone, x)) if segment + 1 == usize::from(zone) => x,
                _ => from,
            };
            out.shape(Shape::Split {
                rect: Rect {
                    left: from,
                    top: gauge.start,
                    right: from + segments.width,
                    bottom: gauge.end,
                },
                radius: 2,
                split,
                colors: [zone_color(segment as u8 + 1), TERTIARY],
            });
        }
        if let Some([low, high]) = data.target(effort).filter(|_| gauged.target) {
            let from = segments.x(low);
            let to = segments.x(high).max(from + 3).min(right);
            let top = gauge.end + at.unit(UNDER_GAUGE_UNITS);
            out.shape(Shape::Bar {
                rect: Rect {
                    left: from,
                    top,
                    right: to,
                    bottom: top + at.unit(BAND_UNITS),
                },
                radius: 0,
                color: WHITE,
            });
        }
    }

    fn glide(gauged: Gauged, placed: &Placed, at: &Scaled, data: &dyn Data) -> Option<Landed> {
        let on = data.gauge(gauged.effort)?;
        let (_, gauge) = rows(placed, at);
        let segments = Segments::across(placed.rect, gauged.effort, at);
        Some(Landed(Rect16 {
            left: placed.rect.left,
            top: gauge.start as u16,
            right: segments.x(on) as u16,
            bottom: gauge.end as u16,
        }))
    }
}
