use crate::ride::homing::{TURN, distance};
use core::f32::consts::TAU;
use core::ops::Range;
use libm::{cosf, roundf, sinf};

pub type Point = (f32, f32);

pub const WIDTH: f32 = crate::screens::chrome::WIDTH as f32;
/// The map's rows; the footer is below.
pub const GROUND: Range<usize> = 22..290;
/// Three quarters of the way down the map, so most of it is the road ahead.
pub const RIDER_Y: i16 = (GROUND.start + (GROUND.end - GROUND.start) * 3 / 4) as i16;
const ROWS_AHEAD: f32 = (RIDER_Y as usize - GROUND.start) as f32;
pub(super) const ARROW_SHAFT_PX: f32 = 26.0;
pub(super) const ARROW_BEFORE_TURN_PX: f32 = 10.0;
pub(super) const ARROW_HEAD_LENGTH_PX: f32 = 10.5;
pub(super) const ARROW_HEAD_HALF_WIDTH_PX: f32 = 7.9;
pub(super) const ARROW_OUTLINE_PX: f32 = 1.75;
pub(super) const ARROW_SHAFT_HALF_PX: f32 = 4.25;
pub(super) const ARROW_STROKES: usize = 24;

const START_AHEAD_M: f32 = 300.0;
const AHEAD_S: f32 = 40.0;
const NEAREST_AHEAD_M: f32 = 150.0;
const FURTHEST_AHEAD_M: f32 = 1_000.0;
/// Slower than about 5 km/h the rider is stopped, or the speed is a
/// standing receiver's noise.
const STOPPED_BELOW_MM_S: u32 = 1_400;
/// The share of the way to its goal the zoom moves on each fix.
const EASE: f32 = 0.2;
const BAR_MAX_PX: f32 = 90.0;
const BARS: [(f32, &[u8]); 3] = [(250.0, b"250 M"), (100.0, b"100 M"), (50.0, b"50 M")];

/// Metres the map shows from the rider up to its top edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reach(u16);

impl Default for Reach {
    fn default() -> Self {
        Zoom::default().reach()
    }
}

impl Reach {
    pub fn px_per_m(self) -> f32 {
        ROWS_AHEAD / f32::from(self.0)
    }

    /// The scale bar's length in pixels, and what it reads.
    pub fn bar(self) -> (usize, &'static [u8]) {
        let s = self.px_per_m();
        let (m, label) = BARS
            .into_iter()
            .find(|&(m, _)| m * s <= BAR_MAX_PX)
            .unwrap_or(BARS[BARS.len() - 1]);
        (roundf(m * s) as usize, label)
    }
}

/// The map's scale, eased towards about `AHEAD_S` of riding ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    ahead_m: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            ahead_m: START_AHEAD_M,
        }
    }
}

impl Zoom {
    /// A new fix at `speed_mm_s`; none while the ride is paused.
    pub fn observe(&mut self, speed_mm_s: Option<u32>) {
        let Some(speed) = speed_mm_s.filter(|&v| v >= STOPPED_BELOW_MM_S) else {
            return;
        };
        let goal = (speed as f32 * (AHEAD_S / 1000.0)).clamp(NEAREST_AHEAD_M, FURTHEST_AHEAD_M);
        self.ahead_m += (goal - self.ahead_m) * EASE;
    }

    pub fn reach(self) -> Reach {
        Reach(roundf(self.ahead_m) as u16)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pose {
    pub reach: Reach,
    /// The rider's screen position.
    pub rider: (i16, i16),
    /// The heading that is up, in `TURN`ths clockwise from north.
    pub heading: u16,
}

/// A pose about one point of the ride: east-north metres to screen pixels.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    here: Point,
    sin: f32,
    cos: f32,
    scale: f32,
    rider: Point,
}

impl Pose {
    /// The rider in the middle of the map, whose left edge is `left`.
    pub fn new(reach: Reach, left: usize, heading: u16) -> Self {
        Self {
            reach,
            rider: (((left + WIDTH as usize) / 2) as i16, RIDER_Y),
            heading,
        }
    }

    pub fn projection(&self, here: Point) -> Projection {
        let (sin, cos) = rotation(self.heading);
        Projection {
            here,
            sin,
            cos,
            scale: self.reach.px_per_m(),
            rider: (f32::from(self.rider.0), f32::from(self.rider.1)),
        }
    }
}

impl Projection {
    pub fn at(&self, p: Point) -> Point {
        let (east, north) = (p.0 - self.here.0, p.1 - self.here.1);
        let (right, ahead) = (
            east * self.cos - north * self.sin,
            east * self.sin + north * self.cos,
        );
        (
            self.rider.0 + right * self.scale,
            self.rider.1 - ahead * self.scale,
        )
    }
}

fn rotation(heading: u16) -> (f32, f32) {
    let turns = f32::from(heading % TURN) / f32::from(TURN);
    (sinf(turns * TAU), cosf(turns * TAU))
}

pub(super) struct ArrowShape {
    pub(super) shaft: heapless::Vec<Point, { ARROW_STROKES + 1 }>,
    pub(super) head: [Point; 3],
}

pub(super) fn arrow_shape(
    route: &impl Fn(&mut dyn FnMut(Point, bool)),
    turn: Point,
) -> Option<ArrowShape> {
    let (mut walked, mut last, mut base, mut rider) = (0.0, None::<Point>, None, None);
    route(&mut |p, before| {
        if base.is_some() {
            return;
        }
        rider.get_or_insert(p);
        if let Some(last) = last {
            if !before {
                base = Some(walked + distance(last, turn));
                return;
            }
            walked += distance(last, p);
        }
        last = Some(p);
    });
    let (rider, base) = rider.zip(base)?;
    let start = (base - ARROW_BEFORE_TURN_PX.min(distance(rider, turn) / 4.0)).max(0.0);
    let end = start + ARROW_SHAFT_PX;
    let mut shaft: heapless::Vec<Point, { ARROW_STROKES + 1 }> = heapless::Vec::new();
    let (mut walked, mut last, mut done) = (0.0, None::<Point>, false);
    route(&mut |p, _| {
        if done {
            return;
        }
        let Some(from) = last.replace(p) else {
            return;
        };
        let length = distance(from, p);
        let point = |s: f32| {
            let t = ((s - walked) / length.max(1e-3)).clamp(0.0, 1.0);
            (from.0 + t * (p.0 - from.0), from.1 + t * (p.1 - from.1))
        };
        if shaft.is_empty() && walked + length >= start {
            let _ = shaft.push(point(start));
        }
        if walked + length >= end {
            let _ = shaft.push(point(end));
            done = true;
            return;
        }
        if !shaft.is_empty() && shaft.push(p).is_err() {
            done = true;
            return;
        }
        walked += length;
    });
    let &tip = shaft.last()?;
    let (ux, uy) = shaft.windows(2).rev().find_map(|pair| {
        let length = distance(pair[0], pair[1]);
        (length >= 0.5).then(|| {
            (
                (pair[1].0 - pair[0].0) / length,
                (pair[1].1 - pair[0].1) / length,
            )
        })
    })?;
    let head = [
        (
            tip.0 + ux * ARROW_HEAD_LENGTH_PX,
            tip.1 + uy * ARROW_HEAD_LENGTH_PX,
        ),
        (
            tip.0 - uy * ARROW_HEAD_HALF_WIDTH_PX,
            tip.1 + ux * ARROW_HEAD_HALF_WIDTH_PX,
        ),
        (
            tip.0 + uy * ARROW_HEAD_HALF_WIDTH_PX,
            tip.1 - ux * ARROW_HEAD_HALF_WIDTH_PX,
        ),
    ];
    Some(ArrowShape { shaft, head })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KMH: f32 = 1_000_000.0 / 3_600.0;

    fn settled(kmh: f32) -> Zoom {
        let mut zoom = Zoom::default();
        for _ in 0..60 {
            zoom.observe(Some((kmh * KMH) as u32));
        }
        zoom
    }

    #[test]
    fn a_projection_puts_the_heading_up_and_the_rider_where_posed() {
        let pose = Pose::new(Reach(335), 0, TURN / 4);
        assert_eq!(pose.rider, (120, 223));
        let at = pose.projection((100.0, 50.0));
        assert_eq!(at.at((100.0, 50.0)), (120.0, 223.0));
        let east = at.at((200.0, 50.0));
        assert!((east.0 - 120.0).abs() < 1e-3 && (east.1 - 163.0).abs() < 1e-3);
        let north = at.at((100.0, 150.0));
        assert!((north.0 - 60.0).abs() < 1e-3 && (north.1 - 223.0).abs() < 1e-3);
    }

    #[test]
    fn beside_a_radar_the_rider_is_in_the_middle_of_what_is_left() {
        assert_eq!(Pose::new(Reach(300), 28, 0).rider, (134, 223));
    }

    #[test]
    fn a_ride_starts_showing_300_m_ahead() {
        assert_eq!(Zoom::default().reach(), Reach(300));
    }

    #[test]
    fn at_a_steady_speed_the_map_shows_40_s_ahead_within_150_m_to_1_km() {
        for (kmh, ahead_m) in [(10.0, 150), (25.0, 278), (40.0, 444), (120.0, 1_000)] {
            assert_eq!(settled(kmh).reach(), Reach(ahead_m), "{kmh} km/h");
        }
    }

    #[test]
    fn a_change_of_speed_eases_in_over_several_fixes_without_turning_back() {
        for (from, to) in [(10.0, 40.0), (40.0, 10.0)] {
            let mut zoom = settled(from);
            let goal = settled(to).reach().0;
            let mut seen = std::vec![zoom.reach().0];
            for _ in 0..30 {
                zoom.observe(Some((to * KMH) as u32));
                seen.push(zoom.reach().0);
            }
            let steps: std::vec::Vec<i32> = seen
                .windows(2)
                .map(|w| i32::from(w[1]) - i32::from(w[0]))
                .collect();
            assert!(
                steps
                    .iter()
                    .all(|&d| d * (i32::from(goal) - i32::from(seen[0])) >= 0),
                "{from} to {to} km/h turns back: {seen:?}"
            );
            let first = steps[0].unsigned_abs();
            let whole = (i32::from(goal) - i32::from(seen[0])).unsigned_abs();
            assert!(first * 4 < whole, "{from} to {to} km/h jumps: {seen:?}");
            let halfway = seen
                .iter()
                .position(|&m| 2 * m.abs_diff(goal) <= whole as u16)
                .unwrap();
            assert!(
                (3..=5).contains(&halfway),
                "half the way in {halfway} fixes: {seen:?}"
            );
            assert_eq!(*seen.last().unwrap(), goal, "{seen:?}");
        }
    }

    #[test]
    fn stopped_or_paused_the_scale_holds() {
        let riding = settled(40.0);
        for still in [None, Some(0), Some(1_000)] {
            let mut zoom = riding;
            for _ in 0..30 {
                zoom.observe(still);
            }
            assert_eq!(zoom, riding, "{still:?}");
        }
    }

    #[test]
    fn the_scale_bar_reads_a_round_distance_up_to_90_px() {
        let bar = |ahead_m| Reach(ahead_m).bar();
        assert_eq!(bar(150), (67, &b"50 M"[..]));
        assert_eq!(bar(278), (72, &b"100 M"[..]));
        assert_eq!(bar(335), (60, &b"100 M"[..]));
        assert_eq!(bar(444), (45, &b"100 M"[..]));
        assert_eq!(bar(1_000), (50, &b"250 M"[..]));
    }
}
