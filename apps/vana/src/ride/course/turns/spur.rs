//! Out-and-back spurs: a planner's waypoint just off the road snaps the
//! route into a side street and straight back along it. A rider riding on
//! past it would hear a turn, a U-turn and a turn, none of which they take.
//!
//! A spur is cut only where the course is sure to be one. Anywhere else the
//! course is read as drawn, as it was before spurs were found. So at each
//! spur the course says either what it would say with the spur's path taken
//! out, or what it said before spurs were cut: never a turn neither says,
//! and never without one both say.
//!
//! `Spurs` reads the course as it is, ahead of the scan, the scan's own
//! chord a step at a time. Where that chord swings by `REVERSAL_DEG` over
//! `REVERSAL_STEPS`, or `WIDE_REVERSAL_DEG` over `WIDE_REVERSAL_STEPS`, the
//! course has turned back on itself. From there its way back is walked a
//! metre at a time while it stays within `CLOSE_M` of its way out, read
//! without the spurs already found, as a planner's way back retraces its
//! way out. Where it came beside the point of the way out furthest back is
//! the spur's base. The spur is cut when it goes `SPUR_MIN_M` to `SPUR_M`
//! from its base, the way on is `PARTED_M` off the way out `RAY_M` on, and
//! the way in, the spur and the way on each leave the base `APART_DEG` or
//! more from the others at each of `RAYS_M` along. A way back that drifts
//! off, a fork too narrow to tell from the road, and a way on back along the
//! way in, as at the far end of an out-and-back, each fail this. Where the
//! course turns back again during a walk, it is walked in turn once that
//! walk ends, so spurs one after another, and a spur off a spur's way out,
//! are each found.
//!
//! A walk whose spur is not sure walks on while the way back stays within
//! `LOOSE_M` of the way out, and no further than `SPUR_M`, to find the whole
//! stretch that may or may not be a spur.
//!
//! A course with a spur cut says what the course would say with the spur's
//! path taken out, each turn after the spur as much further on as the spur
//! is long. That course runs along the way in to the base on the way out,
//! then on from where the way back came beside it. The scan reads it
//! through `Spurs::at` (`Spurs::skip`), far enough behind `Spurs` that every
//! spur before what it reads is found, and its turns are moved onto the
//! course by `Spurs::real`. The course itself keeps every point of a spur,
//! and its length.
use super::{Cursor, HALF_CHORD_M, STEP_M, Turn, swing, way};
use crate::ride::geo::{distance, mm};
use crate::ride::thin::off_segment;
use libm::{atan2f, fabsf, sqrtf};

const WAY_OUT_M: f32 = SPUR_M + 2.0 * WAY_M;
const HELD_M: f32 = WAY_OUT_M + UNCUT_REACH_M;
const UNCUT_REACH_M: f32 = LOOSE_M + ALONE_M + 2.0 * SPUR_M;
const REVERSAL_DEG: f32 = 120.0;
const REVERSAL_STEPS: usize = 3;
/// A dead end whose way back comes back metres to the side of its way out
/// swings the chord round more slowly.
const WIDE_REVERSAL_DEG: f32 = 160.0;
const WIDE_REVERSAL_STEPS: usize = 8;
const CLOSE_M: f32 = 1.0;
const LOOSE_M: f32 = 4.0;
pub(super) const ALONE_M: f32 = 250.0;
const END_SLACK_M: f32 = LOOSE_M;
const PARTED_M: f32 = 2.0 * CLOSE_M;
const SPUR_MIN_M: f32 = 13.0;
const SPUR_M: f32 = 150.0;
const RAYS_M: [f32; 4] = [1.5, 3.0, 6.0, RAY_M];
const RAY_M: f32 = 10.0;
const APART_DEG: f32 = 30.0;
/// The way into a spur and on from it is the chord this long.
const WAY_M: f32 = 2.0 * HALF_CHORD_M;
const WALK_M: f32 = 1.0;
const FURTHER_BACK_M: f32 = 0.05;
/// Spurs the scan reads past at once; one more found before it is past
/// the first is ridden as drawn.
const SKIPS: usize = 4;
const REVERSALS: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Stretch {
    base_m: f32,
    rejoin: Cursor,
    rejoin_m: f32,
}

#[derive(Clone, Copy, Debug)]
enum Ended {
    Spur(Stretch),
    Unsure(Span),
}

#[derive(Clone, Copy, Debug, Default)]
enum Walk {
    #[default]
    None,
    /// The course turned back at `tip_m`, the point `tip`; `back` has
    /// walked `back_m` of its way back.
    Back {
        tip_m: f32,
        tip: Cursor,
        back: Cursor,
        back_m: f32,
        step_end_m: f32,
        furthest_back: Stretch,
        left_m: Option<f32>,
        loose: Option<Span>,
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
            furthest_back: Stretch {
                base_m: tip_m,
                rejoin: tip,
                rejoin_m: tip_m,
            },
            left_m: None,
            loose: None,
        }
    }

    /// Walk on as far as `read_m`, beside the way out from `trail`,
    /// `trail_m` along, read without `skips`.
    fn on(
        &mut self,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
        (trail, trail_m, skips): (&Cursor, f32, &[Skip]),
        read_m: f32,
    ) -> Option<Ended> {
        let Self::Back {
            tip_m,
            tip,
            mut back,
            mut back_m,
            mut step_end_m,
            mut furthest_back,
            mut left_m,
            mut loose,
        } = *self
        else {
            return None;
        };
        let way_out = WayOut {
            trail,
            trail_m,
            skips,
            tip: &tip,
        };
        *self = Self::None;
        let far_end = Some(Ended::Unsure(Span {
            from_m: tip_m,
            to_m: tip_m,
        }));
        while tip_m + back_m + WALK_M <= read_m {
            let to_end = step_end_m - back_m;
            let step = back.stride(len, at, to_end);
            if !back.on(len, at, step) {
                let from_m = loose.map_or(furthest_back.base_m, |loose| loose.from_m);
                let to_m = tip_m + back_m;
                return Some(Ended::Unsure(Span { from_m, to_m }));
            }
            back_m += step;
            if step >= to_end {
                (back_m, step_end_m) = (step_end_m, step_end_m + WALK_M);
            }
            if back_m > SPUR_M {
                return far_end;
            }
            if let Some(loose) = &mut loose {
                let (off, along_m) = way_out.nearest(back.here, at);
                if off > LOOSE_M {
                    return Some(Ended::Unsure(*loose));
                }
                let walked_m = tip_m + back_m - loose.to_m;
                if further_back(loose.from_m, walked_m, LOOSE_M).contains(&along_m) {
                    *loose = Span {
                        from_m: along_m,
                        to_m: tip_m + back_m,
                    };
                }
                continue;
            }
            if let Some(left_m) = left_m {
                if back_m - left_m < RAY_M {
                    continue;
                }
                let parted = way_out.nearest(back.here, at).0 >= PARTED_M;
                if parted && way_out.sure(&furthest_back, len, at) {
                    return Some(Ended::Spur(furthest_back));
                }
                loose = Some(Span::of(&furthest_back));
                continue;
            }
            let (off, along_m) = way_out.nearest(back.here, at);
            let walked_m = tip_m + back_m - furthest_back.rejoin_m;
            let further_back = further_back(furthest_back.base_m, walked_m, WAY_M);
            if off > CLOSE_M {
                if trail_m > 0.0 && along_m - trail_m < WALK_M {
                    return far_end;
                }
                left_m = Some(back_m);
            } else if further_back.contains(&along_m)
                && along_m < furthest_back.base_m - FURTHER_BACK_M
            {
                furthest_back = Stretch {
                    base_m: along_m,
                    rejoin: back,
                    rejoin_m: tip_m + back_m,
                };
            }
        }
        *self = Self::Back {
            tip_m,
            tip,
            back,
            back_m,
            step_end_m,
            furthest_back,
            left_m,
            loose,
        };
        None
    }
}

/// A spur cut from `from_m` along the course, at `out`, to `rejoin_m`, at
/// `back`: points `first` to before `last` read as one of them.
#[derive(Clone, Copy, Debug, Default)]
struct Skip {
    first: usize,
    last: usize,
    out: (f32, f32),
    back: (f32, f32),
    from_m: f32,
    rejoin_m: f32,
    cut_from_m: f32,
    cut_m: f32,
}

impl Skip {
    fn back_from(&self) -> usize {
        (self.first + 1).min(self.last - 1)
    }
}

fn skip_of(skips: &[Skip], k: usize) -> Option<&Skip> {
    skips
        .iter()
        .find(|skip| (skip.first..skip.last).contains(&k))
}

/// Where a turn starting `start_mm` along the course as read starts on the
/// course, `onto` it. One that starts a step or less past a spur's base
/// turns at the base, as the course runs into the spur.
pub(crate) fn starts_at(start_mm: u32, onto: impl Fn(u32) -> u32) -> u32 {
    let step_mm = mm(STEP_M);
    onto(start_mm).min(onto(start_mm.saturating_sub(step_mm)) + step_mm)
}

/// Point `k` of the course read without `skips`, `at` the course.
fn read(skips: &[Skip], at: &impl Fn(usize) -> (f32, f32), k: usize) -> (f32, f32) {
    match skip_of(skips, k) {
        Some(skip) if k < skip.back_from() => skip.out,
        Some(skip) => skip.back,
        None => at(k),
    }
}

/// Metres of course cut between point `k` and the one read before it.
fn cut_at(skips: &[Skip], k: usize) -> f32 {
    skip_of(skips, k)
        .filter(|skip| k == skip.back_from())
        .map_or(0.0, |skip| skip.cut_m)
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
    chord_swings: [f32; WIDE_REVERSAL_STEPS],
    way_out: Cursor,
    way_out_m: f32,
    held: Cursor,
    held_m: f32,
    walk: Walk,
    /// Where the course turned back while a spur was walked, and the point
    /// there: each is walked in turn once the walk before it ends.
    reversals: [(f32, Cursor); REVERSALS],
    reversed: usize,
    skips: [Skip; SKIPS],
    count: usize,
    cluster: Option<Cluster>,
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
        self.held.next
    }

    pub(crate) fn done(&self) -> bool {
        self.done
    }

    pub(super) fn found_to(&self, m: f32) -> bool {
        self.done || m <= self.held_m
    }

    pub(crate) fn oldest(&self) -> usize {
        self.behind.next.min(self.held.next).saturating_sub(1)
    }

    pub(crate) fn step(&mut self, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        if self.done {
            return;
        }
        if !self.started {
            self.started = true;
            self.behind = Cursor::start(at);
            (self.ahead, self.way_out, self.held) = (self.behind, self.behind, self.behind);
            self.done = !self.ahead.on(len, at, WAY_M);
            self.s = HALF_CHORD_M;
            self.heading = way(self.behind.here, self.ahead.here);
            return;
        }
        if !self.behind.on(len, at, STEP_M) || !self.ahead.on(len, at, STEP_M) {
            self.walk_on(f32::INFINITY, len, at);
            self.done = true;
            return;
        }
        self.s += STEP_M;
        self.walk_on(self.s + HALF_CHORD_M, len, at);
        let Some(now) = way(self.behind.here, self.ahead.here) else {
            return;
        };
        let turned = self.heading.map_or(0.0, |heading| swing(heading, now));
        self.heading = Some(now);
        self.chord_swings.copy_within(1.., 0);
        self.chord_swings[WIDE_REVERSAL_STEPS - 1] = turned;
        let swung = |steps: usize| {
            let swings = &self.chord_swings[WIDE_REVERSAL_STEPS - steps..];
            fabsf(swings.iter().sum::<f32>())
        };
        if swung(REVERSAL_STEPS) < REVERSAL_DEG && swung(WIDE_REVERSAL_STEPS) < WIDE_REVERSAL_DEG {
            return;
        }
        self.chord_swings = [0.0; WIDE_REVERSAL_STEPS];
        let mut tip = self.behind;
        tip.on(len, at, HALF_CHORD_M);
        match self.walk {
            Walk::None => self.walk = Walk::reversed(self.s, tip),
            Walk::Back { .. } if self.reversed < REVERSALS => {
                self.reversals[self.reversed] = (self.s, tip);
                self.reversed += 1;
            }
            Walk::Back { .. } => {}
        }
    }

    fn walk_on(&mut self, read_m: f32, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        loop {
            if matches!(self.walk, Walk::None) {
                if self.reversed == 0 {
                    self.follow(self.s, len, at);
                    return;
                }
                let (tip_m, tip) = self.reversals[0];
                self.reversals.copy_within(1..self.reversed, 0);
                self.reversed -= 1;
                self.follow(tip_m, len, at);
                self.walk = Walk::reversed(tip_m, tip);
            }
            let skips = &self.skips[..self.count];
            let way_out = (&self.way_out, self.way_out_m, skips);
            let ended = match self.walk.on(len, at, way_out, read_m) {
                Some(Ended::Spur(spur)) => Some((Span::of(&spur), Some(spur))),
                Some(Ended::Unsure(span)) => Some((span, None)),
                None => None,
            };
            if let Some((span, spur)) = ended {
                if let Some(cluster) = self.join(span, spur.is_some()) {
                    self.uncut_near(cluster);
                } else if let Some(spur) = spur {
                    self.skip(spur, len, at);
                    let within = self.reversals[..self.reversed]
                        .iter()
                        .take_while(|(tip_m, _)| *tip_m <= spur.rejoin_m)
                        .count();
                    self.reversals.copy_within(within..self.reversed, 0);
                    self.reversed -= within;
                }
            }
            if matches!(self.walk, Walk::Back { .. }) {
                return;
            }
        }
    }

    /// The cluster of spurs found before it that a spur found at `span`,
    /// sure or not, joins. A sure spur takes the unsure ones within it.
    fn join(&mut self, span: Span, sure: bool) -> Option<Span> {
        let taken = |cluster: &Cluster| sure && !cluster.any_sure && span.takes(&cluster.span);
        let joined = self
            .cluster
            .filter(|cluster| cluster.span.near(&span) && !taken(cluster));
        let cluster = match joined {
            Some(cluster) => Cluster {
                span: Span {
                    from_m: span.from_m.min(cluster.span.from_m),
                    to_m: span.to_m.max(cluster.span.to_m),
                },
                any_sure: sure || cluster.any_sure,
            },
            None => Cluster {
                span,
                any_sure: sure,
            },
        };
        self.cluster = Some(cluster);
        joined.map(|_| cluster.span)
    }

    fn uncut_near(&mut self, cluster: Span) {
        while let Some(skip) = self.skips[..self.count].last() {
            let cut = Span {
                from_m: skip.from_m,
                to_m: skip.rejoin_m,
            };
            if skip.from_m < self.held_m || !cut.near(&cluster) {
                break;
            }
            self.count -= 1;
        }
    }

    fn follow(&mut self, front_m: f32, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        let skips = &self.skips[..self.count];
        let bring = |cursor: &mut Cursor, cursor_m: &mut f32, back_m: f32| {
            let mut lag = front_m - back_m;
            for skip in skips.iter().rev() {
                if skip.rejoin_m > lag {
                    lag -= skip.rejoin_m - skip.from_m;
                }
            }
            if lag > *cursor_m && cursor.on(len, at, lag - *cursor_m) {
                *cursor_m = lag;
            }
        };
        bring(&mut self.way_out, &mut self.way_out_m, WAY_OUT_M);
        bring(&mut self.held, &mut self.held_m, HELD_M);
    }

    fn on_course(&self, m: f32, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> OnCourse {
        let mut here = self.way_out;
        here.on(len, at, m - self.way_out_m);
        OnCourse {
            before_m: m - distance(at(here.next - 1), here.here),
            here,
        }
    }

    fn skip(&mut self, spur: Stretch, len: usize, at: &impl Fn(usize) -> (f32, f32)) {
        #[cfg(test)]
        if tests::UNCUT.get() {
            return;
        }
        let out = self.on_course(spur.base_m, len, at);
        let (back, base_m, rejoin_m) = (spur.rejoin, spur.base_m, spur.rejoin_m);
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
            from_m,
            rejoin_m,
            cut_from_m: from_m - self.passed_cut_m - cut_m,
            cut_m: rejoin_m - from_m - distance(from, back.here),
        };
        self.count += 1;
    }

    /// Point `k` of the course as the scan reads it, `at` the course.
    pub(crate) fn at(&self, at: &impl Fn(usize) -> (f32, f32), k: usize) -> (f32, f32) {
        read(&self.skips[..self.count], at, k)
    }

    pub(crate) fn cut_at(&self, k: usize) -> f32 {
        cut_at(&self.skips[..self.count], k)
    }

    /// Whether the course read without its spurs has no point `k`.
    pub(crate) fn skipped(&self, k: usize) -> bool {
        skip_of(&self.skips[..self.count], k).is_some()
    }

    /// `turn`, found on the course as the scan reads it, on the course.
    pub(super) fn real(&self, turn: Turn) -> Turn {
        let real = |read_mm: u32| mm(self.real_m(read_mm as f32 / 1000.0));
        Turn {
            start_mm: starts_at(turn.start_mm, real),
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

#[derive(Clone, Copy, Debug)]
struct Span {
    from_m: f32,
    to_m: f32,
}

impl Span {
    fn of(spur: &Stretch) -> Self {
        Self {
            from_m: spur.base_m,
            to_m: spur.rejoin_m,
        }
    }

    fn near(&self, other: &Span) -> bool {
        let near_m = ALONE_M - END_SLACK_M;
        self.from_m <= other.to_m + near_m && other.from_m - near_m <= self.to_m
    }

    fn takes(&self, other: &Span) -> bool {
        self.from_m <= other.from_m && other.to_m <= self.to_m
    }
}

#[derive(Clone, Copy, Debug)]
struct Cluster {
    span: Span,
    any_sure: bool,
}

struct OnCourse {
    here: Cursor,
    before_m: f32,
}

/// The course from `trail`, `trail_m` along, to the point `tip`, read
/// without `skips`.
struct WayOut<'a> {
    trail: &'a Cursor,
    trail_m: f32,
    skips: &'a [Skip],
    tip: &'a Cursor,
}

#[derive(Clone, Copy)]
struct Segment {
    a: (f32, f32),
    b: (f32, f32),
    a_m: f32,
    along: f32,
    read_m: f32,
}

impl WayOut<'_> {
    fn each(&self, at: &impl Fn(usize) -> (f32, f32), mut segment: impl FnMut(Segment)) {
        let (mut a, mut a_m, mut read_m) = (self.trail.here, self.trail_m, 0.0);
        let points = (self.trail.next..self.tip.next)
            .map(|k| (read(self.skips, at, k), cut_at(self.skips, k)));
        for (b, cut_m) in points.chain(core::iter::once((self.tip.here, 0.0))) {
            let length = distance(a, b);
            let along = if cut_m > 0.0 { 0.0 } else { length };
            segment(Segment {
                a,
                b,
                a_m,
                along,
                read_m,
            });
            (a, a_m, read_m) = (b, a_m + length + cut_m, read_m + length);
        }
    }

    /// How far `p` is from it, and metres along to its point nearest `p`.
    fn nearest(&self, p: (f32, f32), at: &impl Fn(usize) -> (f32, f32)) -> (f32, f32) {
        let mut nearest = (f32::INFINITY, self.trail_m);
        self.each(
            at,
            |Segment {
                 a, b, a_m, along, ..
             }| {
                let off = off_segment(p, a, b);
                if off <= nearest.0 {
                    let to_foot = sqrtf((distance(a, p) * distance(a, p) - off * off).max(0.0));
                    nearest = (off, a_m + to_foot.min(along));
                }
            },
        );
        nearest
    }

    fn point(&self, m: f32, by: f32, at: &impl Fn(usize) -> (f32, f32)) -> (f32, f32) {
        let mut to_m = None;
        self.each(at, |segment| {
            if to_m.is_none() && segment.a_m + segment.along >= m {
                to_m = Some(segment.read_m + (m - segment.a_m).clamp(0.0, segment.along) + by);
            }
        });
        let to_m = to_m.unwrap_or(f32::INFINITY);
        let mut point = self.trail.here;
        self.each(at, |Segment { a, b, read_m, .. }| {
            let length = distance(a, b);
            if to_m >= read_m {
                let t = if length > 0.0 {
                    ((to_m - read_m) / length).min(1.0)
                } else {
                    0.0
                };
                point = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
            }
        });
        point
    }

    fn sure(&self, spur: &Stretch, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> bool {
        let base = self.point(spur.base_m, 0.0, at);
        let parted = |&ray_m: &f32| {
            let mut on = spur.rejoin;
            let way_on = on
                .on(len, at, ray_m)
                .then(|| bearing(spur.rejoin.here, on.here))?;
            let way_in = bearing(base, self.point(spur.base_m, -ray_m, at));
            let out = bearing(base, self.point(spur.base_m, ray_m, at));
            let apart = |a: f32, b: Option<f32>| b.is_none_or(|b| fabsf(swing(a, b)) >= APART_DEG);
            Some(apart(out?, way_in) && apart(out?, way_on) && apart(way_on?, way_in))
        };
        (SPUR_MIN_M..=SPUR_M).contains(&self.deepest(spur.base_m, at))
            && RAYS_M.iter().all(|ray_m| parted(ray_m) == Some(true))
    }

    /// How far from its point `base_m` along it goes, on to the tip.
    fn deepest(&self, base_m: f32, at: &impl Fn(usize) -> (f32, f32)) -> f32 {
        let base = self.point(base_m, 0.0, at);
        let mut deepest = 0.0f32;
        self.each(at, |Segment { b, a_m, along, .. }| {
            if a_m + along >= base_m {
                deepest = deepest.max(distance(base, b));
            }
        });
        deepest
    }
}

/// Where the way back, `walked_m` on from coming beside the way out
/// `from_m` along it, may come beside it further back: no further than it
/// has walked, give or take `slack_m`.
fn further_back(from_m: f32, walked_m: f32, slack_m: f32) -> core::ops::Range<f32> {
    from_m - walked_m - slack_m..from_m
}

fn bearing(a: (f32, f32), b: (f32, f32)) -> Option<f32> {
    (distance(a, b) >= 1.0).then(|| atan2f(b.0 - a.0, b.1 - a.1).to_degrees())
}

#[cfg(test)]
mod tests;
