use super::Point;
use crate::ride::homing::distance;

const ARROW_SHAFT_PX: f32 = 26.0;
const ARROW_BEFORE_TURN_PX: f32 = 10.0;
const ARROW_HEAD_LENGTH_PX: f32 = 10.5;
const ARROW_HEAD_HALF_WIDTH_PX: f32 = 7.9;
pub(super) const ARROW_OUTLINE_PX: f32 = 1.75;
pub(super) const ARROW_SHAFT_HALF_PX: f32 = 4.25;
pub(super) const ARROW_STROKES: usize = 24;

pub(super) struct ArrowShape {
    pub(super) shaft: heapless::Vec<Point, { ARROW_STROKES + 1 }>,
    pub(super) head: [Point; 3],
}

pub(super) fn arrow_shape(
    route: &impl Fn(&mut dyn FnMut(Point, bool)),
    turn: Point,
) -> Option<ArrowShape> {
    let (mut walked, mut last, mut base, mut rider) = (0.0, None::<Point>, None, None);
    route(&mut |p, before| {
        if base.is_some() {
            return;
        }
        rider.get_or_insert(p);
        if let Some(last) = last {
            if !before {
                base = Some(walked + distance(last, turn));
                return;
            }
            walked += distance(last, p);
        }
        last = Some(p);
    });
    let (rider, base) = rider.zip(base)?;
    let start = (base - ARROW_BEFORE_TURN_PX.min(distance(rider, turn) / 4.0)).max(0.0);
    let end = start + ARROW_SHAFT_PX;
    let mut shaft: heapless::Vec<Point, { ARROW_STROKES + 1 }> = heapless::Vec::new();
    let (mut walked, mut last, mut done) = (0.0, None::<Point>, false);
    route(&mut |p, _| {
        if done {
            return;
        }
        let Some(from) = last.replace(p) else {
            return;
        };
        let length = distance(from, p);
        let point = |s: f32| {
            let t = ((s - walked) / length.max(1e-3)).clamp(0.0, 1.0);
            (from.0 + t * (p.0 - from.0), from.1 + t * (p.1 - from.1))
        };
        if shaft.is_empty() && walked + length >= start {
            let _ = shaft.push(point(start));
        }
        if walked + length >= end {
            let _ = shaft.push(point(end));
            done = true;
            return;
        }
        if !shaft.is_empty() && shaft.push(p).is_err() {
            done = true;
            return;
        }
        walked += length;
    });
    let &tip = shaft.last()?;
    let (ux, uy) = shaft.windows(2).rev().find_map(|pair| {
        let length = distance(pair[0], pair[1]);
        (length >= 0.5).then(|| {
            (
                (pair[1].0 - pair[0].0) / length,
                (pair[1].1 - pair[0].1) / length,
            )
        })
    })?;
    let head = [
        (
            tip.0 + ux * ARROW_HEAD_LENGTH_PX,
            tip.1 + uy * ARROW_HEAD_LENGTH_PX,
        ),
        (
            tip.0 - uy * ARROW_HEAD_HALF_WIDTH_PX,
            tip.1 + ux * ARROW_HEAD_HALF_WIDTH_PX,
        ),
        (
            tip.0 + uy * ARROW_HEAD_HALF_WIDTH_PX,
            tip.1 - ux * ARROW_HEAD_HALF_WIDTH_PX,
        ),
    ];
    Some(ArrowShape { shaft, head })
}
