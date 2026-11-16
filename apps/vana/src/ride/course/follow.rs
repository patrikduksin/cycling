use crate::ride::course::nav::{
    self, Event, Guidance, MatchedFix, Navigation, Navigator, OFF_M, ON_M, Observed, Sign,
};
use crate::ride::course::turns::{SCAN_STEPS, Scan, Turn};
use crate::ride::course::{self, along_m};
use crate::ride::homing::{Crumb, Fix, Frame, TURN, bearing, distance};
use crate::ride::thin::off_segment;

const END_M: f32 = 30.0;
const SEARCH_BACK: usize = 64;
const SEARCH_AHEAD: usize = 4;
/// Only segments reaching from this far on to `BACK_M` behind the last match
/// are searched, so the far leg of an out-and-back or the end of a loop never
/// stands in for where the rider is.
const JUMP_M: f32 = 500.0;
const BACK_M: f32 = ON_M;
/// Where the course passes the same road twice, these keep the match on the
/// pass the rider is on.
const NEAR_M: f32 = 100.0;
const SLIP_M: f32 = 10.0;
const AGAINST_M: f32 = 5.0;
/// A fix this far from the last one matched, or this long after the last
/// fix, follows a gap: the rider may be past the near stretch, and the
/// course is matched as if just joined.
const HOP_M: f32 = 40.0;
const GAP_MS: u64 = 5_000;
const JOIN_FIXES: u8 = 5;
const TURN_AROUND_FIXES: u8 = 3;
const MOVING_M: f32 = 2.0;
const MOVING_MM_S: u32 = 1_400;
const MORE_THAN_45_DEG_OFF: core::ops::RangeInclusive<u16> = TURN / 8..=TURN * 7 / 8;
const MORE_THAN_135_DEG_OFF: core::ops::Range<u16> = TURN * 3 / 8 + 1..TURN * 5 / 8;

type Point = (f32, f32);

/// A fix this far on from the last one, at least this fast when the ride's
/// speed is known: a stop's jitter passes only by jumping that far with no
/// speed known.
fn moving(travelled_m: Option<f32>, speed_mm_s: Option<u32>) -> bool {
    travelled_m.is_some_and(|m| m >= MOVING_M) && speed_mm_s.is_none_or(|v| v >= MOVING_MM_S)
}

pub(crate) fn project(p: Point, a: Point, b: Point) -> (Point, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((a.0 + t * dx, a.1 + t * dy), t)
}

fn nearest_on(line: &[Crumb], p: Point, end: usize) -> Nearest {
    let (a, b) = (line[end - 1], line[end]);
    let (point, t) = project(p, (a.east, a.north), (b.east, b.north));
    Nearest {
        end,
        off: distance(p, point),
        point,
        to_end_m: a.along + t * (b.along - a.along),
    }
}

fn point_at(end_first: &[Crumb], along_mm: u32) -> Point {
    let (Some(start), true) = (end_first.last(), end_first.len() >= 2) else {
        return end_first.first().map_or((0.0, 0.0), |c| (c.east, c.north));
    };
    let to_end_m = (start.along - along_mm as f32 / 1000.0).max(0.0);
    let end = end_first
        .partition_point(|c| c.along < to_end_m)
        .clamp(1, end_first.len() - 1);
    let (a, b) = (end_first[end - 1], end_first[end]);
    let t = match b.along > a.along {
        true => ((to_end_m - a.along) / (b.along - a.along)).clamp(0.0, 1.0),
        false => 0.0,
    };
    (
        a.east + t * (b.east - a.east),
        a.north + t * (b.north - a.north),
    )
}

enum Matched {
    On(Nearest),
    End,
}

#[derive(Clone, Copy)]
struct Nearest {
    end: usize,
    off: f32,
    point: Point,
    to_end_m: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Placement(u32);

impl Placement {
    pub(crate) fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Riding {
    /// Along the segment ending at this point: the first pass ridden its way.
    Along(usize),
    Against,
    Across,
    Away,
}

pub struct Follower<'a> {
    line: &'a mut [Crumb],
    len: usize,
    placement: Placement,
    unplaced: bool,
    held: bool,
    join_fixes: u8,
    join_end: usize,
    turn_around_fixes: u8,
    last_fix: Option<Point>,
    following: bool,
    segment: usize,
    matched: Option<Point>,
    to_end_m: f32,
    furthest_to_end_m: f32,
    matched_fix: Option<Point>,
    search_whole_course: bool,
    heading: Option<u16>,
    taken_loop_ms: u64,
    turns: &'a mut [Turn],
    turn_count: usize,
    scan: Scan,
    nav: Navigator,
}

impl<'a> Follower<'a> {
    /// Room for a course of up to `line.len()` points and `turns.len()` of
    /// its turns.
    pub fn new(line: &'a mut [Crumb], turns: &'a mut [Turn]) -> Self {
        Self {
            line,
            len: 0,
            placement: Placement::default(),
            unplaced: false,
            held: false,
            join_fixes: 0,
            join_end: 0,
            turn_around_fixes: 0,
            last_fix: None,
            following: false,
            segment: 0,
            matched: None,
            to_end_m: 0.0,
            furthest_to_end_m: 0.0,
            matched_fix: None,
            search_whole_course: false,
            heading: None,
            taken_loop_ms: 0,
            turns,
            turn_count: 0,
            scan: Scan::default(),
            nav: Navigator::default(),
        }
    }

    /// A new ride: following may start again, and a loaded course waits
    /// (`unplaced`) to be set again in the new ride's frame.
    pub fn reset(&mut self) {
        self.unplaced |= self.len >= 2;
        self.len = 0;
        self.placement = self.placement.next();
        self.held = false;
        self.join_fixes = 0;
        self.turn_around_fixes = 0;
        self.last_fix = None;
        self.following = false;
        self.matched = None;
        self.search_whole_course = false;
        self.heading = None;
        self.nav = Navigator::default();
    }

    /// The course to follow, start first; fewer than two points clear it.
    /// It is placed in the ride's `frame` now if the ride has one, else it
    /// waits (`unplaced`) to be set again once it does.
    pub fn set_course(&mut self, points: &[course::Point], frame: Option<Frame>) {
        self.stop();
        self.len = 0;
        self.placement = self.placement.next();
        self.join_fixes = 0;
        self.turn_around_fixes = 0;
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
        self.nav = Navigator::default();
    }

    /// The turn list, to find a course's turns into while it uploads. Only
    /// while no course is placed: placing one finds its own.
    pub fn turn_room(&mut self) -> &mut [Turn] {
        (self.scan, self.turn_count) = (Scan::finished(), 0);
        self.turns
    }

    pub fn following(&self) -> bool {
        self.following
    }

    pub fn joinable(&self) -> bool {
        !self.following && !self.held && self.len >= 2
    }

    /// If following, stop for the rest of the ride.
    pub fn stop(&mut self) {
        self.let_go();
        self.nav = Navigator::default();
    }

    fn let_go(&mut self) {
        if core::mem::take(&mut self.following) {
            self.held = true;
            self.matched = None;
            self.search_whole_course = false;
        }
    }

    /// The course, end first, in metres from the ride's origin; empty until
    /// placed.
    pub fn course(&self) -> &[Crumb] {
        &self.line[..self.len]
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// While following: what is ahead is the course's first `end` points,
    /// from its end, and then this point on the stretch the rider is on, if
    /// matched yet.
    pub fn ahead(&self) -> Option<(usize, Option<(f32, f32)>)> {
        self.following.then_some((self.segment, self.matched))
    }

    /// Metres along the course to its end from where the rider was last
    /// matched, while following it.
    pub fn to_end_m(&self) -> Option<u32> {
        self.matched.map(|_| (self.to_end_m + 0.5) as u32)
    }

    pub fn sign(&self, now: u64) -> Sign {
        let fresh = now.saturating_sub(self.taken_loop_ms) <= nav::LOST_MS;
        if self.joinable() && self.turn_around_fixes >= TURN_AROUND_FIXES && fresh {
            return Sign::TurnAround;
        }
        self.nav.sign(&self.turns[..self.turn_count], now)
    }

    pub fn holds(&self, cue: Navigation) -> bool {
        self.following && self.nav.holds(cue, &self.turns[..self.turn_count])
    }

    pub fn guidance(&self, now: u64) -> Option<Guidance> {
        let line = &self.line[..self.len];
        self.nav
            .guidance(&self.turns[..self.turn_count], now, |mm| point_at(line, mm))
    }

    /// This main-loop pass's fix, if the ride has a fresh one
    /// (`Homing::observe`), and the ride's live speed. Every pass finds a
    /// few more of the course's turns.
    pub fn observe(
        &mut self,
        fix: Option<Fix>,
        now: u64,
        speed_mm_s: Option<u32>,
        paused: bool,
    ) -> Option<Event> {
        self.scan_turns();
        let Some(fix) = fix else {
            let lost = self.following && now.saturating_sub(self.taken_loop_ms) > nav::LOST_MS;
            return lost.then(|| self.navigate(Observed::Lost)).flatten();
        };
        if fix.taken_loop_ms.saturating_sub(self.taken_loop_ms) > GAP_MS {
            self.matched_fix = None;
        }
        self.taken_loop_ms = fix.taken_loop_ms;
        self.heading = fix.heading;
        let travelled_m = self
            .last_fix
            .replace(fix.at)
            .map(|last| distance(last, fix.at));
        let moving = moving(travelled_m, speed_mm_s);
        let joined = !self.following && fix.moved && self.joining(fix.at, moving);
        if joined {
            self.follow_from(self.join_end);
        }
        if !self.following {
            return None;
        }
        let observed = match self.update(fix.at)? {
            Matched::End => Observed::Arrived { now_ms: now },
            Matched::On(near) => {
                let line = self.course();
                let fixed = MatchedFix {
                    taken_loop_ms: fix.taken_loop_ms,
                    // A float cast saturates, so no position overflows.
                    along_mm: ((line[self.len - 1].along - near.to_end_m) * 1000.0) as u32,
                    to_end_mm: (near.to_end_m * 1000.0) as u32,
                    off_m: near.off,
                    here: fix.at,
                    forward: self.heading.is_some()
                        && !self.against(&line[near.end - 1], &line[near.end]),
                    speed_mm_s,
                    paused,
                };
                match joined {
                    true => Observed::Joined(fixed),
                    false => Observed::Fix(fixed),
                }
            }
        };
        self.navigate(observed)
    }

    fn navigate(&mut self, observed: Observed) -> Option<Event> {
        let line = &self.line[..self.len];
        self.nav
            .step(observed, &self.turns[..self.turn_count], |mm| {
                point_at(line, mm)
            })
    }

    /// Where the course passes twice, the first pass ridden its way is
    /// joined. Only a `moving` fix counts towards TURN AROUND.
    fn joining(&mut self, p: Point, moving: bool) -> bool {
        let Some(heading) = self.heading.filter(|_| !self.held && self.len >= 2) else {
            self.join_fixes = 0;
            return false;
        };
        let riding = self.riding(p, heading);
        self.turn_around_fixes = match riding {
            Riding::Against if moving => self.turn_around_fixes.saturating_add(1),
            Riding::Against | Riding::Across => self.turn_around_fixes,
            Riding::Along(_) | Riding::Away => 0,
        };
        let Riding::Along(end) = riding else {
            self.join_fixes = 0;
            return false;
        };
        self.join_end = end;
        self.join_fixes = self.join_fixes.saturating_add(1);
        self.join_fixes >= JOIN_FIXES
    }

    fn riding(&self, p: Point, heading: u16) -> Riding {
        let mut riding = Riding::Away;
        for end in (1..self.len).rev() {
            let (a, b) = (self.line[end - 1], self.line[end]);
            let (a, b) = ((a.east, a.north), (b.east, b.north));
            if off_segment(p, a, b) >= ON_M {
                continue;
            }
            let off_its_way = (heading + TURN - bearing(b, a)) % TURN;
            if !MORE_THAN_45_DEG_OFF.contains(&off_its_way) {
                return Riding::Along(end);
            }
            if MORE_THAN_135_DEG_OFF.contains(&off_its_way) {
                riding = Riding::Against;
            } else if riding == Riding::Away {
                riding = Riding::Across;
            }
        }
        riding
    }

    fn follow_from(&mut self, end: usize) {
        self.following = true;
        self.join_fixes = 0;
        self.to_end_m = self.course().get(end).map_or(0.0, |c| c.along);
        self.furthest_to_end_m = self.to_end_m;
        self.matched_fix = None;
        self.segment = end;
        self.matched = None;
        self.search_whole_course = false;
    }

    fn scan_turns(&mut self) {
        if self.scan.done() {
            return;
        }
        if self.scan.waiting() {
            let (count, room) = (self.turn_count, self.turns.len());
            let passed = self.nav.passed().min(count);
            if passed < room / 2 {
                return;
            }
            self.turns.copy_within(passed..count, 0);
            self.turn_count -= passed;
            self.nav.forget(passed);
        }
        let len = self.len;
        let line = &self.line[..len];
        let at = |k: usize| {
            let c = line[len - 1 - k];
            (c.east, c.north)
        };
        self.turn_count = self
            .scan
            .step(len, at, SCAN_STEPS, self.turns, self.turn_count);
    }

    fn nearest(&self, p: Point, ends: core::ops::RangeInclusive<usize>) -> Option<Nearest> {
        let plain = |near: &Nearest, _: &Crumb, _: &Crumb| Some(near.off);
        let (to_end_m, furthest) = (self.to_end_m, self.furthest_to_end_m);
        if self.after_gap(p) {
            return self.nearest_within(p, ends, to_end_m - JUMP_M, to_end_m + BACK_M, plain);
        }
        let on = |near: &Nearest, a: &Crumb, b: &Crumb| {
            let back = (near.to_end_m - furthest - SLIP_M).max(0.0);
            let against = if self.against(a, b) { AGAINST_M } else { 0.0 };
            (near.off <= ON_M).then_some(near.off + back + against)
        };
        let Some(near) =
            self.nearest_within(p, ends.clone(), to_end_m - NEAR_M, to_end_m + BACK_M, on)
        else {
            return self.nearest_within(p, ends, to_end_m - JUMP_M, to_end_m + BACK_M, plain);
        };
        let line = self.course();
        if near.to_end_m > furthest + SLIP_M && self.against(&line[near.end - 1], &line[near.end]) {
            let ahead = |near: &Nearest, a: &Crumb, b: &Crumb| {
                (near.off <= ON_M && !self.against(a, b)).then_some(near.off)
            };
            let window = to_end_m - JUMP_M..=to_end_m + BACK_M;
            if let Some(ahead) = self.nearest_within(p, ends, *window.start(), *window.end(), ahead)
            {
                return Some(ahead);
            }
        }
        Some(near)
    }

    fn after_gap(&self, p: Point) -> bool {
        self.matched_fix.is_none_or(|fix| distance(fix, p) > HOP_M)
    }

    fn against(&self, a: &Crumb, b: &Crumb) -> bool {
        let way = bearing((b.east, b.north), (a.east, a.north));
        self.heading.is_some_and(|heading| {
            (TURN / 4..=TURN * 3 / 4).contains(&((heading + TURN - way) % TURN))
        })
    }

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

    fn update(&mut self, p: Point) -> Option<Matched> {
        let len = self.len;
        let window = self.segment.saturating_sub(SEARCH_BACK).max(1)
            ..=(self.segment + SEARCH_AHEAD).min(len - 1);
        let mut near = self.nearest(p, window)?;
        if (self.search_whole_course || near.off > OFF_M)
            && let Some(anywhere) = self.nearest(p, 1..=len - 1)
            && anywhere.off < near.off
        {
            near = anywhere;
        }
        if near.off <= ON_M && near.to_end_m < END_M {
            self.let_go();
            return Some(Matched::End);
        }
        self.segment = near.end;
        self.matched = Some(near.point);
        self.to_end_m = near.to_end_m;
        self.furthest_to_end_m = self.furthest_to_end_m.min(near.to_end_m);
        self.matched_fix = Some(p);
        self.search_whole_course = match self.search_whole_course {
            true => near.off > ON_M,
            false => near.off > OFF_M,
        };
        Some(Matched::On(near))
    }
}

#[cfg(test)]
mod tests;
