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
/// While off track the nudge repeats this often.
const REMIND_MS: u64 = 60_000;
/// Standing still, the guide is brought up to date this often.
const STILL_MS: u64 = 1_000;
/// Following stops this far from the breadcrumb: the rider chose another way.
const DROP_M: f32 = 500.0;
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
    /// Following the breadcrumb back, whatever `mode` says now.
    pub following: bool,
    /// Straight to the start.
    pub direct_m: u32,
    /// Back along the breadcrumb, from here.
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
}

type Point = (f32, f32);

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

/// The nearest breadcrumb segment to a position.
#[derive(Clone, Copy)]
struct Nearest {
    /// The segment's far end: it runs from `end - 1` to `end`.
    end: usize,
    off: f32,
    point: Point,
    /// Metres along the breadcrumb from the start to `point`.
    along: f32,
}

pub struct Homing<'a> {
    crumbs: &'a mut [Crumb],
    len: usize,
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
    following: bool,
    /// The segment last matched while following.
    segment: usize,
    /// Where on the breadcrumb the rider last was while following.
    matched: Option<Point>,
    /// Fixes in a row riding back along the breadcrumb, and its length
    /// before the first of them.
    retrace: u8,
    retrace_len: usize,
    off: bool,
    arrived: bool,
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
            following: false,
            segment: 0,
            matched: None,
            retrace: 0,
            retrace_len: 0,
            off: false,
            arrived: false,
            reminded_ms: 0,
            updated_ms: 0,
            guide: None,
        }
    }

    /// Forget the breadcrumb: a new ride starts where its first fix is.
    pub fn reset(&mut self) {
        self.len = 0;
        self.tolerance = TOLERANCE_M;
        self.origin = None;
        self.window_len = 0;
        self.fed = None;
        self.here = None;
        self.course_from = None;
        self.heading = None;
        self.following = false;
        self.segment = 0;
        self.matched = None;
        self.retrace = 0;
        self.off = false;
        self.arrived = false;
        self.guide = None;
    }

    /// Points kept now.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn following(&self) -> bool {
        self.following
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
        self.following.then_some((self.segment, self.matched))
    }

    /// The way back as of the last fix, or none without a recent fix.
    pub fn guide(&self) -> Option<Guide> {
        self.guide
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
        let (lat0, lon0) = *self.origin.get_or_insert((lat, lon));
        if self.len == 0 {
            let radians = lat0 as f32 * 1e-7 * core::f32::consts::PI / 180.0;
            self.east_per_e7 = M_PER_E7 * cosf(radians);
        }
        // Across the antimeridian the short way round is the way ridden.
        let mut east = i64::from(lon) - i64::from(lon0);
        if east > 1_800_000_000 {
            east -= 3_600_000_000;
        } else if east < -1_800_000_000 {
            east += 3_600_000_000;
        }
        let p = (
            east as f32 * self.east_per_e7,
            (i64::from(lat) - i64::from(lat0)) as f32 * M_PER_E7,
        );
        self.here = Some(p);
        self.track_heading(p, now);
        if self.following {
            return self.update(p, now);
        }
        if self.retracing(p) {
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

    /// Follow the breadcrumb back, or stop following. Following needs a
    /// breadcrumb to follow; without one this returns false.
    pub fn set_following(&mut self, follow: bool, now: u64) -> bool {
        if !follow {
            self.stop_following();
        } else if !self.following {
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
        self.following = true;
        self.segment = self.len - 1;
        self.matched = None;
        self.off = false;
        self.reminded_ms = now;
        true
    }

    /// Recording resumes: the breadcrumb keeps the way home from where the
    /// rider left it, and goes on from there.
    fn stop_following(&mut self) {
        self.following = false;
        self.off = false;
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

    fn nearest(&self, p: Point, ends: core::ops::RangeInclusive<usize>) -> Option<Nearest> {
        let mut best: Option<Nearest> = None;
        for end in ends {
            let (a, b) = (self.crumbs[end - 1], self.crumbs[end]);
            let (point, t) = project(p, (a.east, a.north), (b.east, b.north));
            let off = distance(p, point);
            if best.is_none_or(|best| off < best.off) {
                best = Some(Nearest {
                    end,
                    off,
                    point,
                    along: a.along + t * (b.along - a.along),
                });
            }
        }
        best
    }

    /// The breadcrumb `along` metres from the start, at or before segment `end`.
    fn at_along(&self, end: usize, along: f32) -> Point {
        let mut end = end;
        while end > 1 && self.crumbs[end - 1].along > along {
            end -= 1;
        }
        let (a, b) = (self.crumbs[end - 1], self.crumbs[end]);
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

    fn update(&mut self, p: Point, now: u64) -> Option<Event> {
        self.updated_ms = now;
        let start = (0.0, 0.0);
        let direct = distance(p, start);
        let mut event = None;
        let (mode, track, off, target) = if self.following && self.len >= 2 {
            let window = self.segment.saturating_sub(SEARCH_BACK).max(1)
                ..=(self.segment + SEARCH_AHEAD).min(self.len - 1);
            let mut near = self.nearest(p, window)?;
            if (self.off || near.off > OFF_M)
                && let Some(anywhere) = self.nearest(p, 1..=self.len - 1)
                && anywhere.off < near.off
            {
                near = anywhere;
            }
            if near.off > DROP_M {
                self.stop_following();
                self.update(p, now);
                return Some(Event::Dropped);
            }
            self.segment = near.end;
            self.matched = Some(near.point);
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
            } else if self.off && now.saturating_sub(self.reminded_ms) >= REMIND_MS {
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
        let long_enough = self.len >= 2 && self.crumbs[self.len - 1].along >= MIN_TRACK_M;
        if self.arrived && direct > LEAVE_M {
            self.arrived = false;
        } else if !self.arrived && long_enough && direct < ARRIVE_M {
            self.arrived = true;
            self.off = false;
            event = Some(Event::Arrived);
        }
        let mode = if self.arrived { Mode::Arrived } else { mode };
        let target = if distance(p, target) < 1.0 {
            start
        } else {
            target
        };
        self.guide = Some(Guide {
            mode,
            following: self.following,
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
}
