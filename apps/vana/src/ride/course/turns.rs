//! Turns on a course, and when to warn of the next one while following it.
//!
//! A point of the course bends by the angle between the way in, the chord
//! from `WINDOW_M` back along the course to it, and the way out, the chord
//! from it to `WINDOW_M` on. Chords rather than single segments, so a
//! hairpin that thinning (`ride::thin`) smoothed into a few points still
//! bends as much as the road did. Points that bend by `BEND_DEG` or more,
//! each within `MERGE_M` of the one before, may be one turn, as a
//! roundabout is, up to `SPAN_M` along. The turn's angle is from the way in before its first
//! point to the way out after its last, and it is a turn only from
//! `TURN_DEG`: a roundabout says where it leaves, and straight over one, a
//! jog left and right or a long bend is no turn.
//!
//! The course is read by index through `at`, start first, in metres east and
//! north, so nothing is copied or kept; a scan reaches at most `AHEAD_MM`
//! and `SCAN_POINTS` ahead, and a chord walks at most `CHORD_POINTS`, so a
//! course of thousands of points in one spot costs no more than a road.
//! Distances along the course are u32 millimetres, saturating.
use libm::{atan2f, fabsf, sqrtf};

/// The chords each side of a point.
const WINDOW_M: f32 = 15.0;
/// A chord shorter than this, at either end of the course, bends nothing.
const MIN_CHORD_M: f32 = WINDOW_M / 2.0;
/// A point that bends this much may be part of a turn.
const BEND_DEG: f32 = 45.0;
/// Such points this close along the course are one turn, at most this long.
const MERGE_M: f32 = 2.0 * WINDOW_M;
const SPAN_M: f32 = 80.0;
/// The most points one scan, or one chord, walks.
const SCAN_POINTS: usize = 256;
const CHORD_POINTS: usize = 64;
/// The most groups of turning points one warning looks past.
const GROUPS: usize = 4;
/// A turn turns by this much or more; a sharp one and a U-turn by more.
const TURN_DEG: f32 = 60.0;
const SHARP_DEG: f32 = 120.0;
const U_TURN_DEG: f32 = 150.0;
/// The first warning comes this far before a turn, the second this far.
const AHEAD_MM: u32 = 155_000;
const NOW_MM: u32 = 30_000;
/// A turn first in reach nearer than this is warned of only at it, so two
/// warnings never come a second apart.
const AHEAD_MIN_MM: u32 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Turn(Side),
    Sharp(Side),
    UTurn,
}

impl Kind {
    fn of(degrees: f32) -> Option<Self> {
        let side = if degrees > 0.0 {
            Side::Right
        } else {
            Side::Left
        };
        match fabsf(degrees) {
            a if a >= U_TURN_DEG => Some(Self::UTurn),
            a if a >= SHARP_DEG => Some(Self::Sharp(side)),
            a if a >= TURN_DEG => Some(Self::Turn(side)),
            _ => None,
        }
    }
}

/// Turning points ahead, `first` to `last`, and how far it is to `first`;
/// a turn when they turn enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Option<Kind>,
    pub first: usize,
    pub last: usize,
    pub ahead_mm: u32,
}

/// Where the rider is: at `here`, on the segment that ends at point `next`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub next: usize,
    pub here: (f32, f32),
}

/// What to tell the rider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// A turn about `metres` ahead, to the nearest 10, half down.
    Ahead { kind: Kind, metres: u32 },
    /// The turn is here.
    Now(Kind),
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (east, north) = (b.0 - a.0, b.1 - a.1);
    sqrtf(east * east + north * north)
}

/// A float cast saturates, so no length overflows.
fn mm(metres: f32) -> u32 {
    (metres * 1000.0) as u32
}

/// The angle from the way `a` to the way `b`, in degrees, right positive.
fn angle(a: (f32, f32), b: (f32, f32)) -> f32 {
    let cross = a.0 * b.1 - a.1 * b.0;
    let dot = a.0 * b.0 + a.1 * b.1;
    -atan2f(cross, dot).to_degrees()
}

/// The point `metres` along the course from point `from`, towards the
/// start when `back`, or the course's end point when it is nearer, or the
/// point `CHORD_POINTS` on when that is.
fn along(
    len: usize,
    at: &impl Fn(usize) -> (f32, f32),
    from: usize,
    back: bool,
    metres: f32,
) -> (f32, f32) {
    let (mut k, mut walked) = (from, 0.0);
    for _ in 0..CHORD_POINTS {
        let to = match back {
            true if k == 0 => return at(0),
            true => k - 1,
            false if k + 1 >= len => return at(len - 1),
            false => k + 1,
        };
        let (a, b) = (at(k), at(to));
        let step = distance(a, b);
        if walked + step >= metres {
            let t = (metres - walked) / step;
            return (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        }
        walked += step;
        k = to;
    }
    at(k)
}

fn way(from: (f32, f32), to: (f32, f32)) -> Option<(f32, f32)> {
    (distance(from, to) >= MIN_CHORD_M).then_some((to.0 - from.0, to.1 - from.1))
}

/// How point `k` bends: from the chord `WINDOW_M` back to it to the chord
/// `WINDOW_M` on from it, or 0 where a chord is too short to tell.
fn bend(len: usize, at: &impl Fn(usize) -> (f32, f32), k: usize) -> f32 {
    let into = way(along(len, at, k, true, WINDOW_M), at(k));
    let out = way(at(k), along(len, at, k, false, WINDOW_M));
    match (into, out) {
        (Some(into), Some(out)) => angle(into, out),
        _ => 0.0,
    }
}

/// How a turn from point `first` to point `last` turns: from the way in,
/// the chord that ends `WINDOW_M` before `first`, to the way out, the chord
/// that starts `WINDOW_M` after `last`, both clear of the turn itself.
fn turned(len: usize, at: &impl Fn(usize) -> (f32, f32), first: usize, last: usize) -> f32 {
    let (w, w2) = (WINDOW_M, 2.0 * WINDOW_M);
    let into = way(
        along(len, at, first, true, w2),
        along(len, at, first, true, w),
    );
    let out = way(
        along(len, at, last, false, w),
        along(len, at, last, false, w2),
    );
    match (into, out) {
        (Some(into), Some(out)) => angle(into, out),
        _ => 0.0,
    }
}

/// The first group of turning points from point `from` on, of a course of
/// `len` points, while its first point is within `reach_mm` of the rider at
/// `place`.
pub fn next(
    len: usize,
    at: impl Fn(usize) -> (f32, f32),
    place: Place,
    from: usize,
    reach_mm: u32,
) -> Option<Group> {
    if place.next == 0 || place.next >= len {
        return None;
    }
    let step = |k: usize| distance(at(k), at(k + 1));
    let mut ahead_mm = mm(distance(place.here, at(place.next)));
    let mut k = place.next;
    let mut budget = SCAN_POINTS;
    while k < from && k + 1 < len && budget > 0 {
        ahead_mm = ahead_mm.saturating_add(mm(step(k)));
        k += 1;
        budget -= 1;
    }
    let turns = |k: usize| fabsf(bend(len, &at, k)) >= BEND_DEG;
    // The course's start and end points never turn.
    while k + 1 < len && ahead_mm <= reach_mm && budget > 0 {
        budget -= 1;
        if !turns(k) {
            ahead_mm = ahead_mm.saturating_add(mm(step(k)));
            k += 1;
            continue;
        }
        let (first, mut last) = (k, k);
        let (mut u, mut gap, mut span) = (k, 0.0, 0.0);
        while u + 2 < len && budget > 0 {
            budget -= 1;
            let metres = step(u);
            (gap, span) = (gap + metres, span + metres);
            u += 1;
            if gap > MERGE_M || span > SPAN_M {
                break;
            }
            if turns(u) {
                (last, gap) = (u, 0.0);
            }
        }
        return Some(Group {
            kind: Kind::of(turned(len, &at, first, last)),
            first,
            last,
            ahead_mm,
        });
    }
    None
}

/// The group of turning points last found ahead, so each turn is warned of
/// once ahead and once at it, and a group the rider is in is never taken
/// for a turn from partway through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Warner {
    #[default]
    Quiet,
    /// Warned of ahead.
    Ahead { first: usize, last: usize },
    /// Warned of at it; the next is looked for once the rider is past its
    /// first point.
    Now { first: usize, last: usize },
    /// Not a turn: the points after it are looked at instead.
    Skip { first: usize, last: usize },
}

impl Warner {
    /// The warning for the rider at `place` on a course of `len` points, if
    /// one is due.
    pub fn step(
        &mut self,
        len: usize,
        at: impl Fn(usize) -> (f32, f32),
        place: Place,
    ) -> Option<Warning> {
        let mut from = match *self {
            Self::Quiet => place.next,
            Self::Now { first, .. } if place.next <= first => return None,
            Self::Ahead { first, .. } if place.next <= first => first,
            Self::Ahead { last, .. } | Self::Now { last, .. } | Self::Skip { last, .. } => {
                last.saturating_add(1)
            }
        };
        for _ in 0..GROUPS {
            let group = next(len, &at, place, from, AHEAD_MM)?;
            let (first, last) = (group.first, group.last);
            let Some(kind) = group.kind else {
                *self = Self::Skip { first, last };
                from = last.saturating_add(1);
                continue;
            };
            if group.ahead_mm <= NOW_MM {
                *self = Self::Now { first, last };
                return Some(Warning::Now(kind));
            }
            if group.ahead_mm < AHEAD_MIN_MM || *self == (Self::Ahead { first, last }) {
                return None;
            }
            *self = Self::Ahead { first, last };
            let metres = group.ahead_mm.saturating_add(4_999) / 10_000 * 10;
            return Some(Warning::Ahead { kind, metres });
        }
        None
    }
}

#[cfg(test)]
mod tests;
