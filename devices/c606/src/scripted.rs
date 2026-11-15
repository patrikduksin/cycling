//! A measurement build's receiver: NMEA for a ride through central
//! Santiago, fed to the parser in place of the UART's bytes, so the map's
//! roads can be timed on the device in the densest view (#361). Only
//! `build.sh scripted-gnss` compiles it; the image riders flash never does.
//!
//! The rider waits at Plaza de Armas for `HOLD_MS` after boot, long enough
//! to start a ride and page to the map, then rides the committed fixture
//! course at `SPEED_M_S` for `ON_COURSE_M`, and carries straight on for
//! `OFF_COURSE_M`, so the map also shows the way back to a junction far
//! behind, at its widest step. Then it stands at the end.
use core::fmt::Write as _;

const COURSE: &str =
    include_str!("../../../packages/terrain/tests/data/santiago-centro-course.gpx");
const HOLD_MS: u64 = 120_000;
const SPEED_M_S: f64 = 10.0;
const ON_COURSE_M: f64 = 3_500.0;
const OFF_COURSE_M: f64 = 1_600.0;
const M_PER_DEGREE: f64 = 111_195.0;

/// Where the script is: the last fix it sent and when.
pub struct Script {
    started_ms: u64,
    sent_s: Option<u64>,
}

impl Script {
    pub fn new(now: u64) -> Self {
        Self {
            started_ms: now,
            sent_s: None,
        }
    }

    /// This second's sentences into `bytes`, once a second; the byte count.
    pub fn sentences(&mut self, now: u64, bytes: &mut [u8]) -> usize {
        let elapsed_ms = now.saturating_sub(self.started_ms);
        let second = elapsed_ms / 1000;
        if self.sent_s == Some(second) {
            return 0;
        }
        self.sent_s = Some(second);
        let ridden_m = elapsed_ms.saturating_sub(HOLD_MS) as f64 / 1000.0 * SPEED_M_S;
        let (lat, lon, bearing) = place(ridden_m.min(ON_COURSE_M + OFF_COURSE_M));
        let moving = ridden_m > 0.0 && ridden_m < ON_COURSE_M + OFF_COURSE_M;
        let knots = if moving { SPEED_M_S * 1.943_844 } else { 0.0 };
        let clock = 36_000 + second;
        let time = (clock / 3600 % 24, clock / 60 % 60, clock % 60);
        let mut out = Out { bytes, len: 0 };
        let (lat, lon) = (Angle::of(lat, 2), Angle::of(lon, 3));
        sentence(
            &mut out,
            format_args!(
                "GPGGA,{:02}{:02}{:02}.00,{},{},1,10,0.8,560.0,M,0.0,M,,",
                time.0, time.1, time.2, lat, lon
            ),
        );
        sentence(
            &mut out,
            format_args!(
                "GPRMC,{:02}{:02}{:02}.00,A,{},{},{:.2},{:.1},151126,,,A",
                time.0, time.1, time.2, lat, lon, knots, bearing
            ),
        );
        out.len
    }
}

/// `$body*checksum\r\n`, dropped if it does not fit.
fn sentence(out: &mut Out, body: core::fmt::Arguments) {
    let mut bytes = [0u8; 120];
    let mut line = Out {
        bytes: &mut bytes,
        len: 0,
    };
    if line.write_fmt(body).is_err() {
        return;
    }
    let line = &line.bytes[..line.len];
    let sum = line.iter().fold(0u8, |sum, b| sum ^ b);
    let text = core::str::from_utf8(line).unwrap_or("");
    let _ = write!(out, "${text}*{sum:02X}\r\n");
}

struct Out<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for Out<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        let room = self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        room.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Degrees as NMEA's `dddmm.mmmmm,H`.
struct Angle {
    degrees: u32,
    minutes: f64,
    hemisphere: char,
    width: usize,
}

impl Angle {
    fn of(value: f64, width: usize) -> Self {
        let hemisphere = match (width, value < 0.0) {
            (2, false) => 'N',
            (2, true) => 'S',
            (_, false) => 'E',
            (_, true) => 'W',
        };
        let value = value.abs();
        Self {
            degrees: value as u32,
            minutes: (value - f64::from(value as u32)) * 60.0,
            hemisphere,
            width,
        }
    }
}

impl core::fmt::Display for Angle {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "{:0width$}{:08.5},{}",
            self.degrees,
            self.minutes,
            self.hemisphere,
            width = self.width
        )
    }
}

/// The course's points, (lat, lon) in degrees.
fn points() -> impl Iterator<Item = (f64, f64)> + Clone {
    COURSE.split("<trkpt ").skip(1).filter_map(|point| {
        let value = |key: &str| -> Option<f64> {
            let start = point.find(key)? + key.len();
            let end = start + point[start..].find('"')?;
            point[start..end].parse().ok()
        };
        Some((value("lat=\"")?, value("lon=\"")?))
    })
}

/// Metres east and north from `a` to `b`.
fn east_north(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let east = (b.1 - a.1) * M_PER_DEGREE * libm::cos(a.0.to_radians());
    (east, (b.0 - a.0) * M_PER_DEGREE)
}

/// Where the rider is `ridden_m` along: on the course up to
/// `ON_COURSE_M`, then straight on along its last bearing. With the bearing
/// of travel, degrees from north.
fn place(ridden_m: f64) -> (f64, f64, f64) {
    let mut walked = 0.0;
    let mut last = None;
    let mut bearing = 0.0;
    for (a, b) in points().zip(points().skip(1)) {
        let (east, north) = east_north(a, b);
        let length = libm::hypot(east, north);
        if length < 0.01 {
            continue;
        }
        bearing = libm::atan2(east, north).to_degrees();
        if bearing < 0.0 {
            bearing += 360.0;
        }
        let along = ridden_m.min(ON_COURSE_M) - walked;
        if along <= length {
            let t = along.max(0.0) / length;
            let at = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
            return ahead(at, bearing, ridden_m - ON_COURSE_M.min(ridden_m));
        }
        walked += length;
        last = Some(b);
    }
    let end = last.unwrap_or((-33.4378, -70.6505));
    ahead(end, bearing, ridden_m - walked)
}

/// `metres` on from `at` along `bearing`.
fn ahead(at: (f64, f64), bearing: f64, metres: f64) -> (f64, f64, f64) {
    let (sin, cos) = (
        libm::sin(bearing.to_radians()),
        libm::cos(bearing.to_radians()),
    );
    let lat = at.0 + metres * cos / M_PER_DEGREE;
    let lon = at.1 + metres * sin / (M_PER_DEGREE * libm::cos(at.0.to_radians()));
    (lat, lon, bearing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_services::positioning::Acquisition;

    fn fix(script: &mut Script, now: u64) -> (i32, i32, Option<u32>) {
        let mut bytes = [0u8; 2048];
        let count = script.sentences(now, &mut bytes);
        assert!(count > 0, "a sentence a second");
        let mut parser = Acquisition::default();
        parser.ingest(now, &bytes[..count], 0, 0);
        let gps = parser.snapshot(now).gps;
        (
            gps.latitude_e7.unwrap(),
            gps.longitude_e7.unwrap(),
            gps.speed_mm_s,
        )
    }

    #[test]
    fn the_rider_waits_at_the_plaza_then_rides_the_course_and_off_it() {
        let mut script = Script::new(1_000);
        let (lat, lon, speed) = fix(&mut script, 1_000);
        let start = points().next().unwrap();
        assert!((f64::from(lat) * 1e-7 - start.0).abs() < 1e-6, "{lat}");
        assert!((f64::from(lon) * 1e-7 - start.1).abs() < 1e-6, "{lon}");
        assert_eq!(speed, Some(0));
        let mut zero = [0u8; 2048];
        assert_eq!(script.sentences(1_500, &mut zero), 0, "one fix a second");
        let (_, _, speed) = fix(&mut script, 1_000 + HOLD_MS + 10_000);
        assert!(
            speed.is_some_and(|mm_s| mm_s.abs_diff(10_000) < 50),
            "{speed:?}"
        );
        let off = place(ON_COURSE_M + OFF_COURSE_M);
        let on = place(ON_COURSE_M);
        let (east, north) = east_north((on.0, on.1), (off.0, off.1));
        assert!((libm::hypot(east, north) - OFF_COURSE_M).abs() < 1.0);
    }
}
