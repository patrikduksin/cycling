//! Out-and-back spurs: a planner's waypoint just off the road draws the
//! route into a side street and straight back, which a rider riding on past
//! it would hear as a turn, a U-turn and a turn, and then be told is off
//! course. A finished course drops each spur up to `SPUR_M` deep.
use super::turns::Kind;
use super::{Follow, Point};
use crate::ride::thin::off_segment;
use libm::{atan2f, sqrtf};

const BASE_M: f32 = 15.0;
const SPUR_M: f32 = 150.0;
const RETRACE_M: f32 = 10.0;
const PATH_M: f32 = 2.0 * (SPUR_M + BASE_M);

type Metres = (f32, f32);

/// Drop the spurs from `points`, metres east and north of somewhere by
/// `at`; how many points are left. `follow` hears of each cut, and of each
/// point moved down after it.
pub(super) fn prune(
    points: &mut [Point],
    at: impl Fn(Point) -> Metres,
    follow: &mut impl Follow,
) -> usize {
    let mut len = points.len();
    let mut from = 1;
    while from + 1 < len {
        let Some(Spur { rejoins_at, corner }) = spur(&points[..len], from, &at) else {
            from += 1;
            continue;
        };
        follow.cut(from, rejoins_at, corner);
        let gone = rejoins_at - from - 1;
        for k in rejoins_at..len {
            points[k - gone] = points[k];
            follow.moved(k - gone, k);
        }
        len -= gone;
    }
    len
}

struct Spur {
    rejoins_at: usize,
    corner: Option<Kind>,
}

fn spur(points: &[Point], from: usize, at: &impl Fn(Point) -> Metres) -> Option<Spur> {
    let base = at(points[from]);
    let way_in = minus(base, at(points[from - 1]));
    let (mut path, mut last, mut deepest, mut tip) = (0.0, base, 0.0f32, from);
    let mut found = None;
    for k in from + 1..points.len() - 1 {
        let here = at(points[k]);
        path += length(minus(here, last));
        last = here;
        if path > PATH_M {
            break;
        }
        let gap = squared(minus(here, base));
        if gap <= BASE_M * BASE_M && deepest > BASE_M * BASE_M {
            let depth = sqrtf(deepest);
            let way_on = minus(at(points[k + 1]), here);
            let along = dot(way_in, way_on);
            let not_a_corner = sqrtf(gap) + RETRACE_M < depth;
            let not_a_longer_out_and_back = depth <= SPUR_M;
            let not_a_hairpin = along > 0.0;
            let not_a_roundabout = || {
                path <= 2.0 * (depth + BASE_M)
                    && retraced(&points[from..=tip], &points[tip + 1..k], at)
            };
            if not_a_corner && not_a_longer_out_and_back && not_a_hairpin && not_a_roundabout() {
                let left = way_in.0 * way_on.1 - way_in.1 * way_on.0;
                found = Some(Spur {
                    rejoins_at: k,
                    corner: Kind::of(-atan2f(left, along).to_degrees()),
                });
            }
        }
        if gap > deepest {
            (deepest, tip) = (gap, k);
        }
    }
    found
}

fn retraced(way_out: &[Point], way_back: &[Point], at: &impl Fn(Point) -> Metres) -> bool {
    way_back.iter().all(|&p| {
        let p = at(p);
        way_out
            .windows(2)
            .any(|pair| off_segment(p, at(pair[0]), at(pair[1])) <= RETRACE_M)
    })
}

fn minus(a: Metres, b: Metres) -> Metres {
    (a.0 - b.0, a.1 - b.1)
}

fn dot(a: Metres, b: Metres) -> f32 {
    a.0 * b.0 + a.1 * b.1
}

fn squared(a: Metres) -> f32 {
    dot(a, a)
}

fn length(a: Metres) -> f32 {
    sqrtf(squared(a))
}

#[cfg(test)]
mod tests;
