//! Turns on a course, found once when the course is placed, and when to warn
//! of the next one while following it.
//!
//! `Scan` reads the course every `STEP_M` along it from its start, never at
//! its own points, so a corner drawn with points 3 m or 8 m apart turns the
//! same, and lists its turns by distance along (`Turn`). It runs a few steps
//! a fix (`SCAN_STEPS`), so a long course never holds up one. `Warner` then
//! looks only at the list and how far along the rider is: the turns cannot
//! move with the rider or with GPS jitter, and the furthest the rider has
//! been stands for where they are, so jitter never repeats or flips a word.
//!
//! At each step the course's way is the chord from `HALF_CHORD_M` behind to
//! `HALF_CHORD_M` ahead, which also smooths a jittery track. Where the way
//! swings by `RATE_DEG_PER_M` or more the course is turning; a run of turning
//! steps the same way is a bend, and one that never swings `BEND_MIN_DEG`
//! from where it began is a wobble.
//!
//! Bends make one group, at most `SPAN_M` long, and a group that turns by
//! `TURN_DEG` or more is a turn, named by its net way: a roundabout by the way
//! it leaves, and straight over one or a long gentle bend is no turn. A bend
//! of `FLICK_DEG` or more is a full corner, and two full corners are one turn
//! only as a hairpin's are, or as a roundabout's way in, ring and way out
//! are; otherwise each corner is a turn of its own, warned of in order. So:
//! bends the same way join only across as little straight as thinning
//! (`ride::thin`) leaves of a curve (`same_way_gap`); bends against each
//! other join within `GAP_M`, or `SAME_GAP_M` when one is less than a full
//! corner, and two full corners against each other only when one is an arc
//! (`ARC_M`) with a way out after it. A U-turn whose ways in and out are far
//! apart with a straight between is two corners the same way
//! (`two_corners`), as round a city block.
//!
//! A group whose word might send the rider the wrong way first says nothing
//! instead: one whose first full corner goes against its net way (unless it
//! is a roundabout), and coarsely drawn corners (`SPARSE_M`, as a course
//! thinned to fit or a planner's route is) within `CROWD_M` of each other.
//! Coarse bends that are not both full corners join within `SPARSE_GAP_M`,
//! as a thinned roundabout's do, and are named only roughly (`Kind::rough`).
//! Corners left unnamed hold back the warning of what is after them until
//! the rider is at them.
//!
//! The course is read by index through `at`, start first, in metres east and
//! north, so nothing of it is copied; the list holds at most `TURNS`.
//! Distances along the course are u32 millimetres, saturating.
use libm::{atan2f, fabsf, sqrtf, tanf};

/// The course is read this often along it, at whole steps from its start,
/// so a turn is found at the same place from wherever the rider is.
const STEP_M: f32 = 2.0;
/// The course's way at a step is the chord this far behind to this far on.
const HALF_CHORD_M: f32 = 7.5;
const HALF_CHORD_MM: u32 = 7_500;
/// The way swinging this fast is turning: 60 degrees within 50 m.
const RATE_DEG_PER_M: f32 = 1.2;
/// Turning steps this close are one bend, and a bend that never swings
/// this far from where it began is a wobble, not part of a turn.
const BEND_GAP_M: f32 = 4.0;
const BEND_MIN_DEG: f32 = 20.0;
/// A bend against the way of the one before, and less than this, may be a
/// roundabout's way in or out: a thinned one swings up to about 65 degrees.
const FLICK_DEG: f32 = 75.0;
/// Bends against each other this close are one group, and this close when
/// one of them is a flick.
const GAP_M: f32 = 12.0;
/// A bend this long turns all along, as a roundabout's ring does: a street
/// corner, even with a wide kerb, turns in less.
const ARC_M: f32 = 29.0;
const SAME_GAP_M: f32 = 22.0;
/// A course drawn with segments this long near a turn, as a planner's route
/// or a course thinned to fit is, shows a roundabout as a few corners with
/// long straights between, of either way: there, bends this close are one
/// group, whatever their way, and named only roughly.
const SPARSE_M: f32 = 20.0;
const SPARSE_GAP_M: f32 = 50.0;
/// A group is at most this long.
const SPAN_M: f32 = 300.0;
/// A turn turns by this much or more; a sharp one and a U-turn by more.
const TURN_DEG: f32 = 60.0;
const SHARP_DEG: f32 = 120.0;
const U_TURN_DEG: f32 = 150.0;
/// A turn of several bends is known only to some 30 degrees, as a thinned
/// roundabout's way in or out may fall outside it: it is a turn only from
/// this, never sharp, and a U-turn from this.
const ROUGH_TURN_DEG: f32 = 80.0;
const ROUGH_U_TURN_DEG: f32 = 160.0;
/// A U-turn whose ways in and out are farther apart than this, with this
/// much straight between, as the short chord sees it, is two corners.
const WIDE_M: f32 = 21.0;
const STRAIGHT_M: f32 = 5.0;
const STRAIGHT_DEG: f32 = 3.0;
const SHORT_HALF_M: f32 = 2.5;
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
/// Coarsely drawn corners closer than this, in one group or two, say
/// nothing: which way they go first is not known.
const CROWD_M: f32 = 10.0;
/// Bends one way adding up to this are a roundabout's ring.
const RING_DEG: f32 = 200.0;
/// A bend against a coarse turn this big may be a corner of its own.
const AGAINST_DEG: f32 = 45.0;

/// The most turns a course keeps; past them a long course warns of none.
pub const TURNS: usize = 1024;
/// Steps of the course scanned per fix while its turns are being found.
pub const SCAN_STEPS: usize = 1_000;

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
    /// A turn of about `degrees`, give or take 30: only what that cannot
    /// get wrong.
    fn rough(degrees: f32) -> Option<Self> {
        let a = fabsf(degrees);
        if a >= ROUGH_U_TURN_DEG {
            Some(Self::UTurn)
        } else if (ROUGH_TURN_DEG..=U_TURN_DEG).contains(&a) {
            Self::of(degrees.signum() * TURN_DEG)
        } else {
            None
        }
    }

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

    fn side(self) -> Option<Side> {
        match self {
            Self::Turn(side) | Self::Sharp(side) => Some(side),
            Self::UTurn => None,
        }
    }
}

/// A stretch of the course that turns, `start_mm` to `end_mm` from its
/// start; a turn when it has a `kind`. All zero bytes are a valid `Turn`, so
/// composition may hand over zeroed memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    pub start_mm: u32,
    pub end_mm: u32,
    pub kind: Option<Kind>,
    /// It turns back as two corners, `kind` each: a city block.
    pub twice: bool,
    /// One of its bends is a turn's size (`TURN_DEG` or more): unnamed, what
    /// is after it waits until the rider is at it.
    pub cornered: bool,
    /// It is drawn coarsely (`SPARSE_M`).
    sparse: bool,
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

/// A point moving on along the course, start first.
#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    /// The point it is heading for.
    next: usize,
    here: (f32, f32),
}

impl Cursor {
    fn start(at: &impl Fn(usize) -> (f32, f32)) -> Self {
        Self {
            next: 1,
            here: at(0),
        }
    }

    /// How long the segment it is on is.
    fn segment(&self, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> f32 {
        match self.next {
            next if next >= 1 && next < len => distance(at(next - 1), at(next)),
            _ => 0.0,
        }
    }

    /// Move `metres` on; false at the course's end.
    fn on(&mut self, len: usize, at: &impl Fn(usize) -> (f32, f32), mut metres: f32) -> bool {
        while metres > 0.0 {
            if self.next >= len {
                return false;
            }
            let to = at(self.next);
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
        }
        true
    }
}

/// The way from `a` to `b` in degrees clockwise from north, if they are far
/// enough apart to tell.
fn way(a: (f32, f32), b: (f32, f32)) -> Option<f32> {
    (distance(a, b) >= SHORT_HALF_M).then(|| atan2f(b.0 - a.0, b.1 - a.1).to_degrees())
}

/// The swing from way `from` to way `to`, right positive, -180 to 180.
pub(crate) fn swing(from: f32, to: f32) -> f32 {
    let d = to - from;
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// Whether a U-turn from `start` to `end` metres along is two corners: its
/// ways in and out more than `WIDE_M` apart, with at least `STRAIGHT_M` of
/// straight between. A hairpin turns all the way round, and one whose legs
/// are close is a U-turn whatever its shape. `from` is `from_m` along, at or
/// before `start` less a chord.
fn two_corners(
    len: usize,
    at: &impl Fn(usize) -> (f32, f32),
    mut walk: Cursor,
    from_m: f32,
    start: f32,
    end: f32,
) -> bool {
    let before = (start - 2.0 * HALF_CHORD_M).max(from_m);
    if !walk.on(len, at, before - from_m) {
        return false;
    }
    let from = walk.here;
    if !walk.on(len, at, start - before) {
        return false;
    }
    let corner = walk.here;
    let into = (corner.0 - from.0, corner.1 - from.1);
    let length = sqrtf(into.0 * into.0 + into.1 * into.1);
    if length < HALF_CHORD_M {
        return false;
    }
    let (mut tail, mut head) = (walk, walk);
    if !head.on(len, at, 2.0 * SHORT_HALF_M) {
        return false;
    }
    let (mut run, mut run_way, mut straight) = (0.0, None::<f32>, false);
    let mut s = start;
    while s < end && !straight {
        if !tail.on(len, at, 1.0) || !head.on(len, at, 1.0) {
            return false;
        }
        s += 1.0;
        let Some(now) = way(tail.here, head.here).or(run_way) else {
            continue;
        };
        match run_way {
            Some(first) if fabsf(swing(first, now)) < STRAIGHT_DEG => run += 1.0,
            _ => (run, run_way) = (0.0, Some(now)),
        }
        straight = run >= STRAIGHT_M;
    }
    if !walk.on(len, at, end - start) {
        return false;
    }
    let out = (walk.here.0 - corner.0, walk.here.1 - corner.1);
    let wide = fabsf(into.0 * out.1 - into.1 * out.0) / length;
    straight && wide > WIDE_M
}

/// The most straight between two bends that turn the same way, each by up
/// to `degrees`, for them to be one turn: what a course thinned to fit
/// (`ride::course`) leaves of a curve, a polygon whose sides are at most
/// `2 * tolerance * cot(degrees / 4)` long, one chord less, and a little
/// room. Two street corners farther apart are two turns: a city block, not
/// a U-turn.
fn same_way_gap(degrees: f32) -> f32 {
    let side = 2.0 * super::TOLERANCE_M / tanf((degrees / 4.0).to_radians());
    (side - 2.0 * HALF_CHORD_M + 2.0).min(SAME_GAP_M)
}

/// A run of turning steps the same way, in metres along; the swing so far
/// before its first.
#[derive(Clone, Copy, Debug)]
struct Bend {
    start: f32,
    last: f32,
    before: f32,
    /// The course is drawn coarsely here (`SPARSE_M`).
    sparse: bool,
    /// The most it swung from its start, either way.
    peak: f32,
    right: bool,
    /// The course a chord and a step before its start, for `two_corners`.
    tail: Cursor,
    tail_m: f32,
}

/// A group being found: from its first bend's start to its last bend's
/// last turning step, in metres along; the swing so far before it, and
/// across it.
#[derive(Clone, Copy, Debug)]
struct Open {
    start: f32,
    last: f32,
    before: f32,
    swung: f32,
    right: bool,
    /// The most a bend of it swung right, and left; whether one is a
    /// turn's size (`TURN_DEG`); which way its first bend went.
    most_right: f32,
    most_left: f32,
    /// How far its bends swung right in all, and left.
    all_right: f32,
    all_left: f32,
    turning: bool,
    first_right: bool,
    /// The swing of its last bend, and how long that bend is.
    bent: f32,
    bent_m: f32,
    /// A corner joined to an arc the other way, as a roundabout's way in
    /// is to its ring: `Some(true)` once a bend against the arc follows, its
    /// way out. A roundabout has both; corners that only look like one, a
    /// corner then two close ones the other way, have no way out.
    ring: Option<bool>,
    /// It is more than one bend, so its swing is known only roughly.
    joined: bool,
    /// One of its bends is drawn coarsely.
    sparse: bool,
    tail: Cursor,
    tail_m: f32,
}

impl Open {
    fn new(bend: Bend, swung: f32) -> Self {
        Self {
            start: bend.start,
            last: bend.last,
            before: bend.before,
            swung,
            right: swung > 0.0,
            most_right: swung.max(0.0),
            most_left: (-swung).max(0.0),
            all_right: swung.max(0.0),
            all_left: (-swung).max(0.0),
            turning: fabsf(swung) >= TURN_DEG,
            first_right: swung > 0.0,
            bent: swung,
            bent_m: bend.last - bend.start,
            ring: None,
            joined: false,
            sparse: bend.sparse,
            tail: bend.tail,
            tail_m: bend.tail_m,
        }
    }

    /// Whether `bend`, of `swung`, is part of this group.
    fn takes(&mut self, bend: &Bend, swung: f32) -> bool {
        let right = swung > 0.0;
        let gap = bend.start - self.last;
        // A roundabout's way in and out swing against it, and less. Two
        // full corners are never one turn the opposite way, and the same way
        // only as a hairpin's are, however drawn.
        let corners = fabsf(swung) >= FLICK_DEG && fabsf(self.bent) >= FLICK_DEG;
        let near = if (bend.sparse || self.sparse) && !corners {
            gap <= SPARSE_GAP_M
        } else if right == self.right {
            gap <= same_way_gap(fabsf(swung).max(fabsf(self.bent)))
        } else {
            let small = fabsf(swung) < FLICK_DEG || fabsf(self.swung) < FLICK_DEG;
            let arc = (bend.last - bend.start).max(self.bent_m) >= ARC_M;
            let near = match corners {
                true => arc && gap <= GAP_M,
                false => gap <= GAP_M || (gap <= SAME_GAP_M && small),
            };
            if near {
                self.ring = match self.ring {
                    Some(_) => Some(true),
                    None if arc => Some(false),
                    None => None,
                };
            }
            near
        };
        near && bend.last - self.start <= SPAN_M
    }

    fn join(&mut self, bend: &Bend, swung: f32, total: f32) {
        (self.last, self.swung) = (bend.last, total - self.before);
        (self.right, self.bent, self.joined) = (swung > 0.0, swung, true);
        self.bent_m = bend.last - bend.start;
        self.most_right = self.most_right.max(swung);
        self.most_left = self.most_left.max(-swung);
        self.all_right += swung.max(0.0);
        self.all_left += (-swung).max(0.0);
        self.turning |= fabsf(swung) >= TURN_DEG;
        self.sparse |= bend.sparse;
    }

    /// The turn this group makes, and whether the next should wait on it.
    fn close(&self, len: usize, at: &impl Fn(usize) -> (f32, f32)) -> Turn {
        let mut kind = Kind::of(self.swung);
        let twice = kind == Some(Kind::UTurn)
            && two_corners(len, at, self.tail, self.tail_m, self.start, self.last);
        if twice {
            kind = Kind::of(self.swung / 2.0);
        } else if self.joined && self.sparse {
            kind = Kind::rough(self.swung);
        }
        let ring = self.ring == Some(true);
        // Where its first bend goes the other way from what it adds up to,
        // or coarse corners crowd it, its word could send the rider the
        // wrong way first: it says nothing.
        let side = kind.and_then(Kind::side);
        // A ring swings its way further than any two corners can: then the
        // way in against it is a roundabout's.
        let round = |side: Side| match side {
            Side::Right => self.all_right >= RING_DEG,
            Side::Left => self.all_left >= RING_DEG,
        };
        let against =
            side.is_some_and(|side| (side == Side::Right) != self.first_right && !round(side));
        // Coarse full corners with a sizeable bend against them in between:
        // which comes first is not known.
        let crowded = side.is_some_and(|side| {
            let (own, other) = match side {
                Side::Right => (self.most_right, self.most_left),
                Side::Left => (self.most_left, self.most_right),
            };
            self.sparse && own >= FLICK_DEG && other >= AGAINST_DEG
        });
        if self.ring == Some(false) || (!ring && (against || crowded)) {
            kind = None;
        }
        Turn {
            start_mm: mm(self.start),
            end_mm: mm(self.last),
            kind,
            twice,
            cornered: self.turning,
            sparse: self.sparse,
        }
    }
}

/// Finds a course's turns once, start to end, a few steps at a time, so a
/// long course never holds up a fix.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scan {
    behind: Cursor,
    ahead: Cursor,
    tail: Cursor,
    tail_m: f32,
    /// Metres along to the middle of the chord.
    s: f32,
    heading: Option<f32>,
    total: f32,
    bend: Option<Bend>,
    open: Option<Open>,
    started: bool,
    done: bool,
}

impl Scan {
    pub fn done(&self) -> bool {
        self.done
    }

    /// Scan up to `steps` more steps of a course of `len` points, adding the
    /// turns found to `turns` from `found`; returns how many there are.
    pub fn step(
        &mut self,
        len: usize,
        at: impl Fn(usize) -> (f32, f32),
        steps: usize,
        turns: &mut [Turn],
        mut found: usize,
    ) -> usize {
        let room = turns.len();
        let mut push = |turn: Turn, found: &mut usize| {
            // Coarse corners close together, apart or not, say nothing.
            if let Some(last) = found.checked_sub(1).and_then(|k| turns.get_mut(k))
                && last.sparse
                && turn.sparse
                && last.cornered
                && turn.cornered
                && turn.start_mm.saturating_sub(last.end_mm) < mm(CROWD_M)
            {
                last.kind = None;
                if let Some(slot) = turns.get_mut(*found) {
                    *slot = Turn { kind: None, ..turn };
                    *found += 1;
                }
                return;
            }
            if let Some(slot) = turns.get_mut(*found) {
                *slot = turn;
                *found += 1;
            }
        };
        if self.done || len < 2 {
            self.done = true;
            return found;
        }
        if !self.started {
            self.started = true;
            self.behind = Cursor::start(&at);
            (self.ahead, self.tail) = (self.behind, self.behind);
            if !self.ahead.on(len, &at, 2.0 * HALF_CHORD_M) {
                self.done = true;
                return found;
            }
            self.s = HALF_CHORD_M;
            self.heading = way(self.behind.here, self.ahead.here);
        }
        for _ in 0..steps {
            if found >= room {
                self.done = true;
                return found;
            }
            if !self.behind.on(len, &at, STEP_M) || !self.ahead.on(len, &at, STEP_M) {
                self.finish(len, &at, &mut found, &mut push);
                return found;
            }
            self.s += STEP_M;
            let lag = self.s - STEP_M - 2.0 * HALF_CHORD_M;
            if lag > self.tail_m && self.tail.on(len, &at, lag - self.tail_m) {
                self.tail_m = lag;
            }
            let Some(now) = way(self.behind.here, self.ahead.here) else {
                continue;
            };
            let turned = self.heading.map_or(0.0, |heading| swing(heading, now));
            self.heading = Some(now);
            self.total += turned;
            let sparse = self.behind.segment(len, &at) >= SPARSE_M
                || self.ahead.segment(len, &at) >= SPARSE_M;
            if let Some(bend) = &mut self.bend {
                bend.sparse |= sparse;
                bend.peak = bend.peak.max(fabsf(self.total - bend.before));
            }
            let s = self.s;
            if fabsf(turned) >= RATE_DEG_PER_M * STEP_M {
                let right = turned > 0.0;
                if let Some(done) = self
                    .bend
                    .filter(|bend| s - bend.start > SPAN_M || bend.right != right)
                {
                    self.bend = None;
                    self.take(done, self.total - turned, len, &at, &mut found, &mut push);
                }
                match &mut self.bend {
                    Some(bend) => bend.last = s,
                    None => {
                        self.bend = Some(Bend {
                            start: s - STEP_M,
                            last: s,
                            before: self.total - turned,
                            sparse,
                            peak: fabsf(turned),
                            right,
                            tail: self.tail,
                            tail_m: self.tail_m,
                        });
                    }
                }
                continue;
            }
            match self.bend {
                Some(done) if s - done.last > BEND_GAP_M => {
                    self.bend = None;
                    self.take(done, self.total, len, &at, &mut found, &mut push);
                }
                Some(_) => {}
                None => {
                    let reach = |group: &Open| match group.sparse {
                        true => SPARSE_GAP_M,
                        false => SAME_GAP_M,
                    };
                    if let Some(group) = self.open.filter(|group| s - group.last > reach(group)) {
                        self.open = None;
                        push(group.close(len, &at), &mut found);
                    }
                }
            }
        }
        found
    }

    /// What a finished bend does: start a group, join one, or end one and
    /// start the next.
    fn take(
        &mut self,
        bend: Bend,
        total: f32,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
        found: &mut usize,
        push: &mut impl FnMut(Turn, &mut usize),
    ) {
        if bend.peak < BEND_MIN_DEG {
            return;
        }
        let swung = total - bend.before;
        if let Some(group) = &mut self.open {
            if group.takes(&bend, swung) {
                group.join(&bend, swung, total);
                return;
            }
            push(group.close(len, at), found);
        }
        self.open = Some(Open::new(bend, swung));
    }

    fn finish(
        &mut self,
        len: usize,
        at: &impl Fn(usize) -> (f32, f32),
        found: &mut usize,
        push: &mut impl FnMut(Turn, &mut usize),
    ) {
        if let Some(done) = self.bend.take() {
            self.take(done, self.total, len, at, found, push);
        }
        if let Some(group) = self.open.take() {
            push(group.close(len, at), found);
        }
        self.done = true;
    }
}

/// Where the rider stands with the course's turns: which is next, and what
/// was said of it, so each turn is warned of once ahead and once at it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Warner {
    /// The first turn the rider is not yet past.
    next: usize,
    /// The furthest the rider has been along the course since; the rider's
    /// place for warnings, so jitter never moves it back.
    furthest_mm: u32,
    said: Said,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Said {
    #[default]
    Nothing,
    /// Turn `0` was warned of ahead.
    Ahead(usize),
    /// Turns up to `0` were warned of at them.
    Now(usize),
}

impl Warner {
    /// The warning for the rider `along_mm` from the course's start, from
    /// its `turns`, if one is due.
    pub fn step(&mut self, turns: &[Turn], along_mm: u32) -> Option<Warning> {
        if along_mm.saturating_add(REWIND_MM) < self.furthest_mm {
            *self = Self {
                furthest_mm: along_mm,
                ..Self::default()
            };
        }
        self.furthest_mm = self.furthest_mm.max(along_mm);
        let here = self.furthest_mm;
        let past = |turn: &Turn| turn.end_mm <= here.saturating_add(HALF_CHORD_MM);
        while turns.get(self.next).is_some_and(past) {
            self.next += 1;
        }
        // Nothing more until the rider is at the last corner warned of.
        if let Said::Now(upto) = self.said
            && turns.get(upto).is_some_and(|turn| !past(turn))
        {
            return None;
        }
        let mut k = self.next;
        while turns
            .get(k)
            .is_some_and(|turn| turn.kind.is_none() && !turn.cornered)
        {
            k += 1;
        }
        let turn = turns.get(k)?;
        // Corners left unnamed come before anything after them.
        let kind = turn.kind?;
        if turn.start_mm < here {
            // The rider is in it already: too late to say.
            self.said = Said::Now(k);
            return None;
        }
        let ahead_mm = turn.start_mm - here;
        if ahead_mm <= NOW_MM {
            let after = turns.get(k + 1).filter(|after| {
                !turn.twice
                    && after.kind.is_some()
                    && after.start_mm.saturating_sub(turn.start_mm) <= THEN_MM
            });
            self.said = Said::Now(after.map_or(k, |_| k + 1));
            let then = match turn.twice {
                true => Some(kind),
                false => after.and_then(|after| after.kind),
            };
            return Some(Warning::Now { kind, then });
        }
        if !(AHEAD_MIN_MM..=AHEAD_MM).contains(&ahead_mm) || self.said == Said::Ahead(k) {
            return None;
        }
        self.said = Said::Ahead(k);
        let metres = ahead_mm.saturating_add(4_999) / 10_000 * 10;
        Some(Warning::Ahead { kind, metres })
    }
}

#[cfg(test)]
pub(crate) mod tests;
