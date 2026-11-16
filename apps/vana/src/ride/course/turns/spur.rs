//! Out-and-back spurs: a planner's waypoint just off the road snaps the
//! route into a side street and straight back along it. A rider riding on
//! past it would hear a turn, a U-turn and a turn, none of which they take.
//!
//! Where the scan's chord swings by `REVERSAL_DEG` in one step, the course
//! has turned back on itself. From there its way back is walked a metre at
//! a time while it stays within `RETRACE_M` of its way out. Ending within
//! `SPUR_M` of the tip, it is a spur from where its last metre lies beside
//! the way out to where it ends. Retracing further, the course is a longer
//! out-and-back, whose legs' corners the rider is told; but a dead end at
//! its far end, `TIP_SPUR_M` or less past the last corner, is a spur from
//! that corner. Either way the way out must go at least `SPUR_MIN_M` from
//! the spur's base, as a stop's jitter never does. A hairpin's legs part
//! too soon and roads side by side are too far apart to be one. So a dead
//! end visited no deeper than `SPUR_M` is taken for a planner's artifact.
//!
//! The course keeps every point of a spur, and its length. The turns found
//! on it and half a chord either side say nothing. In their place is one
//! turn at its base, named by the course's way before the first of them
//! and after the last, so a corner at the base is never lost. A way back
//! that meets the course past the base, as thinning draws one, may turn
//! onto it within a chord after: that turn goes too when it ends within
//! half a chord more and, with it, the course runs on as it came. A turn
//! ending further is the course's own, as a dogleg just past a spur is:
//! one that began on the spur says nothing, but its corner, if it has one,
//! still holds back what is after it.
use super::{Cursor, HALF_CHORD_M, Kind, TURN_DEG, Turn, distance, mm, swing, way};
use crate::ride::thin::off_segment;
use libm::{fabsf, sqrtf};

/// The scan keeps this far of the course behind its chord's middle: a
/// spur's whole way out and the way in before it.
pub(super) const BACK_M: f32 = SPUR_M + 2.0 * WAY_M;
pub(super) const REVERSAL_DEG: f32 = 90.0;
const RETRACE_M: f32 = 3.0;
const SPUR_MIN_M: f32 = 10.0;
const SPUR_M: f32 = 150.0;
const TIP_SPUR_M: f32 = 2.0 * WAY_M;
/// The way into a spur and on from it is the chord this long.
const WAY_M: f32 = 2.0 * HALF_CHORD_M;
const WALK_M: f32 = 1.0;

/// A spur's base and where its way back meets the course, metres along.
#[derive(Clone, Copy, Debug)]
pub(super) struct Stretch {
    base_m: f32,
    rejoin_m: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) enum Spur {
    #[default]
    None,
    /// The course turned back at `tip_m`, the point `tip`; `back` has
    /// walked `back_m` of its way back, whose last metre lies beside the
    /// way out `beside_m` along. `corner` is the spur past the furthest
    /// corner the way out turned back from the tip.
    Walking {
        tip_m: f32,
        tip: Cursor,
        back: Cursor,
        back_m: f32,
        beside_m: f32,
        corner: Option<Stretch>,
    },
    /// A spur waiting for its turns to be found.
    Found(Stretch),
}

impl Spur {
    pub(super) fn reversed(tip_m: f32, tip: Cursor) -> Self {
        Self::Walking {
            tip_m,
            tip,
            back: tip,
            back_m: 0.0,
            beside_m: tip_m,
            corner: None,
        }
    }

    /// Walk on as far as the scan has read, `read_m`, beside the way out
    /// from `trail`, `trail_m` along.
    pub(super) fn walk(
        &mut self,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
        (trail, trail_m): (&Cursor, f32),
        read_m: f32,
    ) {
        let Self::Walking {
            tip_m,
            tip,
            mut back,
            mut back_m,
            mut beside_m,
            mut corner,
        } = *self
        else {
            return;
        };
        let way_out = WayOut {
            trail,
            trail_m,
            tip: &tip,
        };
        let found = |spur: Stretch| match way_out.deepest(spur.base_m, at) >= SPUR_MIN_M {
            true => Self::Found(spur),
            false => Self::None,
        };
        while tip_m + back_m + WALK_M <= read_m {
            if !back.on(len, at, WALK_M) {
                *self = Self::None;
                return;
            }
            if back_m + WALK_M > SPUR_M {
                *self = corner.map_or(Self::None, found);
                return;
            }
            back_m += WALK_M;
            let (off, along_m) = way_out.nearest(back.here, at);
            if off > RETRACE_M {
                *self = found(Stretch {
                    base_m: beside_m,
                    rejoin_m: tip_m + back_m - WALK_M,
                });
                return;
            }
            beside_m = along_m;
            let past_corner = (SPUR_MIN_M..=TIP_SPUR_M).contains(&(tip_m - beside_m));
            if past_corner && way_out.turns_at(beside_m, at) {
                corner = Some(Stretch {
                    base_m: beside_m,
                    rejoin_m: tip_m + back_m,
                });
            }
        }
        *self = Self::Walking {
            tip_m,
            tip,
            back,
            back_m,
            beside_m,
            corner,
        };
    }

    /// A found spur, once every turn the scan has yet to list begins after
    /// `open_from_m`, past it.
    pub(super) fn settled(&self, open_from_m: f32) -> Option<Stretch> {
        match *self {
            Self::Found(spur) if open_from_m > spur.rejoin_m + WAY_M => Some(spur),
            _ => None,
        }
    }
}

/// The course from `trail`, `trail_m` along, to the point `tip`.
struct WayOut<'a> {
    trail: &'a Cursor,
    trail_m: f32,
    tip: &'a Cursor,
}

impl WayOut<'_> {
    /// Each segment, with metres along to its start.
    fn each(
        &self,
        at: &impl Fn(usize) -> (f32, f32),
        mut segment: impl FnMut((f32, f32), (f32, f32), f32),
    ) {
        let (mut from, mut from_m) = (self.trail.here, self.trail_m);
        let points = (self.trail.next..self.tip.next).map(at);
        for to in points.chain(core::iter::once(self.tip.here)) {
            segment(from, to, from_m);
            from_m += distance(from, to);
            from = to;
        }
    }

    /// How far `p` is from it, and metres along to its point nearest `p`.
    fn nearest(&self, p: (f32, f32), at: &impl Fn(usize) -> (f32, f32)) -> (f32, f32) {
        let mut nearest = (f32::INFINITY, self.trail_m);
        self.each(at, |a, b, a_m| {
            let off = off_segment(p, a, b);
            if off < nearest.0 {
                let along = distance(a, b);
                let to_foot = sqrtf((distance(a, p) * distance(a, p) - off * off).max(0.0));
                nearest = (off, a_m + to_foot.min(along));
            }
        });
        nearest
    }

    /// Its point `m` along.
    fn point(&self, m: f32, at: &impl Fn(usize) -> (f32, f32)) -> (f32, f32) {
        let mut point = self.tip.here;
        let mut found = false;
        self.each(at, |a, b, a_m| {
            let along = distance(a, b);
            if !found && a_m + along >= m {
                let t = if along > 0.0 {
                    ((m - a_m) / along).max(0.0)
                } else {
                    0.0
                };
                point = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
                found = true;
            }
        });
        point
    }

    /// Whether it turns a corner at its point `m` along.
    fn turns_at(&self, m: f32, at: &impl Fn(usize) -> (f32, f32)) -> bool {
        let half = WAY_M / 2.0;
        let here = self.point(m, at);
        let way_in = way(self.point(m - half, at), here);
        let way_on = way(here, self.point(m + half, at));
        way_in
            .zip(way_on)
            .is_some_and(|(way_in, way_on)| fabsf(swing(way_in, way_on)) >= TURN_DEG)
    }

    /// How far from its point `base_m` along it goes, on to the tip.
    fn deepest(&self, base_m: f32, at: &impl Fn(usize) -> (f32, f32)) -> f32 {
        let mut base = None;
        let mut deepest = 0.0f32;
        self.each(at, |a, b, a_m| {
            let along = distance(a, b);
            if base.is_none() && a_m + along >= base_m {
                let t = if along > 0.0 {
                    (base_m - a_m) / along
                } else {
                    0.0
                };
                base = Some((a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1)));
            }
            if let Some(base) = base {
                deepest = deepest.max(distance(base, b));
            }
        });
        deepest
    }
}

/// Put `spur` into `turns`, the first `found` of them listed, each after
/// `from_mm`; `trail` is the course `trail_m` along, the furthest back it
/// may be read.
pub(super) fn mark(
    Stretch { base_m, rejoin_m }: Stretch,
    len: usize,
    at: &impl Fn(usize) -> (f32, f32),
    (trail, trail_m): (Cursor, f32),
    turns: &mut [Turn],
    found: &mut usize,
    from_mm: u32,
) {
    if mm(base_m) <= from_mm {
        return;
    }
    let from = mm(base_m - HALF_CHORD_M);
    let first = turns[..*found].partition_point(|turn| turn.end_mm <= from);
    let on_it = &turns[first..*found];
    let on_spur = on_it.partition_point(|turn| turn.start_mm < mm(rejoin_m + HALF_CHORD_M));
    let point = |m: f32| {
        let mut cursor = trail;
        cursor.on(len, at, (m - trail_m).max(0.0));
        cursor.here
    };
    let way_in_m = on_it
        .first()
        .map_or(base_m, |turn| base_m.min(turn.start_mm as f32 / 1000.0));
    let way_in = way(point(way_in_m - WAY_M), point(way_in_m));
    let corner_after = |on_it: &[Turn]| {
        let way_on_m = on_it
            .last()
            .map_or(rejoin_m, |turn| rejoin_m.max(turn.end_mm as f32 / 1000.0));
        let way_on = way(point(way_on_m), point(way_on_m + WAY_M));
        way_in
            .zip(way_on)
            .and_then(|(way_in, way_on)| Kind::of(swing(way_in, way_on)))
    };
    let reach_mm = mm(rejoin_m + WAY_M + HALF_CHORD_M);
    let onto = on_it
        .partition_point(|turn| turn.end_mm <= reach_mm)
        .max(on_spur);
    let (silenced, kind) = match corner_after(&on_it[..onto]) {
        None => (onto, None),
        Some(_) => (on_spur, corner_after(&on_it[..on_spur])),
    };
    for turn in &mut turns[first..first + silenced] {
        *turn = Turn {
            start_mm: turn.start_mm,
            end_mm: turn.end_mm,
            cornered: turn.cornered && turn.end_mm > reach_mm,
            ..Turn::default()
        };
    }
    let Some(kind) = kind else {
        return;
    };
    let base = Turn {
        start_mm: mm(base_m - HALF_CHORD_M),
        end_mm: mm(base_m + HALF_CHORD_M),
        kind: Some(kind),
        cornered: true,
        ..Turn::default()
    };
    if silenced > 0 {
        turns[first] = base;
    } else if *found < turns.len() {
        turns.copy_within(first..*found, first + 1);
        turns[first] = base;
        *found += 1;
    }
}

#[cfg(test)]
mod tests;
