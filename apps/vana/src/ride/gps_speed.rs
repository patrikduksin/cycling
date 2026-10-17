//! Ground speed and distance from the GNSS receiver, for rides without a
//! wheel sensor.
//!
//! The receiver's own ground speed (Doppler, from RMC or VTG) is used when it
//! sends one for the fix. Otherwise speed comes from how far the fix moved
//! over the last few seconds, which averages out most position jitter. Either
//! way a reading is only as good as its fix: no fix or a fix older than
//! `FRESH_MS` is `Lost`, and a fix with poor geometry (`MAX_HDOP_TENTHS`, too
//! few satellites) is `Unknown`, so it can neither move nor stop a ride.
//! Below `STOPPED_BELOW_MM_S` the rider is standing: GPS cannot tell a crawl
//! from jitter, so that reads zero and adds no distance.
//!
//! A gap (a tunnel) has no speed and so no distance: the ride keeps
//! recording, and the recorded speeds, the ride page and the FIT export
//! agree on what was measured.
use device_api::gps::{FixState, Snapshot};

/// Below this (3.6 km/h) GPS speed reads zero: at a standstill, Doppler noise
/// and position jitter reach about this much.
pub const STOPPED_BELOW_MM_S: u32 = 1_000;
/// Fixes with a larger horizontal dilution (5.0) are too poor for speed.
pub const MAX_HDOP_TENTHS: u16 = 50;
/// A fix from fewer satellites is too poor for speed.
pub const MIN_SATELLITES: u8 = 4;
/// A reading whose fix arrived longer ago than this is lost. A 1 Hz receiver
/// sends a fix every second; two missed epochs are a dropout.
pub const FRESH_MS: u64 = 2_000;
/// Consecutive fixes further apart than this do not form a baseline.
const EPOCH_GAP_MS: u32 = 2_500;
/// Position-derived speed spans up to this many fixes, one per second.
const WINDOW: usize = 5;
/// Fixes closer than this to the newest kept one are not kept, so a faster
/// receiver gets the same multi-second baseline as a 1 Hz one.
const FIX_SPACING_MS: u32 = 900;
/// Position-derived speed needs at least this long a baseline.
const MIN_BASELINE_MS: u32 = 1_800;
/// Doppler from an epoch this close before the newest fix still describes it
/// (the newest epoch's position can arrive a few ms before its speed).
const DOPPLER_CARRY_MS: u32 = 1_500;
/// Faster than this (144 km/h) is a receiver fault, not a bicycle.
const MAX_SPEED_MM_S: u32 = 40_000;
const DAY_MS: u32 = 86_400_000;

/// What GPS says about the rider's speed right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ground {
    /// No fresh fix: no data, no fix, or the receiver went quiet.
    Lost,
    /// A fresh fix that cannot give a speed yet: poor geometry, or too few
    /// fixes for a baseline without Doppler.
    Unknown,
    /// Ground speed in mm/s; zero when standing.
    Measured(u32),
}

#[derive(Clone, Copy, Default)]
struct Fix {
    epoch_ms: u32,
    latitude_e7: i32,
    longitude_e7: i32,
}

#[derive(Clone, Copy)]
struct Latest {
    epoch_ms: u32,
    received_ms: u64,
    good: bool,
}

#[derive(Default)]
pub struct GpsSpeed {
    latest: Option<Latest>,
    /// Good fixes, oldest first, with no gap between them.
    window: [Fix; WINDOW],
    window_len: usize,
    /// The last three position-derived speeds, for a median.
    deltas: [u32; 3],
    deltas_len: usize,
    deltas_next: usize,
    doppler: Option<(u32, u32)>,
}

impl GpsSpeed {
    /// Take the receiver's snapshot.
    pub fn observe(&mut self, now: u64, gps: Option<&Snapshot>) {
        let fix = gps
            .filter(|gps| gps.state == FixState::Fresh)
            .and_then(|gps| Some((gps, gps.latitude_e7?, gps.longitude_e7?, gps.epoch_ms?)));
        let Some((gps, latitude_e7, longitude_e7, epoch_ms)) = fix else {
            // No fix now is no speed now, whatever the last fix said.
            self.latest = None;
            self.restart();
            return;
        };
        let good = gps.hdop_tenths.is_none_or(|hdop| hdop <= MAX_HDOP_TENTHS)
            && gps.satellites.is_none_or(|count| count >= MIN_SATELLITES);
        if good && let Some(speed) = gps.speed_mm_s {
            self.doppler = Some((epoch_ms, speed));
        }
        let received_ms = now.saturating_sub(gps.age_ms.unwrap_or(0));
        if let Some(latest) = &mut self.latest
            && latest.epoch_ms == epoch_ms
        {
            // Another sentence of the same epoch; its geometry may arrive late.
            if latest.good && !good {
                latest.good = false;
                self.restart();
            }
            return;
        }
        self.latest = Some(Latest {
            epoch_ms,
            received_ms,
            good,
        });
        if !good {
            self.restart();
            return;
        }
        let fix = Fix {
            epoch_ms,
            latitude_e7,
            longitude_e7,
        };
        self.push(fix);
    }

    /// The ground speed now.
    pub fn reading(&self, now: u64) -> Ground {
        let Some(latest) = self.latest else {
            return Ground::Lost;
        };
        if now.saturating_sub(latest.received_ms) > FRESH_MS {
            return Ground::Lost;
        }
        if !latest.good {
            return Ground::Unknown;
        }
        let doppler = self
            .doppler
            .filter(|&(epoch, _)| since(epoch, latest.epoch_ms) <= DOPPLER_CARRY_MS)
            .map(|(_, speed)| speed);
        let speed = doppler.or_else(|| self.median());
        match speed {
            Some(speed) if speed <= MAX_SPEED_MM_S => {
                Ground::Measured(if speed < STOPPED_BELOW_MM_S { 0 } else { speed })
            }
            _ => Ground::Unknown,
        }
    }

    fn restart(&mut self) {
        self.window_len = 0;
        self.deltas_len = 0;
        self.deltas_next = 0;
    }

    fn push(&mut self, fix: Fix) {
        if let Some(previous) = self.window[..self.window_len].last() {
            let after = since(previous.epoch_ms, fix.epoch_ms);
            if after < FIX_SPACING_MS {
                return;
            }
            if after > EPOCH_GAP_MS {
                self.restart();
            }
        }
        if self.window_len == WINDOW {
            self.window.copy_within(1.., 0);
            self.window_len -= 1;
        }
        self.window[self.window_len] = fix;
        self.window_len += 1;
        let oldest = self.window[0];
        let baseline = since(oldest.epoch_ms, fix.epoch_ms);
        if baseline < MIN_BASELINE_MS {
            return;
        }
        let elapsed = u64::from(baseline);
        let speed = (distance_mm(oldest, fix) * 1000 / elapsed).min(u64::from(u32::MAX)) as u32;
        self.deltas[self.deltas_next] = speed;
        self.deltas_next = (self.deltas_next + 1) % self.deltas.len();
        self.deltas_len = (self.deltas_len + 1).min(self.deltas.len());
    }

    fn median(&self) -> Option<u32> {
        let mut values = self.deltas;
        let values = &mut values[..self.deltas_len];
        values.sort_unstable();
        values.get(values.len() / 2).copied()
    }
}

/// Milliseconds from `from` to `to` within a UTC day, across midnight.
fn since(from: u32, to: u32) -> u32 {
    (to + DAY_MS - from) % DAY_MS
}

/// cos(latitude) in Q30, from a Taylor series good to 1e-5 within ±90°.
fn cos_q30(latitude_e7: i32) -> i64 {
    const ONE: i64 = 1 << 30;
    // Radians in Q30: latitude_e7 * pi / 1.8e9, with pi * 2^30 = 3_373_259_426.
    let x = i64::from(latitude_e7) * 3_373_259_426 / 1_800_000_000;
    let x2 = (x * x) >> 30;
    // 1 - x²/2 + x⁴/24 - x⁶/720 + x⁸/40320, by Horner.
    let mut sum = ONE;
    for divisor in [56, 30, 12, 2] {
        sum = ONE - ((x2 * sum) >> 30) / divisor;
    }
    sum.max(0)
}

/// Distance between two fixes in mm, on a local flat approximation of the
/// WGS-84 sphere; within 0.1% at bicycle distances.
fn distance_mm(a: Fix, b: Fix) -> u64 {
    const TURN_E7: i64 = 3_600_000_000;
    let north = i64::from(b.latitude_e7) - i64::from(a.latitude_e7);
    let mut east = (i64::from(b.longitude_e7) - i64::from(a.longitude_e7)).rem_euclid(TURN_E7);
    if east > TURN_E7 / 2 {
        east -= TURN_E7;
    }
    let middle = ((i64::from(a.latitude_e7) + i64::from(b.latitude_e7)) / 2) as i32;
    let east = (east * cos_q30(middle)) >> 30;
    let e7 = isqrt((north * north + east * east) as u128);
    // 1e-7 degree of a great circle is 11.131949 mm.
    (e7 * 11_131_949 / 1_000_000) as u64
}

fn isqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut x = 1u128 << (value.ilog2() / 2 + 1);
    loop {
        let next = (x + value / x) / 2;
        if next >= x {
            return x;
        }
        x = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// A fresh fix at `epoch_ms`, received at that time on the uptime clock.
    fn fix(epoch_ms: u32, north_m: f64, east_m: f64) -> Snapshot {
        let lat = -33.45 + north_m / 111_319.49;
        let lon = -70.66 + east_m / (111_319.49 * (-33.45f64).to_radians().cos());
        Snapshot {
            state: FixState::Fresh,
            latitude_e7: Some((lat * 1e7).round() as i32),
            longitude_e7: Some((lon * 1e7).round() as i32),
            satellites: Some(9),
            hdop_tenths: Some(9),
            epoch_ms: Some(epoch_ms),
            age_ms: Some(0),
            ..Snapshot::default()
        }
    }

    fn doppler(mut snapshot: Snapshot, speed_mm_s: u32) -> Snapshot {
        snapshot.speed_mm_s = Some(speed_mm_s);
        snapshot
    }

    /// Ride east at `speed` m/s for `seconds`, one fix a second without Doppler.
    fn ride(gps: &mut GpsSpeed, from_s: u32, seconds: u32, speed: f64) -> Vec<Ground> {
        (from_s..from_s + seconds)
            .map(|s| {
                let at = u64::from(s) * 1000;
                gps.observe(at, Some(&fix(s * 1000, 0.0, f64::from(s) * speed)));
                gps.reading(at)
            })
            .collect()
    }

    #[test]
    fn distance_is_accurate_at_bicycle_scale_and_across_the_antimeridian() {
        let at = |lat: f64, lon: f64| Fix {
            epoch_ms: 0,
            latitude_e7: (lat * 1e7) as i32,
            longitude_e7: (lon * 1e7) as i32,
        };
        // 0.01 degree of latitude is 1113.2 m.
        let north = distance_mm(at(45.0, 7.0), at(45.01, 7.0));
        assert!((north as i64 - 1_113_195).abs() < 1_000, "{north}");
        // 0.01 degree of longitude at 60 degrees is half that.
        let east = distance_mm(at(60.0, 7.0), at(60.0, 7.01));
        assert!((east as i64 - 556_597).abs() < 600, "{east}");
        let across = distance_mm(at(0.0, 179.999), at(0.0, -179.999));
        assert!((across as i64 - 222_639).abs() < 300, "{across}");
        assert_eq!(cos_q30(0), 1 << 30);
        assert!(cos_q30(900_000_000) < 1 << 17);
    }

    #[test]
    fn doppler_speed_is_used_and_reads_zero_when_standing() {
        let mut gps = GpsSpeed::default();
        assert_eq!(gps.reading(0), Ground::Lost);
        gps.observe(0, Some(&doppler(fix(0, 0.0, 0.0), 8_000)));
        assert_eq!(gps.reading(0), Ground::Measured(8_000));
        gps.observe(1000, Some(&doppler(fix(1000, 0.0, 8.0), 900)));
        assert_eq!(gps.reading(1000), Ground::Measured(0));
        // The next epoch's position arrives a moment before its speed.
        gps.observe(2000, Some(&fix(2000, 0.0, 8.0)));
        assert_eq!(gps.reading(2000), Ground::Measured(0));
        // A fix that stops coming is lost after two missed epochs.
        assert_eq!(gps.reading(4000), Ground::Measured(0));
        assert_eq!(gps.reading(4001), Ground::Lost);
    }

    #[test]
    fn no_fix_stale_and_poor_geometry_are_not_speeds() {
        let mut gps = GpsSpeed::default();
        let mut stale = doppler(fix(0, 0.0, 0.0), 8_000);
        stale.state = FixState::Stale;
        gps.observe(0, Some(&stale));
        assert_eq!(gps.reading(0), Ground::Lost);
        let none = Snapshot {
            state: FixState::NoFix,
            ..Snapshot::default()
        };
        gps.observe(0, Some(&none));
        gps.observe(0, None);
        assert_eq!(gps.reading(0), Ground::Lost);
        let mut poor = doppler(fix(0, 0.0, 0.0), 8_000);
        poor.hdop_tenths = Some(51);
        gps.observe(0, Some(&poor));
        assert_eq!(gps.reading(0), Ground::Unknown);
        let mut few = doppler(fix(1000, 0.0, 0.0), 8_000);
        few.satellites = Some(3);
        gps.observe(1000, Some(&few));
        assert_eq!(gps.reading(1000), Ground::Unknown);
        // Poor geometry reported by a later sentence of the same epoch.
        let mut gps = GpsSpeed::default();
        gps.observe(0, Some(&doppler(fix(0, 0.0, 0.0), 8_000)));
        gps.observe(5, Some(&poor));
        assert_eq!(gps.reading(5), Ground::Unknown);
        // An old fix's age counts: received 2.5 s ago is lost now.
        let mut gps = GpsSpeed::default();
        let mut late = doppler(fix(0, 0.0, 0.0), 8_000);
        late.age_ms = Some(2_500);
        gps.observe(2_500, Some(&late));
        assert_eq!(gps.reading(2_500), Ground::Lost);
    }

    #[test]
    fn position_speed_needs_a_baseline_and_follows_the_ride() {
        let mut gps = GpsSpeed::default();
        let readings = ride(&mut gps, 0, 20, 7.0);
        assert_eq!(&readings[..2], &[Ground::Unknown, Ground::Unknown]);
        for reading in &readings[2..] {
            let Ground::Measured(speed) = *reading else {
                panic!("{reading:?}");
            };
            assert!((6_930..=7_070).contains(&speed), "{speed}");
        }
        // Stopping: within about four seconds it reads zero.
        let mut stopped = (20..30).map(|s| {
            let at = u64::from(s) * 1000;
            gps.observe(at, Some(&fix(s * 1000, 0.0, 19.0 * 7.0)));
            gps.reading(at)
        });
        assert!(stopped.nth(4) == Some(Ground::Measured(0)));
        assert!(stopped.all(|reading| reading == Ground::Measured(0)));
    }

    #[test]
    fn standstill_jitter_reads_zero_and_never_reaches_the_resume_speed() {
        // Each fix lands up to 2.5 m from the true position in each axis.
        let mut gps = GpsSpeed::default();
        let mut seed = 0x2545_f491u32;
        let mut jitter = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (f64::from(seed % 5001) / 1000.0) - 2.5
        };
        let mut zero = 0;
        for s in 0..600u32 {
            let at = u64::from(s) * 1000;
            let snapshot = fix(s * 1000, jitter(), jitter());
            gps.observe(at, Some(&snapshot));
            match gps.reading(at) {
                Ground::Measured(0) => zero += 1,
                Ground::Measured(speed) => {
                    assert!(speed < crate::ride::auto::GPS.resume_at, "{speed} at {s}")
                }
                Ground::Unknown => assert!(s < 2),
                Ground::Lost => panic!("lost at {s}"),
            }
        }
        assert!(zero > 500, "{zero} of 600 seconds read zero");
    }

    #[test]
    fn a_gap_restarts_the_baseline_and_epochs_wrap_at_midnight() {
        let mut gps = GpsSpeed::default();
        ride(&mut gps, 0, 5, 7.0);
        // Three seconds without a fix: the old fixes are no baseline.
        gps.observe(8000, Some(&fix(8000, 0.0, 56.0)));
        gps.observe(9000, Some(&fix(9000, 0.0, 63.0)));
        assert_eq!(gps.reading(9000), Ground::Unknown);
        let day = DAY_MS;
        let mut gps = GpsSpeed::default();
        for (i, epoch) in [day - 2000, day - 1000, 0, 1000].into_iter().enumerate() {
            let at = i as u64 * 1000;
            gps.observe(at, Some(&fix(epoch, 0.0, i as f64 * 5.0)));
        }
        let Ground::Measured(speed) = gps.reading(3000) else {
            panic!()
        };
        assert!((4_950..=5_050).contains(&speed), "{speed}");
    }

    #[test]
    fn a_no_fix_report_ends_the_speed_at_once() {
        let mut gps = GpsSpeed::default();
        gps.observe(0, Some(&doppler(fix(0, 0.0, 0.0), 0)));
        assert_eq!(gps.reading(0), Ground::Measured(0));
        let none = Snapshot {
            state: FixState::NoFix,
            ..Snapshot::default()
        };
        gps.observe(500, Some(&none));
        assert_eq!(gps.reading(500), Ground::Lost);
    }

    #[test]
    fn a_five_hertz_receiver_keeps_a_multi_second_baseline() {
        // Fixes every 200 ms, 2.5 m of jitter, no Doppler.
        let mut gps = GpsSpeed::default();
        let mut seed = 0x1234_5678u32;
        let mut jitter = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (f64::from(seed % 5001) / 1000.0) - 2.5
        };
        for tick in 0..3000u32 {
            let at = u64::from(tick) * 200;
            gps.observe(at, Some(&fix(tick * 200, jitter(), jitter())));
            if let Ground::Measured(speed) = gps.reading(at) {
                assert!(speed < crate::ride::auto::GPS.resume_at, "{speed} at {at}");
            }
        }
        // Riding at 7 m/s still reads 7 m/s.
        let mut gps = GpsSpeed::default();
        for tick in 0..50u32 {
            let at = u64::from(tick) * 200;
            gps.observe(at, Some(&fix(tick * 200, 0.0, f64::from(tick) * 1.4)));
        }
        let Ground::Measured(speed) = gps.reading(9800) else {
            panic!()
        };
        assert!((6_900..=7_100).contains(&speed), "{speed}");
    }
}
