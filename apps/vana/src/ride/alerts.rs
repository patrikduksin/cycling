//! Ride alerts: heart rate and power outside a band, low cadence, an eat and
//! drink reminder, a battery that may not last, and radar vehicle counts.
//! Each alert is a short sound and a toast.
//!
//! Pure policy: the caller feeds the live sample, the ride state and the radar
//! once per tick, then plays and shows what `update` returns. Band alerts
//! fire only while recording, after the reading has stayed outside its band
//! for `HOLD_MS`, so a surge or a dropout says nothing. While the reading
//! stays outside, the alert repeats every `REPEAT_MS`. It re-arms once the
//! reading has been back inside for `HOLD_MS`. At most one alert fires per
//! tick, and every alert waits `SPACING_MS` after the last one so they never
//! pile up.
//!
//! The rider's choices are `Settings`, stored with the VANA preferences.
//! 0 turns a limit off.
use crate::clock::Zone;
use crate::daylight::Sun;
use crate::ride::log::{Sample, Status};
use crate::ride::turn_back::{self, Turn};
use crate::sensors::radar::{self, Snapshot};

/// A reading must stay outside its band this long to alert, and back inside
/// this long to re-arm.
pub const HOLD_MS: u64 = 10_000;
/// A reading still outside its band alerts again after this long.
pub const REPEAT_MS: u64 = 120_000;
/// Two alerts are at least this far apart.
pub const SPACING_MS: u64 = 3_000;
/// The radar must stay empty this long before ALL CLEAR, so a target that
/// flickers out and back does not call it.
pub const CLEAR_AFTER_MS: u64 = 2_000;
/// With no workout length known, the battery warns below this many minutes.
pub const BATTERY_HOUR_MIN: u32 = 60;
/// The estimate must stay under its threshold this long before it warns: it
/// is least reliable just after it first appears.
pub const BATTERY_HOLD_MS: u64 = 5 * 60_000;
/// Bytes in an encoded `Settings`.
pub const SETTINGS_BYTES: usize = 9;
/// Allowed reminder intervals, in minutes.
pub const FUEL_MINUTES: [u8; 5] = [0, 20, 30, 45, 60];
/// The Settings choices for each limit; 0 is off. Every low choice is
/// below every high one, so any pair stays a valid band.
pub const HEART_HIGH_BPM: [u8; 6] = [0, 150, 160, 170, 180, 190];
pub const HEART_LOW_BPM: [u8; 6] = [0, 90, 100, 110, 120, 130];
pub const POWER_HIGH_W: [u16; 6] = [0, 250, 300, 350, 400, 500];
pub const POWER_LOW_W: [u16; 4] = [0, 100, 150, 200];
pub const CADENCE_LOW_RPM: [u8; 4] = [0, 60, 70, 80];

/// The rider's alert choices. A zero limit is off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub heart_low_bpm: u8,
    pub heart_high_bpm: u8,
    pub power_low_w: u16,
    pub power_high_w: u16,
    pub cadence_low_rpm: u8,
    /// Minutes of ride time between eat and drink reminders (`FUEL_MINUTES`).
    pub fuel_every_min: u8,
    /// Vehicle counts and ALL CLEAR from a radar. The approach beep is a
    /// safety cue and does not depend on this.
    pub radar: bool,
}

impl Default for Settings {
    /// Everything off except the radar cues.
    fn default() -> Self {
        Self {
            heart_low_bpm: 0,
            heart_high_bpm: 0,
            power_low_w: 0,
            power_high_w: 0,
            cadence_low_rpm: 0,
            fuel_every_min: 0,
            radar: true,
        }
    }
}

impl Settings {
    pub fn encode(&self) -> [u8; SETTINGS_BYTES] {
        let [power_low_0, power_low_1] = self.power_low_w.to_le_bytes();
        let [power_high_0, power_high_1] = self.power_high_w.to_le_bytes();
        [
            self.heart_low_bpm,
            self.heart_high_bpm,
            power_low_0,
            power_low_1,
            power_high_0,
            power_high_1,
            self.cadence_low_rpm,
            self.fuel_every_min,
            u8::from(self.radar),
        ]
    }

    /// Settings from `encode`, or `None` when the bytes are short or a value
    /// is out of range: a low limit at or above its high limit, heart rate
    /// over 250, power over 2000 W, cadence over 200, an unknown reminder
    /// interval or a radar flag other than 0 or 1.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; SETTINGS_BYTES] = bytes.get(..SETTINGS_BYTES)?.try_into().ok()?;
        let settings = Self {
            heart_low_bpm: bytes[0],
            heart_high_bpm: bytes[1],
            power_low_w: u16::from_le_bytes([bytes[2], bytes[3]]),
            power_high_w: u16::from_le_bytes([bytes[4], bytes[5]]),
            cadence_low_rpm: bytes[6],
            fuel_every_min: bytes[7],
            radar: match bytes[8] {
                0 => false,
                1 => true,
                _ => return None,
            },
        };
        settings.valid().then_some(settings)
    }

    pub fn valid(&self) -> bool {
        let band = |low: u16, high: u16, max: u16| {
            low <= max && high <= max && (low == 0 || high == 0 || low < high)
        };
        band(
            u16::from(self.heart_low_bpm),
            u16::from(self.heart_high_bpm),
            250,
        ) && band(self.power_low_w, self.power_high_w, 2000)
            && self.cadence_low_rpm <= 200
            && FUEL_MINUTES.contains(&self.fuel_every_min)
    }
}

/// Which sound an alert plays; the runtime maps it to a cue of its palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cue {
    /// A reading left its band.
    Band,
    /// Time to eat and drink.
    Fuel,
    /// The last vehicle passed.
    RadarClear,
    /// The light is about to end, or has; the last turn home is near.
    Sunset,
    /// The battery may not last the ride.
    Battery,
}

/// One alert to play and show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Alert {
    HeartHigh(u16),
    HeartLow(u16),
    PowerHigh(u16),
    PowerLow(u16),
    CadenceLow(u16),
    Fuel,
    /// The battery may not last: the minutes it should.
    BatteryLeft(u16),
    /// More vehicles behind than before: the count now.
    Vehicles(u8),
    RadarClear,
    /// Sunset is about half an hour away.
    SunsetSoon,
    /// The sun has set: lights on.
    SunsetNow,
    /// The latest turn home that is still home by sunset: minutes after local
    /// midnight.
    TurnBackBy(u16),
    /// That turn has passed.
    TurnBackNow,
}

impl Alert {
    /// The sound, if any. A rising vehicle count has none: the approach beep
    /// already sounds for the vehicle that matters.
    pub fn cue(self) -> Option<Cue> {
        match self {
            Self::Fuel => Some(Cue::Fuel),
            Self::RadarClear => Some(Cue::RadarClear),
            Self::SunsetSoon | Self::SunsetNow | Self::TurnBackBy(_) | Self::TurnBackNow => {
                Some(Cue::Sunset)
            }
            Self::BatteryLeft(_) => Some(Cue::Battery),
            Self::Vehicles(_) => None,
            _ => Some(Cue::Band),
        }
    }

    /// The toast text, such as `HR HIGH 172`.
    pub fn text(self) -> heapless::String<20> {
        use core::fmt::Write;
        let mut text = heapless::String::new();
        let _ = match self {
            Self::HeartHigh(bpm) => write!(text, "HR HIGH {bpm}"),
            Self::HeartLow(bpm) => write!(text, "HR LOW {bpm}"),
            Self::PowerHigh(watts) => write!(text, "POWER HIGH {watts}W"),
            Self::PowerLow(watts) => write!(text, "POWER LOW {watts}W"),
            Self::CadenceLow(rpm) => write!(text, "CADENCE LOW {rpm}"),
            Self::Fuel => write!(text, "EAT AND DRINK"),
            Self::BatteryLeft(minutes) => write!(
                text,
                "BATTERY {} LEFT",
                crate::ride::battery_life::about(minutes)
            ),
            Self::Vehicles(1) => write!(text, "1 VEHICLE"),
            Self::Vehicles(count) => write!(text, "{count} VEHICLES"),
            Self::RadarClear => write!(text, "ALL CLEAR"),
            // The font is ASCII: a dash stands for the middle dot.
            Self::SunsetSoon => write!(text, "SUNSET IN 30 MIN"),
            Self::SunsetNow => write!(text, "SUNSET - LIGHTS ON"),
            Self::TurnBackBy(minute) => {
                write!(
                    text,
                    "TURN BACK BY {:02}:{:02}",
                    minute / 60 % 24,
                    minute % 60
                )
            }
            Self::TurnBackNow => write!(text, "TURN BACK NOW"),
        };
        text
    }
}

/// One band limit's debounce.
#[derive(Clone, Copy, Default)]
struct Limit {
    /// Since when the reading has been outside (or, once fired, back inside).
    /// Times are the clock's low 32 bits, compared wrapping, to keep the
    /// runtime small on the main stack.
    since: Option<u32>,
    /// When it last fired; `None` while armed.
    fired: Option<u32>,
}

impl Limit {
    /// `outside` is whether the reading breaks this limit now; `None` when
    /// there is no reading, which holds the state as it is.
    fn update(&mut self, now: u64, outside: Option<bool>) -> bool {
        let now = now as u32;
        let Some(outside) = outside else {
            self.since = None;
            return false;
        };
        match self.fired {
            None if outside => {
                let since = *self.since.get_or_insert(now);
                u64::from(now.wrapping_sub(since)) >= HOLD_MS
            }
            None => {
                self.since = None;
                false
            }
            Some(fired) if outside => {
                self.since = None;
                u64::from(now.wrapping_sub(fired)) >= REPEAT_MS
            }
            Some(_) => {
                let since = *self.since.get_or_insert(now);
                if u64::from(now.wrapping_sub(since)) >= HOLD_MS {
                    *self = Self::default();
                }
                false
            }
        }
    }

    fn fire(&mut self, now: u64) {
        self.fired = Some(now as u32);
        self.since = None;
    }
}

#[derive(Default)]
pub struct Alerts {
    limits: [Limit; 5],
    /// The ride time of the last eat and drink reminder (0 at the start).
    fuel_at_ms: u64,
    /// The ride time seen last tick; less means a new ride.
    active_ms: u64,
    last: Option<u64>,
    /// The most vehicles seen since the road was last clear, and since when
    /// the radar has been empty.
    peak: u8,
    empty_since: Option<u64>,
    sunset: SunsetWatch,
    turn_back: turn_back::Watch,
    /// The battery warning was given this ride.
    battery_told: bool,
    /// Since when the battery estimate has been under its threshold.
    battery_short_since: Option<u64>,
}

/// The last Unix time a clock is asked to read: the end of year 9999, so a
/// hostile time cannot overflow the calendar sums.
const LATEST_UNIX: i64 = 253_402_300_799;

/// The sunset warning is due this long before sunset, and still given until
/// `SUNSET_LATEST_S` before it: a ride resumed or a page returned to a few
/// minutes late still gets it, but never as "30 MIN" when it is 10.
pub const SUNSET_WARN_S: i64 = 30 * 60;
pub const SUNSET_LATEST_S: i64 = 20 * 60;

/// A ride already this old when its watch began was recovered after a
/// restart, so the warning it may have given before is not given again.
const RECOVERED_AFTER_MS: u64 = 120_000;

/// What this ride has seen of the sun and said about it.
#[derive(Clone, Copy, Default)]
struct SunsetWatch {
    warned: bool,
    /// The sun was up at some point of this ride, so its setting is news.
    saw_up: bool,
    lit: bool,
    began: bool,
}

impl SunsetWatch {
    fn observe(&mut self, sun: Sun, active_ms: u64) {
        if !self.began {
            self.began = true;
            self.warned = active_ms > RECOVERED_AFTER_MS;
        }
        self.saw_up |= matches!(sun, Sun::Up { .. });
    }

    fn due(&self, sun: Sun, now_unix: i64) -> Option<Alert> {
        match sun {
            Sun::Up { sets_at } => {
                let left = sets_at - now_unix;
                (!self.warned && (SUNSET_LATEST_S..=SUNSET_WARN_S).contains(&left))
                    .then_some(Alert::SunsetSoon)
            }
            // A ride that starts after sunset says nothing: the rider can
            // see it is dark. It is news only when the sun sets mid-ride.
            Sun::Down => (self.saw_up && !self.lit).then_some(Alert::SunsetNow),
            Sun::Midnight => None,
        }
    }

    fn given(&mut self, alert: Alert) {
        match alert {
            Alert::SunsetSoon => self.warned = true,
            _ => {
                self.lit = true;
                self.warned = true;
            }
        }
    }
}

impl Alerts {
    /// Decide this tick's alert. `active_ms` is the ride's recorded time.
    pub fn update(
        &mut self,
        now: u64,
        settings: &Settings,
        status: Status,
        active_ms: u64,
        sample: &Sample,
        radar: Snapshot,
    ) -> Option<Alert> {
        // A new ride: reminders count from its start, bands re-arm.
        if active_ms < self.active_ms {
            *self = Self::default();
        }
        self.active_ms = active_ms;
        let radar = self.radar(now, settings, radar);
        let recording = status == Status::Recording;
        let below = |value: Option<u16>, limit: u16| {
            (recording && limit != 0)
                .then_some(value)
                .flatten()
                .map(|value| value < limit)
        };
        let above = |value: Option<u16>, limit: u16| {
            (recording && limit != 0)
                .then_some(value)
                .flatten()
                .map(|value| value > limit)
        };
        let heart = sample.heart_bpm;
        let power = sample.power_watts;
        // Coasting is not low cadence: only pedalling counts.
        let cadence = sample
            .cadence_tenths
            .filter(|&tenths| tenths != 0)
            .map(|tenths| tenths / 10);
        let checks = [
            (
                above(heart, u16::from(settings.heart_high_bpm)),
                heart.map(Alert::HeartHigh),
            ),
            (
                below(heart, u16::from(settings.heart_low_bpm)),
                heart.map(Alert::HeartLow),
            ),
            (
                above(power, settings.power_high_w),
                power.map(Alert::PowerHigh),
            ),
            (
                below(power, settings.power_low_w),
                power.map(Alert::PowerLow),
            ),
            (
                below(cadence, u16::from(settings.cadence_low_rpm)),
                cadence.map(Alert::CadenceLow),
            ),
        ];
        let mut due = None;
        for (index, (outside, alert)) in checks.into_iter().enumerate() {
            if self.limits[index].update(now, outside) && due.is_none() {
                due = alert.map(|alert| (Some(index), alert));
            }
        }
        let every_ms = u64::from(settings.fuel_every_min) * 60_000;
        if due.is_none()
            && recording
            && every_ms != 0
            && active_ms >= self.fuel_at_ms.saturating_add(every_ms)
        {
            due = Some((None, Alert::Fuel));
        }
        // Radar news is about now: it never waits for the spacing.
        if let Some(alert) = radar {
            self.last = Some(now);
            return Some(alert);
        }
        let (index, alert) = due?;
        if self
            .last
            .is_some_and(|last| now.saturating_sub(last) < SPACING_MS)
        {
            return None;
        }
        self.last = Some(now);
        match index {
            Some(index) => self.limits[index].fire(now),
            None => self.fuel_at_ms = active_ms,
        }
        Some(alert)
    }

    /// The sunset alert of this tick, once per ride each, from the sun as
    /// last worked out (`None` without a position and time) and the Unix
    /// time now. It needs a recording ride, the rider's setting on, and
    /// `shown`: the ride page is up, so the toast can be seen; otherwise it
    /// waits for the rider's return to that page. Call after `update` on the
    /// same tick: an alert that tick, or one within `SPACING_MS`, holds it
    /// back unspent.
    pub fn sunset(
        &mut self,
        now: u64,
        enabled: bool,
        status: Status,
        shown: bool,
        sun: Option<(Sun, i64)>,
    ) -> Option<Alert> {
        let (sun, now_unix) = sun?;
        if !enabled || !matches!(status, Status::Recording | Status::Paused) {
            return None;
        }
        self.sunset.observe(sun, self.active_ms);
        let spaced = self
            .last
            .is_none_or(|last| now.saturating_sub(last) >= SPACING_MS);
        if status != Status::Recording || !shown || !spaced {
            return None;
        }
        let alert = self.sunset.due(sun, now_unix)?;
        self.sunset.given(alert);
        self.last = Some(now);
        Some(alert)
    }

    /// TURN BACK BY, once per ride (`ride::turn_back` says when it re-arms):
    /// `turn` is the latest turn from the sun and the way home, `None` when it
    /// cannot be said (no fix, no sunset ahead, riding home, a recovered ride
    /// still reading its track). Local time follows `zone`. Needs a recording
    /// ride, the SUNSET setting on and `shown`, as `sunset` does, and goes
    /// after it on the same tick.
    pub fn turn_back(
        &mut self,
        now: u64,
        enabled: bool,
        status: Status,
        shown: bool,
        turn: Option<Turn>,
        zone: Zone,
    ) -> Option<Alert> {
        let turn = turn.filter(|_| enabled && status == Status::Recording);
        let spaced = self
            .last
            .is_none_or(|last| now.saturating_sub(last) >= SPACING_MS);
        if !self.turn_back.due(now, turn, !shown || !spaced) {
            return None;
        }
        let turn = turn?;
        self.last = Some(now);
        Some(if turn.left_s <= 0 {
            Alert::TurnBackNow
        } else {
            let local = zone.local(turn.by.clamp(0, LATEST_UNIX) as u64).of_day();
            Alert::TurnBackBy((local / 60) as u16)
        })
    }

    /// Once per ride, warn that the battery may not last: its estimated
    /// minutes left stay under a quarter more than the workout still planned
    /// (an hour at least) for `BATTERY_HOLD_MS`. Call after `update`, which
    /// starts a new ride's count; it waits its turn behind other alerts.
    /// `held` keeps the warning back (a battery toast is on) without
    /// restarting the wait.
    pub fn battery(
        &mut self,
        now: u64,
        status: Status,
        left_min: Option<u16>,
        ride_left_min: Option<u32>,
        held: bool,
    ) -> Option<Alert> {
        let needed = ride_left_min.unwrap_or(0).max(BATTERY_HOUR_MIN) * 5 / 4;
        let short = left_min.filter(|&left| u32::from(left) < needed);
        let (Some(left), Status::Recording, false) = (short, status, self.battery_told) else {
            self.battery_short_since = None;
            return None;
        };
        let since = *self.battery_short_since.get_or_insert(now);
        if held
            || now.saturating_sub(since) < BATTERY_HOLD_MS
            || self
                .last
                .is_some_and(|last| now.saturating_sub(last) < SPACING_MS)
        {
            return None;
        }
        self.battery_told = true;
        self.last = Some(now);
        Some(Alert::BatteryLeft(left))
    }

    /// The rider left the ride pages: the wait starts over when they return,
    /// for a warning they could not see was never given.
    pub fn battery_away(&mut self) {
        self.battery_short_since = None;
    }

    /// More vehicles than at any time since the road was last clear, or
    /// the radar staying empty after vehicles.
    fn radar(&mut self, now: u64, settings: &Settings, radar: Snapshot) -> Option<Alert> {
        if !settings.radar || radar.status != radar::Status::Ready {
            // An unknown radar is not an empty road.
            self.peak = 0;
            self.empty_since = None;
            return None;
        }
        let count = radar.targets.iter().flatten().count() as u8;
        if count != 0 {
            self.empty_since = None;
            let rising = count > self.peak;
            self.peak = self.peak.max(count);
            return rising.then_some(Alert::Vehicles(count));
        }
        if self.peak == 0 {
            return None;
        }
        let since = *self.empty_since.get_or_insert(now);
        if now.saturating_sub(since) < CLEAR_AFTER_MS {
            return None;
        }
        self.peak = 0;
        self.empty_since = None;
        Some(Alert::RadarClear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::radar::{Side, Target, Threat};
    use std::vec::Vec;

    fn riding(heart: u16, watts: u16, rpm: u16) -> Sample {
        Sample {
            heart_bpm: Some(heart),
            power_watts: Some(watts),
            cadence_tenths: Some(rpm * 10),
            ..Sample::default()
        }
    }

    fn quiet() -> Snapshot {
        Snapshot {
            status: radar::Status::Ready,
            targets: [None; 8],
        }
    }

    fn vehicles(count: usize) -> Snapshot {
        let mut snapshot = quiet();
        for slot in snapshot.targets.iter_mut().take(count) {
            *slot = Some(Target {
                threat: Threat::Approach,
                side: Side::Behind,
                range_mm: 60_000,
                closing_speed_cm_s: 500,
            });
        }
        snapshot
    }

    /// Run a constant sample once a second, collecting (second, alert).
    fn ride(
        alerts: &mut Alerts,
        settings: &Settings,
        from_s: u64,
        seconds: u64,
        sample: Sample,
        status: Status,
    ) -> Vec<(u64, Alert)> {
        (from_s..from_s + seconds)
            .filter_map(|s| {
                let now = s * 1000;
                alerts
                    .update(now, settings, status, now, &sample, quiet())
                    .map(|alert| (s, alert))
            })
            .collect()
    }

    #[test]
    fn settings_round_trip_and_reject_nonsense() {
        let settings = Settings {
            heart_low_bpm: 110,
            heart_high_bpm: 172,
            power_low_w: 150,
            power_high_w: 320,
            cadence_low_rpm: 70,
            fuel_every_min: 45,
            radar: false,
        };
        assert_eq!(Settings::decode(&settings.encode()), Some(settings));
        assert_eq!(
            Settings::decode(&Settings::default().encode()),
            Some(Settings::default())
        );
        assert!(Settings::default().radar);
        let bad = |change: fn(&mut [u8; SETTINGS_BYTES])| {
            let mut bytes = settings.encode();
            change(&mut bytes);
            Settings::decode(&bytes)
        };
        assert_eq!(bad(|b| b[0] = 180), None, "low above high");
        assert_eq!(bad(|b| b[1] = 251), None);
        assert_eq!(
            bad(|b| b[4..6].copy_from_slice(&2001u16.to_le_bytes())),
            None
        );
        assert_eq!(bad(|b| b[6] = 201), None);
        assert_eq!(bad(|b| b[7] = 25), None);
        assert_eq!(bad(|b| b[8] = 2), None);
        assert_eq!(Settings::decode(&settings.encode()[..8]), None);
        // Off limits never conflict with each other.
        assert!(bad(|b| b[0] = 0).is_some());
    }

    #[test]
    fn a_band_alert_needs_ten_seconds_outside_repeats_and_rearms() {
        let settings = Settings {
            heart_high_bpm: 170,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let high = riding(175, 200, 90);
        let fine = riding(160, 200, 90);
        // A 9 s surge says nothing.
        assert!(ride(&mut alerts, &settings, 0, 9, high, Status::Recording).is_empty());
        assert!(ride(&mut alerts, &settings, 9, 5, fine, Status::Recording).is_empty());
        let fired = ride(&mut alerts, &settings, 14, 300, high, Status::Recording);
        // At 10 s, then every two minutes while still high.
        assert_eq!(
            fired,
            [
                (24, Alert::HeartHigh(175)),
                (144, Alert::HeartHigh(175)),
                (264, Alert::HeartHigh(175))
            ]
        );
        // Back in the band for 10 s re-arms: the next surge alerts at 10 s.
        assert!(ride(&mut alerts, &settings, 314, 11, fine, Status::Recording).is_empty());
        let again = ride(&mut alerts, &settings, 325, 20, high, Status::Recording);
        assert_eq!(again, [(335, Alert::HeartHigh(175))]);
    }

    #[test]
    fn bands_stay_quiet_when_off_paused_or_without_a_reading() {
        let all = Settings {
            heart_low_bpm: 100,
            heart_high_bpm: 170,
            power_low_w: 150,
            power_high_w: 300,
            cadence_low_rpm: 70,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let wild = riding(190, 500, 40);
        assert!(ride(&mut alerts, &all, 0, 60, wild, Status::Paused).is_empty());
        assert!(
            ride(
                &mut alerts,
                &all,
                60,
                60,
                Sample::default(),
                Status::Recording
            )
            .is_empty()
        );
        assert!(
            ride(
                &mut alerts,
                &Settings::default(),
                120,
                600,
                wild,
                Status::Recording
            )
            .is_empty()
        );
        // Coasting (cadence 0) is not low cadence.
        let coasting = riding(140, 0, 0);
        let only_cadence = Settings {
            cadence_low_rpm: 70,
            ..Settings::default()
        };
        assert!(
            ride(
                &mut alerts,
                &only_cadence,
                720,
                60,
                coasting,
                Status::Recording
            )
            .is_empty()
        );
    }

    #[test]
    fn several_bands_out_at_once_alert_in_turn_spaced_apart() {
        let all = Settings {
            heart_high_bpm: 170,
            power_high_w: 300,
            cadence_low_rpm: 70,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let fired = ride(
            &mut alerts,
            &all,
            0,
            30,
            riding(180, 400, 60),
            Status::Recording,
        );
        assert_eq!(
            fired,
            [
                (10, Alert::HeartHigh(180)),
                (13, Alert::PowerHigh(400)),
                (16, Alert::CadenceLow(60))
            ]
        );
        assert_eq!(Alert::PowerHigh(400).text().as_str(), "POWER HIGH 400W");
        assert_eq!(Alert::CadenceLow(60).cue(), Some(Cue::Band));
    }

    #[test]
    fn eat_and_drink_every_interval_of_ride_time() {
        let settings = Settings {
            fuel_every_min: 30,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let fired = ride(
            &mut alerts,
            &settings,
            0,
            100 * 60,
            riding(140, 200, 90),
            Status::Recording,
        );
        assert_eq!(
            fired,
            [
                (1800, Alert::Fuel),
                (3600, Alert::Fuel),
                (5400, Alert::Fuel)
            ]
        );
        assert_eq!(Alert::Fuel.text().as_str(), "EAT AND DRINK");
        // A new ride counts from its own start.
        let next = ride(
            &mut alerts,
            &settings,
            0,
            1801,
            riding(140, 200, 90),
            Status::Recording,
        );
        assert_eq!(next, [(1800, Alert::Fuel)]);
    }

    #[test]
    fn radar_counts_new_vehicles_and_calls_all_clear_once_they_pass() {
        let settings = Settings::default();
        let mut alerts = Alerts::default();
        let sample = Sample::default();
        let mut feed = |at: u64, radar: Snapshot| {
            alerts.update(at, &settings, Status::Recording, at, &sample, radar)
        };
        assert_eq!(feed(0, quiet()), None, "an empty road is not news");
        assert_eq!(feed(1000, vehicles(1)), Some(Alert::Vehicles(1)));
        assert_eq!(feed(1500, vehicles(3)), Some(Alert::Vehicles(3)));
        // Slots flicker: fewer, then as many again, is not a new vehicle.
        assert_eq!(feed(2000, vehicles(2)), None);
        assert_eq!(feed(2500, vehicles(3)), None);
        assert_eq!(feed(3000, quiet()), None);
        assert_eq!(feed(4000, vehicles(1)), None, "back within the clear delay");
        assert_eq!(feed(5000, quiet()), None);
        assert_eq!(feed(6900, quiet()), None);
        assert_eq!(feed(7000, quiet()), Some(Alert::RadarClear));
        assert_eq!(feed(9000, quiet()), None);
        assert_eq!(Alert::RadarClear.cue(), Some(Cue::RadarClear));
        assert_eq!(Alert::Vehicles(3).cue(), None);
        assert_eq!(Alert::Vehicles(1).text().as_str(), "1 VEHICLE");
        // A radar that drops out is not an all clear.
        assert_eq!(feed(10_000, vehicles(2)), Some(Alert::Vehicles(2)));
        assert_eq!(feed(11_000, Snapshot::default()), None);
        assert_eq!(feed(20_000, Snapshot::default()), None);
    }

    /// Feed the sunset watch once a minute: sunset at 1000 s of Unix time.
    fn sunset_ride(
        alerts: &mut Alerts,
        from_s: i64,
        to_s: i64,
        status: impl Fn(i64) -> Status,
        sets_at: i64,
    ) -> Vec<(i64, Alert)> {
        (from_s..to_s)
            .step_by(60)
            .filter_map(|t| {
                let sun = if t < sets_at {
                    Sun::Up { sets_at }
                } else {
                    Sun::Down
                };
                alerts
                    .sunset(t as u64 * 1000, true, status(t), true, Some((sun, t)))
                    .map(|alert| (t, alert))
            })
            .collect()
    }

    fn turn(left_s: i64) -> Option<Turn> {
        Some(Turn {
            by: 1_000_000 + left_s,
            left_s,
        })
    }

    #[test]
    fn turn_back_names_the_local_time_and_needs_its_conditions() {
        // 1_000_000 s is 13:46:40 UTC on day 11; Chile is UTC-3 then.
        let zone = Zone::utc(-3);
        let mut alerts = Alerts::default();
        let rec = Status::Recording;
        assert_eq!(alerts.turn_back(0, false, rec, true, turn(60), zone), None);
        assert_eq!(
            alerts.turn_back(1, true, Status::Paused, true, turn(60), zone),
            None
        );
        assert_eq!(alerts.turn_back(2, true, rec, false, turn(60), zone), None);
        assert_eq!(alerts.turn_back(3, true, rec, true, None, zone), None);
        let alert = alerts
            .turn_back(4, true, rec, true, turn(60), zone)
            .unwrap();
        assert_eq!(alert, Alert::TurnBackBy(10 * 60 + 47));
        assert_eq!(alert.text().as_str(), "TURN BACK BY 10:47");
        assert_eq!(alert.cue(), Some(Cue::Sunset));
        assert_eq!(
            alerts.turn_back(10_000, true, rec, true, turn(60), zone),
            None
        );
    }

    #[test]
    fn turn_back_reads_any_time_without_overflow() {
        for by in [i64::MIN, -1, i64::MAX] {
            let mut alerts = Alerts::default();
            let turn = Some(Turn { by, left_s: 60 });
            for zone in [Zone::Chile, Zone::utc(14)] {
                alerts.turn_back = turn_back::Watch::default();
                alerts.last = None;
                let rec = Status::Recording;
                assert!(alerts.turn_back(0, true, rec, true, turn, zone).is_some());
            }
        }
    }

    #[test]
    fn turn_back_says_now_once_the_turn_has_passed() {
        let mut alerts = Alerts::default();
        let alert = alerts
            .turn_back(0, true, Status::Recording, true, turn(-30), Zone::utc(0))
            .unwrap();
        assert_eq!(alert, Alert::TurnBackNow);
        assert_eq!(alert.text().as_str(), "TURN BACK NOW");
    }

    #[test]
    fn turn_back_keeps_its_distance_from_another_alert_and_a_new_ride_rearms_it() {
        let zone = Zone::utc(0);
        let rec = Status::Recording;
        let mut alerts = Alerts {
            last: Some(1_000),
            ..Alerts::default()
        };
        assert_eq!(
            alerts.turn_back(2_000, true, rec, true, turn(60), zone),
            None
        );
        assert!(
            alerts
                .turn_back(4_000, true, rec, true, turn(60), zone)
                .is_some()
        );
        let sample = Sample::default();
        alerts.update(
            10_000,
            &Settings::default(),
            rec,
            60_000,
            &sample,
            Snapshot::default(),
        );
        alerts.update(
            20_000,
            &Settings::default(),
            rec,
            0,
            &sample,
            Snapshot::default(),
        );
        assert!(
            alerts
                .turn_back(30_000, true, rec, true, turn(60), zone)
                .is_some()
        );
    }

    #[test]
    fn sunset_warns_once_half_an_hour_ahead_and_again_at_sunset() {
        let sets = 100_000;
        let mut alerts = Alerts::default();
        let fired = sunset_ride(
            &mut alerts,
            sets - 3 * 3600,
            sets + 3600,
            |_| Status::Recording,
            sets,
        );
        assert_eq!(
            fired,
            [(sets - 1800, Alert::SunsetSoon), (sets, Alert::SunsetNow)]
        );
        assert_eq!(Alert::SunsetSoon.text().as_str(), "SUNSET IN 30 MIN");
        assert_eq!(Alert::SunsetNow.cue(), Some(Cue::Sunset));
        // Crossing midnight UTC changes nothing: only the time left counts.
        let midnight = 86_400 * 20_000;
        let mut alerts = Alerts::default();
        let over = sunset_ride(
            &mut alerts,
            midnight - 3600,
            midnight + 3600,
            |_| Status::Recording,
            midnight + 1800,
        );
        assert_eq!(
            over,
            [
                (midnight, Alert::SunsetSoon),
                (midnight + 1800, Alert::SunsetNow)
            ]
        );
        // Already inside 15 minutes at the start: no 30 MIN, just LIGHTS ON.
        let mut alerts = Alerts::default();
        let soon = sunset_ride(
            &mut alerts,
            midnight,
            midnight + 3600,
            |_| Status::Recording,
            midnight + 900,
        );
        assert_eq!(soon, [(midnight + 900, Alert::SunsetNow)]);
    }

    #[test]
    fn sunset_waits_out_a_pause_and_a_late_start_stays_quiet() {
        let sets = 100_000;
        // Paused across the 30-minute mark, resumed at 25 min left: warned then.
        let mut alerts = Alerts::default();
        let paused = |t: i64| {
            if (sets - 2400..sets - 1500).contains(&t) {
                Status::Paused
            } else {
                Status::Recording
            }
        };
        let fired = sunset_ride(&mut alerts, sets - 3600, sets + 600, paused, sets);
        assert_eq!(fired[0].1, Alert::SunsetSoon);
        assert!(
            fired[0].0 >= sets - 1500 && fired[0].0 < sets - 1400,
            "{fired:?}"
        );
        assert_eq!(fired.len(), 2);
        // Resumed after the warning window: no 30 MIN, only LIGHTS ON.
        let mut alerts = Alerts::default();
        let late = |t: i64| {
            if (sets - 3000..sets - 600).contains(&t) {
                Status::Paused
            } else {
                Status::Recording
            }
        };
        let fired = sunset_ride(&mut alerts, sets - 3600, sets + 600, late, sets);
        assert_eq!(fired, [(sets, Alert::SunsetNow)]);
        // Starting after sunset: nothing at all, then a new ride warns afresh.
        let mut alerts = Alerts::default();
        assert!(
            sunset_ride(
                &mut alerts,
                sets + 60,
                sets + 7200,
                |_| Status::Recording,
                sets
            )
            .is_empty()
        );
        let sample = Sample::default();
        let _ = alerts.update(
            0,
            &Settings::default(),
            Status::Recording,
            5_000,
            &sample,
            quiet(),
        );
        let _ = alerts.update(
            0,
            &Settings::default(),
            Status::Recording,
            0,
            &sample,
            quiet(),
        );
        let sets = 500_000;
        let fired = sunset_ride(
            &mut alerts,
            sets - 3600,
            sets + 60,
            |_| Status::Recording,
            sets,
        );
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn a_band_or_radar_alert_on_the_same_tick_leaves_the_sunset_alert_unspent() {
        let settings = Settings::default();
        let sample = Sample::default();
        let sun = Some((Sun::Up { sets_at: 1_700 }, 0));
        let mut alerts = Alerts::default();
        let radar = alerts.update(
            10_000,
            &settings,
            Status::Recording,
            10_000,
            &sample,
            vehicles(1),
        );
        assert_eq!(radar, Some(Alert::Vehicles(1)));
        assert_eq!(
            alerts.sunset(10_000, true, Status::Recording, true, sun),
            None
        );
        assert_eq!(
            alerts.sunset(12_000, true, Status::Recording, true, sun),
            None,
            "spacing"
        );
        assert_eq!(
            alerts.sunset(13_000, true, Status::Recording, true, sun),
            Some(Alert::SunsetSoon)
        );
        assert_eq!(
            alerts.sunset(20_000, true, Status::Recording, true, sun),
            None
        );
    }

    #[test]
    fn a_recovered_ride_does_not_repeat_the_warning_but_still_says_lights_on() {
        let settings = Settings::default();
        let sample = Sample::default();
        let mut alerts = Alerts::default();
        let ten_minutes = 600_000;
        let _ = alerts.update(
            0,
            &settings,
            Status::Recording,
            ten_minutes,
            &sample,
            quiet(),
        );
        let up = Some((Sun::Up { sets_at: 1_500 }, 0));
        assert_eq!(alerts.sunset(0, true, Status::Recording, true, up), None);
        let down = Some((Sun::Down, 1_501));
        assert_eq!(
            alerts.sunset(60_000, true, Status::Recording, true, down),
            Some(Alert::SunsetNow)
        );
    }

    #[test]
    fn sunset_needs_the_setting_a_known_sky_and_the_ride_page() {
        let sun = Some((Sun::Up { sets_at: 1_700 }, 0));
        let mut alerts = Alerts::default();
        assert_eq!(alerts.sunset(0, false, Status::Recording, true, sun), None);
        assert_eq!(alerts.sunset(0, true, Status::Recording, true, None), None);
        assert_eq!(alerts.sunset(0, true, Status::Recording, false, sun), None);
        assert_eq!(alerts.sunset(0, true, Status::Ready, true, sun), None);
        assert_eq!(
            alerts.sunset(0, true, Status::Recording, true, sun),
            Some(Alert::SunsetSoon),
            "still due once the rider is back on the ride page"
        );
        assert_eq!(alerts.sunset(0, true, Status::Recording, true, sun), None);
    }

    #[test]
    fn radar_cues_follow_their_setting() {
        let off = Settings {
            radar: false,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let sample = Sample::default();
        assert_eq!(
            alerts.update(0, &off, Status::Recording, 0, &sample, vehicles(2)),
            None
        );
    }

    #[test]
    fn a_battery_that_may_not_last_warns_once_per_ride_after_holding() {
        let mut alerts = Alerts::default();
        let rec = Status::Recording;
        let hold = BATTERY_HOLD_MS;
        // Enough for the ride (with a quarter to spare), or for over 75 min.
        assert_eq!(alerts.battery(0, rec, Some(150), Some(120), false), None);
        assert_eq!(alerts.battery(hold, rec, Some(150), Some(120), false), None);
        assert_eq!(alerts.battery(hold, rec, Some(76), None, false), None);
        // No estimate: quiet, never a guess.
        assert_eq!(alerts.battery(hold, rec, None, Some(120), false), None);
        // Not recording: quiet.
        assert_eq!(
            alerts.battery(hold, Status::Paused, Some(30), None, false),
            None
        );
        // Short, but not for long enough yet.
        assert_eq!(alerts.battery(0, rec, Some(90), Some(120), false), None);
        assert_eq!(
            alerts.battery(hold - 1, rec, Some(90), Some(120), false),
            None
        );
        // A recovered estimate restarts the wait.
        assert_eq!(alerts.battery(hold, rec, Some(200), Some(120), false), None);
        assert_eq!(
            alerts.battery(hold + 1, rec, Some(90), Some(120), false),
            None
        );
        let at = 2 * hold + 1;
        assert_eq!(
            alerts.battery(at, rec, Some(90), Some(120), false),
            Some(Alert::BatteryLeft(90))
        );
        assert_eq!(
            alerts.battery(at + 1, rec, Some(30), None, false),
            None,
            "once"
        );
        // A new ride tells again.
        let sample = Sample::default();
        alerts.update(0, &Settings::default(), rec, 5_000, &sample, quiet());
        alerts.update(10_000, &Settings::default(), rec, 0, &sample, quiet());
        assert_eq!(alerts.battery(20_000, rec, Some(59), None, false), None);
        assert_eq!(
            alerts.battery(20_000 + hold, rec, Some(59), None, false),
            Some(Alert::BatteryLeft(59))
        );
    }

    #[test]
    fn a_held_battery_warning_keeps_its_wait_and_leaving_the_page_resets_it() {
        let mut alerts = Alerts::default();
        let rec = Status::Recording;
        let hold = BATTERY_HOLD_MS;
        assert_eq!(alerts.battery(0, rec, Some(40), None, false), None);
        // Held past the wait by a toast: due the moment it is released.
        assert_eq!(
            alerts.battery(hold + 1_000, rec, Some(40), None, true),
            None
        );
        assert_eq!(
            alerts.battery(hold + 2_000, rec, Some(40), None, false),
            Some(Alert::BatteryLeft(40))
        );
        let mut alerts = Alerts::default();
        assert_eq!(alerts.battery(0, rec, Some(40), None, false), None);
        alerts.battery_away();
        assert_eq!(alerts.battery(hold, rec, Some(40), None, false), None);
    }

    #[test]
    fn the_battery_warning_waits_its_turn_and_reads_in_words() {
        let mut alerts = Alerts {
            last: Some(BATTERY_HOLD_MS + 10_000),
            ..Alerts::default()
        };
        let rec = Status::Recording;
        assert_eq!(alerts.battery(0, rec, Some(40), None, false), None);
        let now = BATTERY_HOLD_MS + 11_000;
        assert_eq!(
            alerts.battery(now, rec, Some(40), None, false),
            None,
            "spacing"
        );
        let alert = alerts
            .battery(now + SPACING_MS, rec, Some(40), None, false)
            .unwrap();
        assert_eq!(alert.text().as_str(), "BATTERY 40 MIN LEFT");
        assert_eq!(Alert::BatteryLeft(60).text().as_str(), "BATTERY 1 H LEFT");
        assert_eq!(Alert::BatteryLeft(90).text().as_str(), "BATTERY 1 H LEFT");
        assert_eq!(alert.cue(), Some(Cue::Battery));
    }
}
