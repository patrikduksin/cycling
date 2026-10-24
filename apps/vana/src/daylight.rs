//! Day and night from the sun: its altitude at a place and time, and the
//! fallbacks VANA uses when it lacks either.
//!
//! The altitude follows the low-precision solar coordinates of the
//! Astronomical Almanac, good to about a hundredth of a degree for decades
//! around 2000; far better than a backlight needs.

/// Night starts once the sun is this far below the horizon (degrees): mid
/// civil twilight, when the sky has gone dim.
pub const NIGHT_BELOW_DEG: f64 = -4.0;
/// Day returns once the sun climbs back above this, so a reading near the
/// threshold does not flicker between the two.
pub const DAY_ABOVE_DEG: f64 = -2.0;
/// Without a position, local clock hours from this one up to `MORNING_HOUR`
/// count as night.
pub const EVENING_HOUR: u64 = 20;
pub const MORNING_HOUR: u64 = 7;

/// The sun's altitude above the horizon in degrees, at `unix_seconds`, seen
/// from `latitude` and `longitude` in degrees (east positive).
pub fn sun_altitude(unix_seconds: i64, latitude: f64, longitude: f64) -> f64 {
    use libm::{asin, atan2, cos, sin};
    let radians = core::f64::consts::PI / 180.0;
    // Days since 2000-01-01 12:00 UTC.
    let days = unix_seconds as f64 / 86_400.0 - 10_957.5;
    let mean_longitude = 280.460 + 0.985_647_4 * days;
    let anomaly = (357.528 + 0.985_600_3 * days) * radians;
    let ecliptic = (mean_longitude + 1.915 * sin(anomaly) + 0.020 * sin(2.0 * anomaly)) * radians;
    let obliquity = (23.439 - 0.000_000_4 * days) * radians;
    let declination = asin(sin(obliquity) * sin(ecliptic));
    let right_ascension = atan2(cos(obliquity) * sin(ecliptic), cos(ecliptic));
    // Greenwich mean sidereal time, in degrees.
    let sidereal = 280.460_618_37 + 360.985_647_366_29 * days;
    let hour_angle = sidereal * radians + longitude * radians - right_ascension;
    let latitude = latitude * radians;
    asin(sin(latitude) * sin(declination) + cos(latitude) * cos(declination) * cos(hour_angle))
        / radians
}

/// The sun's centre this far below the horizon (degrees) is the standard
/// sunset: the disc's upper edge touches it, with refraction.
pub const SUNSET_DEG: f64 = -0.833;
/// How far ahead `sun_state` looks for a sunset, and how finely.
const LOOKAHEAD_S: i64 = 26 * 3600;
const SCAN_S: i64 = 1800;
/// Bisection halvings: 1800 s down to a fraction of a second.
const BISECTIONS: u32 = 14;

/// Where the sun stands for the light a rider has left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sun {
    /// Below the horizon: after sunset, before sunrise, or polar night.
    Down,
    /// Above it, and sets at this Unix time.
    Up { sets_at: i64 },
    /// Above it with no sunset within a day: polar day.
    Midnight,
}

/// The sun at `unix_seconds` from `latitude`, `longitude`: down, or up with
/// the time it next sets. Scans ahead every half hour for the sun crossing
/// `SUNSET_DEG` downward, then bisects: about 60 altitudes, so call it
/// once a minute rather than per frame. A dip below the horizon shorter than
/// half an hour, at the edge of polar day, can be missed.
pub fn sun_state(unix_seconds: i64, latitude: f64, longitude: f64) -> Sun {
    let above = |t: i64| sun_altitude(t, latitude, longitude) > SUNSET_DEG;
    if !above(unix_seconds) {
        return Sun::Down;
    }
    let mut low = unix_seconds;
    while low - unix_seconds < LOOKAHEAD_S {
        let high = low + SCAN_S;
        if !above(high) {
            let mut high = high;
            for _ in 0..BISECTIONS {
                let middle = low + (high - low) / 2;
                if above(middle) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            return Sun::Up { sets_at: high };
        }
        low = high;
    }
    Sun::Midnight
}

/// Whether it is night at this sun altitude, given whether it was.
pub fn night_at(altitude: f64, was_night: bool) -> bool {
    if was_night {
        altitude <= DAY_ABOVE_DEG
    } else {
        altitude < NIGHT_BELOW_DEG
    }
}

/// Night by the local clock alone, in seconds since local midnight.
pub fn night_by_clock(local_seconds: u64) -> bool {
    let hour = local_seconds % 86_400 / 3600;
    !(MORNING_HOUR..EVENING_HOUR).contains(&hour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_services::gps::unix_seconds;

    const SANTIAGO: (f64, f64) = (-33.45, -70.66);

    /// Santiago's sun sets about 19:51 local (22:51 UTC) on 14 October 2026,
    /// as NOAA's calculator has it: well up at 22:00, dark by 23:20.
    #[test]
    fn the_sun_rises_and_sets_where_almanacs_say() {
        let at = |time: &[u8; 6]| unix_seconds(*b"141026", *time).unwrap();
        let (lat, lon) = SANTIAGO;
        assert!(sun_altitude(at(b"220000"), lat, lon) > 8.0);
        let near = sun_altitude(at(b"225000"), lat, lon);
        assert!((0.0..0.4).contains(&near), "{near}");
        let dusk = sun_altitude(at(b"231000"), lat, lon);
        assert!((-4.0..-3.8).contains(&dusk), "{dusk}");
        assert!(sun_altitude(at(b"232000"), lat, lon) < NIGHT_BELOW_DEG);
        assert!(sun_altitude(at(b"163000"), lat, lon) > 55.0);
        // Northern winter: London at noon in December is low but up.
        let london = unix_seconds(*b"211226", *b"120000").unwrap();
        let low = sun_altitude(london, 51.5, -0.13);
        assert!((12.0..17.0).contains(&low), "{low}");
    }

    /// Sunset UTC from sunrise-sunset.org (NOAA-style solar position), which
    /// this must match to two minutes.
    #[test]
    fn sunset_matches_published_times_within_two_minutes() {
        let utc = |date: &[u8; 6], h: i64, m: i64, s: i64| {
            unix_seconds(*date, *b"000000").unwrap() + h * 3600 + m * 60 + s
        };
        let cases = [
            // Santiago.
            (SANTIAGO, utc(b"141026", 22, 54, 55)),
            (SANTIAGO, utc(b"211226", 23, 52, 2)),
            (SANTIAGO, utc(b"210626", 21, 42, 32)),
            (SANTIAGO, utc(b"200326", 22, 54, 1)),
            // London, shortest day.
            ((51.5, -0.13), utc(b"211226", 15, 53, 34)),
        ];
        for ((lat, lon), want) in cases {
            // From the morning of that day, as a rider would ask.
            let from = want - 5 * 3600;
            let Sun::Up { sets_at } = sun_state(from, lat, lon) else {
                panic!("sun should be up at {from}");
            };
            assert!((sets_at - want).abs() <= 120, "{} s off", sets_at - want);
        }
    }

    #[test]
    fn sunset_is_down_at_night_and_absent_in_polar_day_and_night() {
        let (lat, lon) = SANTIAGO;
        let night = unix_seconds(*b"141026", *b"233000").unwrap();
        assert_eq!(sun_state(night, lat, lon), Sun::Down);
        // Tromso at midsummer: the sun never sets.
        let june = unix_seconds(*b"210626", *b"100000").unwrap();
        assert_eq!(sun_state(june, 69.65, 18.96), Sun::Midnight);
        // And at midwinter it never rises.
        let december = unix_seconds(*b"211226", *b"120000").unwrap();
        assert_eq!(sun_state(december, 69.65, 18.96), Sun::Down);
        // The next sunset after dark is a day-ish ahead, found from dusk too.
        let after = unix_seconds(*b"141026", *b"230000").unwrap();
        assert_eq!(sun_state(after, lat, lon), Sun::Down);
    }

    #[test]
    fn night_holds_until_the_sun_is_clearly_back() {
        assert!(!night_at(-3.0, false));
        assert!(night_at(-4.5, false));
        assert!(night_at(-3.0, true), "between the thresholds night stays");
        assert!(!night_at(-1.0, true));
        assert!(night_by_clock(21 * 3600));
        assert!(night_by_clock(3 * 3600));
        assert!(!night_by_clock(12 * 3600));
    }
}
