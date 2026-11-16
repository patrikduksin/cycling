//! Out-and-back spurs: a planner's waypoint just off the road snaps the
//! route into a side street and straight back along it. A rider riding on
//! past it would hear a turn, a U-turn and a turn, none of which they take.
//!
//! `Spurs` reads the course as it is, ahead of the scan, the scan's own
//! chord a step at a time. Where that chord swings by `REVERSAL_DEG` in one
//! step, the course has turned back on itself. From there its way back is
//! walked a metre at a time while it stays within `RETRACE_M` of its way
//! out. Ending within `SPUR_M` of the tip, it is a spur from the point of
//! the way out furthest back that it came beside to where it did. Retracing
//! further, the
//! course is a longer out-and-back, whose legs' corners the rider is told;
//! but a dead end at its far end, `TIP_SPUR_M` or less past the last corner,
//! is a spur from that corner. Either way the way out must go at least
//! `SPUR_MIN_M` from the spur's base, as a stop's jitter never does. A
//! hairpin's legs part too soon and roads side by side are too far apart to
//! be one. So a dead end visited no deeper than `SPUR_M` is taken for a
//! planner's artifact.
//!
//! A course with a spur says exactly what the course would say with the
//! spur's out-and-back taken out, each turn after the spur as much further
//! on as the spur is long. The scan reads the course through `Spurs::at`,
//! as though each spur's points were taken out of it (`Spurs::skip`). The
//! scan reads far enough behind `Spurs` that every
//! spur before what it reads is found, and its turns are moved onto the
//! course by `Spurs::real`. The course itself keeps every point of a spur,
//! and its length.
use super::{Cursor, HALF_CHORD_M, STEP_M, TURN_DEG, Turn, distance, mm, swing, way};
use crate::ride::thin::off_segment;
use libm::{fabsf, sqrtf};

/// `Spurs` keeps this far of the course behind its chord's middle: a
/// spur's whole way out and the way in before it.
const BACK_M: f32 = SPUR_M + 2.0 * WAY_M;
const REVERSAL_DEG: f32 = 90.0;
const RETRACE_M: f32 = 3.0;
const SPUR_MIN_M: f32 = 10.0;
const SPUR_M: f32 = 150.0;
const TIP_SPUR_M: f32 = 2.0 * WAY_M;
/// The way into a spur and on from it is the chord this long.
const WAY_M: f32 = 2.0 * HALF_CHORD_M;
const WALK_M: f32 = 1.0;
/// Spurs the scan reads past at once; one more found before it is past
/// the first is ridden as drawn.
const SKIPS: usize = 4;

/// A spur's base, metres along, and where its way back last lies beside
/// it, `rejoin_m` along.
#[derive(Clone, Copy, Debug)]
struct Stretch {
    base_m: f32,
    rejoin_m: f32,
    after_tip: usize,
}

#[derive(Clone, Copy, Debug, Default)]
enum Walk {
    #[default]
    None,
    /// The course turned back at `tip_m`, the point `tip`; `back` has
    /// walked `back_m` of its way back, whose last metre lies beside the
    /// way out `beside_m` along. `corner` is the spur past the furthest
    /// corner the way out turned back from the tip.
    Back {
        tip_m: f32,
        tip: Cursor,
        back: Cursor,
        back_m: f32,
        beside_m: f32,
        furthest_back: Stretch,
        corner: Option<Stretch>,
    },
}

impl Walk {
    fn reversed(tip_m: f32, tip: Cursor) -> Self {
        Self::Back {
            tip_m,
            tip,
            back: tip,
            back_m: 0.0,
            beside_m: tip_m,
            furthest_back: Stretch {
                base_m: tip_m,
                rejoin_m: tip_m,
                after_tip: tip.next,
            },
            corner: None,
        }
    }

    /// Walk on as far as `read_m`, beside the way out from `trail`,
    /// `trail_m` along: the spur, once its way back has left the way out.
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
            mut beside_m,
            mut furthest_back,
            mut corner,
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
            if !back.on(len, at, WALK_M) {
                return None;
            }
            if back_m + WALK_M > SPUR_M {
                return corner.filter(deep);
            }
            back_m += WALK_M;
            let (off, along_m) = way_out.nearest(back.here, at);
            if off > RETRACE_M {
                return Some(furthest_back).filter(deep);
            }
            beside_m = along_m;
            if beside_m < furthest_back.base_m {
                furthest_back = Stretch {
                    base_m: beside_m,
                    rejoin_m: tip_m + back_m,
                    after_tip: tip.next,
                };
            }
            let past_corner = (SPUR_MIN_M..=TIP_SPUR_M).contains(&(tip_m - beside_m));
            if past_corner && way_out.turns_at(beside_m, at) {
                corner = Some(Stretch {
                    base_m: beside_m,
                    rejoin_m: tip_m + back_m,
                    after_tip: tip.next,
                });
            }
        }
        *self = Self::Back {
            tip_m,
            tip,
            back,
            back_m,
            beside_m,
            furthest_back,
            corner,
        };
        None
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Skip {
    first: usize,
    last: usize,
    base: (f32, f32),
    /// Metres along the course as the scan reads it to the base, and how
    /// much less that is than along the course past the spur.
    base_m: f32,
    cut_m: f32,
}

/// Finds a course's spurs ahead of the scan, and reads the course to the
/// scan without them.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Spurs {
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
    /// Metres along the course to the middle of its chord; it has read the
    /// course half a chord beyond.
    pub(super) fn along_m(&self) -> f32 {
        self.s.max(HALF_CHORD_M)
    }

    pub(super) fn found_to(&self, m: f32) -> bool {
        self.done || m <= self.trail_m
    }

    pub(super) fn oldest(&self) -> usize {
        self.behind.next.min(self.trail.next).saturating_sub(1)
    }

    pub(super) fn step(&mut self, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
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

    fn around(&self, m: f32, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> Around {
        let mut on = self.trail;
        on.on(len, at, m - self.trail_m);
        let k = on.next - 1;
        Around {
            at_or_before: Placed {
                k,
                m: m - distance(at(k), on.here),
            },
            after: (on.next < len).then(|| Placed {
                k: on.next,
                m: m + distance(on.here, at(on.next)),
            }),
        }
    }

    /// The points either side of `spur`'s points, for the scan to read
    /// straight from one to the other, if there are any.
    fn ends(
        &self,
        spur: Stretch,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
    ) -> Option<(Placed, Placed)> {
        let out = self.around(spur.base_m, len, at).at_or_before;
        let back = self.around(spur.rejoin_m, len, at);
        let last = back.after.unwrap_or(back.at_or_before);
        let through_its_points = mirrored(at, spur.after_tip, (out, last))
            .filter(|(base, _)| base.m <= spur.base_m + WAY_M)
            .map(|meet| widened(at, meet, (out, last)));
        let back_beside_base = (distance(at(out.k), at(back.at_or_before.k)) <= RETRACE_M)
            .then_some((out, back.at_or_before));
        let past_the_retrace = back.after.map(|past| (out, past));
        through_its_points.or(back_beside_base).or(past_the_retrace)
    }

    fn skip(&mut self, spur: Stretch, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        let Some((base, rejoin)) = self.ends(spur, len, at) else {
            return;
        };
        let trail = Cursor {
            next: rejoin.k + 1,
            here: at(rejoin.k),
        };
        (self.trail, self.trail_m) = (trail, rejoin.m);
        if self.count >= SKIPS || rejoin.k <= base.k + 1 {
            return;
        }
        let cut_m: f32 = self.skips[..self.count].iter().map(|skip| skip.cut_m).sum();
        self.skips[self.count] = Skip {
            first: base.k + 1,
            last: rejoin.k,
            base: at(base.k),
            base_m: base.m - self.passed_cut_m - cut_m,
            cut_m: rejoin.m - base.m - distance(at(base.k), at(rejoin.k)),
        };
        self.count += 1;
    }

    /// Point `k` of the course as the scan reads it, `at` the course.
    pub(super) fn at(&self, at: &impl Fn(usize) -> (f32, f32), k: usize) -> (f32, f32) {
        let skips = &self.skips[..self.count];
        match skips
            .iter()
            .find(|skip| (skip.first..skip.last).contains(&k))
        {
            Some(skip) => skip.base,
            None => at(k),
        }
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
            .filter(|skip| read_m > skip.base_m)
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
    pub(super) fn passed(&mut self, oldest: usize) {
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

#[derive(Clone, Copy, Debug)]
struct Around {
    at_or_before: Placed,
    after: Option<Placed>,
}

#[derive(Clone, Copy, Debug)]
struct Placed {
    k: usize,
    m: f32,
}

impl Placed {
    fn after(self, at: &impl Fn(usize) -> (f32, f32)) -> Self {
        Self {
            k: self.k + 1,
            m: self.m + distance(at(self.k), at(self.k + 1)),
        }
    }

    fn before(self, at: &impl Fn(usize) -> (f32, f32)) -> Self {
        Self {
            k: self.k - 1,
            m: self.m - distance(at(self.k - 1), at(self.k)),
        }
    }
}

/// The outermost pair of points either side of the point the course turned
/// back at, `after_tip` or the one before, between which every pair at the
/// same count from it are one point, within `(out, last)`.
fn mirrored(
    at: &impl Fn(usize) -> (f32, f32),
    after_tip: usize,
    (out, last): (Placed, Placed),
) -> Option<(Placed, Placed)> {
    [after_tip.saturating_sub(1), after_tip]
        .into_iter()
        .find_map(|tip| {
            let meets =
                |j: usize| tip >= out.k + j && tip + j <= last.k && at(tip - j) == at(tip + j);
            let mut j = 0;
            while meets(j + 1) {
                j += 1;
            }
            (j > 0).then(|| {
                let mut base = out;
                while base.k < tip - j {
                    base = base.after(at);
                }
                let mut rejoin = last;
                while rejoin.k > tip + j {
                    rejoin = rejoin.before(at);
                }
                (base, rejoin)
            })
        })
}

/// `(base, rejoin)` moved out a point at a time, as far as `(out, last)`,
/// while the points either side lie within `RETRACE_M` of each other.
fn widened(
    at: &impl Fn(usize) -> (f32, f32),
    (mut base, mut rejoin): (Placed, Placed),
    (out, last): (Placed, Placed),
) -> (Placed, Placed) {
    while base.k > out.k
        && rejoin.k < last.k
        && distance(at(base.k - 1), at(rejoin.k + 1)) <= RETRACE_M
    {
        (base, rejoin) = (base.before(at), rejoin.after(at));
    }
    (base, rejoin)
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

#[cfg(test)]
mod tests;
