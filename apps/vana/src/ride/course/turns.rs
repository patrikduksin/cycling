//! Turns on a course, and when to warn of the next one while following it.
//!
//! A point of the course bends by the angle between the way in, the chord
//! from `WINDOW_M` back along the course to it, and the way out, the chord
//! from it to `WINDOW_M` on. Chords rather than single segments, so a
//! hairpin that thinning (`ride::thin`) smoothed into a few points still
//! bends as much as the road did. Points that bend by `BEND_DEG` or more,
//! each within `MERGE_M` of the one before, may be one turn, as a
//! roundabout is. The turn's angle is from the way in before its first
//! point to the way out after its last, and it is a turn only from
//! `TURN_DEG`: a roundabout says where it leaves, and straight over one, a
//! jog left and right or a long bend is no turn.
//!
//! The course is read by index through `at`, start first, in metres east and
//! north, so nothing is copied or kept; a scan reaches at most `AHEAD_MM`
//! ahead. Distances along the course are u32 millimetres, saturating.
use libm::{atan2f, fabsf, sqrtf};

/// The chords each side of a point.
const WINDOW_M: f32 = 15.0;
/// A chord shorter than this, at either end of the course, bends nothing.
const MIN_CHORD_M: f32 = WINDOW_M / 2.0;
/// A point that bends this much may be part of a turn.
const BEND_DEG: f32 = 45.0;
/// Such points this close along the course are one turn.
const MERGE_M: f32 = 2.0 * WINDOW_M;
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

/// A turn ahead: its points `first` to `last`, and how far it is to `first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub kind: Kind,
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
/// start when `back`, or the course's end point when it is nearer.
fn along(
    len: usize,
    at: &impl Fn(usize) -> (f32, f32),
    from: usize,
    back: bool,
    metres: f32,
) -> (f32, f32) {
    let (mut k, mut walked) = (from, 0.0);
    loop {
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

/// The first turn from point `from` on, of a course of `len` points, while
/// its first point is within `reach_mm` of the rider at `place`.
pub fn next(
    len: usize,
    at: impl Fn(usize) -> (f32, f32),
    place: Place,
    from: usize,
    reach_mm: u32,
) -> Option<Turn> {
    if place.next == 0 || place.next >= len {
        return None;
    }
    let step = |k: usize| mm(distance(at(k), at(k + 1)));
    let mut ahead_mm = mm(distance(place.here, at(place.next)));
    let mut k = place.next;
    while k < from && k + 1 < len {
        ahead_mm = ahead_mm.saturating_add(step(k));
        k += 1;
    }
    let turns = |k: usize| fabsf(bend(len, &at, k)) >= BEND_DEG;
    // The course's start and end points never turn.
    while k + 1 < len && ahead_mm <= reach_mm {
        if !turns(k) {
            ahead_mm = ahead_mm.saturating_add(step(k));
            k += 1;
            continue;
        }
        let (first, mut last, mut gap) = (k, k, 0.0);
        let mut u = k;
        while u + 2 < len {
            gap += distance(at(u), at(u + 1));
            u += 1;
            if gap > MERGE_M {
                break;
            }
            if turns(u) {
                (last, gap) = (u, 0.0);
            }
        }
        if let Some(kind) = Kind::of(turned(len, &at, first, last)) {
            return Some(Turn {
                kind,
                first,
                last,
                ahead_mm,
            });
        }
        while k <= last {
            ahead_mm = ahead_mm.saturating_add(step(k));
            k += 1;
        }
    }
    None
}

/// The turn last warned of, so each is warned of once ahead and once at it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Warner {
    #[default]
    Quiet,
    Ahead {
        first: usize,
    },
    /// Warned at the turn; the next is looked for once the rider is past
    /// its first point, from its last.
    Now {
        first: usize,
        last: usize,
    },
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
        let from = match *self {
            Self::Now { first, .. } if place.next <= first => return None,
            Self::Now { last, .. } => last.saturating_add(1),
            _ => place.next,
        };
        let turn = next(len, at, place, from, AHEAD_MM)?;
        if turn.ahead_mm <= NOW_MM {
            *self = Self::Now {
                first: turn.first,
                last: turn.last,
            };
            return Some(Warning::Now(turn.kind));
        }
        if turn.ahead_mm < AHEAD_MIN_MM || *self == (Self::Ahead { first: turn.first }) {
            return None;
        }
        *self = Self::Ahead { first: turn.first };
        let metres = turn.ahead_mm.saturating_add(4_999) / 10_000 * 10;
        Some(Warning::Ahead {
            kind: turn.kind,
            metres,
        })
    }
}

#[cfg(test)]
mod tests;
