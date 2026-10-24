//! Fitness, fatigue and form from the rider's daily training stress.
//!
//! Each day's stress (TSS of the rides that had power) feeds two
//! exponentially weighted averages, `average += (stress - average) / n`: the
//! 42-day one is fitness (CTL), the 7-day one fatigue (ATL), and form (TSB) is
//! fitness minus fatigue. A day without a ride feeds zero, so both decay.
//!
//! Only the last [`DAYS`] days are folded, from the ride index and the facts,
//! and nothing is stored. Both averages start at the mean of the first week
//! rather than at zero, so a rider who trains steadily reads even form, not
//! tired form, from a window that began mid-habit. A ride without power has
//! no stress and counts for nothing. The averages are fixed point (8 fractional
//! bits, integer division toward zero), and one day's stress is capped at
//! [`DAY_MAX`], so no input can overflow them.
use super::week::local_day;

/// Days folded, oldest first and today last.
pub const DAYS: usize = 49;
/// Form samples kept for the trend, a week apart, today last.
pub const TREND: usize = 6;
/// Days between trend samples.
const WEEK: usize = 7;
const FITNESS_DAYS: i64 = 42;
const FATIGUE_DAYS: i64 = 7;
/// More than any rider's day; a bigger stress is a sensor fault.
pub const DAY_MAX: u32 = 1000;
const ONE: i64 = 256;

/// The stress of each of the last [`DAYS`] local days.
#[derive(Clone, Debug)]
pub struct Daily {
    /// Local day number of the last entry.
    today: i64,
    stress: [u32; DAYS],
    /// Rides with power counted in.
    pub powered: u16,
}

impl Default for Daily {
    fn default() -> Self {
        Self {
            today: 0,
            stress: [0; DAYS],
            powered: 0,
        }
    }
}

impl Daily {
    /// Empty days up to the day `now_utc_ms` falls in on the clock.
    pub fn new(now_utc_ms: u64, offset_at: &impl Fn(i64) -> i64) -> Self {
        let mut daily = Self::default();
        daily.reset(now_utc_ms, offset_at);
        daily
    }

    /// Count a ride by the day it started; a ride without power, without a
    /// start time or outside the window adds nothing.
    pub fn add(
        &mut self,
        start_utc_ms: Option<u64>,
        stress: Option<u32>,
        offset_at: &impl Fn(i64) -> i64,
    ) {
        let (Some(start_ms), Some(stress)) = (start_utc_ms, stress) else {
            return;
        };
        let start_s = i64::try_from(start_ms / 1000).unwrap_or(i64::MAX / 2);
        let age = self.today.saturating_sub(local_day(start_s, offset_at));
        let Some(slot) = usize::try_from(age)
            .ok()
            .and_then(|age| (DAYS - 1).checked_sub(age))
        else {
            return;
        };
        self.stress[slot] = self.stress[slot].saturating_add(stress);
        self.powered = self.powered.saturating_add(1);
    }

    /// Start over as empty days up to the day `now_utc_ms` falls in.
    pub fn reset(&mut self, now_utc_ms: u64, offset_at: &impl Fn(i64) -> i64) {
        let now_s = i64::try_from(now_utc_ms / 1000).unwrap_or(i64::MAX / 2);
        self.today = local_day(now_s, offset_at);
        self.stress.fill(0);
        self.powered = 0;
    }

    pub fn form(&self) -> Form {
        fold(&self.stress)
    }
}

/// Fitness, fatigue and form as of the last day, in whole stress points,
/// with form at the end of each of the last weeks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Form {
    pub fitness: u16,
    pub fatigue: u16,
    /// Fitness minus fatigue: positive when rested.
    pub form: i16,
    /// Form at the end of the day 5, 4 ... 0 weeks before the last, oldest
    /// first; weeks before the first day of a short fold read 0.
    pub trend: [i16; TREND],
}

struct Average {
    q8: i64,
    days: i64,
}

impl Average {
    fn step(&mut self, stress_q8: i64) {
        self.q8 += (stress_q8 - self.q8) / self.days;
    }
}

fn whole(q8: i64) -> i64 {
    (q8 + ONE / 2).div_euclid(ONE)
}

/// Fold `days` of stress, oldest first.
pub fn fold(days: &[u32]) -> Form {
    let capped = |stress: u32| i64::from(stress.min(DAY_MAX)) * ONE;
    let first = &days[..days.len().min(WEEK)];
    let seed = first.iter().map(|&s| capped(s)).sum::<i64>() / first.len().max(1) as i64;
    let mut fitness = Average {
        q8: seed,
        days: FITNESS_DAYS,
    };
    let mut fatigue = Average {
        q8: seed,
        days: FATIGUE_DAYS,
    };
    let mut trend = [0; TREND];
    for (index, &stress) in days.iter().enumerate() {
        fitness.step(capped(stress));
        fatigue.step(capped(stress));
        let behind = days.len() - 1 - index;
        if behind.is_multiple_of(WEEK) && behind / WEEK < TREND {
            trend[TREND - 1 - behind / WEEK] = whole(fitness.q8 - fatigue.q8) as i16;
        }
    }
    Form {
        fitness: whole(fitness.q8) as u16,
        fatigue: whole(fatigue.q8) as u16,
        form: whole(fitness.q8 - fatigue.q8) as i16,
        trend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(offset_s: i64) -> impl Fn(i64) -> i64 {
        move |_| offset_s
    }

    /// 2026-10-15 10:00 UTC.
    const NOW_MS: u64 = 1_792_058_400_000;
    const DAY_MS: u64 = 86_400_000;

    #[test]
    fn three_days_worked_by_hand() {
        // A zero first week seeds both averages at 0. Values are x256.
        //
        // day 8: stress 84 -> 21504
        //   fitness 0 + (21504 - 0) / 42    = 512      (2.0)
        //   fatigue 0 + (21504 - 0) / 7     = 3072     (12.0)
        // day 9: stress 0
        //   fitness 512 + (0 - 512) / 42    = 512 - 12 = 500     (1.95)
        //   fatigue 3072 + (0 - 3072) / 7   = 3072 - 438 = 2634  (10.29)
        // day 10: stress 126 -> 32256
        //   fitness 500 + (32256 - 500) / 42   = 500 + 756 = 1256   (4.91)
        //   fatigue 2634 + (32256 - 2634) / 7  = 2634 + 4231 = 6865 (26.82)
        //   form 1256 - 6865 = -5609 (-21.91)
        // Rounded: fitness 5, fatigue 27, form -22. The trend samples every
        // seventh day back from day 10: day 10 and day 3.
        let form = fold(&[0, 0, 0, 0, 0, 0, 0, 84, 0, 126]);
        assert_eq!(form.fitness, 5);
        assert_eq!(form.fatigue, 27);
        assert_eq!(form.form, -22);
        assert_eq!(form.trend, [0, 0, 0, 0, 0, -22]);
        let first = fold(&[0, 0, 0, 0, 0, 0, 0, 84]);
        assert_eq!((first.fitness, first.fatigue, first.form), (2, 12, -10));
    }

    #[test]
    fn steady_riding_reads_even_form() {
        let form = fold(&[70; DAYS]);
        assert_eq!((form.fitness, form.fatigue, form.form), (70, 70, 0));
        assert_eq!(form.trend, [0; TREND]);
    }

    #[test]
    fn rest_days_decay_fatigue_faster_than_fitness() {
        let mut days = [60; DAYS];
        days[DAYS - 7..].fill(0);
        let form = fold(&days);
        // A week of rest: fatigue 60 * (6/7)^7 = 20, fitness 60 * (41/42)^7 = 51.
        assert!((i32::from(form.fatigue) - 20).abs() <= 1, "{form:?}");
        assert!((i32::from(form.fitness) - 51).abs() <= 1, "{form:?}");
        assert!(form.form > 25, "{form:?}");
        assert_eq!(form.trend[TREND - 2], 0, "even form a week earlier");
    }

    #[test]
    fn rides_land_on_their_local_day_and_skip_what_has_no_power() {
        let utc = fixed(0);
        let mut daily = Daily::new(NOW_MS, &utc);
        daily.add(Some(NOW_MS - 2 * 3_600_000), Some(80), &utc);
        daily.add(Some(NOW_MS - DAY_MS), Some(50), &utc);
        daily.add(Some(NOW_MS - DAY_MS), Some(10), &utc);
        daily.add(Some(NOW_MS - 48 * DAY_MS), Some(40), &utc);
        daily.add(Some(NOW_MS - 49 * DAY_MS), Some(999), &utc);
        daily.add(Some(NOW_MS + DAY_MS), Some(999), &utc);
        daily.add(Some(NOW_MS), None, &utc);
        daily.add(None, Some(999), &utc);
        assert_eq!(daily.stress[DAYS - 1], 80);
        assert_eq!(daily.stress[DAYS - 2], 60);
        assert_eq!(daily.stress[0], 40);
        assert_eq!(daily.powered, 4);
        assert_eq!(daily.stress.iter().sum::<u32>(), 180);
    }

    #[test]
    fn a_ride_after_local_midnight_counts_on_the_next_day() {
        let chile = fixed(-4 * 3600);
        let mut daily = Daily::new(NOW_MS, &chile);
        // 02:00 UTC on the 15th is 22:00 on the 14th in Chile.
        daily.add(Some(NOW_MS - 8 * 3_600_000), Some(70), &chile);
        assert_eq!(daily.stress[DAYS - 2], 70);
    }

    #[test]
    fn hostile_stress_and_clocks_cannot_overflow() {
        let utc = fixed(i64::MAX / 4);
        for now in [0, u64::MAX, NOW_MS] {
            let mut daily = Daily::new(now, &utc);
            for start in [0, 1, u64::MAX, now] {
                for _ in 0..3 {
                    daily.add(Some(start), Some(u32::MAX), &utc);
                }
            }
            let form = daily.form();
            assert!(form.fatigue <= DAY_MAX as u16, "{form:?}");
        }
        let swamped = fold(&[u32::MAX; DAYS]);
        assert_eq!(swamped.fitness, DAY_MAX as u16);
        assert_eq!(swamped.form, 0);
        assert_eq!(fold(&[]), Form::default());
        assert_eq!(fold(&[u32::MAX]).fitness, DAY_MAX as u16);
    }

    #[test]
    fn a_weeks_rest_after_a_hard_block_shows_a_rising_trend() {
        let mut days = [0; DAYS];
        for (index, stress) in days.iter_mut().enumerate() {
            *stress = if index % 7 < 5 { 90 } else { 0 };
        }
        days[DAYS - 7..].fill(0);
        let form = fold(&days);
        assert!(form.trend[TREND - 1] > form.trend[TREND - 2], "{form:?}");
    }

    #[test]
    fn what_the_screen_carries_stays_small() {
        assert!(core::mem::size_of::<Form>() <= 24);
        assert!(core::mem::size_of::<Daily>() <= 208);
    }
}
