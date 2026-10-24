//! The ride's elevation profile: smoothed altitude against distance, kept
//! at a fixed number of points however long the ride.
//!
//! Points sit a fixed distance apart. When the buffer fills, every second
//! point is dropped and the spacing doubles, so the trace always spans the
//! whole ride in `CAPACITY` points or fewer: 400 bytes for a ride of any
//! length. A reading between two grid points is interpolated onto the grid,
//! and a stretch without a reading is bridged in a straight line.
//!
//! The trace starts at the first altitude reading, so a barometer that
//! reads late charts only the ride from there.
//!
//! The live ride feeds it the altitude its totals follow, and a saved ride
//! read back feeds it the altitude its journal recorded, so both draw the
//! same chart.
use crate::ride::log::Altitude;

/// The most points the trace keeps.
pub const CAPACITY: usize = 200;
/// Distance between points at the start of a ride.
const FIRST_STEP_MM: u64 = 20_000;

#[derive(Clone)]
pub struct Trace {
    points: [Altitude; CAPACITY],
    len: usize,
    step_mm: u64,
    /// Distance of the next point to place.
    next_mm: u64,
    /// The last reading: distance and altitude in decimetres.
    latest: Option<(u64, i32)>,
    /// A point was placed since `take_changed`.
    changed: bool,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            points: [Altitude::from_dm(0); CAPACITY],
            len: 0,
            step_mm: FIRST_STEP_MM,
            next_mm: 0,
            latest: None,
            changed: true,
        }
    }
}

impl Trace {
    /// Account a reading of `altitude_dm` after `distance_mm` of riding in
    /// all. Call it for every credited step; a step without a fresh reading
    /// passes `None`.
    pub fn add(&mut self, distance_mm: u64, altitude_dm: Option<i32>) {
        let Some(altitude_dm) = altitude_dm else {
            return;
        };
        let Some((from_mm, from_dm)) = self.latest.replace((distance_mm, altitude_dm)) else {
            self.next_mm = distance_mm;
            self.place(altitude_dm);
            return;
        };
        let span = distance_mm.saturating_sub(from_mm);
        while self.next_mm <= distance_mm {
            let into = self.next_mm.saturating_sub(from_mm).min(span);
            let rise = i64::from(altitude_dm - from_dm);
            let at = match span {
                0 => altitude_dm,
                _ => from_dm + (rise * into as i64 / span as i64) as i32,
            };
            self.place(at);
        }
    }

    fn place(&mut self, altitude_dm: i32) {
        if self.len == CAPACITY {
            for index in 0..CAPACITY / 2 {
                self.points[index] = self.points[index * 2];
            }
            self.len = CAPACITY / 2;
            self.step_mm *= 2;
        }
        self.changed = true;
        self.points[self.len] = Altitude::from_dm(altitude_dm);
        self.len += 1;
        self.next_mm += self.step_mm;
    }

    /// Whether the trace changed since this was last asked. A fresh trace,
    /// as after a reset, counts as changed.
    pub fn take_changed(&mut self) -> bool {
        core::mem::take(&mut self.changed)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Distance between neighbouring points.
    pub fn step_mm(&self) -> u64 {
        self.step_mm
    }

    /// Altitude of point `index`, in decimetres.
    pub fn altitude_dm(&self, index: usize) -> i32 {
        self.points[index].dm()
    }

    /// The lowest and highest point, in decimetres.
    pub fn range_dm(&self) -> Option<(i32, i32)> {
        let altitudes = || (0..self.len).map(|index| self.altitude_dm(index));
        Some((altitudes().min()?, altitudes().max()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One reading per 5 m of a ride of `metres`.
    fn ride(metres: u64, altitude_dm: impl Fn(u64) -> i32) -> Trace {
        let mut trace = Trace::default();
        for m in (0..=metres).step_by(5) {
            trace.add(m * 1000, Some(altitude_dm(m)));
        }
        trace
    }

    #[test]
    fn a_short_ride_keeps_a_point_per_step() {
        let trace = ride(1000, |m| m as i32);
        assert_eq!(trace.len(), 51);
        assert_eq!(trace.step_mm(), FIRST_STEP_MM);
        assert_eq!(trace.altitude_dm(0), 0);
        assert_eq!(trace.altitude_dm(50), 1000);
        assert_eq!(trace.range_dm(), Some((0, 1000)));
    }

    #[test]
    fn a_long_ride_halves_into_a_bounded_trace_spanning_all_of_it() {
        let hill = |m: u64| (m as i32 / 10) % 400;
        for km in [10, 40, 160, 640] {
            let trace = ride(km * 1000, hill);
            assert!(
                trace.len() <= CAPACITY && trace.len() > CAPACITY / 2,
                "{km}"
            );
            let reach = (trace.len() as u64 - 1) * trace.step_mm();
            let total = km * 1_000_000;
            assert!(
                reach <= total && total - reach < 2 * trace.step_mm(),
                "{km}"
            );
            assert_eq!(trace.altitude_dm(0), 0);
        }
        let trace = ride(40_000, hill);
        assert_eq!(trace.step_mm(), 16 * FIRST_STEP_MM);
        for index in [1, 33, 100, 150] {
            let at = index as u64 * trace.step_mm() / 1000;
            let want = hill(at);
            assert!((trace.altitude_dm(index) - want).abs() <= 2, "{index}");
        }
    }

    #[test]
    fn a_missing_stretch_is_bridged_and_a_late_start_begins_at_its_reading() {
        let mut trace = Trace::default();
        trace.add(1_000_000, None);
        assert!(trace.is_empty());
        trace.add(2_000_000, Some(100));
        trace.add(3_000_000, None);
        trace.add(2_100_000, Some(300));
        assert_eq!(trace.len(), 6);
        assert_eq!(trace.altitude_dm(0), 100);
        assert_eq!(trace.altitude_dm(5), 300);
        assert_eq!(trace.altitude_dm(2), 180);
    }
}
