//! Turns on a course, and when to warn of the next one while following it.
//!
//! The course is read every `STEP_M` along it from the rider, never at its
//! own points, so a corner drawn with points 3 m or 8 m apart, or thinned
//! (`ride::thin`) to a few, turns the same. At each step the course's way is
//! the chord from `HALF_CHORD_M` behind to `HALF_CHORD_M` ahead, which also
//! smooths a jittery track. Where the way swings by `RATE_DEG_PER_M` or more
//! the course is turning. A run of turning steps that swings less than
//! `BEND_MIN_DEG` is a wobble; bends less than `GAP_M` apart, or
//! `SAME_GAP_M` when they turn the same way or one is under `FLICK_DEG` (a
//! roundabout's way in and out), are one group, at most `SPAN_M` long. So a roundabout of up to about 40 m radius is one group, even
//! thinned, and so is a jog whose corners are less than about 25 m apart;
//! a road that never stops turning is named by its net way. Two turns that
//! start within `THEN_MM` of each other are warned of together.
//! A group's angle is how far the way swung across it, and it is a turn only
//! from `TURN_DEG`: a roundabout says where it leaves, and straight over one,
//! a short jog or a long gentle bend is no turn.
//!
//! The course is read by index through `at`, start first, in metres east and
//! north, so nothing is copied or kept. A scan reaches at most `SAMPLES`
//! steps and walks at most `WALK_POINTS` points, so a course of thousands of
//! points in one spot costs no more than a road. Distances along the course
//! are u32 millimetres, saturating.
use libm::{atan2f, fabsf, sqrtf};

/// The course is read this often along it, at whole steps from its start,
/// so a turn is found at the same place from wherever the rider is.
const STEP_M: f32 = 2.0;
const STEP_MM: u32 = 2_000;
/// The course's way at a step is the chord this far behind to this far on.
const HALF_CHORD_M: f32 = 7.5;
/// The way swinging this fast is turning: 60 degrees within 50 m.
const RATE_DEG_PER_M: f32 = 1.2;
/// Turning steps this close are one bend, and a bend that swings less than
/// this is a wobble, not part of a turn.
const BEND_GAP_M: f32 = 4.0;
const BEND_MIN_DEG: f32 = 20.0;
/// A bend against the way of the one before, and less than this, may be a
/// roundabout's way in or out: a thinned one swings up to about 65 degrees.
const FLICK_DEG: f32 = 75.0;
/// Bends this close are one group, and this close when they turn the same
/// way or one of them is a flick, at most this long. A course thinned to fit draws a
/// roundabout as a polygon of sides up to about 35 m, which turns only near
/// its corners.
const GAP_M: f32 = 12.0;
const SAME_GAP_M: f32 = 22.0;
const SPAN_M: f32 = 300.0;
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
/// A turn starting this soon after the start of another is named with it:
/// LEFT THEN RIGHT.
const THEN_MM: u32 = 35_000;
/// The rider this far back from where they were has turned round; what was
/// warned of is forgotten.
const REWIND_MM: u32 = 30_000;
/// The most steps a scan takes, and points each end of its chord walks.
const SAMPLES: usize = 250;
const WALK_POINTS: usize = 512;
/// The most groups one warning looks past.
const GROUPS: usize = 4;

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

/// Where the rider is: at `here`, on the segment that ends at point `next`,
/// `along_mm` from the course's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub next: usize,
    pub here: (f32, f32),
    pub along_mm: u32,
}

/// A stretch of the course that turns, `start_mm` to `end_mm` from its
/// start, `ahead_mm` on from the rider; a turn when it turns enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Option<Kind>,
    pub start_mm: u32,
    pub end_mm: u32,
    pub ahead_mm: u32,
    /// It was already turning where the scan began.
    pub under_way: bool,
}

/// What to tell the rider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    /// A turn about `metres` ahead, to the nearest 10, half down.
    Ahead { kind: Kind, metres: u32 },
    /// The turn is here, and `then` another right after it.
    Now { kind: Kind, then: Option<Kind> },
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (east, north) = (b.0 - a.0, b.1 - a.1);
    sqrtf(east * east + north * north)
}

/// A float cast saturates, so no length overflows.
fn mm(metres: f32) -> u32 {
    (metres * 1000.0) as u32
}

/// A point moving on along the course from the rider.
struct Walk<'a, F> {
    at: &'a F,
    len: usize,
    next: usize,
    here: (f32, f32),
    walked: usize,
}

impl<'a, F: Fn(usize) -> (f32, f32)> Walk<'a, F> {
    fn new(at: &'a F, len: usize, place: Place) -> Self {
        Self {
            at,
            len,
            next: place.next,
            here: place.here,
            walked: 0,
        }
    }

    /// Move `metres` on; false at the course's end or out of points.
    fn on(&mut self, mut metres: f32) -> bool {
        while metres > 0.0 {
            if self.next >= self.len || self.walked >= WALK_POINTS {
                return false;
            }
            let to = (self.at)(self.next);
            let step = distance(self.here, to);
            if step > metres {
                let t = metres / step;
                self.here = (
                    self.here.0 + t * (to.0 - self.here.0),
                    self.here.1 + t * (to.1 - self.here.1),
                );
                return true;
            }
            metres -= step;
            self.here = to;
            self.next += 1;
            self.walked += 1;
        }
        true
    }
}

/// The way from `a` to `b` in degrees clockwise from north, if they are far
/// enough apart to tell.
fn way(a: (f32, f32), b: (f32, f32)) -> Option<f32> {
    (distance(a, b) >= HALF_CHORD_M).then(|| atan2f(b.0 - a.0, b.1 - a.1).to_degrees())
}

/// The swing from way `from` to way `to`, right positive, -180 to 180.
fn swing(from: f32, to: f32) -> f32 {
    let d = to - from;
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// A run of turning steps, metres on from the rider, and the swing so far
/// before its first.
#[derive(Clone, Copy)]
struct Bend {
    start: f32,
    last: f32,
    before: f32,
    first_step: bool,
}

/// A group being found: from its first bend's start to its last bend's
/// last turning step, metres on from the rider; the swing so far before it,
/// and across it.
struct Open {
    start: f32,
    last: f32,
    before: f32,
    swung: f32,
    under_way: bool,
    right: bool,
}

/// The first group that starts `from_mm` or more from the course's start,
/// of a course of `len` points, while it starts within `reach_mm` of the
/// rider at `place`.
pub fn next(
    len: usize,
    at: impl Fn(usize) -> (f32, f32),
    place: Place,
    from_mm: u32,
    reach_mm: u32,
) -> Option<Group> {
    if place.next == 0 || place.next >= len {
        return None;
    }
    let (mut behind, mut ahead) = (Walk::new(&at, len, place), Walk::new(&at, len, place));
    let phase = (STEP_MM - place.along_mm % STEP_MM) % STEP_MM;
    let phase = phase as f32 / 1000.0;
    if !behind.on(phase) || !ahead.on(phase + 2.0 * HALF_CHORD_M) {
        return None;
    }
    let mut heading = way(behind.here, ahead.here)?;
    let mut s = phase + HALF_CHORD_M;
    let close = |open: &Open| {
        let start_mm = mm(open.start);
        Group {
            kind: Kind::of(open.swung),
            start_mm: place.along_mm.saturating_add(start_mm),
            end_mm: place.along_mm.saturating_add(mm(open.last)),
            ahead_mm: start_mm,
            under_way: open.under_way,
        }
    };
    // What a finished bend does to the group: start it, join it, or end
    // it, which ends the scan.
    enum Step {
        On,
        Done(Option<Group>),
    }
    let take = |bend: Bend, total: f32, open: &mut Option<Open>| {
        let swung = total - bend.before;
        if fabsf(swung) < BEND_MIN_DEG {
            return Step::On;
        }
        let right = swung > 0.0;
        match open {
            None => {
                let start_mm = place.along_mm.saturating_add(mm(bend.start));
                if start_mm < from_mm {
                    return Step::On;
                }
                if mm(bend.start) > reach_mm {
                    return Step::Done(None);
                }
                let from_rider = from_mm <= place.along_mm.saturating_add(mm(bend.last));
                *open = Some(Open {
                    start: bend.start,
                    last: bend.last,
                    before: bend.before,
                    swung,
                    under_way: bend.first_step && from_rider,
                    right,
                });
                Step::On
            }
            Some(group) => {
                let gap = bend.start - group.last;
                // A roundabout's way in and out swing against it, and less.
                let small = fabsf(swung) < FLICK_DEG || fabsf(group.swung) < FLICK_DEG;
                let near = gap <= GAP_M || (gap <= SAME_GAP_M && (right == group.right || small));
                if !near || bend.last - group.start > SPAN_M {
                    return Step::Done(Some(close(group)));
                }
                (group.last, group.swung, group.right) = (bend.last, total - group.before, right);
                Step::On
            }
        }
    };
    let (mut total, mut bend, mut open) = (0.0f32, None::<Bend>, None::<Open>);
    for step in 0..SAMPLES {
        if !behind.on(STEP_M) || !ahead.on(STEP_M) {
            break;
        }
        s += STEP_M;
        let Some(now) = way(behind.here, ahead.here) else {
            continue;
        };
        let turned = swing(heading, now);
        heading = now;
        total += turned;
        if fabsf(turned) >= RATE_DEG_PER_M * STEP_M {
            if let Some(long) = bend.filter(|bend| s - bend.start > SPAN_M) {
                bend = None;
                if let Step::Done(group) = take(long, total - turned, &mut open) {
                    return group;
                }
            }
            match &mut bend {
                Some(bend) => bend.last = s,
                None => {
                    bend = Some(Bend {
                        start: s - STEP_M,
                        last: s,
                        before: total - turned,
                        first_step: step == 0,
                    });
                }
            }
            continue;
        }
        match bend {
            Some(done) if s - done.last > BEND_GAP_M => {
                bend = None;
                if let Step::Done(group) = take(done, total, &mut open) {
                    return group;
                }
            }
            Some(_) => {}
            None => {
                if let Some(group) = open.as_ref().filter(|group| s - group.last > SAME_GAP_M) {
                    return Some(close(group));
                }
            }
        }
    }
    if let Some(done) = bend
        && let Step::Done(group) = take(done, total, &mut open)
    {
        return group;
    }
    open.as_ref().map(close)
}

/// The group last found ahead, so each turn is warned of once ahead and
/// once at it, a group is never taken for a turn from partway through, and
/// a rider who turns round hears the turns again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Warner {
    state: State,
    /// The furthest the rider has been along the course since.
    furthest_mm: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Quiet,
    /// Warned of ahead.
    Ahead { start_mm: u32, end_mm: u32 },
    /// Warned of at it; the next is looked for once the rider is past its
    /// start.
    Now { start_mm: u32, end_mm: u32 },
    /// Not a turn, or under way when found: what is after it is looked at.
    Skip { end_mm: u32 },
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
        let here = place.along_mm;
        if here.saturating_add(REWIND_MM) < self.furthest_mm {
            self.state = State::Quiet;
            self.furthest_mm = here;
        }
        self.furthest_mm = self.furthest_mm.max(here);
        let mut from = match self.state {
            State::Quiet => here,
            State::Now { start_mm, .. } if here < start_mm => return None,
            State::Ahead { start_mm, .. } if here <= start_mm => start_mm.saturating_sub(1_000),
            State::Ahead { end_mm, .. } | State::Now { end_mm, .. } | State::Skip { end_mm } => {
                end_mm.saturating_add(1)
            }
        };
        for _ in 0..GROUPS {
            let group = next(len, &at, place, from, AHEAD_MM)?;
            let (start_mm, end_mm) = (group.start_mm, group.end_mm);
            let Some(kind) = group.kind.filter(|_| !group.under_way) else {
                self.state = State::Skip { end_mm };
                from = end_mm.saturating_add(1);
                continue;
            };
            if group.ahead_mm <= NOW_MM {
                let reach = group.ahead_mm.saturating_add(THEN_MM);
                let after = next(len, &at, place, end_mm.saturating_add(1), reach)
                    .filter(|after| after.kind.is_some());
                self.state = State::Now {
                    start_mm,
                    end_mm: after.map_or(end_mm, |after| after.end_mm),
                };
                let then = after.and_then(|after| after.kind);
                return Some(Warning::Now { kind, then });
            }
            let said = matches!(self.state, State::Ahead { start_mm: s, .. } if s.abs_diff(start_mm) < 1_000);
            if group.ahead_mm < AHEAD_MIN_MM || said {
                return None;
            }
            self.state = State::Ahead { start_mm, end_mm };
            let metres = group.ahead_mm.saturating_add(4_999) / 10_000 * 10;
            return Some(Warning::Ahead { kind, metres });
        }
        None
    }
}

#[cfg(test)]
mod tests;
