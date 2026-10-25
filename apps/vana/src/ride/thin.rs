//! Thinning a full line of points in place, for the course and the ride's
//! breadcrumb, so a line of any length fits the room it has.
//!
//! A pass keeps the first point and, from each kept point, reaches on to the
//! farthest point whose chord passes within the tolerance of every point it
//! replaces, at most `SPAN` points on. Passes repeat until a quarter of the
//! room is free, the tolerance doubling only when a pass frees less than
//! that, so a long line keeps its shape to within a few times the tolerance
//! its length needs. Past any sensible tolerance every other point goes. The
//! last point always stays.
use libm::sqrtf;

/// A kept point replaces at most this many points before the next.
const SPAN: usize = 256;
/// Passes at a doubled tolerance before every other point goes.
const ROUNDS: u32 = 16;

/// How far `p` lies from the segment `a`-`b`, all in metres.
pub fn off_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    sqrtf(x * x + y * y)
}

/// Thin the first `len` of `points` until at most three quarters of
/// `points` are in use, at `tolerance` and twice it as needed; `at` places a
/// point in metres. Returns how many points are kept, first and last among
/// them, in order.
pub fn thin<T: Copy>(
    points: &mut [T],
    len: usize,
    tolerance: &mut f32,
    at: impl Fn(&T) -> (f32, f32),
) -> usize {
    thin_moving(points, len, tolerance, at, |_, _| {})
}

/// As `thin`, telling `moved` of each point kept, as `(to, from)` its
/// index after and before a pass, so a list beside `points` can follow.
pub fn thin_moving<T: Copy>(
    points: &mut [T],
    len: usize,
    tolerance: &mut f32,
    at: impl Fn(&T) -> (f32, f32),
    mut moved: impl FnMut(usize, usize),
) -> usize {
    let target = points.len() * 3 / 4;
    let mut len = len;
    let mut rounds = 0;
    loop {
        len = pass(
            &mut points[..len],
            *tolerance,
            rounds > ROUNDS,
            &at,
            &mut moved,
        );
        if len <= target {
            return len;
        }
        rounds += 1;
        *tolerance *= 2.0;
    }
}

/// One pass over `points`, compacted in place; returns how many are kept.
/// Each kept point is written at or before where it was, and the reach
/// reads only from the last kept point on, so nothing it reads is
/// overwritten.
fn pass<T: Copy>(
    points: &mut [T],
    tolerance: f32,
    halve: bool,
    at: &impl Fn(&T) -> (f32, f32),
    moved: &mut impl FnMut(usize, usize),
) -> usize {
    let len = points.len();
    if len <= 2 {
        return len;
    }
    let mut kept = 1;
    let mut anchor = 0;
    while anchor + 1 < len - 1 {
        anchor = if halve {
            (anchor + 2).min(len - 1)
        } else {
            reach(points, anchor, tolerance, at)
        };
        points[kept] = points[anchor];
        moved(kept, anchor);
        kept += 1;
    }
    if anchor != len - 1 {
        points[kept] = points[len - 1];
        moved(kept, len - 1);
        kept += 1;
    }
    kept
}

/// The farthest point after `anchor`, at most `SPAN` on, whose chord from
/// `anchor` passes within `tolerance` of every point between.
fn reach<T: Copy>(
    points: &[T],
    anchor: usize,
    tolerance: f32,
    at: &impl Fn(&T) -> (f32, f32),
) -> usize {
    let a = at(&points[anchor]);
    let mut end = anchor + 2;
    while end < points.len() && end - anchor <= SPAN {
        let b = at(&points[end]);
        if !(anchor + 1..end).all(|j| off_segment(at(&points[j]), a, b) <= tolerance) {
            break;
        }
        end += 1;
    }
    end - 1
}
