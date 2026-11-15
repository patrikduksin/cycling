//! Following the loaded course: placing it in the ride's frame, matching the
//! rider to it, and warning of its turns ahead.
//!
//! The course is placed in the ride's frame (`homing::Frame`) once the ride
//! has its first fix. It is kept end first, so its `along` is metres to its
//! end. Riding along it, within `ON_M` and its way, for `JOIN_FIXES` fixes in
//! a row, follows it by itself; crossing it does not. More than `OFF_M` from
//! it is off course until the rider is back within `ON_M`; `DROP_M` off it,
//! following stops. The match moves at most `JUMP_M` on along the course at a
//! time, so the far leg of an out-and-back or the end of a loop never stands
//! in for where the rider is. Where the course passes the same road twice,
//! the pass that goes on from where the rider was, their way, is the one they
//! are on. Dropping it, reaching its end or stopping it (`stop`) holds it off
//! for the rest of the ride. While it is followed, on it and ridden its way,
//! its turns ahead are warned of (`course::turns`).
use crate::ride::course::turns::{SCAN_STEPS, Scan, Turn, Warner, Warning};
use crate::ride::course::{self, along_m};
use crate::ride::homing::{Crumb, Fix, Frame, TURN, bearing, distance};
use crate::ride::thin::off_segment;

/// Off course beyond this from the course, back on within `ON_M`.
const OFF_M: f32 = 50.0;
const ON_M: f32 = 25.0;
/// Following stops this far from the course: the rider chose another way.
const DROP_M: f32 = 500.0;
/// At the end within this of it, on the course.
const END_M: f32 = 30.0;
/// Segments behind and ahead of the last match searched first.
const SEARCH_BACK: usize = 64;
const SEARCH_AHEAD: usize = 4;
/// The match moves at most this far on towards the course's end, and back
/// only this far behind where it was, in one fix.
const JUMP_M: f32 = 500.0;
const BACK_M: f32 = ON_M;
/// Within `ON_M` and this little on from where the rider was, the course
/// is matched first: where it passes the same road twice, the pass the
/// rider is on, not the other. There a stretch more than `SLIP_M` behind
/// the furthest the rider has been counts as that much farther off, and one
/// against the rider's way `AGAINST_M` farther.
const NEAR_M: f32 = 100.0;
const SLIP_M: f32 = 10.0;
const AGAINST_M: f32 = 5.0;
/// A fix this far from the last one matched, or this long after the last
/// fix, follows a gap: the rider may be past the near stretch, and the
/// course is matched as if just joined.
const HOP_M: f32 = 40.0;
const GAP_MS: u64 = 5_000;
/// Riding along the course: this many fixes in a row within `ON_M` of it,
/// heading its way within an eighth of a turn.
const JOIN_FIXES: u8 = 5;

type Point = (f32, f32);

/// What a fix changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Riding along the course: following it started by itself.
    Following,
    OffCourse,
    OnCourse,
    /// At the course's end: following stopped.
    Arrived,
}

/// Where the rider is on the course, while following it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Metres along the course to its end, from where the rider was matched.
    pub to_end_m: u32,
    pub off_course: bool,
}

/// The point of segment a-b nearest `p`, and how far along the segment it
/// is, 0 to 1.
fn project(p: Point, a: Point, b: Point) -> (Point, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((a.0 + t * dx, a.1 + t * dy), t)
}

/// Where `p` meets segment `end` of `line`, which runs from `end - 1`.
fn nearest_on(line: &[Crumb], p: Point, end: usize) -> Nearest {
    let (a, b) = (line[end - 1], line[end]);
    let (point, t) = project(p, (a.east, a.north), (b.east, b.north));
    Nearest {
        end,
        off: distance(p, point),
        point,
        along: a.along + t * (b.along - a.along),
    }
}

/// The nearest segment of the course to a position.
#[derive(Clone, Copy)]
struct Nearest {
    /// The segment's far end: it runs from `end - 1` to `end`.
    end: usize,
    off: f32,
    point: Point,
    /// Metres along the course from its end to `point`.
    along: f32,
}

pub struct Follower<'a> {
    /// The course, end first, in the ride's frame; empty without one.
    line: &'a mut [Crumb],
    len: usize,
    /// A course is loaded but waits for the ride's first fix to be placed.
    unplaced: bool,
    /// The course is off for the rest of the ride: dropped, ended or stopped.
    held: bool,
    /// Fixes in a row riding along the course, at its segment `join_end`.
    join_fixes: u8,
    join_end: usize,
    following: bool,
    /// The segment last matched while following.
    segment: usize,
    /// Where on the course the rider last was while following.
    matched: Option<Point>,
    /// Metres to the course's end where the rider was last matched.
    along: f32,
    /// Metres to the course's end at the furthest the rider has been along
    /// it since following it.
    reached: f32,
    /// The fix last matched, until a gap in the fixes.
    matched_fix: Option<Point>,
    off: bool,
    heading: Option<u16>,
    fix_ms: u64,
    /// The course's turns, found a few steps a fix once it is placed, and
    /// what was said of them.
    turns: &'a mut [Turn],
    turn_count: usize,
    scan: Scan,
    warner: Warner,
    /// The turn warning from the last fix, until taken.
    turn: Option<Warning>,
}

impl<'a> Follower<'a> {
    /// Room for a course of up to `line.len()` points and `turns.len()` of
    /// its turns.
    pub fn new(line: &'a mut [Crumb], turns: &'a mut [Turn]) -> Self {
        Self {
            line,
            len: 0,
            unplaced: false,
            held: false,
            join_fixes: 0,
            join_end: 0,
            following: false,
            segment: 0,
            matched: None,
            along: 0.0,
            reached: 0.0,
            matched_fix: None,
            off: false,
            heading: None,
            fix_ms: 0,
            turns,
            turn_count: 0,
            scan: Scan::default(),
            warner: Warner::default(),
            turn: None,
        }
    }

    /// A new ride: a loaded course is placed again from its first fix, and
    /// may be followed again.
    pub fn reset(&mut self) {
        self.unplaced |= self.len >= 2;
        self.len = 0;
        self.held = false;
        self.join_fixes = 0;
        self.following = false;
        self.matched = None;
        self.off = false;
        self.heading = None;
        self.turn = None;
    }

    /// The course to follow, start first; fewer than two points clear it.
    /// It is placed in the ride's `frame` now if the ride has one, else it
    /// waits (`unplaced`) to be set again once it does.
    pub fn set_course(&mut self, points: &[course::Point], frame: Option<Frame>) {
        self.stop();
        self.len = 0;
        self.join_fixes = 0;
        let n = points.len().min(self.line.len());
        self.unplaced = n >= 2 && frame.is_none();
        let Some(frame) = frame.filter(|_| n >= 2) else {
            return;
        };
        for k in 0..n {
            let point = points[n - 1 - k];
            let p = frame.place(point.lat_e7, point.lon_e7);
            let along = match k {
                0 => 0.0,
                _ => {
                    let before = self.line[k - 1];
                    before.along + distance((before.east, before.north), p)
                }
            };
            self.line[k] = Crumb {
                east: p.0,
                north: p.1,
                along,
            };
        }
        self.len = n;
        (self.scan, self.turn_count) = (Scan::default(), 0);
        self.warner = Warner::default();
    }

    /// A course is loaded and waits for the ride's first fix to be placed.
    pub fn unplaced(&self) -> bool {
        self.unplaced
    }

    /// The placed course's turns as stored, in place of the scan: `fill`
    /// writes them into the list in millimetres along `points`, the course
    /// as `set_course` was given it, and returns how many and whether the
    /// course has more, or `None` to scan for them. They move into this
    /// ride's frame, kept point by kept point, so the rider's place along
    /// the course finds them; the scan finds any after them.
    pub fn take_turns(
        &mut self,
        points: &[course::Point],
        fill: impl FnOnce(&mut [Turn]) -> Option<(usize, bool)>,
    ) {
        let len = self.len;
        if len < 2 || points.len() != len {
            return;
        }
        let Some((count, more)) = fill(self.turns).filter(|&(count, _)| count <= self.turns.len())
        else {
            (self.scan, self.turn_count) = (Scan::default(), 0);
            return;
        };
        // Kept point `k` from the start is `line[len - 1 - k]`.
        let line = &self.line[..len];
        let frame_m = |k: usize| line[len - 1].along - line[len - 1 - k].along;
        for end in [false, true] {
            let mut course_m = along_m(points).enumerate().peekable();
            let (mut k, mut from_m) = (0, 0.0f32);
            for turn in &mut self.turns[..count] {
                let slot = match end {
                    false => &mut turn.start_mm,
                    true => &mut turn.end_mm,
                };
                let at_m = *slot as f32 / 1000.0;
                while let Some(&(next, next_m)) = course_m.peek() {
                    if next_m > at_m {
                        break;
                    }
                    (k, from_m) = (next, next_m);
                    course_m.next();
                }
                let ride_m = match course_m.peek() {
                    Some(&(next, next_m)) if next_m > from_m => {
                        let t = (at_m - from_m) / (next_m - from_m);
                        frame_m(k) + t * (frame_m(next) - frame_m(k))
                    }
                    _ => frame_m(k),
                };
                // A float cast saturates, so no position overflows.
                *slot = (ride_m * 1000.0) as u32;
            }
        }
        self.scan = match more {
            true => Scan::after(&self.turns[..count]),
            false => Scan::finished(),
        };
        self.turn_count = count;
        self.warner = Warner::default();
    }

    /// The turn list, to find a course's turns into while it uploads. Only
    /// while no course is placed: placing one finds its own.
    pub fn turn_room(&mut self) -> &mut [Turn] {
        (self.scan, self.turn_count) = (Scan::finished(), 0);
        self.turns
    }

    /// The turn ahead to warn of, from the last fix, once.
    pub fn take_turn(&mut self) -> Option<Warning> {
        self.turn.take()
    }

    pub fn following(&self) -> bool {
        self.following
    }

    /// Stop following for the rest of the ride.
    pub fn stop(&mut self) {
        if core::mem::take(&mut self.following) {
            self.held = true;
            self.matched = None;
            self.off = false;
        }
    }

    /// The course, end first, in metres from the ride's origin; empty until
    /// placed.
    pub fn course(&self) -> &[Crumb] {
        &self.line[..self.len]
    }

    /// While following: what is ahead is the course's first `end` points,
    /// from its end, and then this point on the stretch the rider is on, if
    /// matched yet.
    pub fn ahead(&self) -> Option<(usize, Option<(f32, f32)>)> {
        self.following.then_some((self.segment, self.matched))
    }

    /// Where the rider is on the course, once matched while following.
    pub fn progress(&self) -> Option<Progress> {
        self.matched.filter(|_| self.following).map(|_| Progress {
            to_end_m: (self.along + 0.5) as u32,
            off_course: self.off,
        })
    }

    /// This main-loop pass's fix, if the ride has a fresh one
    /// (`Homing::observe`). Every pass finds a few more of the course's
    /// turns.
    pub fn observe(&mut self, fix: Option<Fix>, now: u64) -> Option<Event> {
        self.scan_turns();
        let fix = fix?;
        if now.saturating_sub(self.fix_ms) > GAP_MS {
            self.matched_fix = None;
        }
        self.fix_ms = now;
        self.heading = fix.heading;
        if self.following {
            let event = self.update(fix.at);
            if fix.moved {
                self.turn = self.warn_turn();
            }
            return event;
        }
        if fix.moved && self.joining(fix.at) {
            self.engage(self.join_end);
            self.update(fix.at);
            return Some(Event::Following);
        }
        None
    }

    /// Whether this fix continues a ride along the course, its way, for
    /// long enough to follow it: the earliest stretch within `ON_M` that
    /// runs the rider's way, since where the course passes twice the first
    /// pass is the one.
    fn joining(&mut self, p: Point) -> bool {
        let heading = self.heading.filter(|_| !self.held && self.len >= 2);
        let along = heading.and_then(|heading| {
            (1..self.len).rev().find(|&end| {
                let (a, b) = (self.line[end - 1], self.line[end]);
                let (a, b) = ((a.east, a.north), (b.east, b.north));
                let turn = (heading + TURN - bearing(b, a)) % TURN;
                off_segment(p, a, b) < ON_M && !(TURN / 8..=TURN * 7 / 8).contains(&turn)
            })
        });
        let Some(end) = along else {
            self.join_fixes = 0;
            return false;
        };
        self.join_end = end;
        self.join_fixes = self.join_fixes.saturating_add(1);
        self.join_fixes >= JOIN_FIXES
    }

    /// Follow the course from its segment `end`.
    fn engage(&mut self, end: usize) {
        self.following = true;
        self.join_fixes = 0;
        self.along = self.course().get(end).map_or(0.0, |c| c.along);
        self.reached = self.along;
        self.matched_fix = None;
        self.warner = Warner::default();
        self.segment = end;
        self.matched = None;
        self.off = false;
    }

    /// The turn to warn of, while following the course, on it and riding
    /// its way.
    fn warn_turn(&mut self) -> Option<Warning> {
        let heading = self.heading?;
        self.matched.filter(|_| self.following && !self.off)?;
        let (len, end) = (self.len, self.segment);
        if end == 0 || end >= len {
            return None;
        }
        let (a, b) = (self.line[end - 1], self.line[end]);
        let way = bearing((b.east, b.north), (a.east, a.north));
        if (TURN / 4..=TURN * 3 / 4).contains(&((heading + TURN - way) % TURN)) {
            return None;
        }
        let from_start = self.line[len - 1].along - self.along;
        // A float cast saturates, so no position overflows.
        let along_mm = (from_start * 1000.0) as u32;
        self.warner.step(&self.turns[..self.turn_count], along_mm)
    }

    /// Find a few more of the placed course's turns. With the list full,
    /// the first half the rider is past makes way.
    fn scan_turns(&mut self) {
        if self.scan.done() {
            return;
        }
        if self.scan.waiting() {
            let (count, room) = (self.turn_count, self.turns.len());
            let passed = self.warner.passed().min(count);
            if passed < room / 2 {
                return;
            }
            self.turns.copy_within(passed..count, 0);
            self.turn_count -= passed;
            self.warner.forget(passed);
        }
        let len = self.len;
        // The course is kept end first; the scan reads it start first.
        let line = &self.line[..len];
        let at = |k: usize| {
            let c = line[len - 1 - k];
            (c.east, c.north)
        };
        self.turn_count = self
            .scan
            .step(len, at, SCAN_STEPS, self.turns, self.turn_count);
    }

    /// The nearest segment of the course among `ends`: the pass the rider
    /// is on (`NEAR_M`) comes first; otherwise only segments from `JUMP_M`
    /// on to `BACK_M` behind where the rider was are in reach, and on a tie
    /// the earlier one wins.
    fn nearest(&self, p: Point, ends: core::ops::RangeInclusive<usize>) -> Option<Nearest> {
        let plain = |near: &Nearest, _: &Crumb, _: &Crumb| Some(near.off);
        let (along, reached) = (self.along, self.reached);
        // After a gap in the fixes the rider may be past the near stretch.
        if self.matched_fix.is_none_or(|fix| distance(fix, p) > HOP_M) {
            return self.nearest_within(p, ends, along - JUMP_M, along + BACK_M, plain);
        }
        let on = |near: &Nearest, a: &Crumb, b: &Crumb| {
            let back = (near.along - reached - SLIP_M).max(0.0);
            let against = if self.against(a, b) { AGAINST_M } else { 0.0 };
            (near.off <= ON_M).then_some(near.off + back + against)
        };
        let Some(near) = self.nearest_within(p, ends.clone(), along - NEAR_M, along + BACK_M, on)
        else {
            return self.nearest_within(p, ends, along - JUMP_M, along + BACK_M, plain);
        };
        // Sliding back against the rider's way, a pass ridden its own way
        // close by is the one they are on.
        let line = self.course();
        if near.along > reached + SLIP_M && self.against(&line[near.end - 1], &line[near.end]) {
            let ahead = |near: &Nearest, a: &Crumb, b: &Crumb| {
                (near.off <= ON_M && !self.against(a, b)).then_some(near.off)
            };
            let window = along - JUMP_M..=along + BACK_M;
            if let Some(ahead) = self.nearest_within(p, ends, *window.start(), *window.end(), ahead)
            {
                return Some(ahead);
            }
        }
        Some(near)
    }

    /// Whether the course from `b` to `a` runs against the rider's heading.
    fn against(&self, a: &Crumb, b: &Crumb) -> bool {
        let way = bearing((b.east, b.north), (a.east, a.north));
        self.heading.is_some_and(|heading| {
            (TURN / 4..=TURN * 3 / 4).contains(&((heading + TURN - way) % TURN))
        })
    }

    /// The segment among `ends` reaching from `floor` to `ceiling` metres
    /// along that `cost` puts first, given where it meets `p`; a segment
    /// without a cost is out.
    fn nearest_within(
        &self,
        p: Point,
        ends: core::ops::RangeInclusive<usize>,
        floor: f32,
        ceiling: f32,
        cost: impl Fn(&Nearest, &Crumb, &Crumb) -> Option<f32>,
    ) -> Option<Nearest> {
        let line = self.course();
        let mut best: Option<(Nearest, f32)> = None;
        for end in ends.rev() {
            let (a, b) = (line[end - 1], line[end]);
            if b.along < floor || a.along > ceiling {
                continue;
            }
            let near = nearest_on(line, p, end);
            let Some(cost) = cost(&near, &a, &b) else {
                continue;
            };
            if best.is_none_or(|(_, best)| cost < best) {
                best = Some((near, cost));
            }
        }
        best.map(|(near, _)| near)
    }

    fn update(&mut self, p: Point) -> Option<Event> {
        let len = self.len;
        let window = self.segment.saturating_sub(SEARCH_BACK).max(1)
            ..=(self.segment + SEARCH_AHEAD).min(len - 1);
        let mut near = self.nearest(p, window)?;
        if (self.off || near.off > OFF_M)
            && let Some(anywhere) = self.nearest(p, 1..=len - 1)
            && anywhere.off < near.off
        {
            near = anywhere;
        }
        if near.off > DROP_M {
            self.stop();
            return None;
        }
        if near.off <= ON_M && near.along < END_M {
            self.stop();
            return Some(Event::Arrived);
        }
        self.segment = near.end;
        self.matched = Some(near.point);
        self.along = near.along;
        self.reached = self.reached.min(near.along);
        self.matched_fix = Some(p);
        let was_off = self.off;
        self.off = if self.off {
            near.off > ON_M
        } else {
            near.off > OFF_M
        };
        match (was_off, self.off) {
            (false, true) => Some(Event::OffCourse),
            (true, false) => Some(Event::OnCourse),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
