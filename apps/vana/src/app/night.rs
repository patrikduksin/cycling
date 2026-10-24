//! Night: after dark the backlight steps down to `NIGHT_PERCENT`, so the
//! screen does not glare, and steps back up in the morning.
//!
//! In automatic mode the sun decides: its altitude at the last fixed position
//! and the recorded UTC (network time, else the receiver's clock).
//! Without a position it falls back to the local clock, and without any clock
//! it keeps what it had. The change ramps over about two seconds through the
//! shell's backlight ceiling, which is never stored, so the rider's chosen
//! brightness stays as it was.
use super::Runtime;
use super::preferences::Daylight;
use crate::daylight::{Sun, night_at, night_by_clock, sun_altitude, sun_state};
use device_api::display::Display;
use device_api::gps::FixState;
use device_api::input::InputSource;
use device_api::positioning::Positioning;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::Shell;

/// The most backlight at night.
pub const NIGHT_PERCENT: u8 = 20;
/// The ceiling moves this much per step, every `STEP_MS`.
const STEP_PERCENT: u8 = 4;
const STEP_MS: u64 = 100;
/// How often automatic mode looks at the sun again.
const CHECK_MS: u64 = 10_000;
/// How often the sunset is worked out again (about sixty sun altitudes).
const SUN_MS: u64 = 60_000;
/// The clock VANA shows is UTC-3 (Santiago summer time) in this build.
const LOCAL_OFFSET_S: u64 = 3 * 3600;

pub(super) struct Night {
    night: bool,
    /// The backlight ceiling being ramped, in percent.
    ceiling: u8,
    mode: Option<Daylight>,
    next_check_ms: u64,
    next_step_ms: u64,
    /// The last fixed position, in degrees.
    place: Option<(f64, f64)>,
    /// The time of day of the last fresh fix, carried on by uptime: the
    /// fallback when no UTC is recorded.
    clock: Option<Clock>,
    /// The sun as last worked out, the Unix time it was for and the uptime.
    sun: Option<(Sun, i64, u64)>,
    next_sun_ms: u64,
}

/// A receiver time of day and the uptime it was read at.
#[derive(Clone, Copy)]
struct Clock {
    /// Seconds since UTC midnight.
    seconds: i64,
    at_ms: u64,
}

impl Clock {
    fn at(self, now: u64) -> i64 {
        self.seconds + (now.saturating_sub(self.at_ms) / 1000) as i64
    }
}

impl Default for Night {
    fn default() -> Self {
        Self {
            night: false,
            ceiling: 100,
            mode: None,
            next_check_ms: 0,
            next_step_ms: 0,
            place: None,
            clock: None,
            sun: None,
            next_sun_ms: 0,
        }
    }
}

/// What VANA knows of where and when it is.
#[derive(Clone, Copy, Default)]
pub(super) struct Sky {
    /// Degrees latitude and longitude of a fresh fix.
    pub place: Option<(f64, f64)>,
    pub unix_seconds: Option<i64>,
    /// Seconds since UTC midnight, when only the time of day is known.
    pub utc_of_day: Option<u64>,
}

impl Night {
    pub(super) fn is_night(&self) -> bool {
        self.night
    }

    /// Work out the sun's state at the last fixed place for `unix` (UTC,
    /// dated), at most once a minute, or at once when the sunset it knows
    /// has passed. Without a place or a date nothing is known.
    pub(super) fn watch_sun(&mut self, unix: Option<i64>, now: u64) {
        let (Some(unix), Some((latitude, longitude))) = (unix, self.place) else {
            self.sun = None;
            return;
        };
        let passed = matches!(self.sun, Some((Sun::Up { sets_at }, ..)) if sets_at <= unix);
        if self.sun.is_some() && now < self.next_sun_ms && !passed {
            return;
        }
        self.next_sun_ms = now.saturating_add(SUN_MS);
        self.sun = Some((sun_state(unix, latitude, longitude), unix, now));
    }

    /// The sun's state and the Unix time now, carried on from the last
    /// working out; `None` when position or date is unknown.
    pub(super) fn sun(&self, now: u64) -> Option<(Sun, i64)> {
        let (sun, unix, at_ms) = self.sun?;
        Some((sun, unix + (now.saturating_sub(at_ms) / 1000) as i64))
    }

    /// Decide day or night for `mode` under `sky`. Automatic mode looks
    /// again every `CHECK_MS`; a new mode applies at once.
    pub(super) fn decide(&mut self, mode: Daylight, sky: Sky, now: u64) {
        if let Some(place) = sky.place {
            self.place = Some(place);
        }
        if self.mode == Some(mode) && now < self.next_check_ms {
            return;
        }
        self.mode = Some(mode);
        self.next_check_ms = now.saturating_add(CHECK_MS);
        self.night = match mode {
            Daylight::Day => false,
            Daylight::Night => true,
            Daylight::Auto => match (sky.unix_seconds, self.place) {
                (Some(time), Some((latitude, longitude))) => {
                    night_at(sun_altitude(time, latitude, longitude), self.night)
                }
                _ => match sky.utc_of_day {
                    Some(seconds) => night_by_clock(seconds + 86_400 - LOCAL_OFFSET_S),
                    None => self.night,
                },
            },
        };
    }

    /// Step the ceiling towards day or night under the rider's
    /// `brightness`; returns the ceiling to apply, 100 once fully day.
    pub(super) fn step(&mut self, brightness: u8, now: u64) -> Option<u8> {
        if now < self.next_step_ms {
            return None;
        }
        self.next_step_ms = now.saturating_add(STEP_MS);
        // Ramp from what shows: a ceiling above the brightness changes
        // nothing, so start from the brightness itself.
        let top = brightness.max(NIGHT_PERCENT);
        let target = if self.night { NIGHT_PERCENT } else { top };
        let from = self.ceiling.min(top);
        self.ceiling = if from > target {
            from.saturating_sub(STEP_PERCENT).max(target)
        } else {
            from.saturating_add(STEP_PERCENT).min(target)
        };
        Some(if !self.night && self.ceiling >= top {
            self.ceiling = 100;
            100
        } else {
            self.ceiling
        })
    }
}

impl Runtime {
    /// Whether the screens take the light look now.
    pub(super) fn light(&self) -> bool {
        self.preferences
            .options
            .theme
            .is_light(self.night.is_night())
    }

    /// Follow day and night with the backlight. Call once per loop.
    pub(super) fn follow_daylight<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        position: &impl Positioning,
    ) {
        let gps = position.snapshot(now).map(|snapshot| snapshot.gps);
        let fresh = gps.filter(|gps| gps.state == FixState::Fresh);
        let place = fresh.and_then(|gps| {
            let (latitude, longitude) = (gps.latitude_e7?, gps.longitude_e7?);
            Some((f64::from(latitude) / 1e7, f64::from(longitude) / 1e7))
        });
        // The time of day of a fresh fix, for when no UTC is recorded.
        let read = fresh.and_then(|gps| {
            let time = gps.utc?;
            let pair = |at: usize| i64::from((time[at] - b'0') * 10 + time[at + 1] - b'0');
            time.iter()
                .all(u8::is_ascii_digit)
                .then(|| pair(0) * 3600 + pair(2) * 60 + pair(4))
        });
        if let Some(seconds) = read {
            self.night.clock = Some(Clock {
                seconds,
                at_ms: now,
            });
        }
        let unix = self.readings.live.utc_ms.map(|ms| (ms / 1000) as i64);
        let utc_of_day = unix
            .or_else(|| self.night.clock.map(|clock| clock.at(now)))
            .map(|seconds| seconds.rem_euclid(86_400) as u64);
        self.night.watch_sun(unix, now);
        let was = self.night.is_night();
        let sky = Sky {
            place,
            unix_seconds: unix,
            utc_of_day,
        };
        self.night
            .decide(self.preferences.options.daylight, sky, now);
        if was != self.night.is_night() {
            self.display.redraw();
        }
        if let Some(ceiling) = self.night.step(system.settings.brightness, now) {
            system.set_ceiling(ceiling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_services::gps::unix_seconds;

    #[test]
    fn night_ramps_the_ceiling_down_from_what_shows_and_back_up() {
        let mut night = Night::default();
        night.decide(Daylight::Night, Sky::default(), 0);
        let steps: std::vec::Vec<u8> = (0..12)
            .filter_map(|i| night.step(50, i * STEP_MS))
            .collect();
        assert_eq!(steps[0], 46, "starts from the brightness, not 100");
        assert!(
            steps
                .windows(2)
                .all(|w| w[1] <= w[0] && w[0] - w[1] <= STEP_PERCENT)
        );
        assert_eq!(*steps.last().unwrap(), NIGHT_PERCENT);
        night.decide(Daylight::Day, Sky::default(), 2_000);
        let back: std::vec::Vec<u8> = (20..40)
            .filter_map(|i| night.step(50, i * STEP_MS))
            .collect();
        assert_eq!(back[0], 24);
        assert_eq!(*back.last().unwrap(), 100, "day lifts the ceiling");
    }

    #[test]
    fn automatic_mode_follows_the_sun_then_the_clock() {
        let mut night = Night::default();
        let santiago = Some((-33.45, -70.66));
        let at = |time: &[u8; 6]| unix_seconds(*b"141026", *time);
        let sky = |time| Sky {
            place: santiago,
            unix_seconds: at(time),
            utc_of_day: None,
        };
        night.decide(Daylight::Auto, sky(b"220000"), 0);
        assert!(!night.is_night());
        night.decide(Daylight::Auto, sky(b"232000"), 5_000);
        assert!(!night.is_night(), "waits for the next check");
        night.decide(Daylight::Auto, sky(b"232000"), CHECK_MS);
        assert!(night.is_night());
        // Without a date, the place is kept but the local clock decides:
        // 10:00 UTC is 7:00 in Santiago.
        let clock = |seconds| Sky {
            place: None,
            unix_seconds: None,
            utc_of_day: Some(seconds),
        };
        night.decide(Daylight::Auto, clock(10 * 3600), 2 * CHECK_MS);
        assert!(!night.is_night());
        night.decide(Daylight::Auto, clock(0), 3 * CHECK_MS);
        assert!(night.is_night(), "21:00 local");
        // Nothing known keeps the last answer.
        night.decide(Daylight::Auto, Sky::default(), 4 * CHECK_MS);
        assert!(night.is_night());
    }
}
