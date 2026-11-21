//! The ride's time in each power or heart rate zone: "PWR" or "HR", then a
//! bar with one segment per zone ridden in its colour, black between them,
//! and a dim track before any time.
use crate::compose::data::{Data, Effort, Shares};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect, TERTIARY, zone_color};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Zones;

const BAR_UNITS: usize = 8;
const BAR_GAP_UNITS: usize = 8;
/// The shortest bar still shows seven zones apart.
const LEAST_BAR_UNITS: usize = 64;
const SEGMENT_GAP_UNITS: usize = 2;

const fn caption(effort: Effort) -> &'static [u8] {
    match effort {
        Effort::Power => b"PWR",
        Effort::Heart => b"HR",
    }
}

/// The caption's room: the wider of the two, so both bars start together.
fn caption_width(at: &Scaled) -> usize {
    let face = at.face(FaceRole::Caption);
    face.fit_width(caption(Effort::Power))
        .max(face.fit_width(caption(Effort::Heart)))
}

fn height(at: &Scaled) -> usize {
    at.unit(BAR_UNITS)
        .max(at.face(FaceRole::Caption).fit_height())
}

impl Tile for Zones {
    type Arg = Effort;
    const NAME: &'static str = "zones";

    fn forms(effort: Effort, at: &Scaled) -> Forms {
        forms(&[Form {
            width: (caption_width(at) + at.unit(BAR_GAP_UNITS) + at.unit(LEAST_BAR_UNITS)) as u16,
            height: height(at) as u16,
            shapes: effort.zones(),
            runs: 1,
        }])
    }

    fn prepare(effort: Effort, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let face = at.face(FaceRole::Caption);
        let room = height(at);
        // The caption's capitals and the bar share a middle.
        let base = rect.top + face.fit_height().max((room + usize::from(face.cap)) / 2);
        let width = caption_width(at);
        out.caption(at, caption(effort), width, rect.left, base, Align::Left);
        let top = rect.top + (room - at.unit(BAR_UNITS)) / 2;
        let bar = Rect {
            left: rect.left + width + at.unit(BAR_GAP_UNITS),
            top,
            right: rect.right,
            bottom: top + at.unit(BAR_UNITS),
        };
        let zones = usize::from(effort.zones());
        match data.zone_time(effort) {
            Some(Shares(ends)) => segments(out, at, bar, &ends[..zones], Shares::WHOLE),
            None => segments(out, at, bar, &[], Shares::WHOLE),
        }
    }
}

/// Across `bar`, one segment per zone whose share `ends` (out of `whole`)
/// grew, in zone colours with black between them; without `ends`, a dim
/// track. The last zone with time ends the bar, with no gap after it.
pub fn segments(out: &mut Canvas, at: &Scaled, bar: Rect, ends: &[u16], whole: u16) {
    let radius = at.unit(2);
    if ends.is_empty() {
        out.shape(Shape::Bar {
            rect: bar,
            radius,
            color: TERTIARY,
        });
        return;
    }
    let width = bar.width();
    let end = |zone: usize| usize::from(ends[zone]) * width / usize::from(whole);
    let gap = at.unit(SEGMENT_GAP_UNITS);
    let grew = |zone: usize| end(zone) > zone.checked_sub(1).map_or(0, end);
    let last = (0..ends.len()).rposition(grew).unwrap_or(0);
    let mut start = 0;
    for zone in 0..ends.len() {
        let right = if zone == last {
            end(zone)
        } else {
            end(zone).saturating_sub(gap)
        };
        if right > start {
            out.shape(Shape::Bar {
                rect: Rect {
                    left: bar.left + start,
                    right: bar.left + right,
                    ..bar
                },
                radius,
                color: zone_color(zone as u8 + 1),
            });
        }
        start = start.max(end(zone));
    }
}
