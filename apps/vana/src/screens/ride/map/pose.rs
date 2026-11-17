use super::roads::Detail;
use crate::ride::course::follow::moving;
use crate::ride::geo::rotation;
use core::ops::Range;
use libm::roundf;

pub type Point = (f32, f32);

pub const WIDTH: f32 = crate::screens::chrome::WIDTH as f32;
/// The map's rows; the footer is below.
pub const GROUND: Range<usize> = 22..290;
pub const RIDER_Y: i16 = (GROUND.start + (GROUND.end - GROUND.start) * 3 / 4) as i16;
const ROWS_AHEAD: f32 = (RIDER_Y as usize - GROUND.start) as f32;
const START_AHEAD_M: f32 = 300.0;
const AHEAD_S: f32 = 40.0;
const NEAREST_AHEAD_M: f32 = 150.0;
pub(super) const CLOSEST: Reach = Reach::new(NEAREST_AHEAD_M as u16, Detail::Streets);
const FURTHEST_AHEAD_M: f32 = 1_000.0;
const EASE_PER_FIX: f32 = 0.2;
const BAR_MAX_PX: f32 = 90.0;
const BARS: [(f32, &[u8]); 3] = [(250.0, b"250 M"), (100.0, b"100 M"), (50.0, b"50 M")];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reach {
    ahead_m: u16,
    detail: Detail,
}

impl Default for Reach {
    fn default() -> Self {
        Zoom::default().reach()
    }
}

impl Reach {
    pub fn ahead_m(self) -> f32 {
        f32::from(self.ahead_m)
    }

    pub(super) const fn new(ahead_m: u16, detail: Detail) -> Self {
        Self { ahead_m, detail }
    }

    pub(super) fn detail(self) -> Detail {
        self.detail
    }

    pub fn px_per_m(self) -> f32 {
        ROWS_AHEAD / f32::from(self.ahead_m)
    }

    pub fn bar(self) -> (usize, &'static [u8]) {
        let s = self.px_per_m();
        let (m, label) = BARS
            .into_iter()
            .find(|&(m, _)| m * s <= BAR_MAX_PX)
            .unwrap_or(BARS[BARS.len() - 1]);
        (roundf(m * s) as usize, label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    ahead_m: f32,
    detail: Detail,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            ahead_m: START_AHEAD_M,
            detail: Detail::Streets,
        }
    }
}

impl Zoom {
    pub fn observe_riding_at(&mut self, speed_mm_s: u32) {
        if !moving(speed_mm_s) {
            return;
        }
        let goal =
            (speed_mm_s as f32 * (AHEAD_S / 1000.0)).clamp(NEAREST_AHEAD_M, FURTHEST_AHEAD_M);
        self.ahead_m += (goal - self.ahead_m) * EASE_PER_FIX;
        self.detail = self.detail.at(self.ahead_m);
    }

    pub fn reach(self) -> Reach {
        Reach::new(roundf(self.ahead_m) as u16, self.detail)
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
    pub fn new(reach: Reach, map_left: usize, heading: u16) -> Self {
        Self {
            reach,
            rider: (((map_left + WIDTH as usize) / 2) as i16, RIDER_Y),
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

    pub fn back(&self, p: Point) -> Point {
        let right = (p.0 - self.rider.0) / self.scale;
        let ahead = (self.rider.1 - p.1) / self.scale;
        (
            self.here.0 + right * self.cos + ahead * self.sin,
            self.here.1 - right * self.sin + ahead * self.cos,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::course::follow::MOVING_FROM_MM_S;
    use crate::ride::geo::TURN;

    const KMH: f32 = 1_000_000.0 / 3_600.0;

    fn settled(kmh: f32) -> Zoom {
        let mut zoom = Zoom::default();
        for _ in 0..60 {
            zoom.observe_riding_at((kmh * KMH) as u32);
        }
        zoom
    }

    #[test]
    fn a_projection_puts_the_heading_up_and_the_rider_where_posed() {
        let pose = Pose::new(Reach::new(335, Detail::Streets), 0, TURN / 4);
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
        assert_eq!(
            Pose::new(Reach::new(300, Detail::Streets), 28, 0).rider,
            (134, 223)
        );
    }

    #[test]
    fn a_ride_starts_showing_300_m_ahead() {
        assert_eq!(Zoom::default().reach(), Reach::new(300, Detail::Streets));
    }

    #[test]
    fn at_a_steady_speed_the_map_shows_40_s_ahead_within_150_m_to_1_km() {
        for (kmh, ahead_m) in [(10.0, 150), (25.0, 278), (40.0, 444), (120.0, 1_000)] {
            assert_eq!(settled(kmh).reach().ahead_m, ahead_m, "{kmh} km/h");
        }
    }

    #[test]
    fn a_change_of_speed_eases_in_over_several_fixes_without_turning_back() {
        for (from, to) in [(10.0, 40.0), (40.0, 10.0)] {
            let mut zoom = settled(from);
            let goal = settled(to).reach().ahead_m;
            let mut seen = std::vec![zoom.reach().ahead_m];
            for _ in 0..30 {
                zoom.observe_riding_at((to * KMH) as u32);
                seen.push(zoom.reach().ahead_m);
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
        for still in [0, 1_000] {
            let mut zoom = riding;
            for _ in 0..30 {
                zoom.observe_riding_at(still);
            }
            assert_eq!(zoom, riding, "{still} mm/s");
        }
    }

    #[test]
    fn from_5_kmh_the_scale_follows_the_speed_and_below_it_holds() {
        let riding = settled(40.0);
        let mut zoom = riding;
        for _ in 0..30 {
            zoom.observe_riding_at(MOVING_FROM_MM_S - 1);
        }
        assert_eq!(zoom, riding, "just under 5 km/h");
        assert_eq!(
            settled(5.0).reach(),
            Reach::new(150, Detail::Streets),
            "5 km/h"
        );
    }

    #[test]
    fn the_scale_bar_reads_a_round_distance_up_to_90_px() {
        let bar = |ahead_m| Reach::new(ahead_m, Detail::Streets).bar();
        assert_eq!(bar(150), (67, &b"50 M"[..]));
        assert_eq!(bar(278), (72, &b"100 M"[..]));
        assert_eq!(bar(335), (60, &b"100 M"[..]));
        assert_eq!(bar(444), (45, &b"100 M"[..]));
        assert_eq!(bar(1_000), (50, &b"250 M"[..]));
    }
}
