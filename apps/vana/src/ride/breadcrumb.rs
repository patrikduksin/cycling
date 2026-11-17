//! The ride's breadcrumb, the way to its start, and passes of its start.
//!
//! The same positions tell when the rider rides past the start (`StartPass`),
//! for laps at the start: within `PASS_M` of it, then out beyond `PASS_END_M`
//! again, while moving. The start for this is where the rider first moves
//! (the ride's first fix may be a car park off the road). A ride begins at
//! the start, so a pass counts only once the rider has been `PASS_ARM_M`
//! away, and counts once until they are away again; that gap is the
//! hysteresis jitter near the start cannot cross. Waiting at the start, however
//! long, delays the pass until the rider rides on.
use crate::ride::geo::{Frame, bearing, distance};
use crate::ride::thin::{self, off_segment};
use device_api::gps::{FixState, Snapshot};

pub mod rebuild;

/// Fixes closer than this to the last one add nothing to the breadcrumb.
const MIN_STEP_M: f32 = 5.0;
/// A kept point may sit this far off the straight line it replaces; doubled
/// when a thinning pass frees too little.
const TOLERANCE_M: f32 = 6.0;
/// Fixes since the last kept point, the most the window holds.
const WINDOW: usize = 48;
/// The heading is the way from the fix this far back.
const HEADING_STEP_M: f32 = 15.0;
/// A heading older than this (the rider stopped) is unknown.
const HEADING_STALE_MS: u64 = 15_000;
/// A fix older than this leaves the guide without a position.
const FIX_LOST_MS: u64 = 5_000;
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
/// Standing still, the guide is brought up to date this often.
const STILL_MS: u64 = 1_000;

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

/// The way to the start, as of the last fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub at_start: bool,
    /// Straight to the start.
    pub bearing: u16,
    /// The way the rider is riding, while moving.
    pub heading: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    pub location_e7: (i32, i32),
    pub taken_loop_ms: u64,
}

impl Reported {
    pub fn fresh(gps: &Snapshot, now: u64) -> Option<Self> {
        if gps.state != FixState::Fresh {
            return None;
        }
        Some(Self {
            location_e7: (gps.latitude_e7?, gps.longitude_e7?),
            taken_loop_ms: now.saturating_sub(gps.age_ms?),
        })
    }
}

/// A fresh position, for what follows the rider (`course::follow`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    /// Metres east and north of the ride's first fix.
    pub at: (f32, f32),
    pub heading: Option<u16>,
    /// A new position; else the rider stands still, a second on.
    pub moved: bool,
    pub taken_loop_ms: u64,
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

pub struct Breadcrumb<'a> {
    crumbs: &'a mut [Crumb],
    len: usize,
    tolerance: f32,
    frame: Option<Frame>,
    /// Fixes since the last kept point, oldest first.
    window: [Point; WINDOW],
    window_len: usize,
    fed: Option<(i32, i32)>,
    here: Option<Point>,
    here_ms: u64,
    heading_from: Option<Point>,
    heading: Option<u16>,
    heading_ms: u64,
    arrived: bool,
    pass: StartPass,
    /// Where the rider first moved, the start laps count from.
    pass_start: Option<Point>,
    passed: bool,
    updated_ms: u64,
    guide: Option<Guide>,
}

impl<'a> Breadcrumb<'a> {
    /// A breadcrumb in `crumbs`; its length is the most points kept.
    pub fn new(crumbs: &'a mut [Crumb]) -> Self {
        assert!(crumbs.len() >= 8, "a breadcrumb needs room for a few bends");
        Self {
            crumbs,
            len: 0,
            tolerance: TOLERANCE_M,
            frame: None,
            window: [(0.0, 0.0); WINDOW],
            window_len: 0,
            fed: None,
            here: None,
            here_ms: 0,
            heading_from: None,
            heading: None,
            heading_ms: 0,
            arrived: false,
            pass: StartPass::Spent,
            pass_start: None,
            passed: false,
            updated_ms: 0,
            guide: None,
        }
    }

    /// Forget the breadcrumb: a new ride starts where its first fix is.
    pub fn reset(&mut self) {
        self.len = 0;
        self.tolerance = TOLERANCE_M;
        self.frame = None;
        self.window_len = 0;
        self.fed = None;
        self.here = None;
        self.heading_from = None;
        self.heading = None;
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

    /// The ride's frame, once it has its first fix.
    pub fn frame(&self) -> Option<Frame> {
        self.frame
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

    pub fn position_taken_ms(&self) -> Option<u64> {
        self.here.map(|_| self.here_ms)
    }

    /// While the fix is fresh: metres in a straight line from it to the
    /// ride's start, and metres of track ridden, the way back over it.
    pub fn way_back(&self) -> Option<(u32, u32)> {
        let here = self.here?;
        (self.len != 0).then(|| {
            (
                distance(here, self.start(here)) as u32,
                self.recorded_m(here) as u32,
            )
        })
    }

    /// The way the rider is riding, while moving.
    pub fn heading(&self) -> Option<u16> {
        self.heading
    }

    /// The way to the start as of the last fix, or none without a recent fix.
    pub fn guide(&self) -> Option<Guide> {
        self.guide
    }

    /// The ride's position, if fresh, once per main-loop pass. A receiver
    /// reports about once a second, so only a new position is a new fix;
    /// standing still, the same one is handed on once a second.
    pub fn observe(&mut self, reported: Option<Reported>, now: u64) -> Option<Fix> {
        let Some(Reported {
            location_e7: (lat, lon),
            taken_loop_ms,
        }) = reported
        else {
            if self.here.is_some() && now.saturating_sub(self.here_ms) > FIX_LOST_MS {
                self.here = None;
                self.fed = None;
                self.guide = None;
            }
            return None;
        };
        self.here_ms = taken_loop_ms;
        let moved = self.fed != Some((lat, lon));
        let p = if moved {
            self.fed = Some((lat, lon));
            let p = self.place_in_frame_from_first(lat, lon);
            self.here = Some(p);
            p
        } else {
            let p = self.here?;
            if now.saturating_sub(self.updated_ms) < STILL_MS {
                return None;
            }
            p
        };
        self.track_heading(p, now);
        if moved {
            self.add(p);
        }
        self.update(p, now);
        Some(Fix {
            at: p,
            heading: self.heading,
            moved,
            taken_loop_ms,
        })
    }

    /// A fix from before a restart, read back from the journal, `at_ms` into
    /// the replay: it grows the breadcrumb and moves the start pass on as a
    /// live fix would, but takes no lap.
    pub fn replay(&mut self, (lat, lon): (i32, i32), at_ms: u64) {
        let p = self.place_in_frame_from_first(lat, lon);
        self.track_heading(p, at_ms);
        self.add(p);
        self.step_pass(p);
    }

    /// Live fixes follow the replayed ones. The replay's times are not the
    /// clock's, so the heading starts again from the next fix.
    pub fn end_replay(&mut self) {
        self.heading = None;
        self.heading_from = None;
    }

    fn place_in_frame_from_first(&mut self, lat: i32, lon: i32) -> Point {
        self.frame
            .get_or_insert_with(|| Frame::at((lat, lon)))
            .place(lat, lon)
    }

    fn track_heading(&mut self, p: Point, now: u64) {
        let from = *self.heading_from.get_or_insert(p);
        if distance(from, p) >= HEADING_STEP_M {
            self.heading = Some(bearing(from, p));
            self.heading_ms = now;
            self.heading_from = Some(p);
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

    fn thin(&mut self) {
        self.len = thin::thin(self.crumbs, self.len, &mut self.tolerance, |c| {
            (c.east, c.north)
        });
        // Distances along follow the thinned line.
        for i in 1..self.len {
            let (a, b) = (self.crumbs[i - 1], self.crumbs[i]);
            self.crumbs[i].along = a.along + distance((a.east, a.north), (b.east, b.north));
        }
    }

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

    /// Whether this position completes a pass of the start, which is where
    /// the rider first moved.
    fn step_pass(&mut self, p: Point) -> bool {
        let moving = self.heading.is_some();
        if moving && self.pass_start.is_none() {
            self.pass_start = Some(p);
        }
        self.pass_start
            .is_some_and(|from| self.pass.step(distance(p, from), moving))
    }

    fn update(&mut self, p: Point, now: u64) {
        self.updated_ms = now;
        let start = self.start(p);
        let direct = distance(p, start);
        self.passed |= self.step_pass(p);
        let long_enough = self.len >= 2 && self.crumbs[self.len - 1].along >= MIN_TRACK_M;
        if self.arrived && direct > LEAVE_M {
            self.arrived = false;
        } else if !self.arrived && long_enough && direct < ARRIVE_M {
            self.arrived = true;
        }
        self.guide = Some(Guide {
            at_start: self.arrived,
            bearing: bearing(p, start),
            heading: self.heading,
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ride::course;
    use crate::ride::geo::TURN;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// About Santiago, where 1e-7 degree east is 0.0094 m.
    const LAT: i32 = -334_500_000;
    const LON: i32 = -706_600_000;

    pub(crate) fn taken(location_e7: (i32, i32), now: u64) -> Option<Reported> {
        Some(Reported {
            location_e7,
            taken_loop_ms: now,
        })
    }

    /// A position `east` and `north` metres from (LAT, LON).
    pub(crate) fn at(east: f32, north: f32) -> (i32, i32) {
        Frame::at((LAT, LON)).locate((east, north))
    }

    /// Fixes every `step` metres along straight legs between `corners`.
    pub(crate) fn path(corners: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
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

    fn ride(breadcrumb: &mut Breadcrumb, points: &[(f32, f32)], now: &mut u64) {
        for &(east, north) in points {
            *now += 1_000;
            breadcrumb.observe(taken(at(east, north), *now), *now);
        }
    }

    fn near(actual: u16, degrees: f32, within: f32) -> bool {
        let want = degrees / 360.0 * f32::from(TURN);
        let diff = (f32::from(actual) - want).rem_euclid(f32::from(TURN));
        diff.min(f32::from(TURN) - diff) <= within / 360.0 * f32::from(TURN)
    }

    #[test]
    fn straight_roads_keep_their_corners_and_few_points() {
        let mut crumbs = vec![Crumb::default(); 64];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // 1 km east, 1 km north, 500 m west: fixes every 7 m.
        let route = path(
            &[(0.0, 0.0), (1000.0, 0.0), (1000.0, 1000.0), (500.0, 1000.0)],
            7.0,
        );
        ride(&mut breadcrumb, &route, &mut now);
        // The start, both corners and a point each full window (336 m).
        let kept = breadcrumb.crumbs();
        assert!(kept.len() <= 9, "{kept:?}");
        for corner in [(1000.0, 0.0), (1000.0, 1000.0)] {
            assert!(
                kept.iter()
                    .any(|c| distance((c.east, c.north), corner) < 0.5),
                "{corner:?} in {kept:?}"
            );
        }
        let (direct_m, track_m) = breadcrumb.way_back().unwrap();
        assert!((1115..=1120).contains(&direct_m), "{direct_m}");
        assert!((2495..=2505).contains(&track_m), "{track_m}");
        let guide = breadcrumb.guide().unwrap();
        assert!(!guide.at_start);
        // Riding west, the start lies to the south-west: behind and left.
        assert!(near(guide.bearing, 206.6, 1.0), "{guide:?}");
        assert!(near(guide.heading.unwrap(), 270.0, 1.0), "{guide:?}");
    }

    #[test]
    fn a_full_breadcrumb_thins_itself_and_keeps_its_length() {
        let mut crumbs = vec![Crumb::default(); 32];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // A zig-zag of 200 m legs, each corner 30 m off the line.
        let corners: Vec<_> = (0..120)
            .map(|i| (i as f32 * 200.0, if i % 2 == 0 { 0.0 } else { 30.0 }))
            .collect();
        ride(&mut breadcrumb, &path(&corners, 8.0), &mut now);
        assert!(breadcrumb.crumbs().len() <= 32);
        assert!(breadcrumb.tolerance > TOLERANCE_M);
        // Thinning shortens the zig-zag to its trend, never past it.
        let (_, track_m) = breadcrumb.way_back().unwrap();
        assert!((23_800..=24_300).contains(&track_m), "{track_m}");
        let crumbs = breadcrumb.crumbs();
        assert!(crumbs.windows(2).all(|w| w[1].along > w[0].along));
    }
    /// The fixes a receiver reports riding `road`: each `white` metres off at
    /// most, about a bias that wanders up to `bias` metres.
    fn received(road: &[course::Point], white: f32, bias: f32, seed: u64) -> Vec<(i32, i32)> {
        let mut rng = crate::testing::Rng::new(seed);
        let mut unit = move || rng.below(2001) as f32 / 1000.0 - 1.0;
        let (mut east, mut north) = (0.0f32, 0.0f32);
        road.iter()
            .map(|fix| {
                east = (east * 0.98 + unit() * bias * 0.2).clamp(-bias, bias);
                north = (north * 0.98 + unit() * bias * 0.2).clamp(-bias, bias);
                let (e, n) = (east + unit() * white, north + unit() * white);
                Frame::at((*fix).into()).locate((e, n))
            })
            .collect()
    }

    /// The breadcrumb keeps a subsequence of the fixes, so each fix up to the
    /// last kept one lies between two of them.
    fn deviation(breadcrumb: &Breadcrumb, road: &[course::Point], fixes: &[(i32, i32)]) -> f32 {
        let kept = breadcrumb.crumbs();
        let xy = |c: Crumb| (c.east, c.north);
        let (mut worst, mut k) = (0.0f32, 0);
        for (truth, &(lat, lon)) in road.iter().zip(fixes) {
            if k + 1 == kept.len() {
                break;
            }
            if breadcrumb.frame().unwrap().place(lat, lon) == xy(kept[k + 1]) {
                k += 1;
            }
            let p = breadcrumb
                .frame()
                .unwrap()
                .place(truth.lat_e7, truth.lon_e7);
            let b = xy(kept[(k + 1).min(kept.len() - 1)]);
            worst = worst.max(off_segment(p, xy(kept[k]), b));
        }
        assert_eq!(k + 1, kept.len(), "every kept point is a fix");
        worst
    }

    /// Long winding rides, clean and with a receiver's error, keep their
    /// breadcrumb within about the tolerance it reaches as it thins.
    #[test]
    fn a_long_winding_ride_keeps_its_breadcrumb_on_the_road() {
        let cases = [
            ("200 km @ 8 m", 200.0, 8.0, 0.0, 0.0, 10.0),
            ("600 km @ 8 m", 600.0, 8.0, 0.0, 0.0, 30.0),
            ("1200 km @ 8 m", 1200.0, 8.0, 0.0, 0.0, 130.0),
            ("200 km @ 8 m, GPS 3 m", 200.0, 8.0, 3.0, 5.0, 15.0),
            ("400 km @ 6 m, GPS 3 m", 400.0, 6.0, 3.0, 5.0, 33.0),
        ];
        let mut off = Vec::new();
        for (seed, (name, km, step, white, bias, limit)) in cases.into_iter().enumerate() {
            let road = course::tests::road(km, step, seed as u64 + 1);
            let fixes = received(&road, white, bias, seed as u64 + 1);
            let mut crumbs = vec![Crumb::default(); 4096];
            let mut breadcrumb = Breadcrumb::new(&mut crumbs);
            let mut now = 0;
            for &fix in &fixes {
                now += 1_000;
                breadcrumb.observe(taken(fix, now), now);
            }
            let worst = deviation(&breadcrumb, &road, &fixes);
            std::eprintln!(
                "| {name} | {} | {:.0} m | {worst:.0} m |",
                breadcrumb.crumbs().len(),
                breadcrumb.tolerance
            );
            if worst > limit {
                off.push(name);
            }
        }
        assert!(off.is_empty(), "too far off the road: {off:?}");
    }

    #[test]
    fn no_fix_no_guide_and_a_stopped_rider_has_no_heading() {
        let mut crumbs = vec![Crumb::default(); 16];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        assert_eq!(breadcrumb.guide(), None);
        let mut now = 0;
        ride(
            &mut breadcrumb,
            &path(&[(0.0, 0.0), (0.0, 100.0)], 7.0),
            &mut now,
        );
        assert!(breadcrumb.guide().unwrap().heading.is_some());
        // Standing still: the same position, then a new one nearby.
        breadcrumb.observe(taken(at(0.0, 100.0), now + 20_000), now + 20_000);
        breadcrumb.observe(taken(at(1.0, 101.0), now + 21_000), now + 21_000);
        let guide = breadcrumb.guide().unwrap();
        assert_eq!(guide.heading, None);
        assert!(near(guide.bearing, 180.0, 2.0), "{guide:?}");
        breadcrumb.observe(None, now + 30_000);
        assert_eq!(breadcrumb.guide(), None);
        breadcrumb.reset();
        assert!(breadcrumb.crumbs().is_empty());
    }

    #[test]
    fn standing_still_hands_the_fix_on_once_a_second_and_loses_the_heading() {
        let mut crumbs = vec![Crumb::default(); 64];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        ride(
            &mut breadcrumb,
            &path(&[(0.0, 0.0), (0.0, 600.0)], 7.0),
            &mut now,
        );
        let mut still = Vec::new();
        for _ in 0..40 {
            now += 500;
            still.extend(breadcrumb.observe(taken(at(0.0, 600.0), now), now));
        }
        assert_eq!(still.len(), 20, "{still:?}");
        assert!(still.iter().all(|fix| !fix.moved));
        assert_eq!(breadcrumb.guide().unwrap().heading, None);
        assert_eq!(still.last().unwrap().heading, None);
    }

    #[test]
    fn a_ride_across_the_antimeridian_measures_the_short_way() {
        let mut crumbs = vec![Crumb::default(); 16];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        breadcrumb.observe(taken((0, 1_799_998_000), 1_000), 1_000);
        breadcrumb.observe(taken((0, -1_799_998_000), 2_000), 2_000);
        let (direct_m, _) = breadcrumb.way_back().unwrap();
        assert!((40..=50).contains(&direct_m), "{direct_m}");
    }

    #[test]
    fn back_at_the_start_after_a_loop_is_at_the_start_until_away_again() {
        let mut crumbs = vec![Crumb::default(); 64];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        ride(&mut breadcrumb, &path(&LOOP, 7.0), &mut now);
        assert!(breadcrumb.guide().unwrap().at_start);
        ride(
            &mut breadcrumb,
            &path(&[(0.0, 0.0), (0.0, 60.0)], 7.0),
            &mut now,
        );
        assert!(
            breadcrumb.guide().unwrap().at_start,
            "within the 80 m to leave"
        );
        ride(
            &mut breadcrumb,
            &path(&[(0.0, 60.0), (0.0, 120.0)], 7.0),
            &mut now,
        );
        assert!(!breadcrumb.guide().unwrap().at_start);
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
        breadcrumb: &mut Breadcrumb,
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
            breadcrumb.observe(taken(fix, *now), *now);
            if breadcrumb.take_pass() {
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
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        let found = passes(&mut breadcrumb, &lap_points(3), &mut now, 0.0);
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
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // Standing at the start for two minutes, then riding off.
        let mut points = vec![(0.0, 0.0); 120];
        points.extend(path(&[(0.0, 0.0), (800.0, 0.0)], 7.0));
        assert!(passes(&mut breadcrumb, &points, &mut now, 0.0).is_empty());
    }

    #[test]
    fn gps_jitter_at_the_start_cannot_fire_twice() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // 12 m of jitter on each axis, from a minute at the start to three laps.
        let mut points = vec![(0.0, 0.0); 60];
        points.extend(lap_points(3));
        let found = passes(&mut breadcrumb, &points, &mut now, 12.0);
        assert_eq!(found.len(), 3, "{found:?}");
        // Idling 60 m out, inside the hysteresis band, adds none; the start
        // is the first noisy fix, so it may sit 17 m from where the road starts.
        let idle = vec![(60.0, 0.0); 300];
        assert!(passes(&mut breadcrumb, &idle, &mut now, 12.0).is_empty());
    }

    #[test]
    fn waiting_at_the_start_does_not_lose_the_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // A lap, then three minutes wandering a car park at the start.
        let mut points = path(&LOOP, 7.0);
        let yard: Vec<(f32, f32)> = (0..180)
            .map(|i| {
                let angle = i as f32 / 12.0;
                (15.0 * libm::cosf(angle), 15.0 * libm::sinf(angle))
            })
            .collect();
        points.extend(&yard);
        assert!(passes(&mut breadcrumb, &points, &mut now, 0.0).is_empty());
        // Riding on counts the lap, and the next lap its own pass.
        let found = passes(&mut breadcrumb, &lap_points(1), &mut now, 0.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0] < 20, "the first on leaving: {found:?}");
        // A stop of ninety seconds at the start of a ride, mid-loops, too.
        breadcrumb.reset();
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
        let found = passes(&mut breadcrumb, &stop, &mut now, 0.0);
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_start_off_the_loop_still_gives_laps() {
        // The ride's first fix is a car park, then it joins a road.
        for off in [20.0, 30.0, 40.0] {
            let mut crumbs = vec![Crumb::default(); 256];
            let mut breadcrumb = Breadcrumb::new(&mut crumbs);
            let mut now = 0;
            let mut points = vec![(0.0, -off); 10];
            // Ride up to the road, round the loop three times, and on.
            let mut corners = vec![(0.0, -off), (0.0, 0.0)];
            for _ in 0..3 {
                corners.extend_from_slice(&LOOP[1..]);
            }
            corners.push((300.0, 0.0));
            points.extend(path(&corners, 7.0));
            let found = passes(&mut breadcrumb, &points, &mut now, 0.0);
            assert_eq!(found.len(), 3, "{off} m off: {found:?}");
        }
    }

    #[test]
    fn a_short_loop_passes_every_time() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
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
        let found = passes(&mut breadcrumb, &path(&corners, 7.0), &mut now, 0.0);
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn stopping_at_the_start_after_a_lap_is_not_a_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        let mut points = path(&LOOP, 7.0);
        points.extend(vec![(0.0, 0.0); 120]);
        assert!(passes(&mut breadcrumb, &points, &mut now, 0.0).is_empty());
    }

    #[test]
    fn an_out_and_back_passes_the_start_once_on_the_way_home() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        let out = path(&[(0.0, 0.0), (3000.0, 0.0), (0.0, 0.0), (-300.0, 0.0)], 7.0);
        let found = passes(&mut breadcrumb, &out, &mut now, 0.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0] > 800, "not on the way out: {found:?}");
        // Ending the ride at the start passes nothing.
        breadcrumb.reset();
        let there = path(&[(0.0, 0.0), (3000.0, 0.0), (0.0, 0.0)], 7.0);
        assert!(passes(&mut breadcrumb, &there, &mut now, 0.0).is_empty());
    }

    #[test]
    fn a_road_that_only_grazes_the_start_area_is_no_pass() {
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        // Out east, round and back west along a road 60 m north of the start.
        let route = path(
            &[(0.0, 0.0), (500.0, 0.0), (500.0, 60.0), (-300.0, 60.0)],
            7.0,
        );
        assert!(passes(&mut breadcrumb, &route, &mut now, 0.0).is_empty());
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
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut count = 0;
        for (index, &(east, north)) in points.iter().enumerate() {
            let t = index as f32;
            let w = core::f32::consts::TAU / noise.period_s;
            let fix = at(
                east + noise.bias * libm::sinf(w * t + phase_e) + next() * noise.white,
                north + noise.bias * libm::sinf(w * t + phase_n) + next() * noise.white,
            );
            let now = (index as u64 + 1) * 1_000;
            breadcrumb.observe(taken(fix, now), now);
            count += usize::from(breadcrumb.take_pass());
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
}
