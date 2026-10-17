//! VANA's persisted cycling preferences: the training profile and the ANT
//! sensors the rider selected.
//!
//! The record lives in the application area of the shell settings envelope,
//! which the shell stores and carries through its own saves without reading
//! it. VANA owns the layout below. Later versions only append fields:
//!
//! | Byte | Field | Since |
//! | --- | --- | --- |
//! | 0 | Record version, 2 | 1 |
//! | 1..3 | FTP in watts, little-endian | 1 |
//! | 3..5 | LTHR in bpm, little-endian | 1 |
//! | 5 | Saved ANT sensor count, at most 10 | 2 |
//! | 6.. | Per sensor: device type, device number (little-endian), transmission type | 2 |
//!
//! Version 1 is exactly five bytes; it loads with no saved sensors and the next
//! save rewrites it as version 2. A version 2 record is `6 + 4 * count` bytes,
//! at most 46.
//!
//! An empty area means nothing was saved yet: the defaults apply and the
//! first change writes a record. A record VANA cannot read, from a newer
//! firmware or damaged, also leaves the defaults in force, but stays stored
//! untouched until the rider explicitly overwrites it (`PROFILE OVERWRITE`).
use super::Runtime;
use crate::ride::zones::TrainingProfile;
use crate::sensors::saved::{FIELD_CAPACITY, Saved};
use device_api::display::Display;
use device_api::input::InputSource;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::preferences::AppPreferences;
use firmware_shell::shell::Shell;

const VERSION: u8 = 2;
const PROFILE_LENGTH: usize = 5;
const RECORD_CAPACITY: usize = PROFILE_LENGTH + FIELD_CAPACITY;
/// Edits settle this long before a flash write, so a run of presses saves once.
const DEBOUNCE_MS: u64 = 2_000;
const RETRY_MS: u64 = 5_000;

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

pub fn encode(profile: TrainingProfile, sensors: &Saved) -> heapless::Vec<u8, RECORD_CAPACITY> {
    let [ftp_low, ftp_high] = profile.ftp_watts.to_le_bytes();
    let [lthr_low, lthr_high] = profile.lthr_bpm.to_le_bytes();
    let mut record = heapless::Vec::new();
    // Cannot fail: the capacity covers the profile and every sensor slot.
    let _ = record.extend_from_slice(&[VERSION, ftp_low, ftp_high, lthr_low, lthr_high]);
    let _ = record.extend_from_slice(&sensors.encode());
    record
}

pub fn decode(bytes: &[u8]) -> (TrainingProfile, Saved, Stored) {
    let defaults = (TrainingProfile::default(), Saved::default());
    let (version, profile, sensors) = match bytes {
        [] => return (defaults.0, defaults.1, Stored::Missing),
        [
            version @ (1 | VERSION),
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
        [0 | 1 | VERSION, ..] => return (defaults.0, defaults.1, Stored::Malformed),
        _ => return (defaults.0, defaults.1, Stored::Unsupported),
    };
    let sensors = match version {
        1 if sensors.is_empty() => Some(Saved::default()),
        VERSION => Saved::decode(sensors).ok(),
        _ => None,
    };
    match (profile, sensors) {
        (Some(profile), Some(sensors)) if version == 1 => (profile, sensors, Stored::Migrated),
        (Some(profile), Some(sensors)) => (profile, sensors, Stored::Current),
        _ => (defaults.0, defaults.1, Stored::Malformed),
    }
}

/// The profile in force and how it relates to the stored record.
pub(super) struct Preferences {
    pub(super) profile: TrainingProfile,
    /// Saved ANT sensors as last selected, in tile order.
    pub(super) sensors: Saved,
    pub(super) stored: Stored,
    locked: bool,
    changed_at: Option<u64>,
    retry_at: u64,
    failed: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            profile: TrainingProfile::default(),
            sensors: Saved::default(),
            stored: Stored::Missing,
            locked: false,
            changed_at: None,
            retry_at: 0,
            failed: false,
        }
    }
}

impl Preferences {
    fn load(bytes: &[u8]) -> Self {
        let (profile, sensors, stored) = decode(bytes);
        Self {
            profile,
            sensors,
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
        if self.locked {
            Persistence::Locked
        } else if self.failed {
            Persistence::Failed
        } else if self.changed_at.is_some() {
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
        }
    }

    /// Replace unreadable stored data with the profile in force.
    pub(super) fn overwrite(&mut self, now: u64) {
        self.locked = false;
        self.changed_at = Some(now.saturating_sub(DEBOUNCE_MS));
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
    }

    /// Write settled preference edits through the shell. Call once per loop;
    /// it does flash work only when an edit is due.
    pub fn save_preferences<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
    ) {
        let preferences = &mut self.preferences;
        if !preferences.due(now) {
            return;
        }
        // The record always fits the application area.
        let Some(app) = AppPreferences::new(&encode(preferences.profile, &preferences.sensors))
        else {
            return;
        };
        if system.save_app_preferences(app) {
            preferences.changed_at = None;
            preferences.failed = false;
            preferences.stored = Stored::Current;
        } else {
            preferences.failed = true;
            preferences.retry_at = now.saturating_add(RETRY_MS);
        }
        self.display.redraw();
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

    #[test]
    fn version_two_layout_is_pinned() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        let none = Saved::default();
        assert_eq!(encode(profile, &none).as_slice(), [2, 2, 1, 170, 0, 0]);
        assert_eq!(
            decode(&encode(profile, &none)),
            (profile, none, Stored::Current)
        );
        let mut sensors = Saved::default();
        sensors.select(sensor(120, 0x1234));
        sensors.select(device_api::ant::Identity {
            device_type: 40,
            device_number: 7,
            transmission_type: 5,
        });
        let record = encode(profile, &sensors);
        assert_eq!(
            record.as_slice(),
            [2, 2, 1, 170, 0, 2, 120, 0x34, 0x12, 1, 40, 7, 0, 5]
        );
        assert_eq!(decode(&record), (profile, sensors, Stored::Current));
        // Ten sensors fit the shell application area.
        let mut full = Saved::default();
        for kind in 1..=10 {
            full.select(sensor(kind, u16::from(kind)));
        }
        let record = encode(profile, &full);
        assert_eq!(record.len(), 46);
        assert!(AppPreferences::new(&record).is_some());
        assert_eq!(decode(&record), (profile, full, Stored::Current));
    }

    #[test]
    fn version_one_profile_migrates_with_no_saved_sensors() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        assert_eq!(
            decode(&[1, 2, 1, 170, 0]),
            (profile, Saved::default(), Stored::Migrated)
        );
    }

    #[test]
    fn missing_unknown_and_damaged_records_use_defaults() {
        let defaults = TrainingProfile::default();
        let none = Saved::default();
        assert_eq!(decode(&[]), (defaults, none, Stored::Missing));
        // A newer version, even one that appended fields.
        assert_eq!(
            decode(&[3, 250, 0, 170, 0, 0, 9]),
            (defaults, none, Stored::Unsupported)
        );
        assert_eq!(decode(&[0xff]), (defaults, none, Stored::Unsupported));
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
        ] {
            assert_eq!(
                decode(damaged),
                (defaults, none, Stored::Malformed),
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
        let mut locked = Preferences::load(&[3, 9]);
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
}
