//! Ride alerts: heart rate and power outside a band, low cadence, an eat and
//! drink reminder, a battery that may not last, and the radar road clear.
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
use crate::ride::log::{Sample, Status};
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
    /// ALL CLEAR once a radar shows the road empty. The radar's beeps are
    /// safety cues and do not depend on this.
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
    RadarClear,
}

impl Alert {
    /// The sound, if any.
    pub fn cue(self) -> Option<Cue> {
        match self {
            Self::Fuel => Some(Cue::Fuel),
            Self::RadarClear => Some(Cue::RadarClear),
            Self::BatteryLeft(_) => Some(Cue::Battery),
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
            Self::RadarClear => write!(text, "ALL CLEAR"),
        };
        text
    }
}

/// One band limit's debounce.
#[derive(Clone, Copy, Default)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
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
    /// A vehicle was seen since the road was last clear, and since when the
    /// radar has been empty.
    seen: bool,
    empty_since: Option<u64>,
    /// The battery warning was given this ride.
    battery_told: bool,
    /// Since when the battery estimate has been under its threshold.
    battery_short_since: Option<u64>,
}

state!(Alerts {
    limits,
    fuel_at_ms,
    active_ms,
    last,
    seen,
    empty_since,
    battery_told,
    battery_short_since,
});

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

    /// The radar staying empty after vehicles.
    fn radar(&mut self, now: u64, settings: &Settings, radar: Snapshot) -> Option<Alert> {
        if !settings.radar || radar.status != radar::Status::Ready {
            // An unknown radar is not an empty road.
            self.seen = false;
            self.empty_since = None;
            return None;
        }
        if radar.targets.iter().flatten().next().is_some() {
            self.seen = true;
            self.empty_since = None;
            return None;
        }
        if !self.seen {
            return None;
        }
        let since = *self.empty_since.get_or_insert(now);
        if now.saturating_sub(since) < CLEAR_AFTER_MS {
            return None;
        }
        self.seen = false;
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
    fn radar_calls_all_clear_once_the_road_stays_empty_after_vehicles() {
        let settings = Settings::default();
        let mut alerts = Alerts::default();
        let sample = Sample::default();
        let mut feed = |at: u64, radar: Snapshot| {
            alerts.update(at, &settings, Status::Recording, at, &sample, radar)
        };
        assert_eq!(feed(0, quiet()), None, "an empty road is not news");
        assert_eq!(feed(1000, vehicles(1)), None, "a vehicle has no toast");
        assert_eq!(feed(1500, vehicles(3)), None);
        assert_eq!(feed(3000, quiet()), None);
        assert_eq!(feed(4000, vehicles(1)), None, "back within the clear delay");
        assert_eq!(feed(5000, quiet()), None);
        assert_eq!(feed(6900, quiet()), None);
        assert_eq!(feed(7000, quiet()), Some(Alert::RadarClear));
        assert_eq!(feed(9000, quiet()), None);
        assert_eq!(Alert::RadarClear.cue(), Some(Cue::RadarClear));
        // A radar that drops out is not an all clear.
        assert_eq!(feed(10_000, vehicles(2)), None);
        assert_eq!(feed(11_000, Snapshot::default()), None);
        assert_eq!(feed(20_000, Snapshot::default()), None);
    }

    #[test]
    fn all_clear_follows_its_setting() {
        let off = Settings {
            radar: false,
            ..Settings::default()
        };
        let mut alerts = Alerts::default();
        let sample = Sample::default();
        for (at, snapshot) in [(0, vehicles(2)), (1_000, quiet()), (4_000, quiet())] {
            assert_eq!(
                alerts.update(at, &off, Status::Recording, at, &sample, snapshot),
                None
            );
        }
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
