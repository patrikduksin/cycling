use crate::ride::breadcrumb::{Crumb, Fix};
use crate::ride::course::nav::{
    self, Event, Guidance, MatchedFix, Navigation, Navigator, OFF_M, ON_M, Observed, Sign,
};
use crate::ride::course::turns::{PlacedScan, SCAN_STEPS, Turn};
use crate::ride::course::{self, along_m};
use crate::ride::geo::{Frame, TURN, bearing, box_gap, distance};
use crate::ride::thin::off_segment;
use firmware_services::why;

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
pub(super) const TURN_AROUND_FIXES: u8 = 3;
pub(super) const MOVING_M: f32 = 2.0;
pub(crate) const MOVING_FROM_MM_S: u32 = 5 * 1_000_000 / 3_600;
const MORE_THAN_45_DEG_OFF: core::ops::RangeInclusive<u16> = TURN / 8..=TURN * 7 / 8;
pub(super) const MORE_THAN_135_DEG_OFF: core::ops::Range<u16> = TURN * 3 / 8 + 1..TURN * 5 / 8;

type Point = (f32, f32);

/// A fix this far on from the last one, at least this fast when the ride's
/// speed is known: a stop's jitter passes only by jumping that far with no
/// speed known.
fn fix_moving(travelled_m: Option<f32>, speed_mm_s: Option<u32>) -> bool {
    travelled_m.is_some_and(|m| m >= MOVING_M) && speed_mm_s.is_none_or(moving)
}

pub(crate) fn moving(speed_mm_s: u32) -> bool {
    speed_mm_s >= MOVING_FROM_MM_S
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
#[cfg_attr(not(target_os = "none"), derive(Debug))]
enum Riding {
    /// Along the segment ending at this point: the first pass ridden its way.
    Along(usize),
    Against,
    Across,
    Away,
}

#[derive(Clone, Copy)]
enum Lifecycle {
    NoCourse,
    Unplaced,
    Joining(Joining),
    Following(Following),
    Arrived { nav: Navigator },
    LetGo { awaits_frame: bool },
}

impl Lifecycle {
    fn nav(&self) -> Option<&Navigator> {
        match self {
            Self::Following(following) => Some(&following.nav),
            Self::Arrived { nav } => Some(nav),
            _ => None,
        }
    }

    fn nav_mut(&mut self) -> Option<&mut Navigator> {
        match self {
            Self::Following(following) => Some(&mut following.nav),
            Self::Arrived { nav } => Some(nav),
            _ => None,
        }
    }

    fn phase(&self) -> Phase {
        match self {
            Self::NoCourse => Phase::NoCourse,
            Self::Unplaced => Phase::Unplaced,
            Self::Joining(_) => Phase::Joining,
            Self::Following(_) => Phase::Following,
            Self::Arrived { .. } => Phase::Arrived,
            Self::LetGo { awaits_frame } => Phase::LetGo {
                awaits_frame: *awaits_frame,
            },
        }
    }

    fn unplaced(&self) -> bool {
        matches!(
            self.phase(),
            Phase::Unplaced | Phase::LetGo { awaits_frame: true }
        )
    }

    fn cannot_join_again_this_ride(&self) -> bool {
        matches!(
            self.phase(),
            Phase::Following | Phase::Arrived | Phase::LetGo { .. }
        )
    }
}

#[cfg_attr(not(target_os = "none"), derive(Debug))]
enum Phase {
    NoCourse,
    Unplaced,
    Joining,
    Following,
    Arrived,
    LetGo { awaits_frame: bool },
}

#[derive(Clone, Copy, Default)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
struct Joining {
    fixes: u8,
    against_fixes: u8,
}

enum Joined {
    NotYet,
    FromSegmentEndingAt(usize),
}

#[derive(Clone, Copy)]
struct Following {
    nav: Navigator,
    segment: usize,
    matched: Option<Point>,
    matched_fix: Option<Point>,
    to_end_m: f32,
    furthest_to_end_m: f32,
    search_whole_course: bool,
}

pub struct Follower<'a> {
    line: &'a mut [Crumb],
    len: usize,
    placement: Placement,
    life: Lifecycle,
    last_fix: Option<Point>,
    heading: Option<u16>,
    taken_loop_ms: u64,
    turns: &'a mut [Turn],
    turn_count: usize,
    scan: PlacedScan,
}

pub enum Input<'c> {
    NewRide,
    /// The course to follow, start first; fewer than two points clear it.
    /// It is placed in the ride's `frame` now if the ride has one, else it
    /// waits (`unplaced`) to be given again once it does. Once placed,
    /// `read` writes its stored turns into the list, in millimetres along
    /// `points`, and returns how many and whether the course has more, or
    /// `None` to scan for them.
    Course {
        points: &'c [course::Point],
        frame: Option<Frame>,
        read: &'c mut ReadStored<'c>,
    },
    /// This main-loop pass's fix, if the ride has a fresh one
    /// (`Breadcrumb::observe`), and the ride's live speed.
    Observe {
        now: u64,
        fix: Option<Fix>,
        speed_mm_s: Option<u32>,
        paused: bool,
    },
    /// The rider's long press on the map: if following, stop for the rest
    /// of the ride.
    LetGo,
}

pub type ReadStored<'r> = dyn FnMut(&mut [Turn]) -> Option<(usize, bool)> + 'r;

enum Effect {
    ReadTurns,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shown {
    pub sign: Sign,
    pub guidance: Option<Guidance>,
}

impl<'a> Follower<'a> {
    /// Room for a course of up to `line.len()` points and `turns.len()` of
    /// its turns.
    pub fn new(line: &'a mut [Crumb], turns: &'a mut [Turn]) -> Self {
        Self {
            line,
            len: 0,
            placement: Placement::default(),
            life: Lifecycle::NoCourse,
            last_fix: None,
            heading: None,
            taken_loop_ms: 0,
            turns,
            turn_count: 0,
            scan: PlacedScan::finished(),
        }
    }

    fn clear_turns(&mut self) {
        (self.scan, self.turn_count) = (PlacedScan::finished(), 0);
    }

    pub fn handle(&mut self, input: Input) -> Option<Event> {
        match input {
            Input::NewRide => self.new_ride(),
            Input::Course {
                points,
                frame,
                read,
            } => {
                if let Some(Effect::ReadTurns) = self.set_course(points, frame) {
                    let stored = read(self.turns);
                    self.move_stored_turns_into_frame(points, stored);
                }
            }
            Input::Observe {
                now,
                fix,
                speed_mm_s,
                paused,
            } => return self.observe(fix, now, speed_mm_s, paused),
            Input::LetGo => self.let_go(),
        }
        None
    }

    fn new_ride(&mut self) {
        let waits = self.len >= 2 || self.life.unplaced();
        why!("follow.new_ride", waits);
        self.len = 0;
        self.clear_turns();
        self.placement = self.placement.next();
        self.last_fix = None;
        self.heading = None;
        self.life = match waits {
            true => Lifecycle::Unplaced,
            false => Lifecycle::NoCourse,
        };
    }

    fn set_course(&mut self, points: &[course::Point], frame: Option<Frame>) -> Option<Effect> {
        let cannot_join = self.life.cannot_join_again_this_ride();
        self.len = 0;
        self.placement = self.placement.next();
        let n = points.len().min(self.line.len());
        let unplaced = n >= 2 && frame.is_none();
        let frame = frame.filter(|_| n >= 2);
        self.life = match (cannot_join, frame) {
            (true, _) => Lifecycle::LetGo {
                awaits_frame: unplaced,
            },
            (false, Some(_)) => Lifecycle::Joining(Joining::default()),
            (false, None) if unplaced => Lifecycle::Unplaced,
            (false, None) => Lifecycle::NoCourse,
        };
        why!(
            "follow.course",
            points = n,
            placed = frame.is_some(),
            cannot_join,
            life = self.life.phase()
        );
        let Some(frame) = frame else {
            self.clear_turns();
            return None;
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
        (self.scan, self.turn_count) = (PlacedScan::default(), 0);
        (points.len() == n).then_some(Effect::ReadTurns)
    }

    /// A course is loaded and waits for the ride's first fix to be placed.
    pub fn unplaced(&self) -> bool {
        self.life.unplaced()
    }

    fn move_stored_turns_into_frame(
        &mut self,
        points: &[course::Point],
        stored: Option<(usize, bool)>,
    ) {
        let len = self.len;
        let Some((count, more)) = stored.filter(|&(count, _)| count <= self.turns.len()) else {
            why!("follow.scan_turns", stored, room = self.turns.len());
            (self.scan, self.turn_count) = (PlacedScan::default(), 0);
            return;
        };
        why!("follow.stored_turns", count, more);
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
            true => PlacedScan::after(&self.turns[..count]),
            false => PlacedScan::finished(),
        };
        self.turn_count = count;
    }

    /// The turn list, to find a course's turns into while it uploads. Only
    /// while no course is placed: placing one finds its own.
    pub fn upload_room(&mut self) -> &mut [Turn] {
        debug_assert_eq!(
            self.len, 0,
            "the turn list is lent only while no course is placed"
        );
        self.clear_turns();
        self.turns
    }

    pub fn following(&self) -> bool {
        matches!(self.life, Lifecycle::Following(_))
    }

    pub fn joinable(&self) -> bool {
        matches!(self.life, Lifecycle::Joining(_))
    }

    fn let_go(&mut self) {
        why!("follow.let_go", following = self.following());
        if self.following() {
            self.life = Lifecycle::LetGo {
                awaits_frame: false,
            };
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
        match &self.life {
            Lifecycle::Following(following) => Some((following.segment, following.matched)),
            _ => None,
        }
    }

    /// Metres along the course to its end from where the rider was last
    /// matched, while following it.
    pub fn to_end_m(&self) -> Option<u32> {
        let Lifecycle::Following(following) = &self.life else {
            return None;
        };
        following.matched.map(|_| (following.to_end_m + 0.5) as u32)
    }

    pub fn shown(&self, now: u64) -> Shown {
        let fresh = now.saturating_sub(self.taken_loop_ms) <= nav::LOST_MS;
        if let Lifecycle::Joining(joining) = self.life {
            let against_fixes = joining.against_fixes;
            if against_fixes >= TURN_AROUND_FIXES && fresh {
                why!(
                    "follow.turn_around",
                    against_fixes,
                    turn_around_fixes = TURN_AROUND_FIXES,
                    fresh
                );
                return Shown {
                    sign: Sign::TurnAround,
                    guidance: None,
                };
            }
            why!(
                "follow.no_turn_around",
                against_fixes,
                turn_around_fixes = TURN_AROUND_FIXES,
                fresh
            );
        }
        let Some(nav) = self.life.nav() else {
            why!("follow.no_sign", life = self.life.phase());
            return Shown::default();
        };
        let (line, turns) = (&self.line[..self.len], &self.turns[..self.turn_count]);
        Shown {
            sign: nav.sign(turns, now),
            guidance: nav.guidance(turns, now, |mm| point_at(line, mm)),
        }
    }

    /// Whether `cue`, raised earlier and still unheard, is still due at
    /// `now`. Once arrived, only the arrival chime may be.
    pub fn still_due(&self, cue: Navigation, now: u64) -> bool {
        let nav = match (&self.life, cue) {
            (Lifecycle::Following(following), _) => &following.nav,
            (Lifecycle::Arrived { nav }, Navigation::Arrived) => nav,
            _ => return false,
        };
        nav.still_due(cue, &self.turns[..self.turn_count], now)
    }

    fn observe(
        &mut self,
        fix: Option<Fix>,
        now: u64,
        speed_mm_s: Option<u32>,
        paused: bool,
    ) -> Option<Event> {
        self.scan_turns();
        let line = &self.line[..self.len];
        let turns = &self.turns[..self.turn_count];
        let locate = |mm| point_at(line, mm);
        let Some(fix) = fix else {
            let Lifecycle::Following(following) = &mut self.life else {
                return None;
            };
            let lost = now.saturating_sub(self.taken_loop_ms) > nav::LOST_MS;
            if lost {
                why!(
                    "follow.lost",
                    last_fix_ms = self.taken_loop_ms,
                    lost_ms = nav::LOST_MS
                );
            }
            return lost
                .then(|| following.nav.step(Observed::Lost, turns, locate))
                .flatten();
        };
        if fix.taken_loop_ms.saturating_sub(self.taken_loop_ms) > GAP_MS
            && let Lifecycle::Following(following) = &mut self.life
        {
            why!(
                "follow.gap",
                gap_ms = fix.taken_loop_ms.saturating_sub(self.taken_loop_ms),
                gap_limit_ms = GAP_MS
            );
            following.matched_fix = None;
        }
        self.taken_loop_ms = fix.taken_loop_ms;
        self.heading = fix.heading;
        let travelled_m = self
            .last_fix
            .replace(fix.at)
            .map(|last| distance(last, fix.at));
        let moving = fix_moving(travelled_m, speed_mm_s);
        let joined_at = match &mut self.life {
            Lifecycle::Joining(joining) if fix.moved => {
                match joining.observe(line, self.heading, fix.at, moving) {
                    Joined::FromSegmentEndingAt(end) => Some(end),
                    Joined::NotYet => None,
                }
            }
            _ => None,
        };
        if let Some(end) = joined_at {
            self.life = Lifecycle::Following(Following::from(line, end));
        }
        let Lifecycle::Following(following) = &mut self.life else {
            return None;
        };
        let observed = match following.update(line, self.heading, fix.at)? {
            Matched::End => {
                let mut nav = following.nav;
                let event = nav.step(Observed::Arrived { now_ms: now }, turns, locate);
                self.life = Lifecycle::Arrived { nav };
                return event;
            }
            Matched::On(near) => {
                let fixed = MatchedFix {
                    taken_loop_ms: fix.taken_loop_ms,
                    // A float cast saturates, so no position overflows.
                    along_mm: ((line[line.len() - 1].along - near.to_end_m) * 1000.0) as u32,
                    to_end_mm: (near.to_end_m * 1000.0) as u32,
                    off_m: near.off,
                    here: fix.at,
                    forward: self.heading.is_some()
                        && !against(self.heading, &line[near.end - 1], &line[near.end]),
                    speed_mm_s,
                    paused,
                };
                match joined_at {
                    Some(_) => Observed::Joined(fixed),
                    None => Observed::Fix(fixed),
                }
            }
        };
        following.nav.step(observed, turns, locate)
    }

    fn scan_turns(&mut self) {
        if self.scan.done() {
            return;
        }
        if self.scan.waiting() {
            let (count, room) = (self.turn_count, self.turns.len());
            let passed = self.life.nav().map_or(0, Navigator::passed).min(count);
            if passed < room / 2 {
                return;
            }
            self.turns.copy_within(passed..count, 0);
            self.turn_count -= passed;
            if let Some(nav) = self.life.nav_mut() {
                nav.forget(passed);
            }
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
}

impl Joining {
    fn observe(&mut self, line: &[Crumb], heading: Option<u16>, p: Point, moving: bool) -> Joined {
        let Some(heading) = heading else {
            self.fixes = 0;
            why!(
                "follow.no_heading",
                moving,
                against_fixes = self.against_fixes
            );
            return Joined::NotYet;
        };
        let riding = riding(line, p, heading);
        self.against_fixes = match riding {
            Riding::Against if moving => self.against_fixes.saturating_add(1),
            Riding::Against | Riding::Across => self.against_fixes,
            Riding::Along(_) | Riding::Away => 0,
        };
        let along = match riding {
            Riding::Along(end) => {
                self.fixes = self.fixes.saturating_add(1);
                Some(end)
            }
            Riding::Against | Riding::Across | Riding::Away => {
                self.fixes = 0;
                None
            }
        };
        why!(
            "follow.joining",
            riding,
            moving,
            fixes = self.fixes,
            join_fixes = JOIN_FIXES,
            against_fixes = self.against_fixes
        );
        match along {
            Some(end) if self.fixes >= JOIN_FIXES => {
                why!("follow.joined", from_segment = end, fixes = self.fixes);
                Joined::FromSegmentEndingAt(end)
            }
            _ => Joined::NotYet,
        }
    }
}

fn riding(line: &[Crumb], p: Point, heading: u16) -> Riding {
    let mut riding = Riding::Away;
    for end in (1..line.len()).rev() {
        let (a, b) = (line[end - 1], line[end]);
        let (a, b) = ((a.east, a.north), (b.east, b.north));
        let (off, slack) = box_gap(p, a, b);
        if off > ON_M + slack || off_segment(p, a, b) >= ON_M {
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

fn against(heading: Option<u16>, a: &Crumb, b: &Crumb) -> bool {
    let way = bearing((b.east, b.north), (a.east, a.north));
    heading
        .is_some_and(|heading| (TURN / 4..=TURN * 3 / 4).contains(&((heading + TURN - way) % TURN)))
}

fn nearest_within(
    line: &[Crumb],
    p: Point,
    ends: core::ops::RangeInclusive<usize>,
    floor: f32,
    ceiling: f32,
    cost: impl Fn(&Nearest, &Crumb, &Crumb) -> Option<f32>,
) -> Option<Nearest> {
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

impl Following {
    fn from(line: &[Crumb], end: usize) -> Self {
        let to_end_m = line.get(end).map_or(0.0, |c| c.along);
        Self {
            nav: Navigator::default(),
            segment: end,
            matched: None,
            matched_fix: None,
            to_end_m,
            furthest_to_end_m: to_end_m,
            search_whole_course: false,
        }
    }

    fn nearest(
        &self,
        line: &[Crumb],
        heading: Option<u16>,
        p: Point,
        ends: core::ops::RangeInclusive<usize>,
    ) -> Option<Nearest> {
        let plain = |near: &Nearest, _: &Crumb, _: &Crumb| Some(near.off);
        let (to_end_m, furthest) = (self.to_end_m, self.furthest_to_end_m);
        if self.after_gap(p) {
            return nearest_within(line, p, ends, to_end_m - JUMP_M, to_end_m + BACK_M, plain);
        }
        let on = |near: &Nearest, a: &Crumb, b: &Crumb| {
            let back = (near.to_end_m - furthest - SLIP_M).max(0.0);
            let against = if against(heading, a, b) {
                AGAINST_M
            } else {
                0.0
            };
            (near.off <= ON_M).then_some(near.off + back + against)
        };
        let Some(near) = nearest_within(
            line,
            p,
            ends.clone(),
            to_end_m - NEAR_M,
            to_end_m + BACK_M,
            on,
        ) else {
            return nearest_within(line, p, ends, to_end_m - JUMP_M, to_end_m + BACK_M, plain);
        };
        if near.to_end_m > furthest + SLIP_M
            && against(heading, &line[near.end - 1], &line[near.end])
        {
            let ahead = |near: &Nearest, a: &Crumb, b: &Crumb| {
                (near.off <= ON_M && !against(heading, a, b)).then_some(near.off)
            };
            let window = to_end_m - JUMP_M..=to_end_m + BACK_M;
            if let Some(ahead) =
                nearest_within(line, p, ends, *window.start(), *window.end(), ahead)
            {
                return Some(ahead);
            }
        }
        Some(near)
    }

    fn after_gap(&self, p: Point) -> bool {
        self.matched_fix.is_none_or(|fix| distance(fix, p) > HOP_M)
    }

    fn update(&mut self, line: &[Crumb], heading: Option<u16>, p: Point) -> Option<Matched> {
        let len = line.len();
        let window = self.segment.saturating_sub(SEARCH_BACK).max(1)
            ..=(self.segment + SEARCH_AHEAD).min(len - 1);
        let Some(mut near) = self.nearest(line, heading, p, window.clone()) else {
            why!("follow.unmatched", window, after_gap = self.after_gap(p));
            return None;
        };
        let search_whole = self.search_whole_course || near.off > OFF_M;
        if search_whole
            && let Some(anywhere) = self.nearest(line, heading, p, 1..=len - 1)
            && anywhere.off < near.off
        {
            near = anywhere;
        }
        if near.off <= ON_M && near.to_end_m < END_M {
            why!(
                "follow.end",
                off_m = near.off,
                on_m = ON_M,
                to_end_m = near.to_end_m,
                end_m = END_M
            );
            return Some(Matched::End);
        }
        why!(
            "follow.matched",
            segment = near.end,
            off_m = near.off,
            to_end_m = near.to_end_m,
            searched_whole_course = search_whole,
            after_gap = self.after_gap(p)
        );
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

#[cfg(not(target_os = "none"))]
impl why::State for Follower<'_> {
    fn fields(&self, out: &mut why::Fields<'_>) {
        let joining = match &self.life {
            Lifecycle::Joining(joining) => Some(joining),
            _ => None,
        };
        let following = match &self.life {
            Lifecycle::Following(following) => Some(following),
            _ => None,
        };
        out.field("life", &self.life.phase());
        out.field("placement", &self.placement);
        out.buffer("course", self.course());
        out.buffer("turns", &self.turns[..self.turn_count]);
        out.field("scan_done", &self.scan.done());
        out.field("last_fix", &self.last_fix);
        out.field("heading", &self.heading);
        out.field("taken_loop_ms", &self.taken_loop_ms);
        out.field("joining_fixes", &joining.map(|j| j.fixes));
        out.field("against_fixes", &joining.map(|j| j.against_fixes));
        out.field("segment", &following.map(|f| f.segment));
        out.field("matched", &following.and_then(|f| f.matched));
        out.field("matched_fix", &following.and_then(|f| f.matched_fix));
        out.field("to_end_m", &following.map(|f| f.to_end_m));
        out.field("furthest_to_end_m", &following.map(|f| f.furthest_to_end_m));
        out.field(
            "search_whole_course",
            &following.map(|f| f.search_whole_course),
        );
        if let Some(nav) = self.life.nav() {
            out.owner("nav", nav);
        }
    }
}

#[cfg(test)]
mod tests;
