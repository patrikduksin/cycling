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
