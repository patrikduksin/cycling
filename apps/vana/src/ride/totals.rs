//! Ride totals: distance, time, averages, maxima, normalized power, work and
//! elevation, accumulated from one live sample per recorded second.
//!
//! The caller feeds only seconds the ride is Recording, so a pause adds no time,
//! distance or zeros to the averages. Totals survive pause and stop, and the
//! caller resets them when a new ride starts. Power-off loses them; the ride
//! journal stays the record of truth.
use crate::ride::log::Sample;

/// The wheel sensor cannot measure below about 1.9 km/h (one 2136 mm
/// revolution per its 4 s stop timeout), so anything it reports at or above
/// 1.8 km/h is rolling and counts as moving time.
pub const MOVING_MIN_MM_S: u32 = 500;
/// Normalized power smooths power over this many samples (seconds).
const NP_WINDOW: usize = 30;
/// Altitude must leave a band this wide (3.0 m) around the last turning point
/// before a climb or descent counts, so barometric noise adds no ascent.
pub const ELEVATION_BAND_DM: i32 = 30;
/// Altitude is smoothed by an exponential average over about this many
/// samples (a 4 s time constant at 1 Hz) before the band applies.
const ALTITUDE_SMOOTHING: i32 = 4;

/// Standard-atmosphere altitude in decimetres at 1100, 1080, ... 300 hPa.
const ALTITUDE_DM: [i32; 41] = [
    -6983, -5414, -3821, -2203, -560, 1109, 2805, 4530, 6284, 8069, 9885, 11734, 13618, 15537,
    17494, 19490, 21527, 23606, 25730, 27901, 30122, 32394, 34722, 37107, 39553, 42064, 44644,
    47297, 50028, 52841, 55744, 58743, 61844, 65057, 68390, 71854, 75462, 79226, 83164, 87295,
    91639,
];
const TABLE_TOP_CENTI_PA: u32 = 11_000_000;
const TABLE_STEP_CENTI_PA: u32 = 200_000;

/// Standard-atmosphere altitude for a barometric pressure, in decimetres.
/// Interpolated from a 20 hPa table (within 1 m near sea level), and clamped
/// to the table's 1100-300 hPa range. Weather moves it by metres per hour;
/// differences over a ride are what matter.
pub fn altitude_dm(pressure_centi_pa: u32) -> i32 {
    let below_top = TABLE_TOP_CENTI_PA.saturating_sub(pressure_centi_pa);
    let index = (below_top / TABLE_STEP_CENTI_PA) as usize;
    if index + 1 >= ALTITUDE_DM.len() {
        return ALTITUDE_DM[ALTITUDE_DM.len() - 1];
    }
    let (low, high) = (ALTITUDE_DM[index], ALTITUDE_DM[index + 1]);
    let into = (below_top % TABLE_STEP_CENTI_PA) as i64;
    low + ((i64::from(high - low) * into) / i64::from(TABLE_STEP_CENTI_PA)) as i32
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Trend {
    #[default]
    Level,
    Climbing,
    Descending,
}

/// A running mean of a reading, counting only samples that carried one.
#[derive(Clone, Copy, Default)]
struct Mean {
    sum: u64,
    count: u32,
}

impl Mean {
    fn add(&mut self, value: u64) {
        self.sum = self.sum.saturating_add(value);
        self.count = self.count.saturating_add(1);
    }

    fn value(self) -> Option<u64> {
        (self.count != 0).then(|| (self.sum + u64::from(self.count) / 2) / u64::from(self.count))
    }
}

#[derive(Clone, Copy, Default)]
pub struct Totals {
    distance_mm: u64,
    moving_ms: u64,
    max_speed_mm_s: Option<u32>,
    /// Watt-milliseconds and the time they cover; zeros count.
    energy_mj: u64,
    power_ms: u64,
    max_power: Option<u16>,
    /// The last `NP_WINDOW` power samples and their sum.
    window: [u16; NP_WINDOW],
    window_len: usize,
    window_next: usize,
    window_sum: u32,
    /// Sum of (window sum)^4 over every full window, and how many there were.
    np_sum: u128,
    np_count: u32,
    heart: Mean,
    max_heart: Option<u16>,
    /// Cadence tenths while pedalling; coasting zeros are excluded.
    cadence: Mean,
    altitude_dm: Option<i32>,
    /// Smoothed altitude times `ALTITUDE_SMOOTHING`, keeping its fraction.
    smoothed: i32,
    /// The last turning point, or the start, of the altitude trace.
    anchor_dm: i32,
    trend: Trend,
    ascent_dm: u32,
    descent_dm: u32,
}

impl Totals {
    /// Account one recorded step of `elapsed_ms` that covered `distance_mm`.
    pub fn add(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        sample: &Sample,
        pressure_centi_pa: Option<u32>,
    ) {
        self.distance_mm = self.distance_mm.saturating_add(distance_mm);
        if let Some(speed) = sample.speed_mm_s {
            if speed >= MOVING_MIN_MM_S {
                self.moving_ms = self.moving_ms.saturating_add(elapsed_ms);
            }
            self.max_speed_mm_s = self.max_speed_mm_s.max(Some(speed));
        }
        if let Some(watts) = sample.power_watts {
            self.energy_mj = self.energy_mj.saturating_add(u64::from(watts) * elapsed_ms);
            self.power_ms = self.power_ms.saturating_add(elapsed_ms);
            self.max_power = self.max_power.max(Some(watts));
            self.smooth(watts);
        }
        if let Some(bpm) = sample.heart_bpm {
            self.heart.add(u64::from(bpm));
            self.max_heart = self.max_heart.max(Some(bpm));
        }
        if let Some(cadence) = sample.cadence_tenths.filter(|&c| c != 0) {
            self.cadence.add(u64::from(cadence));
        }
        if let Some(pressure) = pressure_centi_pa {
            self.climb(altitude_dm(pressure));
        }
    }

    fn smooth(&mut self, watts: u16) {
        if self.window_len == NP_WINDOW {
            self.window_sum -= u32::from(self.window[self.window_next]);
        } else {
            self.window_len += 1;
        }
        self.window[self.window_next] = watts;
        self.window_sum += u32::from(watts);
        self.window_next = (self.window_next + 1) % NP_WINDOW;
        if self.window_len == NP_WINDOW {
            self.np_sum = self
                .np_sum
                .saturating_add(u128::from(self.window_sum).pow(4));
            self.np_count += 1;
        }
    }

    fn climb(&mut self, raw: i32) {
        if self.altitude_dm.is_none() {
            self.smoothed = raw * ALTITUDE_SMOOTHING;
            self.altitude_dm = Some(raw);
            self.anchor_dm = raw;
            return;
        }
        self.smoothed += raw - self.smoothed.div_euclid(ALTITUDE_SMOOTHING);
        let altitude = self.smoothed.div_euclid(ALTITUDE_SMOOTHING);
        self.altitude_dm = Some(altitude);
        let rise = altitude - self.anchor_dm;
        match self.trend {
            // Follow the trend from its last turning point; reverse only past the band.
            Trend::Climbing if rise > 0 => {
                self.ascent_dm += rise as u32;
                self.anchor_dm = altitude;
            }
            Trend::Descending if rise < 0 => {
                self.descent_dm += rise.unsigned_abs();
                self.anchor_dm = altitude;
            }
            _ if rise > ELEVATION_BAND_DM => {
                self.ascent_dm += rise as u32;
                self.anchor_dm = altitude;
                self.trend = Trend::Climbing;
            }
            _ if rise < -ELEVATION_BAND_DM => {
                self.descent_dm += rise.unsigned_abs();
                self.anchor_dm = altitude;
                self.trend = Trend::Descending;
            }
            _ => {}
        }
    }

    pub fn distance_mm(&self) -> u64 {
        self.distance_mm
    }

    /// Recorded time at or above `MOVING_MIN_MM_S`.
    pub fn moving_ms(&self) -> u64 {
        self.moving_ms
    }

    /// Distance over moving time.
    pub fn avg_speed_mm_s(&self) -> Option<u32> {
        (self.moving_ms != 0).then(|| (self.distance_mm * 1000 / self.moving_ms) as u32)
    }

    pub fn max_speed_mm_s(&self) -> Option<u32> {
        self.max_speed_mm_s
    }

    /// Time-weighted mean over every second with power, zeros included.
    pub fn avg_power(&self) -> Option<u16> {
        (self.power_ms != 0).then(|| {
            ((self.energy_mj + self.power_ms / 2) / self.power_ms).min(u64::from(u16::MAX)) as u16
        })
    }

    pub fn max_power(&self) -> Option<u16> {
        self.max_power
    }

    /// Fourth root of the mean fourth power of 30 s rolling power; none until
    /// the first 30 power samples.
    pub fn normalized_power(&self) -> Option<u16> {
        (self.np_count != 0).then(|| {
            let root = (self.np_sum / u128::from(self.np_count)).isqrt().isqrt();
            ((root + NP_WINDOW as u128 / 2) / NP_WINDOW as u128).min(u128::from(u16::MAX)) as u16
        })
    }

    /// Intensity factor in hundredths: normalized power over FTP.
    pub fn intensity_hundredths(&self, ftp_watts: u16) -> Option<u16> {
        let np = u32::from(self.normalized_power()?);
        let ftp = u32::from(ftp_watts);
        (ftp != 0).then(|| ((np * 100 + ftp / 2) / ftp).min(u32::from(u16::MAX)) as u16)
    }

    /// Training stress score: an hour at FTP is 100. Seconds with power ×
    /// NP² / (FTP² × 36), so it grows with time and with intensity squared.
    pub fn stress_score(&self, ftp_watts: u16) -> Option<u32> {
        let np = u64::from(self.normalized_power()?);
        let ftp = u64::from(ftp_watts);
        (ftp != 0).then(|| {
            let scale = ftp * ftp * 36_000;
            ((self.power_ms * np * np + scale / 2) / scale).min(u64::from(u32::MAX)) as u32
        })
    }

    /// Mechanical work in kilojoules.
    pub fn work_kj(&self) -> Option<u32> {
        (self.power_ms != 0).then(|| ((self.energy_mj + 500_000) / 1_000_000) as u32)
    }

    pub fn avg_heart_bpm(&self) -> Option<u16> {
        self.heart.value().map(|v| v as u16)
    }

    pub fn max_heart_bpm(&self) -> Option<u16> {
        self.max_heart
    }

    /// Mean cadence while pedalling, in rpm.
    pub fn avg_cadence_rpm(&self) -> Option<u16> {
        self.cadence
            .value()
            .map(|tenths| ((tenths + 5) / 10) as u16)
    }

    /// Latest smoothed standard-atmosphere altitude, in decimetres.
    pub fn altitude_dm(&self) -> Option<i32> {
        self.altitude_dm
    }

    pub fn ascent_dm(&self) -> u32 {
        self.ascent_dm
    }

    pub fn descent_dm(&self) -> u32 {
        self.descent_dm
    }
}

/// The best-effort durations, in seconds: 5 s, 1 min, 5 min and 20 min.
pub const EFFORT_SECONDS: [u32; 4] = [5, 60, 300, 1200];
/// Seconds of power kept one by one, for the short efforts.
const FINE: usize = 60;
/// Seconds summed into one coarse bucket for the long efforts.
const BUCKET_S: u32 = 10;
/// Coarse buckets kept: 20 minutes.
const BUCKETS: usize = 120;

/// The highest average power this ride held for each of
/// [`EFFORT_SECONDS`], from one power sample per recorded second.
///
/// 5 s and 1 min are exact over every window of samples. 5 and 20 min are
/// measured over windows that start on a 10 s bucket, which keeps the state
/// at about 600 bytes; such a window is at most 9 s off the best one, so the
/// result can read a few watts low, never high. Seconds without a power
/// reading are skipped, as normalized power skips them.
#[derive(Clone)]
pub struct Efforts {
    fine: [u16; FINE],
    fine_len: usize,
    fine_next: usize,
    /// Running sums of the last 5 and 60 samples.
    short: [u32; 2],
    bucket: u32,
    bucket_len: u32,
    buckets: [u32; BUCKETS],
    buckets_len: usize,
    buckets_next: usize,
    /// Running sums of the last 30 and 120 buckets.
    long: [u32; 2],
    best: [Option<u16>; 4],
    /// Credited time not yet a whole second, and its energy in W·ms.
    carry_ms: u64,
    carry_energy: u64,
}

impl Default for Efforts {
    fn default() -> Self {
        Self {
            fine: [0; FINE],
            fine_len: 0,
            fine_next: 0,
            short: [0; 2],
            bucket: 0,
            bucket_len: 0,
            buckets: [0; BUCKETS],
            buckets_len: 0,
            buckets_next: 0,
            long: [0; 2],
            best: [None; 4],
            carry_ms: 0,
            carry_energy: 0,
        }
    }
}

impl Efforts {
    /// Account `elapsed_ms` of recorded time at `watts`, resampled onto a
    /// one-second grid: a long step counts for every second it covers, and
    /// a second split between steps averages their power.
    pub fn add_ms(&mut self, watts: u16, elapsed_ms: u64) {
        // Steps are capped at a few seconds; the bound keeps a frame short.
        let mut left = elapsed_ms.min(10_000);
        while left > 0 {
            let take = left.min(1000 - self.carry_ms);
            left -= take;
            self.carry_ms += take;
            self.carry_energy += u64::from(watts) * take;
            if self.carry_ms == 1000 {
                let second = (self.carry_energy / 1000) as u16;
                (self.carry_ms, self.carry_energy) = (0, 0);
                self.add(second);
            }
        }
    }

    /// Account one recorded second of power.
    pub fn add(&mut self, watts: u16) {
        // Short windows: drop what leaves each window, then add the new second.
        for (sum, window) in self.short.iter_mut().zip([5usize, FINE]) {
            if self.fine_len >= window {
                let leaving = (self.fine_next + FINE - window) % FINE;
                *sum -= u32::from(self.fine[leaving]);
            }
            *sum += u32::from(watts);
        }
        self.fine[self.fine_next] = watts;
        self.fine_next = (self.fine_next + 1) % FINE;
        self.fine_len = (self.fine_len + 1).min(FINE);
        for (slot, window) in [(0, 5usize), (1, FINE)] {
            if self.fine_len >= window {
                self.raise(slot, self.short[slot] / window as u32);
            }
        }
        self.bucket += u32::from(watts);
        self.bucket_len += 1;
        if self.bucket_len < BUCKET_S {
            return;
        }
        let bucket = core::mem::take(&mut self.bucket);
        self.bucket_len = 0;
        for (sum, window) in self.long.iter_mut().zip([30usize, BUCKETS]) {
            if self.buckets_len >= window {
                let leaving = (self.buckets_next + BUCKETS - window) % BUCKETS;
                *sum -= self.buckets[leaving];
            }
            *sum += bucket;
        }
        self.buckets[self.buckets_next] = bucket;
        self.buckets_next = (self.buckets_next + 1) % BUCKETS;
        self.buckets_len = (self.buckets_len + 1).min(BUCKETS);
        for (slot, window) in [(2, 30usize), (3, BUCKETS)] {
            if self.buckets_len >= window {
                let seconds = window as u32 * BUCKET_S;
                self.raise(slot, self.long[slot - 2] / seconds);
            }
        }
    }

    fn raise(&mut self, slot: usize, watts: u32) {
        let watts = watts.min(u32::from(u16::MAX)) as u16;
        self.best[slot] = self.best[slot].max(Some(watts));
    }

    /// Best average watts for each of [`EFFORT_SECONDS`]; none until the
    /// ride has that much power.
    pub fn best(&self) -> [Option<u16>; 4] {
        self.best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEA_LEVEL: u32 = 10_132_500;

    #[test]
    fn long_steps_count_for_every_second_they_cover() {
        // Two 2 s bursts at 900 W around one 3 s step at 100 W: the best
        // 5 s is (900 + 900 + 100 * 3) / 5 = 420 W, not 740 W.
        let mut efforts = Efforts::default();
        for (watts, ms) in [(900, 2000), (100, 3000), (900, 2000)] {
            efforts.add_ms(watts, ms);
        }
        assert_eq!(efforts.best()[0], Some(420));
        // Part seconds carry over.
        let mut efforts = Efforts::default();
        for _ in 0..10 {
            efforts.add_ms(300, 500);
        }
        assert_eq!(efforts.best()[0], Some(300));
        // A second split between steps averages them: 1.5 s at 0 W and 1.5 s
        // at 900 W, three times, holds 450 W on average and at most 540 W
        // over any 5 s.
        let mut efforts = Efforts::default();
        for _ in 0..3 {
            efforts.add_ms(0, 1500);
            efforts.add_ms(900, 1500);
        }
        let best = efforts.best()[0].unwrap();
        assert!(best <= 540, "{best}");
    }

    #[test]
    fn best_efforts_find_the_hardest_window_of_each_length() {
        let mut efforts = Efforts::default();
        assert_eq!(efforts.best(), [None; 4]);
        // 10 min at 200 W, a 7 s sprint at 900 W, then 25 min at 250 W.
        for _ in 0..600 {
            efforts.add(200);
        }
        for _ in 0..7 {
            efforts.add(900);
        }
        let best = efforts.best();
        assert_eq!(best[0], Some(900));
        // The best minute holds the whole sprint: 53 s at 200 and 7 at 900.
        assert_eq!(best[1], Some(((53 * 200 + 7 * 900u32) / 60) as u16));
        assert_eq!(best[2], Some(200), "bucket-aligned five minutes");
        assert_eq!(best[3], None, "less than 20 min of buckets");
        for _ in 0..1500 {
            efforts.add(250);
        }
        let best = efforts.best();
        assert_eq!(best[0], Some(900), "a best is never lowered");
        // The sprint lifts the windows that hold it above the steady 250 W.
        assert_eq!(best[2], Some(((7 * 900 + 293 * 250u32) / 300) as u16));
        assert_eq!(best[3], Some(((7 * 900 + 1193 * 250u32) / 1200) as u16));
    }

    #[test]
    fn long_efforts_read_low_by_at_most_one_bucket_never_high() {
        let mut efforts = Efforts::default();
        // 5 min at 300 W that starts 5 s into a bucket, around 100 W.
        for second in 0..1200u32 {
            let hard = (5..305).contains(&second);
            efforts.add(if hard { 300 } else { 100 });
        }
        let five = efforts.best()[2].unwrap();
        assert!((300 - 200 * 9 / 300 - 1..=300).contains(&five), "{five}");
    }

    fn riding(speed_mm_s: u32, watts: u16, bpm: u16, cadence_rpm: u16) -> Sample {
        Sample {
            speed_mm_s: Some(speed_mm_s),
            power_watts: Some(watts),
            heart_bpm: Some(bpm),
            cadence_tenths: Some(cadence_rpm * 10),
            ..Sample::default()
        }
    }

    /// Feed `seconds` one-second steps of `sample`, moving at its speed.
    fn ride(totals: &mut Totals, seconds: u32, sample: Sample, pressure: Option<u32>) {
        for _ in 0..seconds {
            let distance = u64::from(sample.speed_mm_s.unwrap_or(0));
            totals.add(1000, distance, &sample, pressure);
        }
    }

    #[test]
    fn steady_ride_totals_match_hand_calculation() {
        let mut totals = Totals::default();
        // Ten minutes at 30 km/h and 200 W.
        ride(&mut totals, 600, riding(8333, 200, 140, 90), None);
        assert_eq!(totals.distance_mm(), 4_999_800);
        assert_eq!(totals.moving_ms(), 600_000);
        assert_eq!(totals.avg_speed_mm_s(), Some(8333));
        assert_eq!(totals.max_speed_mm_s(), Some(8333));
        assert_eq!(totals.avg_power(), Some(200));
        assert_eq!(totals.max_power(), Some(200));
        assert_eq!(totals.normalized_power(), Some(200));
        assert_eq!(totals.work_kj(), Some(120));
        assert_eq!(totals.avg_heart_bpm(), Some(140));
        assert_eq!(totals.max_heart_bpm(), Some(140));
        assert_eq!(totals.avg_cadence_rpm(), Some(90));
        assert_eq!(totals.altitude_dm(), None);
        assert_eq!(totals.ascent_dm(), 0);
    }

    #[test]
    fn empty_totals_have_no_averages() {
        let totals = Totals::default();
        assert_eq!(totals.avg_speed_mm_s(), None);
        assert_eq!(totals.avg_power(), None);
        assert_eq!(totals.normalized_power(), None);
        assert_eq!(totals.work_kj(), None);
        assert_eq!(totals.avg_heart_bpm(), None);
        assert_eq!(totals.avg_cadence_rpm(), None);
    }

    #[test]
    fn stopped_seconds_are_not_moving_and_coasting_keeps_cadence_average() {
        let mut totals = Totals::default();
        ride(&mut totals, 60, riding(10_000, 250, 150, 100), None);
        // A minute at a red light: power zeros count, cadence zeros do not.
        ride(&mut totals, 60, riding(0, 0, 120, 0), None);
        assert_eq!(totals.moving_ms(), 60_000);
        assert_eq!(totals.avg_speed_mm_s(), Some(10_000));
        assert_eq!(totals.avg_power(), Some(125));
        assert_eq!(totals.max_power(), Some(250));
        assert_eq!(totals.avg_heart_bpm(), Some(135));
        assert_eq!(totals.max_heart_bpm(), Some(150));
        assert_eq!(totals.avg_cadence_rpm(), Some(100));
        // Below the threshold the wheel is not rolling.
        ride(
            &mut totals,
            10,
            riding(MOVING_MIN_MM_S - 1, 0, 120, 0),
            None,
        );
        assert_eq!(totals.moving_ms(), 60_000);
        ride(&mut totals, 10, riding(MOVING_MIN_MM_S, 0, 120, 0), None);
        assert_eq!(totals.moving_ms(), 70_000);
    }

    #[test]
    fn missing_readings_are_skipped_rather_than_counted_as_zero() {
        let mut totals = Totals::default();
        ride(&mut totals, 30, riding(8000, 200, 140, 90), None);
        ride(&mut totals, 30, Sample::default(), None);
        assert_eq!(totals.avg_power(), Some(200));
        assert_eq!(totals.avg_heart_bpm(), Some(140));
        assert_eq!(totals.work_kj(), Some(6));
        assert_eq!(totals.moving_ms(), 30_000);
    }

    #[test]
    fn normalized_power_waits_for_thirty_seconds_and_weights_surges() {
        let mut totals = Totals::default();
        ride(&mut totals, 29, riding(8000, 300, 0, 0), None);
        assert_eq!(totals.normalized_power(), None);
        ride(&mut totals, 1, riding(8000, 300, 0, 0), None);
        assert_eq!(totals.normalized_power(), Some(300));
        // Alternating 60 s at 400 W and 60 s at 0 W averages 200 W, but costs more.
        let mut totals = Totals::default();
        for _ in 0..20 {
            ride(&mut totals, 60, riding(8000, 400, 0, 0), None);
            ride(&mut totals, 60, riding(8000, 0, 0, 0), None);
        }
        assert_eq!(totals.avg_power(), Some(200));
        // The rolling average spends a quarter at 400 W, a quarter at 0 W and
        // half on ramps: NP = 400 * (1/4 + 1/10)^(1/4).
        assert_eq!(totals.normalized_power(), Some(308));
        // 40 minutes of that against FTP 250: IF 308/250, TSS 40/60 × 100 × IF².
        assert_eq!(totals.intensity_hundredths(250), Some(123));
        assert_eq!(totals.stress_score(250), Some(101));
    }

    #[test]
    fn an_hour_at_ftp_is_one_hundred_stress() {
        let mut totals = Totals::default();
        assert_eq!(
            (totals.intensity_hundredths(215), totals.stress_score(215)),
            (None, None)
        );
        ride(&mut totals, 1800, riding(8000, 215, 0, 0), None);
        assert_eq!(totals.intensity_hundredths(215), Some(100));
        assert_eq!(totals.stress_score(215), Some(50));
        ride(&mut totals, 1800, riding(8000, 215, 0, 0), None);
        assert_eq!(totals.stress_score(215), Some(100));
        // Half the FTP: a quarter of the stress for the same hour.
        assert_eq!(totals.intensity_hundredths(430), Some(50));
        assert_eq!(totals.stress_score(430), Some(25));
        assert_eq!(totals.stress_score(0), None);
    }

    #[test]
    fn altitude_follows_the_standard_atmosphere() {
        // Linear interpolation is within half a metre near sea level.
        assert!(altitude_dm(SEA_LEVEL).abs() <= 5);
        // 89 874.6 Pa is 1000 m in the standard atmosphere.
        assert!((altitude_dm(8_987_460) - 10_000).abs() <= 10);
        // 79 495.2 Pa is 2000 m.
        assert!((altitude_dm(7_949_520) - 20_000).abs() <= 10);
        assert_eq!(altitude_dm(u32::MAX), ALTITUDE_DM[0]);
        assert_eq!(altitude_dm(0), ALTITUDE_DM[40]);
        // A metre near sea level is about 12 Pa.
        assert_eq!(altitude_dm(SEA_LEVEL - 1200) - altitude_dm(SEA_LEVEL), 10);
    }

    #[test]
    fn ascent_ignores_noise_inside_the_band_and_counts_whole_climbs() {
        let mut totals = Totals::default();
        // Hold each altitude long enough for the smoothing to settle.
        let mut hold = |metres: i32| {
            let pressure = SEA_LEVEL - (metres * 1200) as u32;
            ride(&mut totals, 20, riding(5000, 0, 0, 0), Some(pressure));
            (totals.ascent_dm(), totals.descent_dm())
        };
        // Two metres of noise around the start never count.
        for metres in [0, 2, 0, 2, 1, 0, 2] {
            assert_eq!(hold(metres), (0, 0));
        }
        // Three metres stays inside the band; a 12 m climb counts in full.
        assert_eq!(hold(3), (0, 0));
        for metres in 4..=12 {
            hold(metres);
        }
        assert_eq!(hold(12), (120, 0));
        // Wobbling down two metres at the top is not a descent.
        for metres in [10, 12, 11] {
            assert_eq!(hold(metres), (120, 0));
        }
        // Going all the way down counts from the summit.
        assert_eq!(hold(0), (120, 120));
        assert_eq!(totals.altitude_dm(), Some(altitude_dm(SEA_LEVEL)));
        // A missing pressure reading keeps the trace.
        ride(&mut totals, 1, riding(5000, 0, 0, 0), None);
        assert_eq!(totals.altitude_dm(), Some(altitude_dm(SEA_LEVEL)));
    }

    #[test]
    fn smoothing_rejects_a_single_spike() {
        let mut totals = Totals::default();
        let at = |metres: i32| Some(SEA_LEVEL - (metres * 1200) as u32);
        ride(&mut totals, 5, riding(5000, 0, 0, 0), at(0));
        // One 8 m outlier moves the smoothed altitude by only 2 m.
        ride(&mut totals, 1, riding(5000, 0, 0, 0), at(8));
        ride(&mut totals, 20, riding(5000, 0, 0, 0), at(0));
        assert_eq!(totals.ascent_dm(), 0);
    }
}
