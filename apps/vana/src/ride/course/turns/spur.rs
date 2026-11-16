//! Out-and-back spurs: a planner's waypoint just off the road snaps the
//! route into a side street and straight back along it. A rider riding on
//! past it would hear a turn, a U-turn and a turn, none of which they take.
//!
//! `Spurs` reads the course as it is, ahead of the scan, the scan's own
//! chord a step at a time. Where that chord swings by `REVERSAL_DEG` in one
//! step, the course has turned back on itself. From there its way back is
//! walked a metre at a time, and onto each of its points, while it stays
//! within `RETRACE_M` of its way out. Ending within `SPUR_M` of the tip, it
//! is a spur from the point of the way out furthest back that it came beside
//! to where it did: a detour off the road the course rides on. Retracing
//! further, or back beside the way in `ON_M` past where its way back left
//! its way out, the course is a longer out-and-back, whose legs' corners the
//! rider is told; but a dead end at its far end, `TIP_SPUR_M` or less past
//! the last corner, is a spur from that corner. Either way the way out must
//! go at least `SPUR_MIN_M` from the spur's base, as a stop's jitter never
//! does. A hairpin's legs part too soon and roads side by side are too far
//! apart to be one. So a dead end visited no deeper than `SPUR_M` is taken
//! for a planner's artifact.
//!
//! A course with a spur says exactly what the course would say with the
//! spur's path taken out, each turn after the spur as much further on as
//! the spur is long. That course runs along the way in to the base on the
//! way out, then on from the base on the way back, either part way along a
//! segment, as on a route drawn at its corners: the way on keeps its own
//! corners. The scan reads it through `Spurs::at` (`Spurs::skip`), far
//! enough behind `Spurs` that every spur before what it reads is found,
//! and its turns are moved onto the course by `Spurs::real`. The course
//! itself keeps every point of a spur, and its length.
use super::{BEND_MIN_DEG, Cursor, HALF_CHORD_M, STEP_M, TURN_DEG, Turn, distance, mm, swing, way};
use crate::ride::thin::off_segment;
use libm::{atan2f, fabsf, sqrtf};

/// `Spurs` keeps this far of the course behind its chord's middle: a
/// spur's whole way out and the way in before it.
const BACK_M: f32 = SPUR_M + 2.0 * WAY_M;
const REVERSAL_DEG: f32 = 90.0;
const RETRACE_M: f32 = 3.0;
const SPUR_MIN_M: f32 = 10.0;
const SPUR_M: f32 = 150.0;
const TIP_SPUR_M: f32 = 2.0 * WAY_M;
const ON_M: f32 = 20.0;
/// The way into a spur and on from it is the chord this long.
const WAY_M: f32 = 2.0 * HALF_CHORD_M;
const WALK_M: f32 = 1.0;
const SAME_M: f32 = 0.05;
const FURTHER_BACK_M: f32 = 0.05;
const TURNS_OFF_DEG: f32 = 10.0;
const PAST_BASE_M: f32 = 2.0 * WAY_M;
/// Spurs the scan reads past at once; one more found before it is past
/// the first is ridden as drawn.
const SKIPS: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Stretch {
    base_m: f32,
    rejoin: Cursor,
    rejoin_m: f32,
    tip: Cursor,
    past_corner: bool,
}

#[derive(Clone, Copy, Debug, Default)]
enum Walk {
    #[default]
    None,
    /// The course turned back at `tip_m`, the point `tip`; `back` has
    /// walked `back_m` of its way back, whose last metre lies beside the
    /// way out `beside_m` along. `corner` is the spur past the furthest
    /// corner the way out turned back from the tip. `left_m` is `back_m`
    /// where the way back left the way out, once it has.
    Back {
        tip_m: f32,
        tip: Cursor,
        back: Cursor,
        back_m: f32,
        step_end_m: f32,
        beside_m: f32,
        furthest_back: Stretch,
        corner: Option<Stretch>,
        left_m: Option<f32>,
    },
}

impl Walk {
    fn reversed(tip_m: f32, tip: Cursor) -> Self {
        Self::Back {
            tip_m,
            tip,
            back: tip,
            back_m: 0.0,
            step_end_m: WALK_M,
            beside_m: tip_m,
            furthest_back: Stretch {
                base_m: tip_m,
                rejoin: tip,
                rejoin_m: tip_m,
                tip,
                past_corner: false,
            },
            corner: None,
            left_m: None,
        }
    }

    /// Walk on as far as `read_m`, beside the way out from `trail`,
    /// `trail_m` along: the spur, once its way back has left the way out
    /// and gone `ON_M` on, or the course ends first.
    fn on(
        &mut self,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
        (trail, trail_m): (&Cursor, f32),
        read_m: f32,
    ) -> Option<Stretch> {
        let Self::Back {
            tip_m,
            tip,
            mut back,
            mut back_m,
            mut step_end_m,
            mut beside_m,
            mut furthest_back,
            mut corner,
            mut left_m,
        } = *self
        else {
            return None;
        };
        let way_out = WayOut {
            trail,
            trail_m,
            tip: &tip,
        };
        let deep = |spur: &Stretch| way_out.deepest(spur.base_m, at) >= SPUR_MIN_M;
        *self = Self::None;
        while tip_m + back_m + WALK_M <= read_m {
            let to_end = step_end_m - back_m;
            let step = back.stride(len, at, to_end);
            if !back.on(len, at, step) {
                return left_m.and(Some(furthest_back));
            }
            if left_m.is_none() && back_m + step > SPUR_M {
                return corner.filter(deep);
            }
            back_m += step;
            if step >= to_end {
                (back_m, step_end_m) = (step_end_m, step_end_m + WALK_M);
            }
            let (off, along_m) = way_out.nearest(back.here, at);
            if let Some(left_m) = left_m {
                if back_m - left_m < ON_M {
                    continue;
                }
                let way_in_again = off <= RETRACE_M && along_m < furthest_back.base_m;
                return match way_in_again {
                    true => corner.filter(deep),
                    false => Some(furthest_back),
                };
            }
            if off > RETRACE_M {
                if !deep(&furthest_back) {
                    return None;
                }
                left_m = Some(back_m);
                continue;
            }
            beside_m = along_m;
            let here = Stretch {
                base_m: beside_m,
                rejoin: back,
                rejoin_m: tip_m + back_m,
                tip,
                past_corner: false,
            };
            if beside_m < furthest_back.base_m - FURTHER_BACK_M {
                furthest_back = here;
            }
            let past_corner = (SPUR_MIN_M..=TIP_SPUR_M).contains(&(tip_m - beside_m));
            if past_corner && way_out.turns_at(beside_m, at) {
                corner = Some(Stretch {
                    past_corner,
                    ..here
                });
            }
        }
        *self = Self::Back {
            tip_m,
            tip,
            back,
            back_m,
            step_end_m,
            beside_m,
            furthest_back,
            corner,
            left_m,
        };
        None
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Skip {
    first: usize,
    last: usize,
    out: (f32, f32),
    back: (f32, f32),
    cut_from_m: f32,
    cut_m: f32,
}

impl Skip {
    fn back_from(&self) -> usize {
        (self.first + 1).min(self.last - 1)
    }
}

/// Finds a course's spurs ahead of the scan, and reads the course to the
/// scan without them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Spurs {
    behind: Cursor,
    ahead: Cursor,
    /// Metres along to the middle of the chord, and its way.
    s: f32,
    heading: Option<f32>,
    /// The course `BACK_M` behind the chord's middle, or further while a
    /// spur is walked, but never before the last spur's end.
    trail: Cursor,
    trail_m: f32,
    walk: Walk,
    skips: [Skip; SKIPS],
    count: usize,
    passed_cut_m: f32,
    started: bool,
    done: bool,
}

impl Spurs {
    /// The point its chord heads for: it has read the course no further.
    pub(crate) fn reads_to(&self) -> usize {
        self.ahead.next
    }

    /// The points before this are in no spur it has yet to find.
    pub(crate) fn settled(&self) -> usize {
        self.trail.next
    }

    pub(crate) fn done(&self) -> bool {
        self.done
    }

    pub(super) fn found_to(&self, m: f32) -> bool {
        self.done || m <= self.trail_m
    }

    pub(crate) fn oldest(&self) -> usize {
        self.behind.next.min(self.trail.next).saturating_sub(1)
    }

    pub(crate) fn step(&mut self, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        if self.done {
            return;
        }
        if !self.started {
            self.started = true;
            self.behind = Cursor::start(at);
            (self.ahead, self.trail) = (self.behind, self.behind);
            self.done = !self.ahead.on(len, at, WAY_M);
            self.s = HALF_CHORD_M;
            self.heading = way(self.behind.here, self.ahead.here);
            return;
        }
        if !self.behind.on(len, at, STEP_M) || !self.ahead.on(len, at, STEP_M) {
            self.done = true;
            return;
        }
        self.s += STEP_M;
        if matches!(self.walk, Walk::None) {
            let lag = self.s - BACK_M;
            if lag > self.trail_m && self.trail.on(len, at, lag - self.trail_m) {
                self.trail_m = lag;
            }
        }
        let read_m = self.s + HALF_CHORD_M;
        if let Some(spur) = self.walk.on(len, at, (&self.trail, self.trail_m), read_m) {
            self.skip(spur, len, at);
        }
        let Some(now) = way(self.behind.here, self.ahead.here) else {
            return;
        };
        let turned = self.heading.map_or(0.0, |heading| swing(heading, now));
        self.heading = Some(now);
        if fabsf(turned) >= REVERSAL_DEG && matches!(self.walk, Walk::None) {
            let mut tip = self.behind;
            tip.on(len, at, HALF_CHORD_M);
            self.walk = Walk::reversed(self.s, tip);
        }
    }

    fn on_course(&self, m: f32, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> OnCourse {
        let mut here = self.trail;
        here.on(len, at, m - self.trail_m);
        OnCourse {
            before_m: m - distance(at(here.next - 1), here.here),
            here,
        }
    }

    fn skip(&mut self, spur: Stretch, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        let mut out = self.on_course(spur.base_m, len, at);
        let (mut back, mut base_m, mut rejoin_m) = (spur.rejoin, spur.base_m, spur.rejoin_m);
        let from = (out.here.next - 1, out.before_m);
        if let Some((left_m, j)) = self.left_by(&spur, from, (len, back.next), at) {
            out = self.on_course(left_m, len, at);
            (back, base_m, rejoin_m) = (Cursor::at(at, j), left_m, along(at, from, j));
        }
        (self.trail, self.trail_m) = (back, rejoin_m);
        let (first, last) = (out.here.next, back.next);
        if self.count >= SKIPS || last <= first {
            return;
        }
        let (from, from_m) = match last > first + 1 {
            true => (out.here.here, base_m),
            false => (at(first - 1), out.before_m),
        };
        let cut_m: f32 = self.skips[..self.count].iter().map(|skip| skip.cut_m).sum();
        self.skips[self.count] = Skip {
            first,
            last,
            out: from,
            back: back.here,
            cut_from_m: from_m - self.passed_cut_m - cut_m,
            cut_m: rejoin_m - from_m - distance(from, back.here),
        };
        self.count += 1;
    }

    fn left_by(
        &self,
        spur: &Stretch,
        from: (usize, f32),
        (len, to): (usize, usize),
        at: &impl Fn(usize) -> (f32, f32),
    ) -> Option<(f32, usize)> {
        let way_out = WayOut {
            trail: &self.trail,
            trail_m: self.trail_m,
            tip: &spur.tip,
        };
        let parts =
            |k: usize, j: usize| k > 0 && j + 1 < len && parted(at(k), at(k - 1), at(j + 1));
        let (left_m, j, parts) = retraced(at, &spur.tip, (from.0, to))
            .map(|(k, j)| (along(at, from, k), j, parts(k, j)))
            .or_else(|| {
                let (left_m, j) = way_out.left_at(spur, (len, to), at)?;
                Some((left_m, j, true))
            })?;
        let walked_on = left_m - spur.base_m;
        let proven = walked_on <= 0.0 || parts || spur.past_corner;
        (fabsf(walked_on) <= WAY_M && proven).then_some((left_m, j))
    }

    fn skip_of(&self, k: usize) -> Option<&Skip> {
        let skips = &self.skips[..self.count];
        skips
            .iter()
            .find(|skip| (skip.first..skip.last).contains(&k))
    }

    /// Point `k` of the course as the scan reads it, `at` the course.
    pub(crate) fn at(&self, at: &impl Fn(usize) -> (f32, f32), k: usize) -> (f32, f32) {
        match self.skip_of(k) {
            Some(skip) if k < skip.back_from() => skip.out,
            Some(skip) => skip.back,
            None => at(k),
        }
    }

    pub(crate) fn cut_at(&self, k: usize) -> f32 {
        self.skip_of(k)
            .filter(|skip| k == skip.back_from())
            .map_or(0.0, |skip| skip.cut_m)
    }

    /// Whether the course read without its spurs has no point `k`.
    pub(crate) fn skipped(&self, k: usize) -> bool {
        self.skip_of(k).is_some()
    }

    /// `turn`, found on the course as the scan reads it, on the course.
    pub(super) fn real(&self, turn: Turn) -> Turn {
        let real = |read_mm: u32| mm(self.real_m(read_mm as f32 / 1000.0));
        Turn {
            start_mm: real(turn.start_mm),
            end_mm: real(turn.end_mm),
            ..turn
        }
    }

    /// Metres along the course to `read_m` along it as the scan reads it.
    pub(super) fn real_m(&self, read_m: f32) -> f32 {
        let skips = self.skips[..self.count].iter();
        let cut_m: f32 = skips
            .filter(|skip| read_m > skip.cut_from_m)
            .map(|skip| skip.cut_m)
            .sum();
        read_m + self.passed_cut_m + cut_m
    }

    #[cfg(test)]
    pub(super) fn spurs_m(&self) -> f32 {
        let skips = self.skips[..self.count].iter();
        self.passed_cut_m + skips.map(|skip| skip.cut_m).sum::<f32>()
    }

    /// The scan reads no point before `oldest` again.
    pub(crate) fn passed(&mut self, oldest: usize) {
        let passed = self.skips[..self.count]
            .iter()
            .take_while(|skip| skip.last <= oldest)
            .count();
        self.passed_cut_m += self.skips[..passed]
            .iter()
            .map(|skip| skip.cut_m)
            .sum::<f32>();
        self.skips.copy_within(passed..self.count, 0);
        self.count -= passed;
    }
}

fn parted(meet: (f32, f32), way_out: (f32, f32), way_back: (f32, f32)) -> bool {
    let bearing = |q: (f32, f32)| atan2f(q.0 - meet.0, q.1 - meet.1).to_degrees();
    fabsf(swing(bearing(way_out), bearing(way_back))) >= TURNS_OFF_DEG
}

fn along(at: &impl Fn(usize) -> (f32, f32), from: (usize, f32), k: usize) -> f32 {
    let steps = (from.0..k).map(|j| distance(at(j), at(j + 1)));
    from.1 + steps.sum::<f32>()
}

fn retraced(
    at: &impl Fn(usize) -> (f32, f32),
    tip: &Cursor,
    (from, to): (usize, usize),
) -> Option<(usize, usize)> {
    (from..=tip.next.min(to))
        .rev()
        .take_while(|&k| k + 1 >= tip.next || distance(at(k), tip.here) <= HALF_CHORD_M)
        .find_map(|apex| {
            let meets = |j: usize| {
                apex >= from + j && apex + j <= to && distance(at(apex - j), at(apex + j)) <= SAME_M
            };
            let mut j = 0;
            while meets(j + 1) {
                j += 1;
            }
            (j > 0).then_some((apex - j, apex + j))
        })
}

struct OnCourse {
    here: Cursor,
    before_m: f32,
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

    fn left_at(
        &self,
        spur: &Stretch,
        (len, to): (usize, usize),
        at: &impl Fn(usize) -> (f32, f32),
    ) -> Option<(f32, usize)> {
        let corner = |j: usize| {
            let ways = way(at(j - 1), at(j)).zip(way(at(j), at(j + 1)));
            ways.is_some_and(|(into, on)| fabsf(swing(into, on)) >= BEND_MIN_DEG)
        };
        (spur.tip.next..to.min(len - 1))
            .rev()
            .take_while(|&j| distance(at(j), spur.rejoin.here) <= PAST_BASE_M)
            .filter(|&j| corner(j))
            .find_map(|j| {
                let (off, m) = self.nearest(at(j), at);
                (off <= SAME_M).then_some((m, j))
            })
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

#[cfg(test)]
mod tests;
