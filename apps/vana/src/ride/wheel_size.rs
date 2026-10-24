//! Wheel circumference learned from GPS, for rides with a wheel-speed sensor.
//!
//! A wrong circumference makes the wheel's speed and distance wrong by the
//! same few percent. GPS distance does not depend on it, so over a long clean
//! stretch the ratio of GPS distance to wheel revolutions is the circumference
//! itself, whatever size is configured.
//!
//! Only clean riding counts. Each new fix is offered with the revolutions
//! counted so far, or as `None` when anything makes the comparison unsafe (no
//! good fix, no wheel, a paused ride). Fixes about `SEGMENT_MS` apart bound a
//! segment: a straight chord, so jitter averages out, and the revolutions
//! over the same span. A telescoping sum keeps the sensor's packet lag from
//! mattering. A segment that is slow (below 15 km/h), turns, or implies an
//! impossible size (slip, a skid, a position jump) ends the window; one odd
//! segment among good ones is dropped alone. A gap in the fixes ends it too: a
//! tunnel has no distance to compare. Once a window holds `WINDOW_MM` of GPS
//! distance it yields one size, and the estimate is the agreeing mean of the
//! last few windows, available only when enough of them agree.
//!
//! The estimate is never applied here; `worth_applying` says whether it
//! differs enough from the size in force to be worth a change. All integer
//! maths: the target has no fast double-precision hardware.
use super::gps_speed::{isqrt, offset_mm};
use core::ops::RangeInclusive;

/// The plausible circumferences, from a 20 inch wheel to a large 29er.
pub const PLAUSIBLE_MM: RangeInclusive<u16> = 1_500..=2_400;
/// A new estimate must differ from the size in force by more than this, in
/// permille (0.5 %), to be applied.
pub const APPLY_PERMILLE: u32 = 5;

/// Fixes further apart than this are a dropout (two missed 1 Hz epochs).
const GAP_MS: u64 = 2_500;
/// A segment spans at least this long, so its chord dwarfs fix jitter.
const SEGMENT_MS: u64 = 10_000;
/// Slower than this (15 km/h), position noise and wheel quantisation are too
/// large a share of the distance.
const MIN_SPEED_MM_S: u64 = 4_167;
/// Faster than this (144 km/h) is a receiver fault.
const MAX_SPEED_MM_S: u64 = 40_000;
/// The GPS distance a window covers before it yields a size.
const WINDOW_MM: u64 = 1_000_000;
/// A segment implying a size outside this is slip, a skid or a position jump,
/// whatever the wheel: roomy, so a mistaken setting never matters here.
const SEGMENT_MM: RangeInclusive<u64> = 1_300..=2_800;
/// A segment this far from its window's mean size (percent) is an outlier.
const OUTLIER_PERCENT: u64 = 10;
/// The window mean is trusted to judge outliers after this many segments.
const TRUSTED_AFTER: u32 = 3;
/// A window that drops more segments than this is not trusted at all.
const MAX_DROPPED: u8 = 3;
/// Heading change between segments beyond which the road is not straight:
/// the cosine, in permille (10 degrees).
const STRAIGHT_PERMILLE: i128 = 985;
/// The Doppler integral and the chord of a segment agree within this
/// (percent), else one of them is wrong and the segment is not used.
const DOPPLER_PERCENT: u64 = 20;
/// Window sizes kept for the estimate.
const KEPT: usize = 8;
/// Windows needed before an estimate exists.
const MIN_WINDOWS: usize = 3;
/// Windows within this of the median (permille) agree with it.
const AGREE_PERMILLE: u32 = 10;

/// One good GPS fix with what the wheel counted by then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// When the fix was seen, in monotonic milliseconds.
    pub at_ms: u64,
    pub latitude_e7: i32,
    pub longitude_e7: i32,
    /// Wheel revolutions counted since the sensor session began.
    pub revolutions: u64,
    /// The receiver's ground speed for this fix (Doppler), in mm/s.
    pub speed_mm_s: Option<u32>,
}

/// A segment's direction, in mm north and east, with its length.
#[derive(Clone, Copy)]
struct Heading {
    north: i64,
    east: i64,
    length: u64,
}

#[derive(Default)]
pub struct Calibrator {
    anchor: Option<Sample>,
    last_ms: u64,
    last_speed_mm_s: Option<u32>,
    /// Ground distance integrated from Doppler speed since the anchor, which
    /// bends and noise do not shorten; none once a fix lacked a speed.
    doppler_mm: Option<u64>,
    heading: Option<Heading>,
    /// What the current window holds so far.
    distance_mm: u64,
    revolutions: u64,
    segments: u32,
    dropped: u8,
    /// Finished windows' sizes in tenths of a mm, newest overwriting oldest.
    sizes: [u32; KEPT],
    sizes_len: usize,
    sizes_next: usize,
}

impl Calibrator {
    /// Offer the latest fix, or `None` when comparing now is unsafe.
    pub fn observe(&mut self, sample: Option<Sample>) {
        let Some(sample) = sample else {
            self.restart();
            return;
        };
        if self.anchor.is_some_and(|anchor| {
            sample.at_ms < anchor.at_ms || sample.at_ms.saturating_sub(self.last_ms) > GAP_MS
        }) {
            self.restart();
        }
        let since_ms = sample.at_ms.saturating_sub(self.last_ms);
        self.doppler_mm = match (self.doppler_mm, self.last_speed_mm_s, sample.speed_mm_s) {
            (Some(total), Some(before), Some(now)) => {
                Some(total + (u64::from(before) + u64::from(now)) / 2 * since_ms / 1000)
            }
            _ => None,
        };
        self.last_ms = sample.at_ms;
        self.last_speed_mm_s = sample.speed_mm_s;
        let Some(anchor) = self.anchor else {
            self.anchor = Some(sample);
            self.doppler_mm = Some(0);
            return;
        };
        let elapsed_ms = sample.at_ms - anchor.at_ms;
        if elapsed_ms < SEGMENT_MS {
            return;
        }
        self.anchor = Some(sample);
        let doppler_mm = self.doppler_mm.replace(0);
        match sample.revolutions.checked_sub(anchor.revolutions) {
            Some(revolutions) => self.segment(anchor, sample, elapsed_ms, revolutions, doppler_mm),
            // The sensor session began again: its count is not comparable.
            None => self.restart_at(sample),
        }
    }

    /// Forget the window and the segment in progress; finished windows stay.
    fn restart(&mut self) {
        self.anchor = None;
        self.doppler_mm = None;
        self.last_speed_mm_s = None;
        self.heading = None;
        self.discard();
    }

    fn restart_at(&mut self, sample: Sample) {
        self.restart();
        self.anchor = Some(sample);
        self.doppler_mm = Some(0);
    }

    fn discard(&mut self) {
        self.distance_mm = 0;
        self.revolutions = 0;
        self.segments = 0;
        self.dropped = 0;
    }

    /// Take one segment between `from` and `to`. Its ground distance is the
    /// Doppler integral when the receiver gave speeds and they agree with the
    /// chord, else the chord.
    fn segment(
        &mut self,
        from: Sample,
        to: Sample,
        elapsed_ms: u64,
        revolutions: u64,
        doppler_mm: Option<u64>,
    ) {
        let (north, east) = offset_mm(
            (from.latitude_e7, from.longitude_e7),
            (to.latitude_e7, to.longitude_e7),
        );
        // A glitch fix far away must not reach the squares below.
        let reach = (MAX_SPEED_MM_S * elapsed_ms / 1000) as i64;
        if north.abs() > reach || east.abs() > reach {
            self.heading = None;
            self.discard();
            return;
        }
        let chord = isqrt((north * north + east * east) as u128) as u64;
        let length = match doppler_mm {
            Some(integrated) if integrated > 0 => {
                if integrated.abs_diff(chord) * 100 > chord * DOPPLER_PERCENT {
                    self.heading = None;
                    self.discard();
                    return;
                }
                integrated
            }
            _ => chord,
        };
        let heading = Heading {
            north,
            east,
            length: chord,
        };
        let speed = length * 1000 / elapsed_ms;
        let size = length.checked_div(revolutions);
        let sound = (MIN_SPEED_MM_S..=MAX_SPEED_MM_S).contains(&speed)
            && size.is_some_and(|size| SEGMENT_MM.contains(&size));
        let straight = self.heading.is_none_or(|before| {
            let dot = i128::from(before.north) * i128::from(north)
                + i128::from(before.east) * i128::from(east);
            dot * 1000 >= STRAIGHT_PERMILLE * i128::from(before.length) * i128::from(chord)
        });
        self.heading = Some(heading);
        if !sound || !straight {
            self.discard();
            return;
        }
        if self.segments >= TRUSTED_AFTER {
            let mean = self.distance_mm / self.revolutions.max(1);
            let size = length / revolutions;
            if size.abs_diff(mean) * 100 > mean * OUTLIER_PERCENT {
                // One odd segment is dropped with its revolutions, which
                // spanned the same time; several make the window untrusted.
                self.dropped += 1;
                if self.dropped > MAX_DROPPED {
                    self.discard();
                }
                return;
            }
        }
        self.distance_mm += length;
        self.revolutions += revolutions;
        self.segments += 1;
        if self.distance_mm >= WINDOW_MM {
            let tenths = (self.distance_mm * 10 + self.revolutions / 2) / self.revolutions;
            self.sizes[self.sizes_next] = tenths.min(u64::from(u32::MAX)) as u32;
            self.sizes_next = (self.sizes_next + 1) % KEPT;
            self.sizes_len = (self.sizes_len + 1).min(KEPT);
            self.discard();
        }
    }

    /// How many windows the estimate rests on, at most the most recent few.
    pub fn windows(&self) -> usize {
        self.sizes_len
    }

    /// The circumference in mm that recent windows agree on, if enough do and
    /// it is plausible.
    pub fn estimate(&self) -> Option<u16> {
        let mut sorted = self.sizes;
        let sorted = &mut sorted[..self.sizes_len];
        if sorted.len() < MIN_WINDOWS {
            return None;
        }
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let agree = |size: &u32| size.abs_diff(median) * 1000 <= median * AGREE_PERMILLE;
        let count = sorted.iter().filter(|size| agree(size)).count();
        // The agreeing windows must be enough, and most of those kept.
        if count < MIN_WINDOWS || count * 2 <= sorted.len() {
            return None;
        }
        let total: u64 = sorted
            .iter()
            .filter(|size| agree(size))
            .map(|size| u64::from(*size))
            .sum();
        let mm = ((total + count as u64 * 5) / (count as u64 * 10)) as u16;
        PLAUSIBLE_MM.contains(&mm).then_some(mm)
    }

    /// Start over, as when the wheel sensor is replaced.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Whether `estimate_mm` differs from `current_mm` enough to apply: by more
/// than half a percent, and within the plausible sizes.
pub fn worth_applying(current_mm: u16, estimate_mm: u16) -> bool {
    PLAUSIBLE_MM.contains(&estimate_mm)
        && u32::from(estimate_mm.abs_diff(current_mm)) * 1000
            > u32::from(current_mm) * APPLY_PERMILLE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 11.131949 mm per e-7 degree, at the equator in both axes.
    const MM_PER_E7: f64 = 11.131_949;

    /// A deterministic uniform error in -1..1.
    struct Noise(u32);

    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            f64::from(self.0 % 20_001) / 10_000.0 - 1.0
        }
    }

    /// A rider on a road with a wheel of `wheel_mm`, seen once a second.
    struct Ride {
        calibrator: Calibrator,
        noise: Noise,
        wheel_mm: f64,
        /// Per-axis fix error, in metres.
        jitter_m: f64,
        at_ms: u64,
        /// Distance along the road, in mm.
        along_mm: f64,
        /// Revolutions as the sensor reports them, which may differ from the
        /// true count while it slips.
        revolutions: f64,
        /// The road bends by this angle every `bend_every_m`, when set.
        bend: Option<(f64, f64)>,
        north_mm: f64,
        east_mm: f64,
        heading: f64,
        /// The receiver sends Doppler ground speed.
        doppler: bool,
        speed_mm_s: u32,
    }

    impl Ride {
        fn new(wheel_mm: f64, jitter_m: f64) -> Self {
            Self {
                calibrator: Calibrator::default(),
                noise: Noise(0x2545_f491),
                wheel_mm,
                jitter_m,
                at_ms: 0,
                along_mm: 0.0,
                revolutions: 0.0,
                bend: None,
                north_mm: 0.0,
                east_mm: 0.0,
                heading: 0.0,
                doppler: true,
                speed_mm_s: 0,
            }
        }

        fn sample(&mut self) -> Sample {
            let jitter = self.jitter_m * 1000.0;
            let north = self.north_mm + self.noise.next() * jitter;
            let east = self.east_mm + self.noise.next() * jitter;
            Sample {
                at_ms: self.at_ms,
                latitude_e7: (north / MM_PER_E7).round() as i32,
                longitude_e7: (east / MM_PER_E7).round() as i32,
                // The count the sensor has reached: whole revolutions.
                revolutions: self.revolutions as u64,
                speed_mm_s: self.doppler.then_some(self.speed_mm_s),
            }
        }

        /// Move for `seconds` at `kmh`; the wheel turns `slip` times as much
        /// as the ground says, and the fixes are offered unless `offered`.
        fn go(&mut self, seconds: u64, kmh: f64, slip: f64, offered: bool) {
            for _ in 0..seconds {
                let step = kmh / 3.6 * 1000.0;
                self.speed_mm_s = step as u32;
                self.at_ms += 1000;
                self.along_mm += step;
                match self.bend {
                    Some((angle, every_m)) if self.along_mm % (every_m * 1000.0) < step => {
                        self.heading += angle;
                    }
                    _ => {}
                }
                self.north_mm += step * self.heading.cos();
                self.east_mm += step * self.heading.sin();
                self.revolutions += step / self.wheel_mm * slip;
                let sample = offered.then(|| self.sample());
                self.calibrator.observe(sample);
            }
        }

        fn cruise(&mut self, seconds: u64, kmh: f64) {
            self.go(seconds, kmh, 1.0, true);
        }
    }

    fn within(estimate: Option<u16>, truth: f64, percent: f64) {
        let estimate = f64::from(estimate.expect("an estimate"));
        let error = (estimate - truth).abs() / truth * 100.0;
        assert!(error <= percent, "{estimate} vs {truth}: {error:.2} %");
    }

    #[test]
    fn a_clean_ride_finds_the_wheel_whatever_size_was_configured() {
        // The configured size never enters: only GPS and revolutions do.
        for truth in [1_900.0, 2_105.0, 2_136.0, 2_200.0, 2_330.0] {
            let mut ride = Ride::new(truth, 0.0);
            ride.cruise(600, 30.0);
            within(ride.calibrator.estimate(), truth, 0.1);
        }
    }

    #[test]
    fn it_needs_three_windows_of_a_kilometre_each() {
        let mut ride = Ride::new(2_200.0, 0.0);
        // 2.9 km at 30 km/h: two windows (a window ends on the first segment
        // past a kilometre) and most of a third.
        ride.cruise(350, 30.0);
        assert_eq!(ride.calibrator.windows(), 2);
        assert_eq!(ride.calibrator.estimate(), None);
        ride.cruise(60, 30.0);
        assert_eq!(ride.calibrator.windows(), 3);
        within(ride.calibrator.estimate(), 2_200.0, 0.1);
    }

    #[test]
    fn noisy_fixes_average_out() {
        for (jitter_m, speed) in [(1.0, 28.0), (3.0, 28.0), (3.0, 20.0), (5.0, 40.0)] {
            let mut ride = Ride::new(2_105.0, jitter_m);
            ride.cruise(1_200, speed);
            within(ride.calibrator.estimate(), 2_105.0, 0.5);
        }
    }

    #[test]
    fn a_tunnel_has_no_distance_to_compare() {
        let mut ride = Ride::new(2_200.0, 2.0);
        ride.cruise(150, 30.0);
        let before = ride.calibrator.windows();
        // Forty seconds with no fixes, the bike still rolling: the window
        // that spans it must not form, or the tunnel's distance would count
        // as revolutions without metres.
        ride.go(40, 30.0, 1.0, false);
        ride.cruise(5, 30.0);
        assert_eq!(ride.calibrator.windows(), before);
        ride.cruise(900, 30.0);
        within(ride.calibrator.estimate(), 2_200.0, 0.3);
    }

    #[test]
    fn a_dropout_between_fixes_ends_the_window_even_without_a_none() {
        let mut calibrator = Calibrator::default();
        let at = |at_ms: u64, mm: i32, revolutions: u64| Sample {
            at_ms,
            latitude_e7: (f64::from(mm) / MM_PER_E7) as i32,
            longitude_e7: 0,
            revolutions,
            speed_mm_s: None,
        };
        let step = 8_333;
        for second in 0..=30u64 {
            let sample = at(
                second * 1000,
                step * second as i32,
                step as u64 * second / 2200,
            );
            if second == 15 {
                // The fixes before and after are 40 s apart.
                calibrator.observe(Some(at(55_000, step * 55, step as u64 * 55 / 2200)));
                continue;
            }
            calibrator.observe(Some(sample));
        }
        assert_eq!(calibrator.windows(), 0);
    }

    #[test]
    fn wheel_slip_and_skids_cost_windows_not_accuracy() {
        let mut ride = Ride::new(2_136.0, 1.0);
        ride.cruise(400, 30.0);
        // Spinning the rear wheel up a wet climb for a while, then a long skid.
        ride.go(120, 30.0, 1.35, true);
        ride.cruise(300, 30.0);
        ride.go(80, 30.0, 0.55, true);
        ride.cruise(600, 30.0);
        within(ride.calibrator.estimate(), 2_136.0, 0.3);
    }

    #[test]
    fn one_slipping_moment_in_a_window_is_dropped_alone() {
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.cruise(60, 30.0);
        ride.go(12, 30.0, 1.5, true);
        ride.cruise(600, 30.0);
        within(ride.calibrator.estimate(), 2_136.0, 0.3);
        // Without dropping it, the slip would skew a window by about 1 %.
        assert!(
            ride.calibrator.windows() >= 5,
            "{}",
            ride.calibrator.windows()
        );
    }

    #[test]
    fn a_stationary_bike_learns_nothing() {
        // A trainer: the wheel turns fast, GPS wanders where it stands.
        let mut ride = Ride::new(2_136.0, 4.0);
        ride.north_mm = 0.0;
        for _ in 0..1_800 {
            ride.at_ms += 1000;
            ride.revolutions += 4.0;
            let sample = ride.sample();
            ride.calibrator.observe(Some(sample));
        }
        assert_eq!(ride.calibrator.windows(), 0);
        assert_eq!(ride.calibrator.estimate(), None);
    }

    #[test]
    fn slow_riding_and_stops_are_not_used() {
        let mut ride = Ride::new(2_136.0, 1.0);
        ride.cruise(900, 14.0);
        ride.cruise(120, 0.0);
        ride.cruise(900, 12.0);
        assert_eq!(ride.calibrator.windows(), 0);
        // Just over the threshold counts.
        ride.cruise(900, 16.0);
        assert!(ride.calibrator.windows() >= 3);
        within(ride.calibrator.estimate(), 2_136.0, 0.5);
    }

    #[test]
    fn a_winding_road_is_not_compared_by_its_chords() {
        // A turn of 30 degrees every 100 m: chords cut the corners.
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.bend = Some((30f64.to_radians(), 100.0));
        ride.cruise(900, 30.0);
        assert_eq!(ride.calibrator.windows(), 0);
        assert_eq!(ride.calibrator.estimate(), None);
    }

    #[test]
    fn a_glitch_fix_thousands_of_kilometres_away_is_dropped_without_overflow() {
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.cruise(30, 30.0);
        for (latitude_e7, longitude_e7) in [(0, 0), (i32::MAX, i32::MAX), (i32::MIN, 1_800_000_000)]
        {
            let mut glitch = ride.sample();
            glitch.at_ms += 10_000;
            glitch.latitude_e7 = latitude_e7;
            glitch.longitude_e7 = longitude_e7;
            ride.calibrator.observe(Some(glitch));
            ride.at_ms += 10_000;
            let mut after = ride.sample();
            after.at_ms = glitch.at_ms + 10_000;
            ride.calibrator.observe(Some(after));
        }
        ride.at_ms += 30_000;
        ride.cruise(900, 30.0);
        within(ride.calibrator.estimate(), 2_136.0, 0.3);
    }

    #[test]
    fn bends_under_the_limit_do_not_shorten_the_distance_when_doppler_is_there() {
        // 6 degrees every 80 m: each chord is a little short of its arc.
        let mut with = Ride::new(2_136.0, 1.0);
        with.bend = Some((6f64.to_radians(), 80.0));
        with.cruise(1_200, 30.0);
        within(with.calibrator.estimate(), 2_136.0, 0.15);
        let mut without = Ride::new(2_136.0, 1.0);
        without.doppler = false;
        without.bend = Some((6f64.to_radians(), 80.0));
        without.cruise(1_200, 30.0);
        within(without.calibrator.estimate(), 2_136.0, 0.4);
    }

    #[test]
    fn a_doppler_speed_that_disagrees_with_the_chord_costs_the_segment() {
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.cruise(30, 30.0);
        let before = ride.calibrator.windows();
        // The receiver reports half the speed it moves at: no window forms.
        for _ in 0..400 {
            ride.go(1, 30.0, 1.0, false);
            let mut sample = ride.sample();
            sample.speed_mm_s = Some(sample.speed_mm_s.unwrap() / 2);
            ride.calibrator.observe(Some(sample));
        }
        assert_eq!(ride.calibrator.windows(), before);
    }

    #[test]
    fn a_gentle_curve_is_fine() {
        // 2 degrees every 100 m, a long sweeping bend.
        let mut ride = Ride::new(2_136.0, 1.0);
        ride.bend = Some((2f64.to_radians(), 100.0));
        ride.cruise(900, 30.0);
        within(ride.calibrator.estimate(), 2_136.0, 0.3);
    }

    #[test]
    fn a_new_wheel_session_restarts_the_count() {
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.cruise(95, 30.0);
        // The sensor reconnects and counts from zero again.
        ride.revolutions = 0.0;
        ride.cruise(95, 30.0);
        assert_eq!(ride.calibrator.windows(), 0, "no window spans the restart");
        ride.cruise(900, 30.0);
        within(ride.calibrator.estimate(), 2_136.0, 0.1);
    }

    #[test]
    fn unoffered_fixes_break_the_window() {
        let mut ride = Ride::new(2_136.0, 0.0);
        // The ride pauses every 80 s: a window needs 120 s at 30 km/h.
        for _ in 0..20 {
            ride.cruise(80, 30.0);
            ride.calibrator.observe(None);
        }
        assert_eq!(ride.calibrator.windows(), 0);
    }

    #[test]
    fn bad_windows_among_good_ones_do_not_move_the_median() {
        let mut calibrator = Calibrator::default();
        for tenths in [21_360, 21_359, 25_000, 21_362, 17_000, 21_361, 21_360] {
            calibrator.sizes[calibrator.sizes_next] = tenths;
            calibrator.sizes_next = (calibrator.sizes_next + 1) % KEPT;
            calibrator.sizes_len = (calibrator.sizes_len + 1).min(KEPT);
        }
        assert_eq!(calibrator.estimate(), Some(2_136));
    }

    #[test]
    fn windows_that_disagree_give_no_estimate() {
        let mut calibrator = Calibrator::default();
        for tenths in [20_000, 21_000, 22_000, 23_000] {
            calibrator.sizes[calibrator.sizes_next] = tenths;
            calibrator.sizes_next = (calibrator.sizes_next + 1) % KEPT;
            calibrator.sizes_len += 1;
        }
        assert_eq!(calibrator.estimate(), None);
    }

    #[test]
    fn implausible_sizes_are_never_estimated() {
        // A scooter wheel and a data glitch: neither is a bicycle.
        for truth in [1_400.0, 2_600.0] {
            let mut ride = Ride::new(truth, 0.0);
            ride.cruise(900, 30.0);
            assert_eq!(ride.calibrator.estimate(), None, "{truth}");
        }
    }

    #[test]
    fn a_change_is_worth_applying_above_half_a_percent() {
        assert!(worth_applying(2_136, 2_200));
        assert!(worth_applying(2_136, 2_105));
        assert!(worth_applying(2_000, 2_011));
        assert!(!worth_applying(2_000, 2_010), "exactly 0.5 % is not more");
        assert!(!worth_applying(2_136, 2_136));
        assert!(!worth_applying(2_136, 2_140));
        assert!(!worth_applying(2_136, 2_127));
        assert!(!worth_applying(2_136, 2_500), "out of bounds");
        assert!(!worth_applying(2_136, 1_400));
    }

    #[test]
    fn reset_forgets_everything() {
        let mut ride = Ride::new(2_136.0, 0.0);
        ride.cruise(600, 30.0);
        assert!(ride.calibrator.estimate().is_some());
        ride.calibrator.reset();
        assert_eq!(ride.calibrator.windows(), 0);
        assert_eq!(ride.calibrator.estimate(), None);
    }
}
