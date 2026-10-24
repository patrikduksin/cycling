//! Simulated GPS route and NMEA epoch generator for host scenarios.

use firmware_services::gps::days_from_civil;
use std::fmt::Write;

const EARTH_RADIUS_M: f64 = 6_371_008.8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
}

impl Point {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            ele: None,
        }
    }
}

/// Haversine metres.
pub fn distance_m(a: Point, b: Point) -> f64 {
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = p2 - p1;
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

fn bearing(a: Point, b: Point) -> f64 {
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlon = (b.lon - a.lon).to_radians();
    let y = dlon.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dlon.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

#[derive(Clone, Debug)]
pub struct Route {
    points: Vec<Point>,
    cumulative: Vec<f64>,
    closed: bool,
}

impl Route {
    fn new(points: Vec<Point>, closed: bool) -> Self {
        let mut cumulative = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                total += distance_m(points[i - 1], *p);
            }
            cumulative.push(total);
        }
        Self {
            points,
            cumulative,
            closed,
        }
    }

    /// Parses `<trkpt>` and `<rtept>` elements with `lat`/`lon` attributes and optional `<ele>`.
    pub fn from_gpx(text: &str) -> Result<Route, String> {
        let mut points = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            rest = &rest[start..];
            let tag_end = rest
                .find('>')
                .ok_or_else(|| "unterminated GPX tag".to_string())?;
            let tag = &rest[..tag_end];
            let name = tag[1..]
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            rest = &rest[tag_end + 1..];
            if name != "trkpt" && name != "rtept" {
                continue;
            }
            let lat = attribute(tag, "lat").ok_or("point without lat")?;
            let lon = attribute(tag, "lon").ok_or("point without lon")?;
            if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                return Err(format!("coordinate out of range: {lat},{lon}"));
            }
            let mut ele = None;
            if !tag.ends_with('/') {
                let close = rest.find(&format!("</{name}>")).unwrap_or(rest.len());
                let body = &rest[..close];
                if let Some(s) = body.find("<ele>") {
                    let value = &body[s + 5..];
                    if let Some(e) = value.find("</ele>") {
                        ele = value[..e].trim().parse().ok();
                    }
                }
                rest = &rest[close..];
            }
            points.push(Point { lat, lon, ele });
        }
        if points.is_empty() {
            return Err("GPX contains no track or route points".into());
        }
        Ok(Route::new(points, false))
    }

    /// A closed loop of `length_m` metres with a sinusoidal climb of `climb_m` amplitude.
    pub fn loop_around(center: Point, length_m: f64, climb_m: f64) -> Route {
        const STEPS: usize = 360;
        let radius = length_m / std::f64::consts::TAU;
        let dlat = (radius / EARTH_RADIUS_M).to_degrees();
        let dlon = dlat / center.lat.to_radians().cos().max(1e-6);
        let base = center.ele.unwrap_or(0.0);
        let mut points: Vec<Point> = (0..=STEPS)
            .map(|i| {
                let t = (i % STEPS) as f64 / STEPS as f64 * std::f64::consts::TAU;
                // Start at the south edge so the first heading is east.
                Point {
                    lat: center.lat - dlat * t.cos(),
                    lon: center.lon + dlon * t.sin(),
                    ele: Some(base + climb_m * t.sin()),
                }
            })
            .collect();
        points[STEPS] = points[0];
        Route::new(points, true)
    }

    /// A straight road east from `start` whose elevation follows `profile`:
    /// (distance m, elevation m) knots, linearly interpolated, in order.
    pub fn profile(start: Point, profile: &[(f64, f64)]) -> Route {
        const STEP_M: f64 = 10.0;
        let length = profile.last().map_or(0.0, |&(distance, _)| distance);
        let dlon = (STEP_M / EARTH_RADIUS_M).to_degrees() / start.lat.to_radians().cos().max(1e-6);
        let elevation = |d: f64| {
            let next = profile.partition_point(|&(at, _)| at <= d);
            match (next.checked_sub(1).map(|i| profile[i]), profile.get(next)) {
                (Some((d0, e0)), Some(&(d1, e1))) => e0 + (e1 - e0) * (d - d0) / (d1 - d0),
                (Some((_, e)), None) | (None, Some(&(_, e))) => e,
                (None, None) => 0.0,
            }
        };
        let steps = (length / STEP_M).ceil() as usize;
        let points = (0..=steps)
            .map(|i| {
                let d = (i as f64 * STEP_M).min(length);
                Point {
                    lat: start.lat,
                    lon: start.lon + dlon * d / STEP_M,
                    ele: Some(elevation(d)),
                }
            })
            .collect();
        Route::new(points, false)
    }

    pub fn length_m(&self) -> f64 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    fn locate(&self, distance_m: f64) -> (usize, f64) {
        let length = self.length_m();
        let d = if self.closed && length > 0.0 {
            distance_m.rem_euclid(length)
        } else {
            distance_m.clamp(0.0, length)
        };
        if self.points.len() < 2 {
            return (0, 0.0);
        }
        let i = self
            .cumulative
            .partition_point(|&c| c <= d)
            .clamp(1, self.points.len() - 1)
            - 1;
        let span = self.cumulative[i + 1] - self.cumulative[i];
        let f = if span > 0.0 {
            ((d - self.cumulative[i]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (i, f)
    }

    /// Position at a distance along the route, clamped (or wrapped for closed loops).
    pub fn at(&self, distance_m: f64) -> Point {
        let (i, f) = self.locate(distance_m);
        let a = self.points[i];
        let Some(b) = self.points.get(i + 1).copied() else {
            return a;
        };
        let lerp = |x: f64, y: f64| x + (y - x) * f;
        Point {
            lat: lerp(a.lat, b.lat),
            lon: lerp(a.lon, b.lon),
            ele: a.ele.zip(b.ele).map(|(x, y)| lerp(x, y)),
        }
    }

    /// Bearing in degrees of the segment at a distance.
    pub fn course(&self, distance_m: f64) -> f64 {
        let (i, _) = self.locate(distance_m);
        match self.points.get(i + 1) {
            Some(b) => bearing(self.points[i], *b),
            None => 0.0,
        }
    }
}

fn attribute(tag: &str, name: &str) -> Option<f64> {
    let mut from = 0;
    while let Some(found) = tag[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let after = tag[from..].trim_start();
        if !tag[..at].ends_with(char::is_whitespace) || !after.starts_with('=') {
            continue;
        }
        let value = after[1..].trim_start();
        let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let value = &value[1..];
        return value[..value.find(quote)?].trim().parse().ok();
    }
    None
}

pub struct Nmea {
    /// Receiver UTC in ms since the Unix epoch at simulated t=0.
    start_ms: i64,
    /// RMC carries the ground speed; without it the field is empty.
    pub doppler: bool,
    /// Horizontal dilution of precision reported in GGA.
    pub hdop: f64,
}

impl Nmea {
    /// `utc_start_s` is seconds since midnight UTC at simulated t=0; `date` is (day, month, year2).
    pub fn new(utc_start_s: u32, date: (u8, u8, u8)) -> Self {
        Self::reading(utc_start_s, date, 0)
    }

    /// A receiver whose clock reads `utc_s` seconds after midnight UTC on
    /// `date` (day, month, year2) at simulated time `now_ms`.
    pub fn reading(utc_s: u32, date: (u8, u8, u8), now_ms: u64) -> Self {
        let (day, month, year) = date;
        let days = days_from_civil(2000 + i64::from(year), u32::from(month), u32::from(day));
        Self {
            start_ms: (days * 86_400 + i64::from(utc_s)) * 1000 - now_ms as i64,
            doppler: true,
            hdop: 0.9,
        }
    }

    /// The receiver's `hhmmss.cc` and `ddmmyy` at simulated time `now_ms`: one
    /// instant, so the date turns at UTC midnight with the time.
    fn stamp(&self, now_ms: u64) -> (String, String) {
        let ms = self.start_ms + now_ms as i64;
        let (year, month, day) = civil_from_days(ms.div_euclid(86_400_000));
        let of_day = ms.rem_euclid(86_400_000);
        let (s, ms) = (of_day / 1000, of_day % 1000);
        (
            format!(
                "{:02}{:02}{:02}.{:02}",
                s / 3600,
                s / 60 % 60,
                s % 60,
                ms / 10
            ),
            format!("{day:02}{month:02}{:02}", year % 100),
        )
    }

    /// One 1 Hz epoch: GNGGA (quality 1) followed by GNRMC (status A).
    pub fn fix(
        &self,
        now_ms: u64,
        point: Point,
        speed_mps: f64,
        course_deg: f64,
        satellites: u8,
    ) -> Vec<u8> {
        let (time, date) = self.stamp(now_ms);
        let (lat, ns) = coordinate(point.lat, 2, 'N', 'S');
        let (lon, ew) = coordinate(point.lon, 3, 'E', 'W');
        let ele = point.ele.unwrap_or(0.0);
        let knots = if self.doppler {
            format!("{:.2}", speed_mps * 3600.0 / 1852.0)
        } else {
            String::new()
        };
        let hdop = self.hdop;
        let course = course_deg.rem_euclid(360.0);
        let mut out = String::new();
        sentence(
            &mut out,
            &format!(
                "GNGGA,{time},{lat},{ns},{lon},{ew},1,{satellites:02},{hdop:.1},{ele:.1},M,0.0,M,,"
            ),
        );
        sentence(
            &mut out,
            &format!("GNRMC,{time},A,{lat},{ns},{lon},{ew},{knots},{course:.1},{date},,,A"),
        );
        out.into_bytes()
    }

    /// A no-fix epoch: GNGGA quality 0 and GNRMC status V, both without position.
    pub fn no_fix(&self, now_ms: u64) -> Vec<u8> {
        let (time, date) = self.stamp(now_ms);
        let mut out = String::new();
        sentence(&mut out, &format!("GNGGA,{time},,,,,0,00,,,M,,M,,"));
        sentence(&mut out, &format!("GNRMC,{time},V,,,,,,,{date},,,N"));
        out.into_bytes()
    }
}

/// The (year, month, day) of a day count from 1970-01-01, the inverse of
/// `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = (of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn coordinate(value: f64, degree_digits: usize, positive: char, negative: char) -> (String, char) {
    let hemisphere = if value < 0.0 { negative } else { positive };
    // Whole ten-thousandths of a minute, so rounding carries into degrees.
    let units = (value.abs() * 60.0 * 10_000.0).round() as u64;
    let (degrees, rest) = (units / 600_000, units % 600_000);
    let text = format!(
        "{degrees:0degree_digits$}{:02}.{:04}",
        rest / 10_000,
        rest % 10_000
    );
    (text, hemisphere)
}

fn sentence(out: &mut String, body: &str) {
    let sum = body.bytes().fold(0, |sum, b| sum ^ b);
    let _ = write!(out, "${body}*{sum:02X}\r\n");
}

/// Converts parser output (lat/lon e7) back to degrees.
pub fn from_e7(lat_e7: i32, lon_e7: i32) -> Point {
    Point::new(f64::from(lat_e7) / 1e7, f64::from(lon_e7) / 1e7)
}

/// Renders points as a minimal GPX 1.1 document with optional elevation and ISO time.
pub fn to_gpx(name: &str, points: &[(Point, Option<u64>)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"cycling-simulator\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n",
    );
    let escaped = name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let _ = writeln!(out, "<trk><name>{escaped}</name><trkseg>");
    for (p, time) in points {
        let _ = write!(out, "<trkpt lat=\"{}\" lon=\"{}\">", p.lat, p.lon);
        if let Some(e) = p.ele {
            let _ = write!(out, "<ele>{e}</ele>");
        }
        if let Some(t) = time {
            let _ = write!(out, "<time>{}</time>", iso(*t));
        }
        out.push_str("</trkpt>\n");
    }
    out.push_str("</trkseg></trk>\n</gpx>\n");
    out
}

fn iso(unix: u64) -> String {
    let (days, secs) = ((unix / 86_400) as i64, unix % 86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::gps::FixState;
    use firmware_services::positioning::Acquisition;

    const CITIES: [(f64, f64); 5] = [
        (-33.45, -70.66),
        (48.13, 11.58),
        (-33.8688, 151.2093),
        (40.7128, -74.006),
        (0.0, 0.0),
    ];

    #[test]
    fn fixes_are_fresh_in_all_hemispheres() {
        let nmea = Nmea::new(12 * 3600, (14, 10, 26));
        for (lat, lon) in CITIES {
            let mut acq = Acquisition::default();
            let point = Point {
                lat,
                lon,
                ele: Some(520.4),
            };
            acq.ingest(1000, &nmea.fix(1000, point, 8.0, 123.0, 9), 0, 0);
            let snap = acq.snapshot(1100).gps;
            assert_eq!(snap.state, FixState::Fresh, "{lat},{lon}");
            assert_eq!(snap.satellites, Some(9));
            assert_eq!(snap.parse_errors + snap.checksum_errors, 0);
            let back = from_e7(snap.latitude_e7.unwrap(), snap.longitude_e7.unwrap());
            assert!((back.lat - lat).abs() < 1e-5, "{lat}");
            assert!((back.lon - lon).abs() < 1e-5, "{lon}");
            assert_eq!(snap.utc, Some(*b"120001"));
        }
    }

    #[test]
    fn minute_rounding_carries_and_midnight_wraps() {
        let nmea = Nmea::new(86_399, (1, 1, 27));
        let point = Point::new(10.999_999_9, -20.0);
        let text = String::from_utf8(nmea.fix(1500, point, 0.0, 0.0, 5)).unwrap();
        assert!(text.contains("GNGGA,000000.50,1100.0000,N,02000.0000,W,"));
        assert!(
            text.contains(",020127,,,A*"),
            "the date turns with the time"
        );
        let mut acq = Acquisition::default();
        acq.ingest(0, text.as_bytes(), 0, 0);
        assert_eq!(acq.snapshot(0).gps.state, FixState::Fresh);
    }

    #[test]
    fn the_date_follows_the_clock_through_months_and_leap_days() {
        let date = |start: (u32, (u8, u8, u8)), now_ms| {
            let (time, date) = Nmea::new(start.0, start.1).stamp(now_ms);
            format!("{date} {time}")
        };
        assert_eq!(date((86_399, (31, 12, 26)), 1_000), "010127 000000.00");
        assert_eq!(date((86_399, (28, 2, 28)), 1_000), "290228 000000.00");
        assert_eq!(date((86_399, (29, 2, 28)), 1_000), "010328 000000.00");
        assert_eq!(date((0, (14, 10, 26)), 0), "141026 000000.00");
    }

    #[test]
    fn no_fix_and_stale() {
        let nmea = Nmea::new(0, (1, 1, 26));
        let mut acq = Acquisition::default();
        acq.ingest(0, &nmea.no_fix(0), 0, 0);
        assert_eq!(acq.snapshot(0).gps.state, FixState::NoFix);
        let epoch = nmea.fix(1000, Point::new(48.13, 11.58), 5.0, 90.0, 7);
        acq.ingest(1000, &epoch, 0, 0);
        assert_eq!(acq.snapshot(1000).gps.state, FixState::Fresh);
        assert_eq!(acq.snapshot(4001).gps.state, FixState::Stale);
    }

    #[test]
    fn profile_roads_follow_their_elevation_knots() {
        let start = Point::new(-33.45, -70.66);
        let route = Route::profile(start, &[(0.0, 500.0), (1000.0, 500.0), (2500.0, 590.0)]);
        assert!(
            (route.length_m() - 2500.0).abs() < 1.0,
            "{}",
            route.length_m()
        );
        assert_eq!(route.at(500.0).ele, Some(500.0));
        let mid = route.at(1750.0).ele.unwrap();
        assert!((mid - 545.0).abs() < 0.5, "{mid}");
        assert_eq!(route.at(9_000.0).ele, Some(590.0));
        assert!((route.course(100.0) - 90.0).abs() < 0.1);
    }

    #[test]
    fn gpx_round_trip() {
        let points = [
            (
                Point {
                    lat: -33.45,
                    lon: -70.66,
                    ele: Some(500.0),
                },
                Some(1_790_000_000),
            ),
            (Point::new(-33.451, -70.659), None),
        ];
        let gpx = to_gpx("a & b", &points);
        assert!(gpx.contains("<time>2026-"));
        let route = Route::from_gpx(&gpx).unwrap();
        assert_eq!(route.points.len(), 2);
        assert_eq!(route.points[0], points[0].0);
        assert_eq!(route.points[1], points[1].0);
        assert!(route.length_m() > 100.0);
        assert!(Route::from_gpx("<gpx></gpx>").is_err());
        let rte = "<rte><rtept lat='1.5' lon='2.5'/></rte>";
        assert_eq!(
            Route::from_gpx(rte).unwrap().points[0],
            Point::new(1.5, 2.5)
        );
    }

    #[test]
    fn loop_is_closed_with_requested_length() {
        let route = Route::loop_around(Point::new(48.13, 11.58), 20_000.0, 50.0);
        assert!((route.length_m() / 20_000.0 - 1.0).abs() < 0.01);
        let (start, end) = (route.points[0], *route.points.last().unwrap());
        assert!(distance_m(start, end) < 1.0);
        let wrapped = route.at(route.length_m() + 500.0);
        assert!(distance_m(wrapped, route.at(500.0)) < 1.0);
        let heading = route.course(1.0);
        assert!((heading - 90.0).abs() < 5.0, "{heading}");
    }

    #[test]
    fn interpolation_is_monotonic_and_clamped() {
        let route = Route::from_gpx(&to_gpx(
            "r",
            &[
                (Point::new(0.0, 0.0), None),
                (Point::new(0.0, 0.01), None),
                (Point::new(0.01, 0.01), None),
            ],
        ))
        .unwrap();
        let mut d = 0.0;
        while d + 10.0 < route.length_m() {
            let step = distance_m(route.at(d), route.at(d + 10.0));
            assert!(step > 5.0 && step <= 10.5, "{d} {step}");
            d += 10.0;
        }
        assert_eq!(route.at(1e9), route.at(route.length_m()));
        assert!((route.course(10.0) - 90.0).abs() < 0.1);
        assert!(route.course(route.length_m() - 10.0).abs() < 0.1);
    }
}
