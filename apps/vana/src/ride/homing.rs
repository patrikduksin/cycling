//! Back to start: a thinned breadcrumb of the ride, and the way back to where
//! it began, either straight or along the breadcrumb.
//!
//! Positions become metres east and north of the ride's first fix. The
//! breadcrumb keeps a point only where the road bends: a streaming window
//! simplification keeps the last point before any point in the window would
//! stray more than the tolerance from a straight line. When the caller's
//! buffer fills, the breadcrumb is thinned again in place at twice the
//! tolerance, so a ride of any length fits. The buffer is the caller's
//! (PSRAM on the device); this state is small.
//!
//! Following the breadcrumb back freezes it and steers to a point a little
//! way along it towards the start. More than `OFF_M` from it is off track
//! until the rider is back within `ON_M`; `DROP_M` off it, following stops.
//! Riding back along the breadcrumb, against the way it was ridden, starts
//! following by itself, so no button is needed; the rider may also start or
//! stop it. Stopping keeps the breadcrumb up to where the rider left it, so
//! it still leads home. The heading comes from successive fixes, so the
//! arrow can point relative to the way the rider rides.
//!
//! A loaded course (`set_course`) is a second line to follow, placed in the
//! ride's frame once the ride has its first fix. It is kept end first, so
//! following it is the same walk towards index 0 as following the breadcrumb
//! home, and its `along` is metres to its end. Riding along it, within
//! `ON_M` and its way, for `JOIN_FIXES` fixes in a row, follows it by itself;
//! crossing it does not. While it is followed the breadcrumb keeps growing.
//! Off the course is OFF TRACK once, with no reminders; the match moves at
//! most `JUMP_M` on along the course at a time, so the far leg of an
//! out-and-back or the end of a loop never stands in for where the rider is.
//! Dropping it, reaching its end (`CourseEnd`) or stopping it holds it off
//! from following by itself for the rest of the ride; the pad can still
//! follow it. Whenever the course is not followed the
//! breadcrumb is followed back as without one.
//!
//! The same positions tell when the rider rides past the start (`StartPass`),
//! for laps at the start: within `PASS_M` of it, then out beyond `PASS_END_M`
//! again, while moving. The start for this is where the rider first moves
//! (the ride's first fix may be a car park off the road). A ride begins at
//! the start, so a pass counts only once the rider has been `PASS_ARM_M`
//! away, and counts once until they are away again; that gap is the
//! hysteresis jitter near the start cannot cross. Waiting at the start, however
//! long, delays the pass until the rider rides on.
use crate::ride::course;
use libm::{atan2f, cosf, sqrtf};

/// Metres per 1e-7 degree of latitude, on a sphere of the mean Earth radius.
const M_PER_E7: f32 = 0.011_119_508;
/// Fixes closer than this to the last one add nothing to the breadcrumb.
const MIN_STEP_M: f32 = 5.0;
/// A kept point may sit this far off the straight line it replaces; doubled
/// on each thinning.
const TOLERANCE_M: f32 = 6.0;
/// Fixes since the last kept point, the most the window holds.
const WINDOW: usize = 48;
/// The heading is the way from the fix this far back.
const HEADING_STEP_M: f32 = 15.0;
/// A heading older than this (the rider stopped) is unknown.
const HEADING_STALE_MS: u64 = 15_000;
/// A fix older than this leaves the guide without a position.
const FIX_LOST_MS: u64 = 5_000;
/// Off track beyond this from the breadcrumb, back on within `ON_M`.
const OFF_M: f32 = 50.0;
const ON_M: f32 = 25.0;
/// Following steers to the breadcrumb this far ahead, towards the start.
const LOOKAHEAD_M: f32 = 40.0;
/// Segments behind and ahead of the last match searched first.
const SEARCH_BACK: usize = 64;
const SEARCH_AHEAD: usize = 4;
/// At the start within this, once the breadcrumb is `MIN_TRACK_M` long;
/// arrives again only after leaving by `LEAVE_M`.
const ARRIVE_M: f32 = 30.0;
const LEAVE_M: f32 = 80.0;
const MIN_TRACK_M: f32 = 300.0;
/// A pass of the start comes within this, and ends when the rider is `PASS_END_M`
/// away again; the gap keeps jitter at the edge from ending it early.
const PASS_M: f32 = 40.0;
const PASS_END_M: f32 = 50.0;
/// Another pass counts only after being this far from the start.
const PASS_ARM_M: f32 = 100.0;
/// While off track the nudge repeats this often.
const REMIND_MS: u64 = 60_000;
/// Standing still, the guide is brought up to date this often.
const STILL_MS: u64 = 1_000;
/// Following stops this far from the breadcrumb: the rider chose another way.
const DROP_M: f32 = 500.0;
/// Following the course, the match moves at most this far on towards its
/// end, and back only this far behind where it was, in one fix.
const JUMP_M: f32 = 500.0;
const BACK_M: f32 = ON_M;
/// Riding along the course: this many fixes in a row within `ON_M` of it,
/// heading its way within an eighth of a turn.
const JOIN_FIXES: u8 = 5;
/// Riding back along the breadcrumb: this close to it, against its way, on
/// this many fixes in a row, away from its newest `RETRACE_SKIP_M`.
const RETRACE_M: f32 = 20.0;
const RETRACE_FIXES: u8 = 8;
const RETRACE_SKIP_M: f32 = 300.0;
/// Angles are in 4096ths of a turn, clockwise from north.
pub const TURN: u16 = 4096;

/// One kept point: metres east and north of the start, and metres along the
/// breadcrumb from it. All zero bytes are a valid `Crumb`, so composition
/// may hand over zeroed memory.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crumb {
    pub east: f32,
    pub north: f32,
    pub along: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Straight to the start.
    Direct,
    /// Along the breadcrumb, on it.
    Track,
    /// Following, but more than `OFF_M` from the breadcrumb: back to it.
    OffTrack,
    /// At the start.
    Arrived,
}

/// The way back, as of the last fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub mode: Mode,
    /// Following the breadcrumb back, or the course, whatever `mode` says now.
    pub following: bool,
    /// Following the course, towards its end.
    pub course: bool,
    /// Not following: the course is loaded and within `DROP_M`, so the pad
    /// follows it rather than the breadcrumb.
    pub course_near: bool,
    /// Straight to the start, or to the course's end while following it.
    pub direct_m: u32,
    /// Back along the breadcrumb, or on along the course, from here.
    pub track_m: u32,
    /// From the breadcrumb, while following.
    pub off_m: u32,
    /// Where to ride: to the start, or to the breadcrumb ahead.
    pub bearing: u16,
    /// The way the rider is riding, while moving.
    pub heading: Option<u16>,
}

impl Guide {
    /// The arrow's angle: relative to the heading, or north up without one.
    pub fn arrow(&self) -> u16 {
        match self.heading {
            Some(heading) => (self.bearing + TURN - heading) % TURN,
            None => self.bearing,
        }
    }
}

/// Which line a follow is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Breadcrumb,
    Course,
}

/// What a fix changed, for a nudge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Riding back along the breadcrumb: following it started by itself.
    Following,
    /// Left the breadcrumb while following, or still off it a while later.
    OffTrack,
    /// Back on the breadcrumb.
    OnTrack,
    /// So far off the breadcrumb that following stopped.
    Dropped,
    Arrived,
    /// At the end of the course.
    CourseEnd,
}

type Point = (f32, f32);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum StartPass {
    /// At or near the start, or just passed it: not counting until the
    /// rider is `PASS_ARM_M` away. A ride begins here.
    #[default]
    Spent,
    /// Away from the start, so the next approach counts.
    Armed,
    /// Within `PASS_M`.
    Inside,
}

impl StartPass {
    /// Take the rider's distance from the start; true as a pass completes:
    /// the rider came near, then left while moving. However long they wait
    /// near the start, the pass completes when they ride on.
    fn step(&mut self, direct_m: f32, moving: bool) -> bool {
        match *self {
            Self::Spent => {
                if direct_m > PASS_ARM_M {
                    *self = Self::Armed;
                }
                false
            }
            Self::Armed => {
                if direct_m < PASS_M {
                    *self = Self::Inside;
                }
                false
            }
            Self::Inside => {
                if direct_m > PASS_END_M {
                    *self = Self::Spent;
                    return moving;
                }
                false
            }
        }
    }
}

fn distance(a: Point, b: Point) -> f32 {
    let (east, north) = (b.0 - a.0, b.1 - a.1);
    sqrtf(east * east + north * north)
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

fn off_segment(p: Point, a: Point, b: Point) -> f32 {
    distance(p, project(p, a, b).0)
}

/// The bearing from `from` to `to`, in `TURN`ths clockwise from north.
fn bearing(from: Point, to: Point) -> u16 {
    let radians = atan2f(to.0 - from.0, to.1 - from.1);
    let turns = radians / core::f32::consts::TAU;
    let turns = if turns < 0.0 { turns + 1.0 } else { turns };
    ((turns * f32::from(TURN)) as u32 % u32::from(TURN)) as u16
}

fn metres(value: f32) -> u32 {
    (value + 0.5) as u32
}

/// Metres east per 1e-7 degree of longitude at this latitude.
fn east_per_e7(lat_e7: i32) -> f32 {
    let radians = lat_e7 as f32 * 1e-7 * core::f32::consts::PI / 180.0;
    M_PER_E7 * cosf(radians)
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

/// The nearest segment of the followed line to a position.
#[derive(Clone, Copy)]
struct Nearest {
    /// The segment's far end: it runs from `end - 1` to `end`.
    end: usize,
    off: f32,
    point: Point,
    /// Metres along the line from its index 0 to `point`.
    along: f32,
}

pub struct Homing<'a> {
    crumbs: &'a mut [Crumb],
    len: usize,
    /// The course, end first, in this frame; empty without one.
    line: &'a mut [Crumb],
    line_len: usize,
    /// A course is loaded but waits for the ride's first fix to be placed.
    unplaced: bool,
    /// The course is off for the rest of the ride: dropped, ended or stopped.
    course_held: bool,
    /// Fixes in a row riding along the course, at its segment `join_end`.
    join_fixes: u8,
    join_end: usize,
    /// Metres to the course's end where the rider was last matched.
    course_along: f32,
    tolerance: f32,
    origin: Option<(i32, i32)>,
    /// Metres per 1e-7 degree of longitude at the start.
    east_per_e7: f32,
    /// Fixes since the last kept point, oldest first.
    window: [Point; WINDOW],
    window_len: usize,
    fed: Option<(i32, i32)>,
    here: Option<Point>,
    here_ms: u64,
    course_from: Option<Point>,
    heading: Option<u16>,
    heading_ms: u64,
    following: Option<Line>,
    /// The segment last matched while following.
    segment: usize,
    /// Where on the breadcrumb the rider last was while following.
    matched: Option<Point>,
    /// Fixes in a row riding back along the breadcrumb, and its length
    /// before the first of them.
    retrace: u8,
    retrace_len: usize,
    /// Riding back along the breadcrumb starts following by itself; off
    /// while the back-to-start page is, since its pad could not stop it.
    auto_follow: bool,
    off: bool,
    arrived: bool,
    pass: StartPass,
    /// Where the rider first moved, the start laps count from.
    pass_start: Option<Point>,
    passed: bool,
    reminded_ms: u64,
    updated_ms: u64,
    guide: Option<Guide>,
}

impl<'a> Homing<'a> {
    /// A breadcrumb in `crumbs`; its length is the most points kept.
    pub fn new(crumbs: &'a mut [Crumb]) -> Self {
        assert!(crumbs.len() >= 8, "a breadcrumb needs room for a few bends");
        Self {
            crumbs,
            len: 0,
            line: &mut [],
            line_len: 0,
            unplaced: false,
            course_held: false,
            join_fixes: 0,
            join_end: 0,
            course_along: 0.0,
            tolerance: TOLERANCE_M,
            origin: None,
            east_per_e7: M_PER_E7,
            window: [(0.0, 0.0); WINDOW],
            window_len: 0,
            fed: None,
            here: None,
            here_ms: 0,
            course_from: None,
            heading: None,
            heading_ms: 0,
            following: None,
            segment: 0,
            matched: None,
            retrace: 0,
            retrace_len: 0,
            auto_follow: true,
            off: false,
            arrived: false,
            pass: StartPass::Spent,
            pass_start: None,
            passed: false,
            reminded_ms: 0,
            updated_ms: 0,
            guide: None,
        }
    }

    /// Room for a course of up to `line.len()` points.
    pub fn with_line(mut self, line: &'a mut [Crumb]) -> Self {
        self.line = line;
        self
    }

    /// Forget the breadcrumb: a new ride starts where its first fix is. A
    /// loaded course stays, to be placed again from that fix.
    pub fn reset(&mut self) {
        self.len = 0;
        self.tolerance = TOLERANCE_M;
        self.origin = None;
        self.unplaced |= self.line_len >= 2;
        self.line_len = 0;
        self.course_held = false;
        self.join_fixes = 0;
        self.window_len = 0;
        self.fed = None;
        self.here = None;
        self.course_from = None;
        self.heading = None;
        self.following = None;
        self.segment = 0;
        self.matched = None;
        self.retrace = 0;
        self.off = false;
        self.arrived = false;
        self.pass = StartPass::Spent;
        self.pass_start = None;
        self.passed = false;
        self.guide = None;
    }

    /// Whether the rider rode past the start since this was last asked.
    pub fn take_pass(&mut self) -> bool {
        core::mem::take(&mut self.passed)
    }

    /// Points kept now.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Following the breadcrumb back or the course.
    pub fn following(&self) -> bool {
        self.following.is_some()
    }

    /// Following the course.
    pub fn following_course(&self) -> bool {
        self.following == Some(Line::Course)
    }

    /// A course is loaded, placed in this ride's frame or waiting for its
    /// first fix.
    pub fn has_course(&self) -> bool {
        self.line_len >= 2 || self.unplaced
    }

    /// The course waits for `set_course` again: the ride has its first fix.
    pub fn needs_course(&self) -> bool {
        self.unplaced && self.origin.is_some()
    }

    /// The course, end first, in metres from the origin; empty until placed.
    pub fn course(&self) -> &[Crumb] {
        &self.line[..self.line_len]
    }

    /// While following the course: what is ahead is its first `end`
    /// points, from its end, and then this point on the stretch the rider
    /// is on, if matched yet.
    pub fn course_ahead(&self) -> Option<(usize, Option<(f32, f32)>)> {
        self.following_course()
            .then_some((self.segment, self.matched))
    }

    /// The course to follow, start first; fewer than two points clear it.
    /// It is placed in the ride's frame now if the ride has a fix, else
    /// once it does (`needs_course`). A follow of the breadcrumb under way
    /// carries on.
    pub fn set_course(&mut self, points: &[course::Point]) {
        if self.following_course() {
            self.stop_following();
        }
        self.line_len = 0;
        self.join_fixes = 0;
        let n = points.len().min(self.line.len());
        self.unplaced = n >= 2 && self.origin.is_none();
        if n < 2 || self.unplaced {
            return;
        }
        for k in 0..n {
            let point = points[n - 1 - k];
            let p = self.frame(point.lat_e7, point.lon_e7);
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
        self.line_len = n;
    }

    /// The kept points, oldest first, for drawing the track.
    pub fn crumbs(&self) -> &[Crumb] {
        &self.crumbs[..self.len]
    }

    /// Metres east and north of the start, as of the last fix, until it is
    /// lost.
    pub fn position(&self) -> Option<(f32, f32)> {
        self.here
    }

    /// The way the rider is riding, while moving.
    pub fn heading(&self) -> Option<u16> {
        self.heading
    }

    /// While following: the route home is the first `end` points and then
    /// this point on the segment the rider is on, if matched yet; without a
    /// match the whole breadcrumb is the way home.
    pub fn route_home(&self) -> Option<(usize, Option<(f32, f32)>)> {
        (self.following == Some(Line::Breadcrumb)).then_some((self.segment, self.matched))
    }

    /// The way back as of the last fix, or none without a recent fix.
    pub fn guide(&self) -> Option<Guide> {
        self.guide
    }

    /// Whether riding back over the breadcrumb, or along the course, starts
    /// following it. Turning it off also stops a follow already under way,
    /// so no OFF TRACK nudge can outlive the page that would stop it.
    pub fn set_auto_follow(&mut self, on: bool) {
        if !on && self.auto_follow {
            self.stop_following();
        }
        self.auto_follow = on;
    }

    /// The ride's position, if fresh, once per main-loop pass. A receiver
    /// reports about once a second, so only a new position is a new fix.
    pub fn observe(&mut self, location_e7: Option<(i32, i32)>, now: u64) -> Option<Event> {
        let Some((lat, lon)) = location_e7 else {
            if self.here.is_some() && now.saturating_sub(self.here_ms) > FIX_LOST_MS {
                self.here = None;
                self.fed = None;
                self.guide = None;
            }
            return None;
        };
        self.here_ms = now;
        if self.fed == Some((lat, lon)) {
            // Standing still: time still passes for the heading and the
            // reminders, once a second, but nothing is recorded.
            let p = self.here?;
            if now.saturating_sub(self.updated_ms) < STILL_MS {
                return None;
            }
            self.track_heading(p, now);
            return self.update(p, now);
        }
        self.fed = Some((lat, lon));
        if self.origin.is_none() {
            self.origin = Some((lat, lon));
            self.east_per_e7 = east_per_e7(lat);
        }
        let p = self.frame(lat, lon);
        self.here = Some(p);
        self.track_heading(p, now);
        if self.following == Some(Line::Breadcrumb) {
            return self.update(p, now);
        }
        if self.following.is_none() && self.joining(p) {
            self.engage_at(Line::Course, self.join_end, now);
            self.add(p);
            self.update(p, now);
            return Some(Event::Following);
        }
        if self.following_course() {
            self.add(p);
            return self.update(p, now);
        }
        if self.auto_follow && self.retracing(p) {
            // The fixes since the turn ran back over the breadcrumb.
            self.len = self.retrace_len;
            self.window_len = 0;
            self.retrace = 0;
            if self.engage(now) {
                self.update(p, now);
                return Some(Event::Following);
            }
        }
        self.add(p);
        self.update(p, now)
    }

    /// Metres east and north of the origin; across the antimeridian the
    /// short way round is the way ridden.
    fn frame(&self, lat: i32, lon: i32) -> Point {
        let (lat0, lon0) = self.origin.unwrap_or((lat, lon));
        let mut east = i64::from(lon) - i64::from(lon0);
        if east > 1_800_000_000 {
            east -= 3_600_000_000;
        } else if east < -1_800_000_000 {
            east += 3_600_000_000;
        }
        (
            east as f32 * self.east_per_e7,
            (i64::from(lat) - i64::from(lat0)) as f32 * M_PER_E7,
        )
    }

    /// Whether this fix continues a ride along the course, its way, for
    /// long enough to follow it: the earliest stretch within `ON_M` that
    /// runs the rider's way, since where the course passes twice the first
    /// pass is the one.
    fn joining(&mut self, p: Point) -> bool {
        let heading = self
            .heading
            .filter(|_| self.auto_follow && !self.course_held && self.line_len >= 2);
        let along = heading.and_then(|heading| {
            (1..self.line_len).rev().find(|&end| {
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

    /// Whether this fix continues a ride back along the breadcrumb.
    fn retracing(&mut self, p: Point) -> bool {
        let Some(heading) = self.heading.filter(|_| self.len >= 2) else {
            self.retrace = 0;
            return false;
        };
        let limit = self.crumbs[self.len - 1].along - RETRACE_SKIP_M;
        let ends = self.crumbs[..self.len].partition_point(|c| c.along <= limit);
        let near = (ends >= 2)
            .then(|| self.nearest(p, 1..=ends - 1))
            .flatten()
            .filter(|near| near.off < RETRACE_M);
        let against = near.is_some_and(|near| {
            let (a, b) = (self.crumbs[near.end - 1], self.crumbs[near.end]);
            let way = bearing((a.east, a.north), (b.east, b.north));
            let turn = (heading + TURN - way) % TURN;
            turn > TURN * 3 / 8 && turn < TURN * 5 / 8
        });
        if !against {
            self.retrace = 0;
            return false;
        }
        if self.retrace == 0 {
            self.retrace_len = self.len;
        }
        self.retrace += 1;
        self.retrace >= RETRACE_FIXES
    }

    /// Follow the course while it is near, else the breadcrumb back; or stop
    /// following, after which the course is followed again only from here,
    /// never by itself, for the rest of the ride. Following needs a line to follow; without one this
    /// returns false.
    pub fn set_following(&mut self, follow: bool, now: u64) -> bool {
        if !follow {
            self.stop_following();
        } else if self.following.is_none()
            && let Some(near) = self.here.and_then(|p| self.course_near(p))
        {
            self.engage_at(Line::Course, near.end, now);
        } else if self.following.is_none() {
            // The breadcrumb ends where the rider turns round.
            if self.window_len > 0 {
                let last = self.window[self.window_len - 1];
                self.keep(last);
                self.window_len = 0;
            }
            if !self.engage(now) {
                return false;
            }
        }
        if let Some(p) = self.here {
            self.update(p, now);
        }
        true
    }

    fn engage(&mut self, now: u64) -> bool {
        if self.len < 2 || self.crumbs[self.len - 1].along < MIN_STEP_M {
            return false;
        }
        self.engage_at(Line::Breadcrumb, self.len - 1, now);
        true
    }

    /// Follow `line` from its segment `end`.
    fn engage_at(&mut self, line: Line, end: usize, now: u64) {
        self.following = Some(line);
        self.join_fixes = 0;
        self.course_along = self.line[..self.line_len].get(end).map_or(0.0, |c| c.along);
        self.segment = end;
        self.matched = None;
        self.off = false;
        self.reminded_ms = now;
    }

    /// Recording resumes: the breadcrumb keeps the way home from where the
    /// rider left it, and goes on from there. A course stopped is held off
    /// for the rest of the ride.
    fn stop_following(&mut self) {
        let line = self.following.take();
        self.off = false;
        match line {
            None => return,
            Some(Line::Course) => {
                self.course_held = true;
                self.matched = None;
                return;
            }
            Some(Line::Breadcrumb) => {}
        }
        self.window_len = 0;
        self.retrace = 0;
        if let Some(point) = self.matched.take() {
            self.len = self.segment;
            self.keep(point);
        }
    }

    fn track_heading(&mut self, p: Point, now: u64) {
        let from = *self.course_from.get_or_insert(p);
        if distance(from, p) >= HEADING_STEP_M {
            self.heading = Some(bearing(from, p));
            self.heading_ms = now;
            self.course_from = Some(p);
        } else if now.saturating_sub(self.heading_ms) > HEADING_STALE_MS {
            self.heading = None;
        }
    }

    /// The window simplification: keep the last fix before one that would
    /// make a straight line from the last kept point stray too far.
    fn add(&mut self, p: Point) {
        if self.len == 0 {
            self.keep(p);
            return;
        }
        let anchor = self.end();
        let last = if self.window_len > 0 {
            self.window[self.window_len - 1]
        } else {
            anchor
        };
        if distance(last, p) < MIN_STEP_M {
            return;
        }
        let strays = self.window[..self.window_len]
            .iter()
            .any(|&w| off_segment(w, anchor, p) > self.tolerance);
        if self.window_len > 0 && (strays || self.window_len == WINDOW) {
            self.keep(last);
            self.window_len = 0;
        }
        self.window[self.window_len] = p;
        self.window_len += 1;
    }

    fn end(&self) -> Point {
        let last = self.crumbs[self.len - 1];
        (last.east, last.north)
    }

    fn keep(&mut self, p: Point) {
        if self.len == self.crumbs.len() {
            self.thin();
        }
        let along = match self.len {
            0 => 0.0,
            n => self.crumbs[n - 1].along + distance(self.end(), p),
        };
        self.crumbs[self.len] = Crumb {
            east: p.0,
            north: p.1,
            along,
        };
        self.len += 1;
    }

    /// Thin the full breadcrumb in place at twice the tolerance, until a
    /// quarter of it is free; past any sensible tolerance, every other
    /// point goes. Only while recording, so no match index moves.
    fn thin(&mut self) {
        let target = self.crumbs.len() * 3 / 4;
        let mut rounds = 0;
        while self.len > target {
            rounds += 1;
            self.tolerance *= 2.0;
            let mut kept = 1;
            for i in 1..self.len - 1 {
                let point = |c: Crumb| (c.east, c.north);
                let a = point(self.crumbs[kept - 1]);
                let b = point(self.crumbs[i + 1]);
                let p = point(self.crumbs[i]);
                if rounds > 8 && i % 2 == 0 || off_segment(p, a, b) > self.tolerance {
                    self.crumbs[kept] = self.crumbs[i];
                    kept += 1;
                }
            }
            self.crumbs[kept] = self.crumbs[self.len - 1];
            self.len = kept + 1;
        }
        // Distances along follow the thinned line.
        for i in 1..self.len {
            let (a, b) = (self.crumbs[i - 1], self.crumbs[i]);
            self.crumbs[i].along = a.along + distance((a.east, a.north), (b.east, b.north));
        }
    }

    /// The line being followed: the course while following it, else the
    /// breadcrumb.
    fn followed(&self) -> &[Crumb] {
        if self.following_course() {
            self.course()
        } else {
            &self.crumbs[..self.len]
        }
    }

    /// The course's nearest stretch to `p` if it is within `DROP_M`: what
    /// the pad follows, even once the course is held off from following by
    /// itself.
    fn course_near(&self, p: Point) -> Option<Nearest> {
        if self.line_len < 2 {
            return None;
        }
        let line = self.course();
        let mut best: Option<Nearest> = None;
        for end in (1..line.len()).rev() {
            let near = nearest_on(line, p, end);
            if best.is_none_or(|best| near.off < best.off) {
                best = Some(near);
            }
        }
        best.filter(|near| near.off <= DROP_M)
    }

    /// The nearest segment of the followed line among `ends`. Following the
    /// course, only segments from `JUMP_M` on to `BACK_M` behind where the
    /// rider was are in reach, and on a tie the earlier one wins; on the
    /// breadcrumb the one nearer home does.
    fn nearest(&self, p: Point, ends: core::ops::RangeInclusive<usize>) -> Option<Nearest> {
        let line = self.followed();
        let course = self.following_course();
        let (floor, ceiling) = if course {
            (self.course_along - JUMP_M, self.course_along + BACK_M)
        } else {
            (f32::MIN, f32::MAX)
        };
        let mut best: Option<Nearest> = None;
        let mut consider = |end: usize| {
            let (a, b) = (line[end - 1], line[end]);
            if b.along < floor || a.along > ceiling {
                return;
            }
            let near = nearest_on(line, p, end);
            if best.is_none_or(|best| near.off < best.off) {
                best = Some(near);
            }
        };
        if course {
            ends.rev().for_each(&mut consider);
        } else {
            ends.for_each(&mut consider);
        }
        best
    }

    /// The followed line `along` metres from its index 0, at or before
    /// segment `end`.
    fn at_along(&self, end: usize, along: f32) -> Point {
        let line = self.followed();
        let mut end = end;
        while end > 1 && line[end - 1].along > along {
            end -= 1;
        }
        let (a, b) = (line[end - 1], line[end]);
        let span = b.along - a.along;
        let t = if span > 0.0 {
            ((along - a.along) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (
            a.east + t * (b.east - a.east),
            a.north + t * (b.north - a.north),
        )
    }

    /// The breadcrumb's length from the start to the rider, while recording.
    fn recorded_m(&self, p: Point) -> f32 {
        match self.len {
            0 => 0.0,
            n => self.crumbs[n - 1].along + distance(self.end(), p),
        }
    }

    /// Where the ride began: its first kept point.
    fn start(&self, p: Point) -> Point {
        self.crumbs[..self.len]
            .first()
            .map_or(p, |first| (first.east, first.north))
    }

    fn update(&mut self, p: Point, now: u64) -> Option<Event> {
        self.updated_ms = now;
        let start = self.start(p);
        let course = self.following_course();
        let goal = match self.course().first() {
            Some(end) if course => (end.east, end.north),
            _ => start,
        };
        let direct = distance(p, goal);
        let moving = self.heading.is_some();
        if moving && self.pass_start.is_none() {
            self.pass_start = Some(p);
        }
        if let Some(from) = self.pass_start
            && self.pass.step(distance(p, from), moving)
        {
            self.passed = true;
        }
        let mut event = None;
        let len = self.followed().len();
        let (mode, track, off, target) = if self.following.is_some() && len >= 2 {
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
                self.stop_following();
                self.update(p, now);
                return Some(Event::Dropped);
            }
            if course && near.off <= ON_M && near.along < ARRIVE_M {
                self.stop_following();
                self.update(p, now);
                return Some(Event::CourseEnd);
            }
            self.segment = near.end;
            self.matched = Some(near.point);
            self.course_along = near.along;
            let was_off = self.off;
            self.off = if self.off {
                near.off > ON_M
            } else {
                near.off > OFF_M
            };
            if self.off && !was_off {
                event = Some(Event::OffTrack);
                self.reminded_ms = now;
            } else if !self.off && was_off {
                event = Some(Event::OnTrack);
            } else if self.off && !course && now.saturating_sub(self.reminded_ms) >= REMIND_MS {
                event = Some(Event::OffTrack);
                self.reminded_ms = now;
            }
            let target = if self.off {
                near.point
            } else {
                self.at_along(near.end, (near.along - LOOKAHEAD_M).max(0.0))
            };
            let mode = if self.off {
                Mode::OffTrack
            } else {
                Mode::Track
            };
            (mode, near.along + near.off, near.off, target)
        } else {
            (Mode::Direct, self.recorded_m(p), 0.0, start)
        };
        // Following a course, the end that counts is the course's.
        let long_enough =
            !course && self.len >= 2 && self.crumbs[self.len - 1].along >= MIN_TRACK_M;
        if self.arrived && direct > LEAVE_M {
            self.arrived = false;
        } else if !self.arrived && long_enough && direct < ARRIVE_M {
            self.arrived = true;
            self.off = false;
            event = Some(Event::Arrived);
        }
        let mode = if self.arrived { Mode::Arrived } else { mode };
        let target = if distance(p, target) < 1.0 {
            goal
        } else {
            target
        };
        let course_near = self.following.is_none() && self.course_near(p).is_some();
        self.guide = Some(Guide {
            mode,
            following: self.following.is_some(),
            course,
            course_near,
            direct_m: metres(direct),
            track_m: metres(track),
            off_m: metres(off),
            bearing: bearing(p, target),
            heading: self.heading,
        });
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// About Santiago, where 1e-7 degree east is 0.0094 m.
    const LAT: i32 = -334_500_000;
    const LON: i32 = -706_600_000;

    /// A position `east` and `north` metres from (LAT, LON).
    fn at(east: f32, north: f32) -> (i32, i32) {
        let cos = cosf(LAT as f32 * 1e-7 * core::f32::consts::PI / 180.0);
        (
            LAT + (north / M_PER_E7) as i32,
            LON + (east / (M_PER_E7 * cos)) as i32,
        )
    }

    /// Fixes every `step` metres along straight legs between `corners`.
    fn path(corners: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
        let mut points = vec![corners[0]];
        for pair in corners.windows(2) {
            let length = distance(pair[0], pair[1]);
            let steps = (length / step).ceil() as usize;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                points.push((
                    pair[0].0 + t * (pair[1].0 - pair[0].0),
                    pair[0].1 + t * (pair[1].1 - pair[0].1),
                ));
            }
        }
        points
    }

    fn ride(homing: &mut Homing, points: &[(f32, f32)], now: &mut u64) -> Vec<Event> {
        let mut events = Vec::new();
        for &(east, north) in points {
            *now += 1_000;
            events.extend(homing.observe(Some(at(east, north)), *now));
        }
        events
    }

    fn near(actual: u16, degrees: f32, within: f32) -> bool {
        let want = degrees / 360.0 * f32::from(TURN);
        let diff = (f32::from(actual) - want).rem_euclid(f32::from(TURN));
        diff.min(f32::from(TURN) - diff) <= within / 360.0 * f32::from(TURN)
    }

    #[test]
    fn straight_roads_keep_their_corners_and_few_points() {
        let mut crumbs = vec![Crumb::default(); 64];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // 1 km east, 1 km north, 500 m west: fixes every 7 m.
        let route = path(
            &[(0.0, 0.0), (1000.0, 0.0), (1000.0, 1000.0), (500.0, 1000.0)],
            7.0,
        );
        assert!(ride(&mut homing, &route, &mut now).is_empty());
        // The start, both corners and a point each full window (336 m).
        let kept = &homing.crumbs[..homing.len()];
        assert!(kept.len() <= 9, "{kept:?}");
        for corner in [(1000.0, 0.0), (1000.0, 1000.0)] {
            assert!(
                kept.iter()
                    .any(|c| distance((c.east, c.north), corner) < 0.5),
                "{corner:?} in {kept:?}"
            );
        }
        let guide = homing.guide().unwrap();
        assert_eq!(guide.mode, Mode::Direct);
        assert!((1115..=1120).contains(&guide.direct_m), "{guide:?}");
        assert!((2495..=2505).contains(&guide.track_m), "{guide:?}");
        // Riding west, the start lies to the south-west: behind and left.
        assert!(near(guide.bearing, 206.6, 1.0), "{guide:?}");
        assert!(near(guide.heading.unwrap(), 270.0, 1.0), "{guide:?}");
        assert!(near(guide.arrow(), 296.6, 1.0), "{guide:?}");
    }

    #[test]
    fn a_full_breadcrumb_thins_itself_and_keeps_its_length() {
        let mut crumbs = vec![Crumb::default(); 32];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // A zig-zag of 200 m legs, each corner 30 m off the line.
        let corners: Vec<_> = (0..120)
            .map(|i| (i as f32 * 200.0, if i % 2 == 0 { 0.0 } else { 30.0 }))
            .collect();
        ride(&mut homing, &path(&corners, 8.0), &mut now);
        assert!(homing.len() <= 32);
        assert!(homing.tolerance > TOLERANCE_M);
        let guide = homing.guide().unwrap();
        // Thinning shortens the zig-zag to its trend, never past it.
        assert!((23_800..=24_300).contains(&guide.track_m), "{guide:?}");
        let crumbs = &homing.crumbs[..homing.len()];
        assert!(crumbs.windows(2).all(|w| w[1].along > w[0].along));
    }

    #[test]
    fn following_back_steers_along_the_breadcrumb_and_nudges_off_track() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // Out: 600 m north, then 800 m east.
        let out = path(&[(0.0, 0.0), (0.0, 600.0), (800.0, 600.0)], 7.0);
        ride(&mut homing, &out, &mut now);
        assert!(homing.set_following(true, now));
        let guide = homing.guide().unwrap();
        assert_eq!((guide.mode, guide.following), (Mode::Track, true));
        assert!((1395..=1405).contains(&guide.track_m), "{guide:?}");
        // Back west along the road: the breadcrumb ahead is due west.
        let back = path(&[(800.0, 600.0), (300.0, 600.0)], 7.0);
        assert!(ride(&mut homing, &back, &mut now).is_empty());
        let guide = homing.guide().unwrap();
        assert!(near(guide.bearing, 270.0, 2.0), "{guide:?}");
        assert!(near(guide.arrow(), 0.0, 2.0), "straight ahead: {guide:?}");
        // A wrong turn south: off track past 50 m, steering back north.
        let wrong = path(&[(300.0, 600.0), (300.0, 520.0)], 7.0);
        assert_eq!(ride(&mut homing, &wrong, &mut now), [Event::OffTrack]);
        let guide = homing.guide().unwrap();
        assert_eq!(guide.mode, Mode::OffTrack);
        assert!((75..=85).contains(&guide.off_m), "{guide:?}");
        assert!(near(guide.bearing, 0.0, 2.0), "{guide:?}");
        assert!(near(guide.arrow(), 180.0, 2.0), "turn round: {guide:?}");
        // Still off a minute later: the nudge repeats.
        now += 59_000;
        assert_eq!(
            homing.observe(Some(at(300.0, 519.0)), now + 1_000),
            Some(Event::OffTrack)
        );
        now += 1_000;
        // Back to the road, then home along it.
        let rejoin = path(&[(300.0, 520.0), (300.0, 590.0)], 7.0);
        assert_eq!(ride(&mut homing, &rejoin, &mut now), [Event::OnTrack]);
        let home = path(&[(300.0, 600.0), (0.0, 600.0), (0.0, 0.0)], 7.0);
        assert_eq!(ride(&mut homing, &home, &mut now), [Event::Arrived]);
        let guide = homing.guide().unwrap();
        assert_eq!(guide.mode, Mode::Arrived);
        assert!(guide.direct_m < 30, "{guide:?}");
    }

    #[test]
    fn a_shortcut_back_to_the_breadcrumb_is_found_anywhere_on_it() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // A square loop of 1 km sides, ridden anticlockwise from its corner.
        let out = path(
            &[(0.0, 0.0), (1000.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0)],
            7.0,
        );
        ride(&mut homing, &out, &mut now);
        assert!(homing.set_following(true, now));
        // Straight across to the first side: a wrong way, then on track
        // again far along the breadcrumb, 900 m from the start.
        let across = path(&[(0.0, 1000.0), (500.0, 500.0), (900.0, 10.0)], 30.0);
        let events = ride(&mut homing, &across, &mut now);
        assert_eq!(events, [Event::OffTrack, Event::OnTrack]);
        let guide = homing.guide().unwrap();
        assert_eq!(guide.mode, Mode::Track);
        assert!((905..=915).contains(&guide.track_m), "{guide:?}");
        // To the breadcrumb 40 m on, towards the start.
        assert!(near(guide.bearing, 256.0, 3.0), "{guide:?}");
    }

    #[test]
    fn no_fix_no_guide_and_a_stopped_rider_has_no_heading() {
        let mut crumbs = vec![Crumb::default(); 16];
        let mut homing = Homing::new(&mut crumbs);
        assert_eq!(homing.guide(), None);
        assert!(!homing.set_following(true, 0), "nothing to follow yet");
        let mut now = 0;
        ride(
            &mut homing,
            &path(&[(0.0, 0.0), (0.0, 100.0)], 7.0),
            &mut now,
        );
        assert!(homing.guide().unwrap().heading.is_some());
        // Standing still: the same position, then a new one nearby.
        homing.observe(Some(at(0.0, 100.0)), now + 20_000);
        homing.observe(Some(at(1.0, 101.0)), now + 21_000);
        let guide = homing.guide().unwrap();
        assert_eq!(guide.heading, None);
        assert_eq!(guide.arrow(), guide.bearing, "north up");
        assert!(near(guide.bearing, 180.0, 2.0), "{guide:?}");
        homing.observe(None, now + 30_000);
        assert_eq!(homing.guide(), None);
        homing.reset();
        assert!(homing.is_empty());
    }

    #[test]
    fn standing_still_loses_the_heading_and_off_track_still_reminds() {
        let mut crumbs = vec![Crumb::default(); 64];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        ride(
            &mut homing,
            &path(&[(0.0, 0.0), (0.0, 600.0)], 7.0),
            &mut now,
        );
        assert!(homing.set_following(true, now));
        let wrong = path(&[(0.0, 600.0), (80.0, 600.0)], 7.0);
        assert_eq!(ride(&mut homing, &wrong, &mut now), [Event::OffTrack]);
        // The receiver keeps reporting the same position.
        let mut events = Vec::new();
        for _ in 0..70 {
            now += 1_000;
            events.extend(homing.observe(Some(at(80.0, 600.0)), now));
        }
        assert_eq!(events, [Event::OffTrack], "the minute's reminder");
        assert_eq!(homing.guide().unwrap().heading, None);
    }

    #[test]
    fn a_ride_across_the_antimeridian_measures_the_short_way() {
        let mut crumbs = vec![Crumb::default(); 16];
        let mut homing = Homing::new(&mut crumbs);
        homing.observe(Some((0, 1_799_998_000)), 1_000);
        homing.observe(Some((0, -1_799_998_000)), 2_000);
        let guide = homing.guide().unwrap();
        assert!((40..=50).contains(&guide.direct_m), "{guide:?}");
    }

    #[test]
    fn retracing_never_follows_while_auto_follow_is_off() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        homing.set_auto_follow(false);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (0.0, 1000.0), (1000.0, 1000.0)], 7.0);
        let back = path(&[(1000.0, 1000.0), (400.0, 1000.0)], 7.0);
        let away = path(&[(400.0, 1000.0), (400.0, 2000.0)], 10.0);
        for points in [&out, &back, &away] {
            assert!(ride(&mut homing, points, &mut now).is_empty());
        }
        assert!(!homing.following());
    }

    #[test]
    fn turning_auto_follow_off_ends_a_follow_and_silences_off_track() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (0.0, 1000.0), (1000.0, 1000.0)], 7.0);
        let back = path(&[(1000.0, 1000.0), (400.0, 1000.0)], 7.0);
        let away = path(&[(400.0, 1000.0), (400.0, 4000.0)], 10.0);
        for points in [&out, &back] {
            ride(&mut homing, points, &mut now);
        }
        assert!(homing.following());
        homing.set_auto_follow(false);
        assert!(!homing.following());
        let events = ride(&mut homing, &away, &mut now);
        assert!(events.is_empty(), "{events:?}");
        assert!(!homing.following());
    }

    #[test]
    fn riding_back_along_the_breadcrumb_follows_it_and_stopping_keeps_the_way_home() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // Out 1 km north and 1 km east, then back west along the road.
        let out = path(&[(0.0, 0.0), (0.0, 1000.0), (1000.0, 1000.0)], 7.0);
        assert!(ride(&mut homing, &out, &mut now).is_empty());
        let back = path(&[(1000.0, 1000.0), (400.0, 1000.0)], 7.0);
        assert_eq!(ride(&mut homing, &back, &mut now), [Event::Following]);
        let guide = homing.guide().unwrap();
        assert_eq!((guide.mode, guide.following), (Mode::Track, true));
        assert!((1395..=1405).contains(&guide.track_m), "{guide:?}");
        // The ride back did not stretch the breadcrumb.
        assert!(homing.crumbs[homing.len() - 1].along <= 2001.0);
        // A new way north: off track, then following stops 500 m off.
        let away = path(&[(400.0, 1000.0), (400.0, 1510.0)], 10.0);
        assert_eq!(
            ride(&mut homing, &away, &mut now),
            [Event::OffTrack, Event::Dropped]
        );
        let guide = homing.guide().unwrap();
        assert_eq!((guide.mode, guide.following), (Mode::Direct, false));
        // The breadcrumb leads home from where the rider left it.
        assert!((1895..=1915).contains(&guide.track_m), "{guide:?}");
        // Laps of a loop, the same way round, never follow by themselves.
        homing.reset();
        let lap = [
            (0.0, 0.0),
            (500.0, 0.0),
            (500.0, 500.0),
            (0.0, 500.0),
            (0.0, 0.0),
        ];
        let laps = path(&[lap, lap, lap].concat(), 7.0);
        let events = ride(&mut homing, &laps, &mut now);
        assert!(!events.contains(&Event::Following), "{events:?}");
    }

    const LOOP: [(f32, f32); 5] = [
        (0.0, 0.0),
        (500.0, 0.0),
        (500.0, 500.0),
        (0.0, 500.0),
        (0.0, 0.0),
    ];

    /// Fixes at which a pass of the start was reported, each displaced by up
    /// to `jitter` metres per axis.
    fn passes(
        homing: &mut Homing,
        points: &[(f32, f32)],
        now: &mut u64,
        jitter: f32,
    ) -> Vec<usize> {
        let mut noise = 0x2545_f491_u32;
        let mut next = move || {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            (noise % 2001) as f32 / 1000.0 - 1.0
        };
        let mut found = Vec::new();
        for (index, &(east, north)) in points.iter().enumerate() {
            *now += 1_000;
            let fix = at(east + next() * jitter, north + next() * jitter);
            homing.observe(Some(fix), *now);
            if homing.take_pass() {
                found.push(index);
            }
        }
        found
    }

    fn lap_points(laps: usize) -> Vec<(f32, f32)> {
        let mut corners = LOOP.to_vec();
        for _ in 1..laps {
            corners.extend_from_slice(&LOOP[1..]);
        }
        // Ride on past the start, as a rider does.
        corners.push((300.0, 0.0));
        path(&corners, 7.0)
    }

    #[test]
    fn each_lap_of_a_loop_passes_the_start_once() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        let found = passes(&mut homing, &lap_points(3), &mut now, 0.0);
        assert_eq!(found.len(), 3, "{found:?}");
        // A lap is 2 km: 288 fixes of 7 m, and the pass ends 35 m past the start.
        for (lap, &at) in found.iter().enumerate() {
            let want = 288 * (lap + 1) + 5;
            assert!(
                at.abs_diff(want) <= 8,
                "lap {lap} at fix {at}, loop ends near {want}"
            );
        }
    }

    #[test]
    fn the_ride_leaving_its_own_start_is_no_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // Standing at the start for two minutes, then riding off.
        let mut points = vec![(0.0, 0.0); 120];
        points.extend(path(&[(0.0, 0.0), (800.0, 0.0)], 7.0));
        assert!(passes(&mut homing, &points, &mut now, 0.0).is_empty());
    }

    #[test]
    fn gps_jitter_at_the_start_cannot_fire_twice() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // 12 m of jitter on each axis, from a minute at the start to three laps.
        let mut points = vec![(0.0, 0.0); 60];
        points.extend(lap_points(3));
        let found = passes(&mut homing, &points, &mut now, 12.0);
        assert_eq!(found.len(), 3, "{found:?}");
        // Idling 60 m out, inside the hysteresis band, adds none; the start
        // is the first noisy fix, so it may sit 17 m from where the road starts.
        let idle = vec![(60.0, 0.0); 300];
        assert!(passes(&mut homing, &idle, &mut now, 12.0).is_empty());
    }

    #[test]
    fn waiting_at_the_start_does_not_lose_the_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // A lap, then three minutes wandering a car park at the start.
        let mut points = path(&LOOP, 7.0);
        let yard: Vec<(f32, f32)> = (0..180)
            .map(|i| {
                let angle = i as f32 / 12.0;
                (15.0 * cosf(angle), 15.0 * libm::sinf(angle))
            })
            .collect();
        points.extend(&yard);
        assert!(passes(&mut homing, &points, &mut now, 0.0).is_empty());
        // Riding on counts the lap, and the next lap its own pass.
        let found = passes(&mut homing, &lap_points(1), &mut now, 0.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0] < 20, "the first on leaving: {found:?}");
        // A stop of ninety seconds at the start of a ride, mid-loops, too.
        homing.reset();
        let mut stop = path(
            &[
                (0.0, 0.0),
                (500.0, 0.0),
                (500.0, 500.0),
                (0.0, 500.0),
                (0.0, 0.0),
            ],
            7.0,
        );
        stop.extend(vec![(0.0, 0.0); 90]);
        stop.extend(lap_points(1));
        let found = passes(&mut homing, &stop, &mut now, 0.0);
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_start_off_the_loop_still_gives_laps() {
        // The ride's first fix is a car park, then it joins a road.
        for off in [20.0, 30.0, 40.0] {
            let mut crumbs = vec![Crumb::default(); 256];
            let mut homing = Homing::new(&mut crumbs);
            let mut now = 0;
            let mut points = vec![(0.0, -off); 10];
            // Ride up to the road, round the loop three times, and on.
            let mut corners = vec![(0.0, -off), (0.0, 0.0)];
            for _ in 0..3 {
                corners.extend_from_slice(&LOOP[1..]);
            }
            corners.push((300.0, 0.0));
            points.extend(path(&corners, 7.0));
            let found = passes(&mut homing, &points, &mut now, 0.0);
            assert_eq!(found.len(), 3, "{off} m off: {found:?}");
        }
    }

    #[test]
    fn a_short_loop_passes_every_time() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        let square = [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 100.0),
            (0.0, 100.0),
            (0.0, 0.0),
        ];
        let mut corners = square.to_vec();
        for _ in 1..5 {
            corners.extend_from_slice(&square[1..]);
        }
        corners.push((200.0, 0.0));
        let found = passes(&mut homing, &path(&corners, 7.0), &mut now, 0.0);
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn stopping_at_the_start_after_a_lap_is_not_a_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        let mut points = path(&LOOP, 7.0);
        points.extend(vec![(0.0, 0.0); 120]);
        assert!(passes(&mut homing, &points, &mut now, 0.0).is_empty());
    }

    #[test]
    fn an_out_and_back_passes_the_start_once_on_the_way_home() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (3000.0, 0.0), (0.0, 0.0), (-300.0, 0.0)], 7.0);
        let found = passes(&mut homing, &out, &mut now, 0.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0] > 800, "not on the way out: {found:?}");
        // Ending the ride at the start passes nothing.
        homing.reset();
        let there = path(&[(0.0, 0.0), (3000.0, 0.0), (0.0, 0.0)], 7.0);
        assert!(passes(&mut homing, &there, &mut now, 0.0).is_empty());
    }

    #[test]
    fn a_road_that_only_grazes_the_start_area_is_no_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut now = 0;
        // Out east, round and back west along a road 60 m north of the start.
        let route = path(
            &[(0.0, 0.0), (500.0, 0.0), (500.0, 60.0), (-300.0, 60.0)],
            7.0,
        );
        assert!(passes(&mut homing, &route, &mut now, 0.0).is_empty());
    }

    /// How the receiver errs: white noise per axis, and a bias that drifts
    /// slowly (a sine of `period_s` per axis, `bias` m) as multipath does.
    struct Noise {
        white: f32,
        bias: f32,
        period_s: f32,
    }

    /// Laps detected riding `laps` loops of `corners` (the first and last at
    /// the start), the first fix `off` m south of the road, under `noise`.
    fn detected(corners: &[(f32, f32)], off: f32, noise: &Noise, seed: u32, laps: usize) -> usize {
        let mut state = seed.wrapping_mul(0x9e37_79b9) | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % 2001) as f32 / 1000.0 - 1.0
        };
        let (phase_e, phase_n) = (
            next() * core::f32::consts::PI,
            next() * core::f32::consts::PI,
        );
        let mut route = vec![(0.0, -off), (0.0, 0.0)];
        for _ in 0..laps {
            route.extend_from_slice(&corners[1..]);
        }
        let last = corners[1];
        route.push((last.0 * 0.6, last.1 * 0.6));
        let mut points = vec![(0.0, -off); 10];
        points.extend(path(&route, 7.0));
        let mut crumbs = vec![Crumb::default(); 256];
        let mut homing = Homing::new(&mut crumbs);
        let mut count = 0;
        for (index, &(east, north)) in points.iter().enumerate() {
            let t = index as f32;
            let w = core::f32::consts::TAU / noise.period_s;
            let fix = at(
                east + noise.bias * libm::sinf(w * t + phase_e) + next() * noise.white,
                north + noise.bias * libm::sinf(w * t + phase_n) + next() * noise.white,
            );
            homing.observe(Some(fix), (index as u64 + 1) * 1_000);
            count += usize::from(homing.take_pass());
        }
        count
    }

    /// Measures the detection for the PR: five seeds each, three laps ridden.
    /// Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "measurement, run with --ignored --nocapture"]
    fn start_detection_table() {
        let shapes: [(&str, &[(f32, f32)]); 4] = [
            ("500 m square, start at a 90 degree corner", &LOOP),
            (
                "start mid-edge, straight through",
                &[
                    (0.0, 0.0),
                    (250.0, 0.0),
                    (250.0, 500.0),
                    (-250.0, 500.0),
                    (-250.0, 0.0),
                    (0.0, 0.0),
                ],
            ),
            (
                "diagonal loop, start at a 90 degree corner",
                &[
                    (0.0, 0.0),
                    (400.0, 400.0),
                    (0.0, 800.0),
                    (-400.0, 400.0),
                    (0.0, 0.0),
                ],
            ),
            (
                "narrow triangle, start at a 28 degree turn",
                &[(0.0, 0.0), (600.0, 150.0), (600.0, -150.0), (0.0, 0.0)],
            ),
        ];
        let noises = [
            (
                "none",
                Noise {
                    white: 0.0,
                    bias: 0.0,
                    period_s: 600.0,
                },
            ),
            (
                "white 12 m",
                Noise {
                    white: 12.0,
                    bias: 0.0,
                    period_s: 600.0,
                },
            ),
            (
                "white 20 m",
                Noise {
                    white: 20.0,
                    bias: 0.0,
                    period_s: 600.0,
                },
            ),
            (
                "drift 15 m / 10 min + white 3 m",
                Noise {
                    white: 3.0,
                    bias: 15.0,
                    period_s: 600.0,
                },
            ),
            (
                "drift 20 m / 10 min + white 3 m",
                Noise {
                    white: 3.0,
                    bias: 20.0,
                    period_s: 600.0,
                },
            ),
            (
                "drift 20 m / 3 min + white 3 m",
                Noise {
                    white: 3.0,
                    bias: 20.0,
                    period_s: 180.0,
                },
            ),
        ];
        std::println!(
            "| loop | GPS error | first fix off the road | laps detected over 5 seeds (3 ridden) |"
        );
        std::println!("| :-- | :-- | :-- | :-- |");
        for (shape, corners) in shapes {
            for (error, noise) in &noises {
                for off in [0.0, 20.0, 35.0] {
                    let found: Vec<usize> = (1..=5)
                        .map(|seed| detected(corners, off, noise, seed, 3))
                        .collect();
                    std::println!("| {shape} | {error} | {off} m | {found:?} |");
                }
            }
        }
    }

    fn course_of(corners: &[(f32, f32)]) -> Vec<course::Point> {
        path(corners, 10.0)
            .into_iter()
            .map(|(east, north)| {
                let (lat, lon) = at(east, north);
                course::Point::new(lat, lon).unwrap()
            })
            .collect()
    }

    /// A 4 km square loop, start and end at the origin.
    const SQUARE: [(f32, f32); 5] = [
        (0.0, 0.0),
        (1000.0, 0.0),
        (1000.0, 1000.0),
        (0.0, 1000.0),
        (0.0, 0.0),
    ];

    /// Ride `points` as the app does: a fix a second, placing the course
    /// once the ride has its first fix.
    fn ride_course(
        homing: &mut Homing,
        course: &[course::Point],
        points: &[(f32, f32)],
        now: &mut u64,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        for &(east, north) in points {
            *now += 1_000;
            events.extend(homing.observe(Some(at(east, north)), *now));
            if homing.needs_course() {
                homing.set_course(course);
            }
        }
        events
    }

    #[test]
    fn a_course_is_followed_along_it_with_one_off_track_nudge_to_its_end() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        let course = course_of(&SQUARE);
        homing.set_course(&course);
        assert!(
            homing.has_course() && homing.course().is_empty(),
            "placed at the first fix"
        );
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
        assert_eq!(
            ride_course(&mut homing, &course, &out, &mut now),
            [Event::Following]
        );
        let guide = homing.guide().unwrap();
        assert!(guide.course && guide.following);
        assert!(
            (3_480..=3_520).contains(&guide.track_m),
            "{}",
            guide.track_m
        );
        assert!(near(guide.arrow(), 0.0, 10.0), "ahead is straight on");
        assert!(homing.route_home().is_none());
        // A wrong turn 150 m south, slowly: one nudge, no reminders.
        let detour = path(
            &[(500.0, 0.0), (520.0, -150.0), (540.0, 0.0), (1000.0, 0.0)],
            2.0,
        );
        assert_eq!(
            ride_course(&mut homing, &course, &detour, &mut now),
            [Event::OffTrack, Event::OnTrack]
        );
        let rest = path(
            &[(1000.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0), (0.0, 0.0)],
            7.0,
        );
        assert_eq!(
            ride_course(&mut homing, &course, &rest, &mut now),
            [Event::CourseEnd]
        );
        assert!(!homing.following());
        // The course stays off: a second lap, the same way, follows nothing.
        let lap = path(&[(0.0, 0.0), (600.0, 0.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &lap, &mut now).is_empty());
        // A new ride follows it again.
        homing.reset();
        assert_eq!(
            ride_course(&mut homing, &course, &lap, &mut now),
            [Event::Following]
        );
    }

    #[test]
    fn crossing_a_course_or_riding_it_backwards_follows_nothing() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        let course = course_of(&SQUARE);
        homing.set_course(&course);
        let mut now = 0;
        let across = path(&[(500.0, -300.0), (500.0, 500.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &across, &mut now).is_empty());
        homing.reset();
        let backwards = path(&[(1000.0, 500.0), (1000.0, 0.0), (300.0, 0.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &backwards, &mut now).is_empty());
    }

    #[test]
    fn sharing_a_kilometre_then_turning_off_nudges_once_and_lets_go() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        let course = course_of(&SQUARE);
        homing.set_course(&course);
        let mut now = 0;
        // Along the first side, then south for good, slowly.
        let ride = path(&[(0.0, 0.0), (1000.0, 0.0), (1000.0, -700.0)], 2.0);
        assert_eq!(
            ride_course(&mut homing, &course, &ride, &mut now),
            [Event::Following, Event::OffTrack, Event::Dropped]
        );
        // Back onto the course its way: it stays off for this ride.
        let back = path(
            &[
                (1000.0, -700.0),
                (700.0, -700.0),
                (700.0, 0.0),
                (1000.0, 0.0),
                (1000.0, 600.0),
            ],
            7.0,
        );
        assert!(ride_course(&mut homing, &course, &back, &mut now).is_empty());
    }

    #[test]
    fn without_the_back_page_or_once_stopped_no_course_is_followed() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        let course = course_of(&SQUARE);
        homing.set_course(&course);
        homing.set_auto_follow(false);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &out, &mut now).is_empty());
        homing.set_auto_follow(true);
        let on = path(&[(500.0, 0.0), (700.0, 0.0)], 7.0);
        assert_eq!(
            ride_course(&mut homing, &course, &on, &mut now),
            [Event::Following]
        );
        // The pad or MAP stops it: riding on along it follows nothing...
        assert!(homing.set_following(false, now));
        let more = path(&[(700.0, 0.0), (1000.0, 0.0), (1000.0, 300.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &more, &mut now).is_empty());
        // ...but the pad follows it again while it is near.
        assert!(homing.guide().unwrap().course_near);
        assert!(homing.set_following(true, now) && homing.following_course());
        let on = path(&[(1000.0, 300.0), (1000.0, 600.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &on, &mut now).is_empty());
        let guide = homing.guide().unwrap();
        assert!(
            guide.course && (2_380..=2_420).contains(&guide.track_m),
            "{guide:?}"
        );
    }

    #[test]
    fn a_course_far_away_leaves_back_to_start_as_it_was() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        // At latitude 60, half a world from the ride at -33.45.
        let course: Vec<course::Point> = (0..100)
            .map(|i| course::Point::new(600_000_000 + i * 1_000, 250_000_000).unwrap())
            .collect();
        homing.set_course(&course);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (0.0, 800.0), (1200.0, 800.0)], 7.0);
        assert!(ride_course(&mut homing, &course, &out, &mut now).is_empty());
        let guide = homing.guide().unwrap();
        assert!(!guide.course_near);
        assert!(
            (1_435..=1_450).contains(&guide.direct_m),
            "{}",
            guide.direct_m
        );
        let home = 180.0 + libm::atan2f(1200.0, 800.0).to_degrees();
        assert!(near(guide.bearing, home, 2.0));
        // Riding back the way it came follows the ride's own track.
        let back = path(&[(1200.0, 800.0), (300.0, 800.0)], 7.0);
        assert_eq!(
            ride_course(&mut homing, &course, &back, &mut now),
            [Event::Following]
        );
        assert!(homing.route_home().is_some() && !homing.following_course());
        // The pad stops it and, with the course far, starts the track again.
        assert!(homing.set_following(false, now));
        assert!(homing.set_following(true, now) && !homing.following_course());
    }

    #[test]
    fn an_out_and_back_course_keeps_to_the_leg_the_rider_is_on() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        // 2.5 km north and back, the way back 4 m to the side.
        let course = course_of(&[(0.0, 0.0), (0.0, 2500.0), (4.0, 2500.0), (4.0, 0.0)]);
        homing.set_course(&course);
        let mut now = 0;
        let mut last = u32::MAX;
        let mut events = Vec::new();
        for leg in [
            path(&[(0.0, 0.0), (0.0, 2500.0)], 7.0),
            path(&[(4.0, 2500.0), (4.0, 0.0)], 7.0),
        ] {
            for point in leg.windows(1) {
                events.extend(ride_course(&mut homing, &course, point, &mut now));
                if let Some(guide) = homing.guide().filter(|g| g.course) {
                    assert!(
                        guide.track_m <= last.saturating_add(30),
                        "{} after {last}",
                        guide.track_m
                    );
                    assert_eq!(guide.mode, Mode::Track);
                    last = guide.track_m;
                }
            }
        }
        assert_eq!(events, [Event::Following, Event::CourseEnd]);
    }

    #[test]
    fn a_course_set_mid_ride_keeps_a_follow_of_the_track() {
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
        let mut homing = Homing::new(&mut crumbs).with_line(&mut line);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
        assert!(ride(&mut homing, &out, &mut now).is_empty());
        assert!(homing.set_following(true, now) && homing.route_home().is_some());
        let course = course_of(&SQUARE);
        homing.set_course(&course);
        assert!(homing.following() && !homing.following_course());
        assert!(
            !homing.course().is_empty(),
            "placed from the ride's first fix"
        );
        homing.set_course(&[]);
        assert!(!homing.has_course() && homing.following());
    }
}
