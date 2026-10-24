//! The rider's weeks: rides, distance and moving time for the current week
//! and the one before, from the rides' start times.
//!
//! A week runs Monday 00:00 to the next Monday 00:00 on the clock. The clock's
//! offset from UTC may change inside a week (Chile changes at Saturday 24:00),
//! so every instant is read with the offset in force at that instant, passed as
//! `offset_at(utc_s)` in seconds east of UTC. A ride belongs to the week and
//! day it started in on that clock, even if it ran past midnight. A ride with no
//! start time (the clock was unknown) is in no week, and neither is one that
//! starts after the current week ended.
//!
//! Pure arithmetic: callers feed each ride once and read the totals.

const DAY_S: i64 = 86_400;

/// Days in a week; day 0 is Monday.
pub const DAYS: usize = 7;

/// What one week held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Week {
    pub rides: u32,
    pub distance_mm: u64,
    pub active_ms: u64,
    /// Distance ridden each day, Monday first.
    pub day_mm: [u64; DAYS],
    /// Rides started each day, Monday first.
    pub day_rides: [u16; DAYS],
}

impl Week {
    fn add(&mut self, day: usize, active_ms: u64, distance_mm: Option<u64>) {
        let distance = distance_mm.unwrap_or(0);
        self.rides = self.rides.saturating_add(1);
        self.distance_mm = self.distance_mm.saturating_add(distance);
        self.active_ms = self.active_ms.saturating_add(active_ms);
        self.day_mm[day] = self.day_mm[day].saturating_add(distance);
        self.day_rides[day] = self.day_rides[day].saturating_add(1);
    }
}

/// This week and last week, as of a moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weeks {
    /// Local day number (days since 1970-01-01 on the clock) of this Monday.
    monday: i64,
    /// Day of this week (Monday 0) at the moment asked about.
    pub today: usize,
    pub this: Week,
    pub last: Week,
}

/// The local day number of a UTC second on a clock with `offset_at`.
fn local_day(utc_s: i64, offset_at: &impl Fn(i64) -> i64) -> i64 {
    (utc_s + offset_at(utc_s)).div_euclid(DAY_S)
}

impl Weeks {
    /// Empty weeks as of `now_utc_ms`.
    pub fn new(now_utc_ms: u64, offset_at: &impl Fn(i64) -> i64) -> Self {
        let now_s = i64::try_from(now_utc_ms / 1000).unwrap_or(i64::MAX / 2);
        let today = local_day(now_s, offset_at);
        // 1970-01-01 was a Thursday.
        let weekday = (today + 3).rem_euclid(7);
        Self {
            monday: today - weekday,
            today: weekday as usize,
            this: Week::default(),
            last: Week::default(),
        }
    }

    /// Count one ride by when it started.
    pub fn add(
        &mut self,
        start_utc_ms: Option<u64>,
        active_ms: u64,
        distance_mm: Option<u64>,
        offset_at: &impl Fn(i64) -> i64,
    ) {
        let Some(start_ms) = start_utc_ms else {
            return;
        };
        let start_s = i64::try_from(start_ms / 1000).unwrap_or(i64::MAX / 2);
        let since = local_day(start_s, offset_at) - self.monday;
        let day = since.rem_euclid(7) as usize;
        match since {
            0..=6 => self.this.add(day, active_ms, distance_mm),
            -7..=-1 => self.last.add(day, active_ms, distance_mm),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC milliseconds of a date and time of day, as UTC.
    fn utc(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> u64 {
        // Howard Hinnant's days_from_civil.
        let y = year - i64::from(month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        ((days * 86_400 + hour * 3600 + minute * 60) * 1000) as u64
    }

    const SUMMER: i64 = -3 * 3600;
    const WINTER: i64 = -4 * 3600;

    fn fixed(offset_s: i64) -> impl Fn(i64) -> i64 {
        move |_| offset_s
    }

    /// Chile's 2026 clock around the September change: UTC-4 until Saturday
    /// 24:00 (04:00 UTC on Sunday 6 September), UTC-3 after.
    fn chile_in_september(utc_s: i64) -> i64 {
        if utc_s < (utc(2026, 9, 6, 4, 0) / 1000) as i64 {
            WINTER
        } else {
            SUMMER
        }
    }

    fn day_of(weeks: &Weeks) -> (usize, [u16; DAYS], [u16; DAYS]) {
        (weeks.today, weeks.this.day_rides, weeks.last.day_rides)
    }

    #[test]
    fn a_week_starts_on_monday_at_local_midnight() {
        // Saturday 2026-10-24 12:00 UTC.
        let now = utc(2026, 10, 24, 12, 0);
        for offset in [0, SUMMER, WINTER, 5 * 3600 + 1800] {
            let weeks = Weeks::new(now, &fixed(offset));
            assert_eq!(weeks.today, 5, "offset {offset}");
        }
        // Three hours behind UTC, Monday begins at 03:00 UTC and ends the
        // week before it at 02:59:59.
        let zone = fixed(SUMMER);
        let mut weeks = Weeks::new(utc(2026, 10, 21, 15, 0), &zone);
        weeks.add(Some(utc(2026, 10, 19, 3, 0)), 1_000, Some(5_000), &zone);
        weeks.add(Some(utc(2026, 10, 19, 2, 59)), 2_000, Some(7_000), &zone);
        weeks.add(Some(utc(2026, 10, 26, 2, 59)), 1, Some(1), &zone);
        weeks.add(Some(utc(2026, 10, 26, 3, 0)), 1, Some(1), &zone);
        assert_eq!(weeks.today, 2);
        assert_eq!(
            (weeks.this.rides, weeks.this.day_mm[0], weeks.this.day_mm[6]),
            (2, 5_000, 1)
        );
        assert_eq!((weeks.last.rides, weeks.last.day_mm[6]), (1, 7_000));
        // East of Greenwich, local midnight is the evening before in UTC.
        let zone = fixed(5 * 3600 + 1800);
        let mut weeks = Weeks::new(utc(2026, 10, 21, 15, 0), &zone);
        weeks.add(Some(utc(2026, 10, 18, 18, 30)), 1, Some(1), &zone);
        weeks.add(Some(utc(2026, 10, 18, 18, 29)), 1, Some(1), &zone);
        assert_eq!((weeks.this.rides, weeks.last.rides), (1, 1));
    }

    #[test]
    fn totals_add_up_by_day_and_week() {
        let zone = fixed(SUMMER);
        let mut weeks = Weeks::new(utc(2026, 10, 24, 18, 0), &zone);
        assert_eq!(weeks.today, 5);
        let mut add = |at, ms, mm| weeks.add(Some(at), ms, Some(mm), &zone);
        add(utc(2026, 10, 19, 12, 0), 3_600_000, 30_000_000);
        add(utc(2026, 10, 19, 22, 0), 1_800_000, 10_000_000);
        // Sunday 2026-10-25 01:00 UTC is still Saturday night in Chile.
        add(utc(2026, 10, 25, 1, 0), 600_000, 2_000_000);
        // The Monday before, and a ride from a fortnight ago.
        add(utc(2026, 10, 12, 12, 0), 7_200_000, 60_000_000);
        add(utc(2026, 10, 5, 12, 0), 7_200_000, 60_000_000);
        assert_eq!(weeks.this.rides, 3);
        assert_eq!(weeks.this.distance_mm, 42_000_000);
        assert_eq!(weeks.this.active_ms, 6_000_000);
        assert_eq!(weeks.this.day_mm, [40_000_000, 0, 0, 0, 0, 2_000_000, 0]);
        assert_eq!(weeks.this.day_rides, [2, 0, 0, 0, 0, 1, 0]);
        assert_eq!(weeks.last.rides, 1);
        assert_eq!(weeks.last.distance_mm, 60_000_000);
    }

    #[test]
    fn rides_without_a_start_time_or_from_the_future_count_nowhere() {
        let zone = fixed(SUMMER);
        let mut weeks = Weeks::new(utc(2026, 10, 21, 15, 0), &zone);
        weeks.add(None, 60_000, Some(1_000), &zone);
        weeks.add(Some(utc(2026, 10, 26, 3, 0)), 60_000, Some(1_000), &zone);
        weeks.add(Some(utc(2027, 10, 26, 3, 0)), 60_000, Some(1_000), &zone);
        assert_eq!((weeks.this, weeks.last), (Week::default(), Week::default()));
        weeks.add(Some(utc(2026, 10, 20, 12, 0)), 60_000, None, &zone);
        assert_eq!(weeks.this.rides, 1);
        assert_eq!(weeks.this.distance_mm, 0);
        assert_eq!(weeks.this.active_ms, 60_000);
    }

    #[test]
    fn weeks_run_across_a_year_boundary() {
        // Friday 2027-01-01 10:00 UTC; the week began Monday 2026-12-28.
        let zone = fixed(SUMMER);
        let mut weeks = Weeks::new(utc(2027, 1, 1, 10, 0), &zone);
        assert_eq!(weeks.today, 4);
        for (at, mm) in [
            (utc(2026, 12, 28, 12, 0), 1_000),
            (utc(2026, 12, 31, 12, 0), 1_000),
            (utc(2027, 1, 1, 5, 0), 1_000),
            (utc(2026, 12, 27, 12, 0), 5_000),
            (utc(2026, 12, 21, 12, 0), 5_000),
        ] {
            weeks.add(Some(at), 1_000, Some(mm), &zone);
        }
        assert_eq!(weeks.this.day_rides, [1, 0, 0, 1, 1, 0, 0]);
        assert_eq!(weeks.last.day_rides, [1, 0, 0, 0, 0, 0, 1]);
        // Tuesday 2024-12-31 belongs to the week of Monday 2024-12-30.
        let zone = fixed(0);
        let mut weeks = Weeks::new(utc(2024, 12, 31, 12, 0), &zone);
        weeks.add(Some(utc(2024, 12, 30, 0, 0)), 1, None, &zone);
        weeks.add(Some(utc(2024, 12, 29, 23, 59)), 1, None, &zone);
        assert_eq!((weeks.today, weeks.this.rides, weeks.last.rides), (1, 1, 1));
        // Leap day 2024 was a Thursday.
        assert_eq!(Weeks::new(utc(2024, 2, 29, 12, 0), &zone).today, 3);
    }

    #[test]
    fn a_ride_near_monday_midnight_after_a_clock_change_lands_in_the_right_week() {
        // Sunday 6 September 2026 04:00 UTC the clock went from UTC-4 to
        // UTC-3, so Monday 7 September began at 03:00 UTC, not 04:00.
        let zone = chile_in_september;
        let mut weeks = Weeks::new(utc(2026, 9, 9, 15, 0), &zone);
        assert_eq!(weeks.today, 2);
        // 23:00 and 23:59 on Sunday: last week.
        weeks.add(Some(utc(2026, 9, 7, 2, 0)), 1, Some(1), &zone);
        weeks.add(Some(utc(2026, 9, 7, 2, 59)), 1, Some(2), &zone);
        // 00:00 and 01:00 on Monday: this week, even though 03:00 to 04:00 UTC
        // read as Sunday night under the winter offset.
        weeks.add(Some(utc(2026, 9, 7, 3, 0)), 1, Some(4), &zone);
        weeks.add(Some(utc(2026, 9, 7, 4, 0)), 1, Some(8), &zone);
        assert_eq!(
            day_of(&weeks),
            (2, [2, 0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0, 2])
        );
        assert_eq!((weeks.this.distance_mm, weeks.last.distance_mm), (12, 3));
        // The week that held the change keeps both of its days apart: Saturday
        // evening is still on the winter clock, Sunday morning on summer's.
        let mut changed = Weeks::new(utc(2026, 9, 8, 15, 0), &zone);
        changed.add(Some(utc(2026, 9, 6, 3, 59)), 1, None, &zone);
        changed.add(Some(utc(2026, 9, 6, 4, 0)), 1, None, &zone);
        changed.add(Some(utc(2026, 9, 7, 2, 59)), 1, None, &zone);
        assert_eq!(changed.last.day_rides, [0, 0, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn the_epoch_and_the_clock_before_it_do_not_overflow() {
        let zone = fixed(SUMMER);
        let mut weeks = Weeks::new(0, &zone);
        weeks.add(Some(u64::MAX), 1, Some(1), &zone);
        weeks.add(Some(0), 1, Some(1), &zone);
        assert_eq!(weeks.this.rides + weeks.last.rides, 1);
    }
}
