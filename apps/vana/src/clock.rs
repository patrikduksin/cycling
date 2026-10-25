//! Local time: the zone the rider's clock keeps, and what it reads at a UTC
//! instant. Recordings keep UTC; only what VANA shows is local.
//!
//! Mainland Chile follows a rule, as the tz database's `Chile` rule writes it
//! since 2023 (Decreto 98 of 2026 renews it to 2029): UTC-4, and UTC-3 from
//! the first Saturday of September at 24:00 to the first Saturday of April at
//! 24:00. Magallanes (since 2016) and Aysén (since 2025) keep UTC-3 all year.
//! Chile changed its dates often before 2023; VANA applies this rule to every
//! year, so a ride from an older year may read an hour off.

const DAY: i64 = 86_400;
const HOUR: i64 = 3_600;

/// Seconds since 1970-01-01 00:00 on the local clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local(pub u64);

impl Local {
    /// Seconds since local midnight.
    pub const fn of_day(self) -> u64 {
        self.0 % DAY as u64
    }
}

/// The time zone the clock keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    /// Mainland Chile, America/Santiago: summer time by `CHILE`.
    #[default]
    Chile,
    /// One offset all year, in quarter hours east of UTC.
    Fixed(i8),
}

/// Offsets a fixed zone may take, in quarter hours: UTC-12 to UTC+14.
const FIXED: core::ops::RangeInclusive<i8> = -48..=56;

impl Zone {
    /// A whole number of `hours` east of UTC all year.
    pub const fn utc(hours: i8) -> Self {
        Self::Fixed(hours * 4)
    }

    /// Seconds east of UTC at `utc_s`.
    pub fn offset_s(self, utc_s: i64) -> i64 {
        match self {
            Self::Chile => CHILE.offset_s(utc_s),
            Self::Fixed(quarters) => i64::from(quarters) * 900,
        }
    }

    /// What the clock reads at `utc_s`, seconds since 1970 UTC.
    pub fn local(self, utc_s: u64) -> Local {
        let utc = utc_s as i64;
        Local(utc.saturating_add(self.offset_s(utc)).max(0) as u64)
    }

    /// What the clock reads at a UTC time of day without a date. Summer time
    /// needs the date, so Chile takes summer time: it holds seven months of
    /// twelve, and it is what the clock read before it knew the rule.
    pub fn local_of_day(self, utc_of_day_s: u64) -> Local {
        let offset = match self {
            Self::Chile => CHILE.summer_s,
            Self::Fixed(_) => self.offset_s(0),
        };
        Local((utc_of_day_s as i64 + offset).rem_euclid(DAY) as u64)
    }

    /// The stored form: 0 for Chile, or 1 and the offset in quarter hours.
    pub const fn encode(self) -> [u8; 2] {
        match self {
            Self::Chile => [0, 0],
            Self::Fixed(quarters) => [1, quarters as u8],
        }
    }

    pub fn decode(bytes: [u8; 2]) -> Option<Self> {
        match bytes {
            [0, 0] => Some(Self::Chile),
            [1, quarters] if FIXED.contains(&(quarters as i8)) => Some(Self::Fixed(quarters as i8)),
            _ => None,
        }
    }
}

/// A yearly change of offset: the first Sunday on or after `day` of `month`,
/// at `utc_s` seconds past UTC midnight.
#[derive(Clone, Copy)]
struct Change {
    month: u8,
    day: u8,
    utc_s: i64,
}

impl Change {
    /// The change in `year`, in seconds since 1970 UTC.
    fn at(self, year: i64) -> i64 {
        let days = days_from_civil(year, self.month, self.day);
        // 1970-01-01 was a Thursday, so a Sunday has (days + 4) % 7 == 0.
        let to_sunday = (7 - (days + 4).rem_euclid(7)) % 7;
        (days + to_sunday) * DAY + self.utc_s
    }
}

/// A zone with summer time: its standard and summer offsets, in seconds east
/// of UTC, and the changes between them.
struct Rule {
    standard_s: i64,
    summer_s: i64,
    summer_starts: Change,
    summer_ends: Change,
}

/// tz `Chile`: summer from Sep Sun>=2 4:00u, standard from Apr Sun>=2 3:00u.
/// Both are Saturday 24:00 on the clock that is ending.
const CHILE: Rule = Rule {
    standard_s: -4 * HOUR,
    summer_s: -3 * HOUR,
    summer_starts: Change {
        month: 9,
        day: 2,
        utc_s: 4 * HOUR,
    },
    summer_ends: Change {
        month: 4,
        day: 2,
        utc_s: 3 * HOUR,
    },
};

impl Rule {
    fn offset_s(&self, utc_s: i64) -> i64 {
        let (year, _, _) = civil(utc_s.div_euclid(DAY));
        let (starts, ends) = (self.summer_starts.at(year), self.summer_ends.at(year));
        // A southern summer spans the new year: it ends early in the year
        // and starts again late in it.
        let summer = if starts < ends {
            (starts..ends).contains(&utc_s)
        } else {
            utc_s < ends || utc_s >= starts
        };
        if summer {
            self.summer_s
        } else {
            self.standard_s
        }
    }
}

/// The proleptic Gregorian date of a day count since 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
pub fn civil(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Days since 1970-01-01 of a proleptic Gregorian date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_services::gps::unix_seconds;

    /// Seconds since 1970 UTC of `ddmmyy` `hhmmss`.
    fn utc(date: &[u8; 6], time: &[u8; 6]) -> i64 {
        unix_seconds(*date, *time).unwrap()
    }

    /// The local clock as `HH:MM` and the day of the month.
    fn reads(zone: Zone, utc_s: i64) -> (u8, std::string::String) {
        let local = zone.local(utc_s as u64).0 as i64;
        let (_, _, day) = civil(local.div_euclid(DAY));
        let minute = local.rem_euclid(DAY) / 60;
        (day, std::format!("{:02}:{:02}", minute / 60, minute % 60))
    }

    #[test]
    fn santiago_is_four_hours_behind_in_winter_and_three_in_summer() {
        let june = utc(b"210626", b"160000");
        assert_eq!(reads(Zone::Chile, june), (21, "12:00".into()));
        let december = utc(b"211226", b"160000");
        assert_eq!(reads(Zone::Chile, december), (21, "13:00".into()));
    }

    /// Saturday 5 September 2026 at 24:00 the clock skips to 01:00 Sunday:
    /// no instant reads 00:xx on the 6th.
    #[test]
    fn spring_forward_skips_the_hour_after_midnight() {
        let change = utc(b"060926", b"040000");
        assert_eq!(reads(Zone::Chile, change - 1), (5, "23:59".into()));
        assert_eq!(reads(Zone::Chile, change), (6, "01:00".into()));
        let mut day = utc(b"050926", b"120000");
        while day < utc(b"060926", b"120000") {
            let (date, clock) = reads(Zone::Chile, day);
            assert!(!(date == 6 && clock.starts_with("00:")), "{clock} exists");
            day += 60;
        }
    }

    /// Saturday 4 April 2026 at 24:00 the clock falls back to 23:00, so the
    /// hour from 23:00 reads twice.
    #[test]
    fn fall_back_repeats_the_last_hour_of_saturday() {
        let change = utc(b"050426", b"030000");
        assert_eq!(reads(Zone::Chile, change - 1), (4, "23:59".into()));
        assert_eq!(reads(Zone::Chile, change), (4, "23:00".into()));
        assert_eq!(
            reads(Zone::Chile, change - 30 * 60),
            reads(Zone::Chile, change + 30 * 60),
            "23:30 reads twice"
        );
        assert_eq!(reads(Zone::Chile, change + HOUR), (5, "00:00".into()));
    }

    /// The changes in the years around today, as tzdata lists them.
    /// Every hour from 2023 to 2029 against the America/Santiago changes
    /// Python's zoneinfo lists for those years.
    #[test]
    fn every_hour_agrees_with_tzdata() {
        const CHANGES: [(i64, i64); 14] = [
            (1_680_404_400, -4),
            (1_693_713_600, -3),
            (1_712_458_800, -4),
            (1_725_768_000, -3),
            (1_743_908_400, -4),
            (1_757_217_600, -3),
            (1_775_358_000, -4),
            (1_788_667_200, -3),
            (1_806_807_600, -4),
            (1_820_116_800, -3),
            (1_838_257_200, -4),
            (1_851_566_400, -3),
            (1_870_311_600, -4),
            (1_883_016_000, -3),
        ];
        let mut expected = -3;
        let mut next = CHANGES.iter().peekable();
        let mut at = utc(b"010123", b"000000");
        while at < utc(b"010130", b"000000") {
            if let Some(&(_, offset)) = next.next_if(|(change, _)| *change <= at) {
                expected = offset;
            }
            assert_eq!(Zone::Chile.offset_s(at), expected * HOUR, "at {at}");
            at += HOUR;
        }
        for (change, offset) in CHANGES {
            assert_eq!(Zone::Chile.offset_s(change), offset * HOUR);
            assert_eq!(Zone::Chile.offset_s(change - 1), (-7 - offset) * HOUR);
        }
    }

    #[test]
    fn summer_time_runs_across_the_new_year() {
        let new_year = utc(b"010127", b"000000");
        assert_eq!(reads(Zone::Chile, new_year), (31, "21:00".into()));
        assert_eq!(reads(Zone::Chile, new_year - 1), (31, "20:59".into()));
        assert_eq!(Zone::Chile.offset_s(utc(b"311226", b"235959")), -3 * HOUR);
        assert_eq!(Zone::Chile.offset_s(utc(b"010127", b"030000")), -3 * HOUR);
    }

    #[test]
    fn a_fixed_zone_keeps_its_offset_all_year() {
        let utc_3 = Zone::utc(-3);
        for date in [b"210626", b"060926", b"211226", b"050426"] {
            assert_eq!(utc_3.offset_s(utc(date, b"120000")), -3 * HOUR);
        }
        assert_eq!(
            Zone::Fixed(22).local_of_day(0),
            Local(5 * HOUR as u64 + 1800)
        );
    }

    #[test]
    fn a_time_of_day_without_a_date_reads_in_summer_time() {
        assert_eq!(
            Zone::Chile.local_of_day(2 * HOUR as u64).0,
            23 * HOUR as u64
        );
        assert_eq!(
            Zone::Chile.local_of_day(16 * HOUR as u64).of_day(),
            13 * HOUR as u64
        );
    }

    #[test]
    fn zones_store_in_two_bytes_and_reject_what_they_cannot_be() {
        for zone in [
            Zone::Chile,
            Zone::utc(-3),
            Zone::utc(-4),
            Zone::Fixed(0),
            Zone::Fixed(56),
        ] {
            assert_eq!(Zone::decode(zone.encode()), Some(zone));
        }
        assert_eq!(Zone::Chile.encode(), [0, 0]);
        assert_eq!(Zone::utc(-3).encode(), [1, 0xf4]);
        for bad in [[0, 1], [2, 0], [1, 57], [1, (-49i8) as u8]] {
            assert_eq!(Zone::decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(20_743), (2026, 10, 17));
        for days in [-1, 0, 11_016, 20_743, 60_000] {
            let (year, month, day) = civil(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }
}
