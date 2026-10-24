//! VANA's persisted cycling preferences: the training profile, the ANT
//! sensors the rider selected and the ride, display and sound options.
//!
//! The record lives in the application area of the shell settings envelope,
//! which the shell stores and carries through its own saves without reading
//! it. VANA owns the layout below. Later versions only append fields:
//!
//! | Byte | Field | Since |
//! | --- | --- | --- |
//! | 0 | Record version, 5 | 1 |
//! | 1..3 | FTP in watts, little-endian | 1 |
//! | 3..5 | LTHR in bpm, little-endian | 1 |
//! | 5 | Saved ANT sensor count, at most 10 | 2 |
//! | 6.. | Per sensor: device type, device number (little-endian), transmission type | 2 |
//! | then | Flags: bit 0 auto-pause, bit 1 sounds, bit 2 sunset alert OFF (clear is on, so records saved before it keep the alert on); other bits zero | 3 |
//! | then | Auto-lap distance in km, 0 for off, at most 100 | 3 |
//! | then | Day and night: 0 automatic, 1 always day, 2 always night | 3 |
//! | then | First-run setup step to resume, 0 when setup is done | 3 |
//! | then | Ride alerts, 9 bytes owned by `ride::alerts` (see `ALERTS_DEFAULT`) | 3 |
//! | then | Theme: 0 dark, 1 light, 2 automatic | 4 |
//! | then | Time zone, 2 bytes owned by `clock::Zone`: 0 0 for Chile | 5 |
//!
//! Version 1 is exactly five bytes and version 2 is `6 + 4 * count` bytes.
//! Both load with the default options and setup done, since a rider already
//! used them, and the next save rewrites them as version 5. A version 3
//! record is `19 + 4 * count` bytes and loads with the dark theme; a version 4
//! record is `20 + 4 * count` bytes. Both load with the Chile time zone, the
//! zone the clock meant before it had a choice. A version 5 record is
//! `22 + 4 * count` bytes, at most 62.
//!
//! An empty area means nothing was saved yet: the defaults apply, first-run
//! setup is due, and the first change writes a record. A record VANA cannot
//! read, from a newer firmware or damaged, also leaves the defaults in force,
//! but stays stored untouched until the rider explicitly overwrites it
//! (`PROFILE OVERWRITE`).
use super::Runtime;
use crate::clock::Zone;
use crate::ride::zones::TrainingProfile;
use crate::sensors::saved::{FIELD_CAPACITY, Saved};
use device_api::display::Display;
use device_api::input::InputSource;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::preferences::AppPreferences;
use firmware_shell::shell::Shell;

const VERSION: u8 = 5;
const PROFILE_LENGTH: usize = 5;
const ALERTS_LENGTH: usize = 9;
const OPTIONS_LENGTH: usize = OPTIONS_LENGTH_V4 + 2;
/// Options as version 4 stored them, without the time zone.
const OPTIONS_LENGTH_V4: usize = OPTIONS_LENGTH_V3 + 1;
/// Options as version 3 stored them, without the theme.
const OPTIONS_LENGTH_V3: usize = 4 + ALERTS_LENGTH;
const RECORD_CAPACITY: usize = PROFILE_LENGTH + FIELD_CAPACITY + OPTIONS_LENGTH;
/// Edits settle this long before a flash write, so a run of presses saves once.
const DEBOUNCE_MS: u64 = 2_000;
const RETRY_MS: u64 = 5_000;
const AUTO_PAUSE: u8 = 1 << 0;
const SOUNDS: u8 = 1 << 1;
/// Set when the rider turned the sunset alert off; clear (as in every record
/// saved before it existed) means on.
const SUNSET_OFF: u8 = 1 << 2;
/// The longest automatic lap a record may hold.
pub const AUTO_LAP_MAX_KM: u8 = 100;

/// Ride alerts as stored: heart low and high (bpm), power low and high (W,
/// little-endian), cadence low (rpm), fuel reminder (minutes) and radar
/// (1 on), with 0 for off. The record carries them whole; the alerts
/// decode and check them. All off but the radar.
pub const ALERTS_DEFAULT: [u8; ALERTS_LENGTH] = [0, 0, 0, 0, 0, 0, 0, 0, 1];

/// When the screen follows the sun, or keeps one look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Daylight {
    #[default]
    Auto,
    Day,
    Night,
}

impl Daylight {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Auto),
            1 => Some(Self::Day),
            2 => Some(Self::Night),
            _ => None,
        }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Day => 1,
            Self::Night => 2,
        }
    }
}

/// Which look the screens have: the dark palette as painted, or the same
/// screens with luminance inverted (`screens::look`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    /// Light by day, dark at night.
    Auto,
}

impl Theme {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Dark),
            1 => Some(Self::Light),
            2 => Some(Self::Auto),
            _ => None,
        }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::Dark => 0,
            Self::Light => 1,
            Self::Auto => 2,
        }
    }

    /// Whether the light look is in force, given whether it is night.
    pub const fn is_light(self, night: bool) -> bool {
        match self {
            Self::Dark => false,
            Self::Light => true,
            Self::Auto => !night,
        }
    }
}

/// Ride, display and sound choices, and how far first-run setup got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Pause and resume the ride from wheel speed.
    pub auto_pause: bool,
    /// Ride cues and battery warnings sound; safety alerts always do.
    pub sounds: bool,
    /// Kilometres between automatic laps; 0 is off.
    pub auto_lap_km: u8,
    pub daylight: Daylight,
    pub theme: Theme,
    pub zone: Zone,
    /// The first-run setup step to resume, 0 when setup is done.
    pub setup: u8,
    /// Ride alert settings, carried for `ride::alerts`.
    pub alerts: [u8; ALERTS_LENGTH],
    /// Warn before sunset and at it, while riding.
    pub sunset_alert: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            auto_pause: true,
            sounds: true,
            auto_lap_km: 5,
            daylight: Daylight::Auto,
            theme: Theme::Dark,
            zone: Zone::Chile,
            setup: 0,
            alerts: ALERTS_DEFAULT,
            sunset_alert: true,
        }
    }
}

impl Options {
    /// A device that never saved preferences starts first-run setup here.
    pub const FIRST_RUN: u8 = 1;

    fn encode(self) -> [u8; OPTIONS_LENGTH] {
        let mut flags = 0;
        if self.auto_pause {
            flags |= AUTO_PAUSE;
        }
        if self.sounds {
            flags |= SOUNDS;
        }
        if !self.sunset_alert {
            flags |= SUNSET_OFF;
        }
        let mut bytes = [0; OPTIONS_LENGTH];
        bytes[..4].copy_from_slice(&[flags, self.auto_lap_km, self.daylight.byte(), self.setup]);
        bytes[4..OPTIONS_LENGTH_V3].copy_from_slice(&self.alerts);
        bytes[OPTIONS_LENGTH_V3] = self.theme.byte();
        bytes[OPTIONS_LENGTH_V4..].copy_from_slice(&self.zone.encode());
        bytes
    }

    /// Decode the options of record `version`.
    fn decode(bytes: &[u8], version: u8) -> Option<Self> {
        let (bytes, zone) = match version {
            3 | 4 => (bytes, Zone::Chile),
            5 => {
                let (bytes, zone) = bytes.split_last_chunk()?;
                (bytes, Zone::decode(*zone)?)
            }
            _ => return None,
        };
        let (bytes, theme) = match version {
            3 => (bytes, Theme::Dark),
            _ => {
                let (&theme, bytes) = bytes.split_last()?;
                (bytes, Theme::from_byte(theme)?)
            }
        };
        let (&[flags, auto_lap_km, daylight, setup], alerts) = bytes.split_first_chunk()?;
        let alerts = <[u8; ALERTS_LENGTH]>::try_from(alerts).ok()?;
        if flags & !(AUTO_PAUSE | SOUNDS | SUNSET_OFF) != 0 || auto_lap_km > AUTO_LAP_MAX_KM {
            return None;
        }
        Some(Self {
            auto_pause: flags & AUTO_PAUSE != 0,
            sounds: flags & SOUNDS != 0,
            auto_lap_km,
            daylight: Daylight::from_byte(daylight)?,
            theme,
            zone,
            setup,
            alerts,
            sunset_alert: flags & SUNSET_OFF == 0,
        })
    }
}

/// Everything the record holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub profile: TrainingProfile,
    pub sensors: Saved,
    pub options: Options,
}

/// What the stored record held when VANA read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stored {
    Missing,
    Current,
    /// An older version, rewritten as current on the next save.
    Migrated,
    /// A version this firmware does not know.
    Unsupported,
    /// A known version with an impossible length or value.
    Malformed,
}

impl Stored {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Current => "current",
            Self::Migrated => "migrated",
            Self::Unsupported => "unsupported",
            Self::Malformed => "malformed",
        }
    }
}

/// Whether the profile in force is the one stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persistence {
    Saved,
    Pending,
    Failed,
    /// Unreadable stored data is kept; edits apply until power-off only.
    Locked,
}

impl Persistence {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Locked => "locked",
        }
    }
}

pub fn encode(record: &Record) -> heapless::Vec<u8, RECORD_CAPACITY> {
    let [ftp_low, ftp_high] = record.profile.ftp_watts.to_le_bytes();
    let [lthr_low, lthr_high] = record.profile.lthr_bpm.to_le_bytes();
    let mut bytes = heapless::Vec::new();
    // Cannot fail: the capacity covers the profile, every sensor slot and
    // the options.
    let _ = bytes.extend_from_slice(&[VERSION, ftp_low, ftp_high, lthr_low, lthr_high]);
    let _ = bytes.extend_from_slice(&record.sensors.encode());
    let _ = bytes.extend_from_slice(&record.options.encode());
    bytes
}

pub fn decode(bytes: &[u8]) -> (Record, Stored) {
    let (version, profile, rest) = match bytes {
        [] => {
            let mut record = Record::default();
            record.options.setup = Options::FIRST_RUN;
            return (record, Stored::Missing);
        }
        [
            version @ 1..=VERSION,
            ftp_low,
            ftp_high,
            lthr_low,
            lthr_high,
            rest @ ..,
        ] => (
            *version,
            TrainingProfile::new(
                u16::from_le_bytes([*ftp_low, *ftp_high]),
                u16::from_le_bytes([*lthr_low, *lthr_high]),
            ),
            rest,
        ),
        [0..=VERSION, ..] => return (Record::default(), Stored::Malformed),
        _ => return (Record::default(), Stored::Unsupported),
    };
    let fields = match version {
        1 => rest
            .is_empty()
            .then(|| (Saved::default(), Options::default())),
        2 => Saved::decode(rest)
            .ok()
            .map(|sensors| (sensors, Options::default())),
        _ => {
            // The sensor field's own count says where the options start.
            let length = rest.first().map_or(0, |&count| 1 + 4 * usize::from(count));
            rest.split_at_checked(length)
                .and_then(|(sensors, options)| {
                    Some((
                        Saved::decode(sensors).ok()?,
                        Options::decode(options, version)?,
                    ))
                })
        }
    };
    match (profile, fields) {
        (Some(profile), Some((sensors, options))) => {
            let stored = if version == VERSION {
                Stored::Current
            } else {
                Stored::Migrated
            };
            let record = Record {
                profile,
                sensors,
                options,
            };
            (record, stored)
        }
        _ => (Record::default(), Stored::Malformed),
    }
}

/// The shell's backlight choices as Settings shows and edits them. The
/// shell owns and stores them; VANA only asks it to change them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayChoice {
    pub brightness: u8,
    /// Seconds without input before the backlight dims; 0 never dims.
    pub dim_secs: u16,
}

impl Default for DisplayChoice {
    fn default() -> Self {
        Self {
            brightness: firmware_shell::preferences::DEFAULT_BRIGHTNESS,
            dim_secs: firmware_shell::preferences::DEFAULT_DIM_TIMEOUT_SECS,
        }
    }
}

/// A backlight edit waiting for the shell to apply and store it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayEdit {
    changed_at: u64,
    applied: bool,
    /// The shell refused to store it; it retries.
    failed: bool,
}

/// The profile in force and how it relates to the stored record.
pub(super) struct Preferences {
    pub(super) profile: TrainingProfile,
    /// Saved ANT sensors as last selected, in tile order.
    pub(super) sensors: Saved,
    pub(super) options: Options,
    /// The shell's backlight choices, as last seen or edited.
    pub(super) display: DisplayChoice,
    display_edit: Option<DisplayEdit>,
    pub(super) stored: Stored,
    locked: bool,
    changed_at: Option<u64>,
    retry_at: u64,
    failed: bool,
    /// Counts edits this boot, so a warning about one edit is not taken for
    /// a later one.
    edit: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            profile: TrainingProfile::default(),
            sensors: Saved::default(),
            options: Options::default(),
            display: DisplayChoice::default(),
            display_edit: None,
            stored: Stored::Missing,
            locked: false,
            changed_at: None,
            retry_at: 0,
            failed: false,
            edit: 0,
        }
    }
}

impl Preferences {
    fn load(bytes: &[u8]) -> Self {
        let (record, stored) = decode(bytes);
        Self {
            profile: record.profile,
            sensors: record.sensors,
            options: record.options,
            stored,
            locked: matches!(stored, Stored::Unsupported | Stored::Malformed),
            ..Self::default()
        }
    }

    /// A save that the flash refused, as `save_preferences` records it.
    #[cfg(test)]
    pub(super) fn save_failed(&mut self, now: u64) {
        self.failed = true;
        self.retry_at = now.saturating_add(RETRY_MS);
    }

    pub(super) fn persistence(&self) -> Persistence {
        let display = self.display_edit;
        if self.locked {
            Persistence::Locked
        } else if self.failed || display.is_some_and(|edit| edit.failed) {
            Persistence::Failed
        } else if self.changed_at.is_some() || display.is_some() {
            Persistence::Pending
        } else {
            Persistence::Saved
        }
    }

    /// Use `profile` from now on and save it once edits settle.
    pub(super) fn set(&mut self, profile: TrainingProfile, now: u64) {
        if profile == self.profile && self.changed_at.is_none() {
            return;
        }
        self.profile = profile;
        if !self.locked {
            self.changed_at = Some(now);
            self.edit = self.edit.wrapping_add(1);
        }
    }

    /// Use these options from now on and save them once edits settle.
    pub(super) fn set_options(&mut self, options: Options, now: u64) {
        if options == self.options && self.changed_at.is_none() {
            return;
        }
        self.options = options;
        if !self.locked {
            self.changed_at = Some(now);
            self.edit = self.edit.wrapping_add(1);
        }
    }

    /// Save a pending edit without waiting for edits to settle.
    pub(super) fn settle_now(&mut self, now: u64) {
        if self.changed_at.is_some() {
            self.changed_at = Some(now.saturating_sub(DEBOUNCE_MS));
        }
    }

    /// The record as it would be saved now.
    pub(super) fn record(&self) -> Record {
        Record {
            profile: self.profile,
            sensors: self.sensors,
            options: self.options,
        }
    }

    /// Use these saved sensors from now on. A deliberate selection change is
    /// saved without waiting for edits to settle.
    pub(super) fn set_sensors(&mut self, sensors: Saved, now: u64) {
        if sensors == self.sensors {
            return;
        }
        self.sensors = sensors;
        if !self.locked {
            self.changed_at = Some(now.saturating_sub(DEBOUNCE_MS));
            self.edit = self.edit.wrapping_add(1);
        }
    }

    /// Replace unreadable stored data with the profile in force.
    pub(super) fn overwrite(&mut self, now: u64) {
        self.locked = false;
        self.changed_at = Some(now.saturating_sub(DEBOUNCE_MS));
        self.edit = self.edit.wrapping_add(1);
    }

    /// The edit waiting for its write, if any.
    pub(super) fn unsaved_edit(&self) -> Option<u32> {
        (self.pending() || self.display_edit.is_some()).then_some(self.edit)
    }

    /// An edit is waiting for its write.
    fn pending(&self) -> bool {
        self.changed_at.is_some() && !self.locked
    }

    fn due(&self, now: u64) -> bool {
        self.changed_at
            .is_some_and(|at| now >= at.saturating_add(DEBOUNCE_MS) && now >= self.retry_at)
    }
}

impl Runtime {
    /// Adopt the preferences the shell loaded at startup.
    pub fn load_preferences(&mut self, app: &AppPreferences) {
        self.preferences = Preferences::load(app.bytes());
        self.links.saved = self.preferences.sensors;
        self.apply_options();
    }

    /// Use these options from now on; they are saved once edits settle.
    pub(super) fn set_options(&mut self, options: Options, now: u64) {
        self.preferences.set_options(options, now);
        self.apply_options();
    }

    fn apply_options(&mut self) {
        let km = self.preferences.options.auto_lap_km;
        self.ride.auto_lap_mm = (km != 0).then(|| u64::from(km) * 1_000_000);
    }

    /// Ask the shell for these backlight choices: they apply at once and
    /// are stored once edits settle.
    pub(super) fn set_display(&mut self, display: DisplayChoice, now: u64) {
        let preferences = &mut self.preferences;
        preferences.display = display;
        preferences.display_edit = Some(DisplayEdit {
            changed_at: now,
            applied: false,
            failed: false,
        });
        preferences.edit = preferences.edit.wrapping_add(1);
    }

    /// Write settled preference edits through the shell. Call once per loop;
    /// it does flash work only when an edit is due.
    pub fn save_preferences<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
    ) {
        if self.preferences.due(now) {
            self.write_preferences(system, now);
        }
        self.save_display(system, now, false);
    }

    /// Apply a backlight edit, and store it once settled or when `flush`.
    /// Without an edit, follow the shell's own choices (the terminal can
    /// change them). Returns whether nothing is left unsaved.
    fn save_display<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        flush: bool,
    ) -> bool {
        let preferences = &mut self.preferences;
        let Some(edit) = &mut preferences.display_edit else {
            preferences.display = DisplayChoice {
                brightness: system.settings.brightness,
                dim_secs: system.settings.dim_timeout_secs,
            };
            return true;
        };
        if !edit.applied {
            edit.applied = true;
            let wanted = preferences.display;
            if let Some(settings) =
                system
                    .settings
                    .with_brightness(wanted.brightness)
                    .and_then(|settings| {
                        settings.with_idle_preferences(wanted.dim_secs, settings.dim_brightness)
                    })
            {
                system.settings = settings;
            }
        }
        if !flush && now < edit.changed_at.saturating_add(DEBOUNCE_MS) {
            return false;
        }
        let saved = system.save();
        if saved {
            preferences.display_edit = None;
        } else {
            edit.failed = true;
            edit.changed_at = now.saturating_add(RETRY_MS - DEBOUNCE_MS);
        }
        self.display.redraw();
        saved
    }

    /// Write a pending edit now, without waiting for it to settle, as before
    /// turning off. Returns whether nothing is left unsaved.
    pub(super) fn flush_preferences<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
    ) -> bool {
        let display = self.save_display(system, now, true);
        (!self.preferences.pending() || self.write_preferences(system, now)) && display
    }

    fn write_preferences<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
    ) -> bool {
        let preferences = &mut self.preferences;
        // The record always fits the application area.
        let Some(app) = AppPreferences::new(&encode(&preferences.record())) else {
            return false;
        };
        let saved = system.save_app_preferences(app);
        if saved {
            preferences.changed_at = None;
            preferences.failed = false;
            preferences.stored = Stored::Current;
        } else {
            preferences.failed = true;
            preferences.retry_at = now.saturating_add(RETRY_MS);
        }
        self.display.redraw();
        saved
    }

    /// The training profile zones use.
    pub fn training_profile(&self) -> TrainingProfile {
        self.preferences.profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(device_type: u8, device_number: u16) -> device_api::ant::Identity {
        device_api::ant::Identity {
            device_type,
            device_number,
            transmission_type: 1,
        }
    }

    fn record(profile: TrainingProfile, sensors: Saved, options: Options) -> Record {
        Record {
            profile,
            sensors,
            options,
        }
    }

    #[test]
    fn version_five_layout_is_pinned() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        let none = Saved::default();
        let defaults = record(profile, none, Options::default());
        assert_eq!(
            encode(&defaults).as_slice(),
            [
                5, 2, 1, 170, 0, 0, 0b11, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0
            ]
        );
        assert_eq!(decode(&encode(&defaults)), (defaults, Stored::Current));
        let mut sensors = Saved::default();
        sensors.select(sensor(120, 0x1234));
        sensors.select(device_api::ant::Identity {
            device_type: 40,
            device_number: 7,
            transmission_type: 5,
        });
        let options = Options {
            auto_pause: false,
            sounds: true,
            auto_lap_km: 10,
            daylight: Daylight::Night,
            theme: Theme::Auto,
            zone: Zone::utc(-3),
            setup: 3,
            alerts: [50, 180, 100, 0, 0x2c, 1, 70, 30, 0],
            sunset_alert: true,
        };
        let chosen = record(profile, sensors, options);
        let bytes = encode(&chosen);
        assert_eq!(
            bytes.as_slice(),
            [
                5, 2, 1, 170, 0, 2, 120, 0x34, 0x12, 1, 40, 7, 0, 5, 0b10, 10, 2, 3, 50, 180, 100,
                0, 0x2c, 1, 70, 30, 0, 2, 1, 0xf4
            ]
        );
        assert_eq!(decode(&bytes), (chosen, Stored::Current));
        // Ten sensors fit the shell application area.
        let mut full = Saved::default();
        for kind in 1..=10 {
            full.select(sensor(kind, u16::from(kind)));
        }
        let full = record(profile, full, options);
        let bytes = encode(&full);
        assert_eq!(bytes.len(), 62);
        assert!(AppPreferences::new(&bytes).is_some());
        assert_eq!(decode(&bytes), (full, Stored::Current));
        // A later version may place its fields differently: never guess.
        assert_eq!(Options::decode(&options.encode(), VERSION + 1), None);
    }

    #[test]
    fn version_three_migrates_with_the_dark_theme() {
        let bytes = [
            3, 2, 1, 170, 0, 0, 0b10, 10, 2, 3, 50, 180, 100, 0, 0x2c, 1, 70, 30, 0,
        ];
        let (migrated, stored) = decode(&bytes);
        assert_eq!(stored, Stored::Migrated);
        assert_eq!(migrated.options.theme, Theme::Dark);
        assert_eq!(migrated.options.daylight, Daylight::Night);
        assert_eq!(
            migrated.options.alerts,
            [50, 180, 100, 0, 0x2c, 1, 70, 30, 0]
        );
        assert_eq!(migrated.options.zone, Zone::Chile);
        // The next save writes version 5 and keeps the rest.
        assert_eq!(decode(&encode(&migrated)), (migrated, Stored::Current));
    }

    /// A version 4 record predates the time zone choice: its clock meant
    /// Santiago, so it loads with the Chile zone.
    #[test]
    fn version_four_migrates_with_the_chile_time_zone() {
        let bytes = [
            4, 2, 1, 170, 0, 0, 0b10, 10, 2, 3, 50, 180, 100, 0, 0x2c, 1, 70, 30, 0, 1,
        ];
        let (migrated, stored) = decode(&bytes);
        assert_eq!(stored, Stored::Migrated);
        assert_eq!(migrated.options.zone, Zone::Chile);
        assert_eq!(migrated.options.theme, Theme::Light);
        assert_eq!(migrated.options.daylight, Daylight::Night);
        assert_eq!(decode(&encode(&migrated)), (migrated, Stored::Current));
    }

    #[test]
    fn the_light_look_follows_the_theme_and_night() {
        for night in [false, true] {
            assert!(!Theme::Dark.is_light(night));
            assert!(Theme::Light.is_light(night));
        }
        assert!(Theme::Auto.is_light(false));
        assert!(!Theme::Auto.is_light(true));
    }

    /// Older records keep every field they had, take the default options and
    /// count as set up: their rider already used VANA.
    #[test]
    fn versions_one_and_two_migrate_with_default_options() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        assert_eq!(
            decode(&[1, 2, 1, 170, 0]),
            (
                record(profile, Saved::default(), Options::default()),
                Stored::Migrated
            )
        );
        let mut sensors = Saved::default();
        sensors.select(sensor(120, 0x1234));
        assert_eq!(
            decode(&[2, 2, 1, 170, 0, 1, 120, 0x34, 0x12, 1]),
            (
                record(profile, sensors, Options::default()),
                Stored::Migrated
            )
        );
        assert_eq!(Options::default().setup, 0);
        // The next save writes version 5 with the same profile and sensors.
        let migrated = decode(&[2, 2, 1, 170, 0, 1, 120, 0x34, 0x12, 1]).0;
        assert_eq!(decode(&encode(&migrated)), (migrated, Stored::Current));
    }

    #[test]
    fn missing_unknown_and_damaged_records_use_defaults() {
        let defaults = Record::default();
        let mut first_run = Record::default();
        first_run.options.setup = Options::FIRST_RUN;
        assert_eq!(decode(&[]), (first_run, Stored::Missing));
        // A newer version, even one that appended fields.
        assert_eq!(
            decode(&[6, 250, 0, 170, 0, 0, 3, 5, 0, 0, 9]),
            (defaults, Stored::Unsupported)
        );
        assert_eq!(decode(&[0xff]), (defaults, Stored::Unsupported));
        for damaged in [
            &[1][..],
            &[1, 250, 0, 170],
            &[1, 250, 0, 170, 0, 0],
            &[1, 0, 0, 170, 0],
            &[1, 250, 0, 0, 1],
            &[0, 250, 0, 170, 0],
            &[2, 250, 0, 170, 0],
            &[2, 250, 0, 170, 0, 1, 120, 1, 0],
            &[2, 250, 0, 170, 0, 1, 120, 0, 0, 1],
            &[2, 250, 0, 170, 0, 11],
            &[2, 0, 0, 170, 0, 0],
            // Version 2 bytes under versions 3 to 5: the options are missing.
            &[3, 250, 0, 170, 0, 0],
            &[3, 250, 0, 170, 0],
            &[4, 250, 0, 170, 0, 0],
            &[5, 250, 0, 170, 0, 0],
            // Version 4 options under version 5: the zone is missing.
            &[
                5, 250, 0, 170, 0, 0, 0b11, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0,
            ],
            // A sensor count that runs past the record.
            &[5, 250, 0, 170, 0, 2, 120, 1, 0, 1],
        ] {
            assert_eq!(
                decode(damaged),
                (defaults, Stored::Malformed),
                "{damaged:?}"
            );
        }
        // A record saved before the sunset alert has its flag clear: on. Off
        // is bit 2, and the other pinned bytes stay as they were.
        let off = Options {
            sunset_alert: false,
            ..Options::default()
        };
        assert_eq!(off.encode()[0], 0b111);
        assert_eq!(Options::decode(&off.encode(), VERSION), Some(off));
        assert!(
            Options::decode(&Options::default().encode(), VERSION)
                .unwrap()
                .sunset_alert
        );
        // Version 5 options: one byte short or long, an unknown flag, too
        // long a lap, an unknown day/night, theme or zone, and an impossible FTP.
        let good = Options::default().encode();
        let with = |at: usize, value: u8| {
            let mut options = good;
            options[at] = value;
            options.to_vec()
        };
        for (profile, options) in [
            ([250, 0], good[..OPTIONS_LENGTH - 1].to_vec()),
            ([250, 0], [&good[..], &[0]].concat()),
            ([250, 0], with(0, 8)),
            ([250, 0], with(1, AUTO_LAP_MAX_KM + 1)),
            ([250, 0], with(2, 3)),
            ([250, 0], with(OPTIONS_LENGTH_V3, 3)),
            ([250, 0], with(OPTIONS_LENGTH_V4, 2)),
            ([250, 0], with(OPTIONS_LENGTH - 1, 57)),
            ([0, 0], good.to_vec()),
        ] {
            let damaged = [&[5, profile[0], profile[1], 170, 0, 0][..], &options].concat();
            assert_eq!(
                decode(&damaged),
                (defaults, Stored::Malformed),
                "{damaged:?}"
            );
        }
    }

    #[test]
    fn sensor_changes_save_at_once_and_locked_records_keep_them_in_memory() {
        let mut sensors = Saved::default();
        sensors.select(sensor(120, 1));
        let mut preferences = Preferences::load(&[1, 250, 0, 170, 0]);
        assert_eq!(preferences.stored, Stored::Migrated);
        preferences.set_sensors(sensors, 10_000);
        assert!(preferences.due(10_000));
        let mut locked = Preferences::load(&[5, 9]);
        locked.set_sensors(sensors, 10_000);
        assert_eq!(locked.sensors, sensors);
        assert!(!locked.due(60_000));
    }

    #[test]
    fn edits_settle_before_saving_and_locked_records_wait_for_overwrite() {
        let mut preferences = Preferences::load(&[]);
        assert_eq!(preferences.persistence(), Persistence::Saved);
        let stronger = TrainingProfile::new(250, 185).unwrap();
        preferences.set(stronger, 1_000);
        assert_eq!(preferences.persistence(), Persistence::Pending);
        assert!(!preferences.due(2_999));
        assert!(preferences.due(3_000));

        let mut locked = Preferences::load(&[9, 9]);
        assert_eq!(locked.stored, Stored::Unsupported);
        assert_eq!(locked.persistence(), Persistence::Locked);
        locked.set(stronger, 1_000);
        assert_eq!(locked.profile, stronger, "edits still apply");
        assert!(!locked.due(60_000), "unreadable data is never replaced");
        locked.overwrite(60_000);
        assert!(locked.due(60_000));
    }

    #[test]
    fn option_edits_settle_like_profile_edits() {
        let mut preferences = Preferences::load(&[2, 250, 0, 170, 0, 0]);
        let quiet = Options {
            sounds: false,
            ..preferences.options
        };
        preferences.set_options(quiet, 1_000);
        assert_eq!(preferences.options, quiet);
        assert!(!preferences.due(2_999));
        assert!(preferences.due(3_000));
        assert_eq!(
            decode(&encode(&preferences.record())).0.options,
            quiet,
            "saved with the profile"
        );
    }
}
