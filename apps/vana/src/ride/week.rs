//! The rider's weeks: rides, distance and moving time for the current week
//! and the one before, from the rides' start times.
//!
//! A week runs Monday 00:00 to the next Monday 00:00 in the clock's time zone,
//! a fixed offset from UTC as the status-bar clock uses (no daylight saving).
//! A ride belongs to the week and day it started in, even if it ran past
//! midnight. A ride with no start time (the clock was unknown) is in no week,
//! and neither is one that starts after the current week ended.
//!
//! Pure arithmetic: callers feed each ride once and read the totals.

const DAY_S: i64 = 86_400;
const WEEK_S: i64 = 7 * DAY_S;

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
    /// UTC second this week began.
    start_s: i64,
    /// Day of this week (Monday 0) at the moment asked about.
    pub today: usize,
    pub this: Week,
    pub last: Week,
}

/// The UTC second of the Monday 00:00 that starts the week holding `utc_s`,
/// for a clock `offset_s` seconds from UTC (negative west of Greenwich).
pub fn week_start(utc_s: i64, offset_s: i64) -> i64 {
    let days = (utc_s + offset_s).div_euclid(DAY_S);
    // 1970-01-01 was a Thursday.
    let weekday = (days + 3).rem_euclid(7);
    (days - weekday) * DAY_S - offset_s
}

impl Weeks {
    /// Empty weeks as of `now_utc_ms`.
    pub fn new(now_utc_ms: u64, offset_s: i64) -> Self {
        let now_s = i64::try_from(now_utc_ms / 1000).unwrap_or(i64::MAX / 2);
        let start_s = week_start(now_s, offset_s);
        Self {
            start_s,
            today: usize::try_from((now_s - start_s) / DAY_S)
                .unwrap_or(0)
                .min(6),
            this: Week::default(),
            last: Week::default(),
        }
    }

    /// Count one ride by when it started.
    pub fn add(&mut self, start_utc_ms: Option<u64>, active_ms: u64, distance_mm: Option<u64>) {
        let Some(start_ms) = start_utc_ms else {
            return;
        };
        let since = i64::try_from(start_ms / 1000).unwrap_or(i64::MAX / 2) - self.start_s;
        let day = |since: i64| (since.rem_euclid(WEEK_S) / DAY_S) as usize;
        if since >= WEEK_S {
            return;
        }
        if since >= 0 {
            self.this.add(day(since), active_ms, distance_mm);
        } else if since >= -WEEK_S {
            self.last.add(day(since), active_ms, distance_mm);
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

    const CHILE_SUMMER: i64 = -3 * 3600;
    const CHILE_WINTER: i64 = -4 * 3600;

    #[test]
    fn a_week_starts_on_monday_at_local_midnight() {
        // Saturday 2026-10-24 12:00 UTC: Monday was 2026-10-19.
        let now = (utc(2026, 10, 24, 12, 0) / 1000) as i64;
        assert_eq!(week_start(now, 0), (utc(2026, 10, 19, 0, 0) / 1000) as i64);
        // Three hours behind UTC, local midnight is 03:00 UTC.
        assert_eq!(
            week_start(now, CHILE_SUMMER),
            (utc(2026, 10, 19, 3, 0) / 1000) as i64
        );
        assert_eq!(
            week_start(now, CHILE_WINTER),
            (utc(2026, 10, 19, 4, 0) / 1000) as i64
        );
        // East of Greenwich, local midnight is the evening before in UTC.
        assert_eq!(
            week_start(now, 5 * 3600 + 1800),
            (utc(2026, 10, 18, 18, 30) / 1000) as i64
        );
        // Monday itself, and the instant before it.
        let monday = (utc(2026, 10, 19, 3, 0) / 1000) as i64;
        assert_eq!(week_start(monday, CHILE_SUMMER), monday);
        assert_eq!(week_start(monday - 1, CHILE_SUMMER), monday - WEEK_S);
        assert_eq!(week_start(monday + WEEK_S - 1, CHILE_SUMMER), monday);
    }

    #[test]
    fn a_ride_exactly_at_monday_midnight_is_in_the_new_week() {
        let mut weeks = Weeks::new(utc(2026, 10, 21, 15, 0), CHILE_SUMMER);
        assert_eq!(weeks.today, 2);
        weeks.add(Some(utc(2026, 10, 19, 3, 0)), 1_000, Some(5_000));
        weeks.add(Some(utc(2026, 10, 19, 2, 59)), 2_000, Some(7_000));
        assert_eq!((weeks.this.rides, weeks.this.day_mm[0]), (1, 5_000));
        assert_eq!((weeks.last.rides, weeks.last.day_mm[6]), (1, 7_000));
    }

    #[test]
    fn totals_add_up_by_day_and_week() {
        let mut weeks = Weeks::new(utc(2026, 10, 24, 18, 0), CHILE_SUMMER);
        assert_eq!(weeks.today, 5);
        weeks.add(Some(utc(2026, 10, 19, 12, 0)), 3_600_000, Some(30_000_000));
        weeks.add(Some(utc(2026, 10, 19, 22, 0)), 1_800_000, Some(10_000_000));
        // Sunday 2026-10-25 01:00 UTC is still Saturday night in Chile.
        weeks.add(Some(utc(2026, 10, 25, 1, 0)), 600_000, Some(2_000_000));
        // The Monday before, and a ride from a fortnight ago.
        weeks.add(Some(utc(2026, 10, 12, 12, 0)), 7_200_000, Some(60_000_000));
        weeks.add(Some(utc(2026, 10, 5, 12, 0)), 7_200_000, Some(60_000_000));
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
        let mut weeks = Weeks::new(utc(2026, 10, 21, 15, 0), CHILE_SUMMER);
        weeks.add(None, 60_000, Some(1_000));
        // The next Monday 00:00 local, and a year on.
        weeks.add(Some(utc(2026, 10, 26, 3, 0)), 60_000, Some(1_000));
        weeks.add(Some(utc(2027, 10, 26, 3, 0)), 60_000, Some(1_000));
        assert_eq!((weeks.this, weeks.last), (Week::default(), Week::default()));
        // A ride with no distance still counts, with its time.
        weeks.add(Some(utc(2026, 10, 20, 12, 0)), 60_000, None);
        assert_eq!(weeks.this.rides, 1);
        assert_eq!(weeks.this.distance_mm, 0);
        assert_eq!(weeks.this.active_ms, 60_000);
    }

    #[test]
    fn weeks_run_across_a_year_boundary() {
        // Friday 2027-01-01 10:00 UTC; the week began Monday 2026-12-28.
        let mut weeks = Weeks::new(utc(2027, 1, 1, 10, 0), CHILE_SUMMER);
        assert_eq!(weeks.today, 4);
        weeks.add(Some(utc(2026, 12, 28, 12, 0)), 1_000, Some(1_000));
        weeks.add(Some(utc(2026, 12, 31, 12, 0)), 1_000, Some(1_000));
        weeks.add(Some(utc(2027, 1, 1, 5, 0)), 1_000, Some(1_000));
        weeks.add(Some(utc(2026, 12, 27, 12, 0)), 1_000, Some(5_000));
        weeks.add(Some(utc(2026, 12, 21, 12, 0)), 1_000, Some(5_000));
        assert_eq!(weeks.this.rides, 3);
        assert_eq!(weeks.this.day_rides, [1, 0, 0, 1, 1, 0, 0]);
        assert_eq!(weeks.last.rides, 2);
        assert_eq!(weeks.last.day_rides, [1, 0, 0, 0, 0, 0, 1]);
        // Dec 31 2024 (a Tuesday) belongs to the week of Monday Dec 30.
        let tuesday = (utc(2024, 12, 31, 12, 0) / 1000) as i64;
        assert_eq!(
            week_start(tuesday, 0),
            (utc(2024, 12, 30, 0, 0) / 1000) as i64
        );
        // Leap day 2024 was a Thursday.
        let leap = (utc(2024, 2, 29, 12, 0) / 1000) as i64;
        assert_eq!(week_start(leap, 0), (utc(2024, 2, 26, 0, 0) / 1000) as i64);
    }

    #[test]
    fn the_epoch_and_the_clock_before_it_do_not_overflow() {
        assert_eq!(week_start(0, 0), -3 * DAY_S);
        assert_eq!(week_start(0, CHILE_SUMMER), -3 * DAY_S - CHILE_SUMMER);
        let mut weeks = Weeks::new(0, CHILE_SUMMER);
        weeks.add(Some(u64::MAX), 1, Some(1));
        weeks.add(Some(0), 1, Some(1));
        assert_eq!(weeks.this.rides + weeks.last.rides, 1);
    }
}
