//! VANA's persisted cycling preferences: the training profile.
//!
//! The record lives in the application area of the shell settings envelope,
//! which the shell stores and carries through its own saves without reading
//! it. VANA owns the layout below. Each version has a fixed length, and later
//! versions may only append fields:
//!
//! | Byte | Version 1 field |
//! | --- | --- |
//! | 0 | Record version, 1 |
//! | 1..3 | FTP in watts, little-endian |
//! | 3..5 | LTHR in bpm, little-endian |
//!
//! An empty area means no profile was saved yet: the defaults apply and the
//! first edit writes one. A record VANA cannot read, from a newer firmware or
//! damaged, also leaves the defaults in force, but stays stored untouched until
//! the rider explicitly overwrites it (`PROFILE OVERWRITE`).
use super::Runtime;
use crate::ride::zones::TrainingProfile;
use device_api::display::Display;
use device_api::input::InputSource;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::preferences::AppPreferences;
use firmware_shell::shell::Shell;

const VERSION: u8 = 1;
const V1_LENGTH: usize = 5;
/// Edits settle this long before a flash write, so a run of presses saves once.
const DEBOUNCE_MS: u64 = 2_000;
const RETRY_MS: u64 = 5_000;

/// What the stored record held when VANA read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stored {
    Missing,
    Current,
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

pub fn encode(profile: TrainingProfile) -> [u8; V1_LENGTH] {
    let [ftp_low, ftp_high] = profile.ftp_watts.to_le_bytes();
    let [lthr_low, lthr_high] = profile.lthr_bpm.to_le_bytes();
    [VERSION, ftp_low, ftp_high, lthr_low, lthr_high]
}

pub fn decode(bytes: &[u8]) -> (TrainingProfile, Stored) {
    let defaults = TrainingProfile::default();
    match bytes {
        [] => (defaults, Stored::Missing),
        [VERSION, ftp_low, ftp_high, lthr_low, lthr_high] => match TrainingProfile::new(
            u16::from_le_bytes([*ftp_low, *ftp_high]),
            u16::from_le_bytes([*lthr_low, *lthr_high]),
        ) {
            Some(profile) => (profile, Stored::Current),
            None => (defaults, Stored::Malformed),
        },
        [0 | VERSION, ..] => (defaults, Stored::Malformed),
        _ => (defaults, Stored::Unsupported),
    }
}

/// The profile in force and how it relates to the stored record.
pub(super) struct Preferences {
    pub(super) profile: TrainingProfile,
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
        let (profile, stored) = decode(bytes);
        Self {
            profile,
            stored,
            locked: matches!(stored, Stored::Unsupported | Stored::Malformed),
            ..Self::default()
        }
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
        let Some(app) = AppPreferences::new(&encode(preferences.profile)) else {
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

    #[test]
    fn version_one_layout_is_pinned() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        assert_eq!(encode(profile), [1, 2, 1, 170, 0]);
        assert_eq!(decode(&encode(profile)), (profile, Stored::Current));
    }

    #[test]
    fn missing_unknown_and_damaged_records_use_defaults() {
        let defaults = TrainingProfile::default();
        assert_eq!(decode(&[]), (defaults, Stored::Missing));
        // A newer version, even one that appended fields.
        assert_eq!(
            decode(&[2, 250, 0, 170, 0, 9]),
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
        ] {
            assert_eq!(
                decode(damaged),
                (defaults, Stored::Malformed),
                "{damaged:?}"
            );
        }
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
