use super::Runtime;
use super::mode::DisplayMode;
use crate::clock::Zone;
use crate::kept::RETRY_MS;
use crate::ride::laps::AutoLap;
use crate::ride::rider::{self, Rider};
use crate::ride::wheel_size::PLAUSIBLE_MM;
use crate::ride::zones::TrainingProfile;
use crate::screens::ride::Deck;
use crate::screens::ride::slots::{self, Slots};
use crate::sensors::saved::{FIELD_CAPACITY, Saved};
use device_api::display::Display;
use device_api::input::InputSource;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::preferences::{AppPreferences, Source};
use firmware_shell::shell::{BrightnessChoice, Shell};

const VERSION: u8 = 14;
/// The circumference assumed until one is set or learned.
pub const DEFAULT_WHEEL_MM: u16 = crate::ride::metrics::WHEEL_MM as u16;
const PROFILE_LENGTH: usize = 5;
const ALERTS_LENGTH: usize = 9;
const WHEEL_LENGTH: usize = 3;
const ZONE_LENGTH: usize = 2;
const OPTIONS_LENGTH: usize = 4
    + Deck::LENGTH
    + ALERTS_LENGTH
    + 1
    + ZONE_LENGTH
    + WHEEL_LENGTH
    + rider::LENGTH
    + slots::LENGTH
    + 2;
const RECORD_CAPACITY: usize = PROFILE_LENGTH + FIELD_CAPACITY + OPTIONS_LENGTH;
/// Edits settle this long before a flash write, so a run of presses saves once.
const DEBOUNCE_MS: u64 = 2_000;
const AUTO_PAUSE: u8 = 1 << 0;
const SOUNDS: u8 = 1 << 1;
const SUNSET: u8 = 1 << 2;
/// The longest automatic lap a record may hold.
pub const AUTO_LAP_MAX_KM: u8 = 100;
const AUTO_LAP_AT_START: u8 = 255;
const AUTO_LAP_FIRST_UNUSED: u8 = AUTO_LAP_MAX_KM + 1;

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

/// The wheel circumference speed and distance are measured with, and who
/// chose it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wheel {
    /// The size in mm; none until the rider or the calibration sets one.
    pub mm: Option<u16>,
    /// The rider chose the size: the calibration leaves it alone.
    pub manual: bool,
}

impl Wheel {
    /// The circumference in force: the one set, else the default.
    pub fn size_mm(self) -> u16 {
        self.mm.unwrap_or(DEFAULT_WHEEL_MM)
    }

    fn encode(self) -> [u8; WHEEL_LENGTH] {
        let [low, high] = self.mm.unwrap_or(0).to_le_bytes();
        [low, high, u8::from(self.manual)]
    }

    fn decode([low, high, manual]: [u8; WHEEL_LENGTH]) -> Option<Self> {
        let mm = match u16::from_le_bytes([low, high]) {
            0 => None,
            mm if PLAUSIBLE_MM.contains(&mm) => Some(mm),
            _ => return None,
        };
        let manual = match manual {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self { mm, manual })
    }
}

const fn auto_lap_byte(auto_lap: AutoLap) -> u8 {
    match auto_lap {
        AutoLap::Off => 0,
        AutoLap::Every(km) => km,
        AutoLap::AtStart => AUTO_LAP_AT_START,
    }
}

const fn flag_from_byte(byte: u8) -> Option<bool> {
    match byte {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

const fn auto_lap_from_byte(byte: u8) -> Option<AutoLap> {
    match byte {
        0 => Some(AutoLap::Off),
        1..=AUTO_LAP_MAX_KM => Some(AutoLap::Every(byte)),
        AUTO_LAP_AT_START => Some(AutoLap::AtStart),
        AUTO_LAP_FIRST_UNUSED..AUTO_LAP_AT_START => None,
    }
}

/// Ride, display and sound choices, and how far first-run setup got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Pause and resume the ride from wheel speed.
    pub auto_pause: bool,
    /// Ride cues and battery warnings sound; safety alerts always do.
    pub sounds: bool,
    pub auto_lap: AutoLap,
    pub daylight: Daylight,
    pub theme: Theme,
    pub zone: Zone,
    pub wheel: Wheel,
    /// Weight, age and sex, for heart-rate calories and W/kg.
    pub rider: Rider,
    /// The first-run setup step to resume, 0 when setup is done.
    pub setup: u8,
    /// Ride alert settings, carried for `ride::alerts`.
    pub alerts: [u8; ALERTS_LENGTH],
    /// Warn before sunset and at it, while riding.
    pub sunset_alert: bool,
    /// The optional ride pages the rider left on, in their order.
    pub deck: Deck,
    /// The readings on the main page's lower line.
    pub main_slots: Slots,
    pub mode: DisplayMode,
    pub auto_brightness: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            auto_pause: true,
            sounds: true,
            auto_lap: AutoLap::Every(5),
            daylight: Daylight::Auto,
            theme: Theme::Dark,
            zone: Zone::Chile,
            wheel: Wheel::default(),
            rider: Rider::default(),
            setup: 0,
            alerts: ALERTS_DEFAULT,
            sunset_alert: true,
            deck: Deck::default(),
            main_slots: Slots::default(),
            mode: DisplayMode::default(),
            auto_brightness: true,
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
        if self.sunset_alert {
            flags |= SUNSET;
        }
        let mut bytes = [0; OPTIONS_LENGTH];
        let mut at = 0;
        for field in [
            &[
                flags,
                auto_lap_byte(self.auto_lap),
                self.daylight.byte(),
                self.setup,
            ][..],
            &self.deck.encode(),
            &self.alerts,
            &[self.theme.byte()],
            &self.zone.encode(),
            &self.wheel.encode(),
            &self.rider.encode(),
            &self.main_slots.encode(),
            &[self.mode.byte(), u8::from(self.auto_brightness)],
        ] {
            bytes[at..at + field.len()].copy_from_slice(field);
            at += field.len();
        }
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; OPTIONS_LENGTH] = bytes.try_into().ok()?;
        let (&[flags, auto_lap, daylight, setup], rest) = bytes.split_first_chunk()?;
        if flags & !(AUTO_PAUSE | SOUNDS | SUNSET) != 0 {
            return None;
        }
        let (&deck, rest) = rest.split_first_chunk()?;
        let (&alerts, rest) = rest.split_first_chunk::<ALERTS_LENGTH>()?;
        let (&theme, rest) = rest.split_first()?;
        let (&zone, rest) = rest.split_first_chunk()?;
        let (&wheel, rest) = rest.split_first_chunk()?;
        let (&rider, rest) = rest.split_first_chunk()?;
        let (&main_slots, rest) = rest.split_first_chunk()?;
        let &[mode, auto_brightness] = rest else {
            return None;
        };
        Some(Self {
            auto_pause: flags & AUTO_PAUSE != 0,
            sounds: flags & SOUNDS != 0,
            auto_lap: auto_lap_from_byte(auto_lap)?,
            daylight: Daylight::from_byte(daylight)?,
            theme: Theme::from_byte(theme)?,
            zone: Zone::decode(zone)?,
            wheel: Wheel::decode(wheel)?,
            rider: Rider::decode(rider)?,
            setup,
            alerts,
            sunset_alert: flags & SUNSET != 0,
            deck: Deck::decode(deck)?,
            main_slots: Slots::decode(main_slots)?,
            mode: DisplayMode::from_byte(mode)?,
            auto_brightness: flag_from_byte(auto_brightness)?,
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
    /// Exactly what this firmware would write.
    Current,
    /// An older version, replaced by the defaults.
    Outdated,
    /// Anything else: a newer version, or damaged. Kept, never overwritten
    /// unasked.
    Foreign,
}

impl Stored {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Current => "current",
            Self::Outdated => "outdated",
            Self::Foreign => "foreign",
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
    if bytes.is_empty() {
        let mut record = Record::default();
        record.options.setup = Options::FIRST_RUN;
        return (record, Stored::Missing);
    }
    match decode_current(bytes) {
        Some(record) if encode(&record)[..] == *bytes => (record, Stored::Current),
        _ if bytes[0] < VERSION => (Record::default(), Stored::Outdated),
        _ => (Record::default(), Stored::Foreign),
    }
}

fn decode_current(bytes: &[u8]) -> Option<Record> {
    let [VERSION, ftp_low, ftp_high, lthr_low, lthr_high, rest @ ..] = bytes else {
        return None;
    };
    let profile = TrainingProfile::new(
        u16::from_le_bytes([*ftp_low, *ftp_high]),
        u16::from_le_bytes([*lthr_low, *lthr_high]),
    )?;
    let (sensors, options) = Saved::decode(rest).ok()?;
    Some(Record {
        profile,
        sensors,
        options: Options::decode(options)?,
    })
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
    changed_at: Option<u64>,
    retry_at: u64,
    failed: bool,
    /// Counts edits this boot, so a warning about one edit is not taken for
    /// a later one.
    edit: u32,
}

state!(Preferences {
    profile,
    sensors,
    options,
    display,
    display_edit,
    stored,
    changed_at,
    retry_at,
    failed,
    edit,
});

impl Default for Preferences {
    fn default() -> Self {
        Self {
            profile: TrainingProfile::default(),
            sensors: Saved::default(),
            options: Options::default(),
            display: DisplayChoice::default(),
            display_edit: None,
            stored: Stored::Missing,
            changed_at: None,
            retry_at: 0,
            failed: false,
            edit: 0,
        }
    }
}

impl Preferences {
    fn load(bytes: &[u8], settings: Source) -> Self {
        let (record, stored) = decode(bytes);
        let stored = if settings == Source::Foreign {
            Stored::Foreign
        } else {
            stored
        };
        Self {
            profile: record.profile,
            sensors: record.sensors,
            options: record.options,
            stored,
            changed_at: (stored == Stored::Outdated).then_some(0),
            ..Self::default()
        }
    }

    /// A save that the flash refused, as `save_preferences` records it.
    #[cfg(test)]
    pub(super) fn save_failed(&mut self, now: u64) {
        self.failed = true;
        self.retry_at = now.saturating_add(RETRY_MS);
    }

    /// Unreadable stored data is kept, so edits do not persist.
    pub(super) fn locked(&self) -> bool {
        self.stored == Stored::Foreign
    }

    pub(super) fn persistence(&self) -> Persistence {
        let display = self.display_edit;
        if self.locked() {
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
        if !self.locked() {
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
        if !self.locked() {
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
        if !self.locked() {
            self.changed_at = Some(now.saturating_sub(DEBOUNCE_MS));
            self.edit = self.edit.wrapping_add(1);
        }
    }

    /// The edit waiting for its write, if any.
    pub(super) fn unsaved_edit(&self) -> Option<u32> {
        (self.pending() || self.display_edit.is_some()).then_some(self.edit)
    }

    /// An edit is waiting for its write.
    fn pending(&self) -> bool {
        self.changed_at.is_some() && !self.locked()
    }

    fn due(&self, now: u64) -> bool {
        self.changed_at
            .is_some_and(|at| now >= at.saturating_add(DEBOUNCE_MS) && now >= self.retry_at)
    }
}

impl Runtime {
    /// Adopt the preferences the shell loaded at startup.
    pub fn load_preferences(&mut self, app: &AppPreferences, settings: Source) {
        self.preferences = Preferences::load(app.bytes(), settings);
        self.settle_wheel(0);
    }

    /// Use these options from now on; they are saved once edits settle.
    pub(super) fn set_options(&mut self, options: Options, now: u64) {
        self.preferences.set_options(options, now);
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
        self.adopt_brightness_choice(system, now);
        if self.preferences.due(now) {
            self.write_preferences(system, now);
        }
        self.save_display(system, now, false);
    }

    fn adopt_brightness_choice<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
    ) {
        if let Some(choice) = system.take_brightness_choice() {
            let mut options = self.preferences.options;
            options.auto_brightness = choice == BrightnessChoice::Auto;
            self.set_options(options, now);
        }
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
    use crate::compose::data::Metric as Reading;
    use crate::screens::ride::RidePage;

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
    fn the_layout_is_pinned() {
        let profile = TrainingProfile::new(0x0102, 170).unwrap();
        let none = Saved::default();
        let defaults = record(profile, none, Options::default());
        assert_eq!(
            encode(&defaults).as_slice(),
            [
                13, 2, 1, 170, 0, 0, 0b111, 5, 0, 0, 1, 2, 3, 5, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1
            ]
        );
        assert_eq!(decode(&encode(&defaults)), (defaults, Stored::Current));
        let mut sensors = Saved::default();
        sensors.add(sensor(120, 0x1234));
        sensors.add(device_api::ant::Identity {
            device_type: 40,
            device_number: 7,
            transmission_type: 5,
        });
        let options = Options {
            auto_pause: false,
            sounds: true,
            auto_lap: AutoLap::Every(10),
            daylight: Daylight::Night,
            theme: Theme::Auto,
            zone: Zone::utc(-3),
            wheel: Wheel {
                mm: Some(2105),
                manual: true,
            },
            rider: Rider::decode([72, 41, 2]).unwrap(),
            setup: 3,
            alerts: [50, 180, 100, 0, 0x2c, 1, 70, 30, 0],
            sunset_alert: true,
            deck: Deck::default().toggled(RidePage::Climb),
            main_slots: Slots {
                left: Reading::Kcal,
                right: Reading::NormalizedPower,
            },
            mode: DisplayMode::default().next(),
            auto_brightness: false,
        };
        let chosen = record(profile, sensors, options);
        let bytes = encode(&chosen);
        assert_eq!(
            bytes.as_slice(),
            [
                13, 2, 1, 170, 0, 2, 120, 0x34, 0x12, 1, 1, 40, 7, 0, 5, 0, 0b110, 10, 2, 3, 1, 2,
                5, 0xff, 50, 180, 100, 0, 0x2c, 1, 70, 30, 0, 2, 1, 0xf4, 0x39, 0x08, 1, 72, 41, 2,
                6, 2, 1, 0
            ]
        );
        assert_eq!(decode(&bytes), (chosen, Stored::Current));
        // Ten sensors fit the shell application area.
        let mut full = Saved::default();
        for kind in 1..=10 {
            full.add(sensor(kind, u16::from(kind)));
        }
        let full = record(profile, full, options);
        let bytes = encode(&full);
        assert_eq!(bytes.len(), 86);
        assert!(AppPreferences::new(&bytes).is_some());
        assert_eq!(decode(&bytes), (full, Stored::Current));
    }

    #[test]
    fn a_brightness_choice_round_trips_and_an_unknown_one_is_damage() {
        let mut manual = Record::default();
        manual.options.auto_brightness = false;
        let bytes = encode(&manual);
        assert_eq!(bytes[bytes.len() - 1], 0);
        assert_eq!(decode(&bytes), (manual, Stored::Current));
        let mut damaged = bytes.to_vec();
        let last = damaged.len() - 1;
        damaged[last] = 2;
        assert_eq!(decode(&damaged), (Record::default(), Stored::Foreign));
    }

    #[test]
    fn a_stored_mode_round_trips_and_an_unknown_row_is_damage() {
        let mut chosen = Record::default();
        chosen.options.mode = DisplayMode::default().next();
        let bytes = encode(&chosen);
        assert_eq!(bytes[bytes.len() - 1], 1);
        assert_eq!(decode(&bytes), (chosen, Stored::Current));
        for row in [2, 9, 255] {
            let mut damaged = bytes.to_vec();
            *damaged.last_mut().unwrap() = row;
            assert_eq!(
                decode(&damaged),
                (Record::default(), Stored::Foreign),
                "{row}"
            );
        }
    }

    /// The slots are stored as ids; an id no firmware wrote is damage, in
    /// either slot, and the record stays as it was.
    #[test]
    fn stored_main_page_readings_round_trip_and_an_unknown_id_is_damage() {
        let mut chosen = Record::default();
        chosen.options.main_slots = Slots {
            left: Reading::Kcal,
            right: Reading::NormalizedPower,
        };
        let bytes = encode(&chosen);
        assert_eq!(bytes[bytes.len() - 4..bytes.len() - 2], [6, 2]);
        assert_eq!(decode(&bytes), (chosen, Stored::Current));
        for tail in [[9, 1], [0, 9], [255, 1], [0, 255]] {
            let mut damaged = bytes.to_vec();
            let at = damaged.len() - 4;
            damaged[at..at + 2].copy_from_slice(&tail);
            assert_eq!(
                decode(&damaged),
                (Record::default(), Stored::Foreign),
                "{tail:?}"
            );
        }
    }

    /// The rider's body is stored whole and a damaged one is never trusted.
    #[test]
    fn a_stored_body_round_trips_and_a_damaged_one_is_refused() {
        let mut chosen = Record::default();
        chosen.options.rider = Rider::decode([72, 41, 2]).unwrap();
        let bytes = encode(&chosen);
        assert_eq!(bytes[bytes.len() - 7..bytes.len() - 4], [72, 41, 2]);
        assert_eq!(decode(&bytes), (chosen, Stored::Current));
        for tail in [
            [29, 41, 2],
            [201, 41, 2],
            [72, 9, 2],
            [72, 100, 2],
            [72, 41, 3],
        ] {
            let mut damaged = bytes.to_vec();
            let at = damaged.len() - 6;
            damaged[at..at + 3].copy_from_slice(&tail);
            assert_eq!(
                decode(&damaged),
                (Record::default(), Stored::Foreign),
                "{tail:?}"
            );
        }
    }

    #[test]
    fn a_learned_wheel_is_stored_and_a_damaged_one_is_not_trusted() {
        let mut learned = Record::default();
        learned.options.wheel = Wheel {
            mm: Some(2200),
            manual: false,
        };
        let bytes = encode(&learned);
        let wheel_at = bytes.len() - 10;
        assert_eq!(&bytes[wheel_at..wheel_at + 3], [0x98, 0x08, 0]);
        assert_eq!(decode(&bytes), (learned, Stored::Current));
        assert_eq!(learned.options.wheel.size_mm(), 2200);
        for tail in [
            [0xdb, 0x05, 0],
            [0x61, 0x09, 0],
            [1, 0, 0],
            [0, 0, 2],
            [0x98, 0x08, 255],
        ] {
            let mut damaged = bytes.to_vec();
            damaged[wheel_at..wheel_at + 3].copy_from_slice(&tail);
            assert_eq!(
                decode(&damaged),
                (Record::default(), Stored::Foreign),
                "{tail:?}"
            );
        }
        for mm in [1500u16, 2400] {
            let mut edge = bytes.to_vec();
            let at = wheel_at;
            edge[at..at + 2].copy_from_slice(&mm.to_le_bytes());
            assert_eq!(decode(&edge).0.options.wheel.mm, Some(mm));
        }
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

    #[test]
    fn the_auto_lap_choice_round_trips_and_an_unused_byte_is_damage() {
        for auto_lap in [
            AutoLap::Off,
            AutoLap::Every(1),
            AutoLap::Every(AUTO_LAP_MAX_KM),
            AutoLap::AtStart,
        ] {
            let options = Options {
                auto_lap,
                ..Options::default()
            };
            assert_eq!(Options::decode(&options.encode()), Some(options));
        }
        let at_start = Options {
            auto_lap: AutoLap::AtStart,
            ..Options::default()
        };
        assert_eq!(at_start.encode()[1], 255);
        for byte in AUTO_LAP_MAX_KM + 1..255 {
            assert_eq!(auto_lap_from_byte(byte), None, "{byte}");
        }
        let record = Record {
            options: at_start,
            ..Record::default()
        };
        assert_eq!(decode(&encode(&record)), (record, Stored::Current));
    }

    #[test]
    fn missing_unknown_and_damaged_records_use_defaults() {
        let defaults = Record::default();
        let mut first_run = Record::default();
        first_run.options.setup = Options::FIRST_RUN;
        assert_eq!(decode(&[]), (first_run, Stored::Missing));
        let current = encode(&defaults);
        for (version, stored) in [
            (0, Stored::Outdated),
            (VERSION - 1, Stored::Outdated),
            (VERSION + 1, Stored::Foreign),
            (0xff, Stored::Foreign),
        ] {
            let mut other = current.to_vec();
            other[0] = version;
            assert_eq!(decode(&other), (defaults, stored), "{version}");
        }
        assert_eq!(decode(&[0xff]), (defaults, Stored::Foreign));
        for damaged in [
            &[VERSION][..],
            &[VERSION, 250, 0, 170],
            &[VERSION, 250, 0, 170, 0],
            &[VERSION, 250, 0, 170, 0, 0],
            // A sensor count that runs past the record.
            &[VERSION, 250, 0, 170, 0, 2, 120, 1, 0, 1],
        ] {
            assert_eq!(decode(damaged), (defaults, Stored::Foreign), "{damaged:?}");
        }
        for older in [&[0, 250, 0, 170, 0][..], &[VERSION - 1, 250, 0, 170, 0, 0]] {
            assert_eq!(decode(older), (defaults, Stored::Outdated), "{older:?}");
        }
        let off = Options {
            sunset_alert: false,
            ..Options::default()
        };
        assert_eq!(off.encode()[0], AUTO_PAUSE | SOUNDS);
        assert_eq!(Options::decode(&off.encode()), Some(off));
        assert!(
            Options::decode(&Options::default().encode())
                .unwrap()
                .sunset_alert
        );
        assert_eq!(Options::default().encode()[0], AUTO_PAUSE | SOUNDS | SUNSET);
        let hidden = Options {
            deck: Deck::default().toggled(RidePage::Lap),
            ..Options::default()
        };
        assert_eq!(hidden.encode()[0], AUTO_PAUSE | SOUNDS | SUNSET);
        assert_eq!(hidden.encode()[4..4 + Deck::LENGTH], [2, 4, 0xff]);
        assert_eq!(Options::decode(&hidden.encode()), Some(hidden));
        const FLAGS: usize = 0;
        const AUTO_LAP: usize = 1;
        const DAYLIGHT: usize = 2;
        const DECK: usize = 4;
        const THEME: usize = DECK + Deck::LENGTH + ALERTS_LENGTH;
        const ZONE: usize = THEME + 1;
        const WHEEL: usize = ZONE + ZONE_LENGTH;
        const WHEEL_MANUAL: usize = WHEEL + 2;
        const RIDER: usize = WHEEL + WHEEL_LENGTH;
        const AUTO_BRIGHTNESS: usize = OPTIONS_LENGTH - 1;
        const FTP: [u8; 2] = [250, 0];
        let good = Options::default().encode();
        let with = |at: usize, value: u8| {
            let mut options = good;
            options[at] = value;
            options.to_vec()
        };
        for (case, profile, options) in [
            ("one byte short", FTP, good[..OPTIONS_LENGTH - 1].to_vec()),
            ("one byte long", FTP, [&good[..], &[0]].concat()),
            ("unknown flag", FTP, with(FLAGS, good[FLAGS] | 1 << 3)),
            ("unknown ride page", FTP, with(DECK, 4)),
            ("a ride page twice", FTP, with(DECK + 1, good[DECK])),
            ("lap too long", FTP, with(AUTO_LAP, AUTO_LAP_FIRST_UNUSED)),
            ("unknown day/night", FTP, with(DAYLIGHT, 3)),
            ("unknown theme", FTP, with(THEME, 3)),
            ("unknown zone", FTP, with(ZONE, 2)),
            ("implausible wheel", FTP, with(WHEEL, 0xdb)),
            ("unknown wheel source", FTP, with(WHEEL_MANUAL, 2)),
            ("rider weight out of range", FTP, with(RIDER, 29)),
            ("rider age out of range", FTP, with(RIDER + 1, 100)),
            ("unknown brightness choice", FTP, with(AUTO_BRIGHTNESS, 3)),
            ("impossible FTP", [0, 0], good.to_vec()),
        ] {
            let damaged = [&[VERSION, profile[0], profile[1], 170, 0, 0][..], &options].concat();
            assert_eq!(decode(&damaged), (defaults, Stored::Foreign), "{case}");
        }
    }

    #[test]
    fn sensor_changes_save_at_once_and_locked_records_keep_them_in_memory() {
        let mut sensors = Saved::default();
        sensors.add(sensor(120, 1));
        let mut preferences = Preferences::load(&encode(&Record::default()), Source::Current);
        assert_eq!(preferences.stored, Stored::Current);
        preferences.set_sensors(sensors, 10_000);
        assert!(preferences.due(10_000));
        let mut locked = Preferences::load(&[VERSION + 1, 9], Source::Current);
        locked.set_sensors(sensors, 10_000);
        assert_eq!(locked.sensors, sensors);
        assert!(!locked.due(60_000));
    }

    #[test]
    fn edits_settle_before_saving_and_a_foreign_record_is_never_replaced() {
        let mut preferences = Preferences::load(&[], Source::Missing);
        assert_eq!(preferences.persistence(), Persistence::Saved);
        let stronger = TrainingProfile::new(250, 185).unwrap();
        preferences.set(stronger, 1_000);
        assert_eq!(preferences.persistence(), Persistence::Pending);
        assert!(!preferences.due(2_999));
        assert!(preferences.due(3_000));

        let mut locked = Preferences::load(&[VERSION + 1, 9], Source::Current);
        assert_eq!(locked.stored, Stored::Foreign);
        assert_eq!(locked.persistence(), Persistence::Locked);
        locked.set(stronger, 1_000);
        assert_eq!(locked.profile, stronger, "edits still apply");
        assert!(!locked.due(60_000), "unreadable data is never replaced");
        let mut shell = Preferences::load(&[], Source::Foreign);
        assert_eq!(shell.persistence(), Persistence::Locked);
        let outdated = Preferences::load(&[VERSION - 1, 9], Source::Current);
        assert_eq!(outdated.persistence(), Persistence::Pending);
        assert!(outdated.due(DEBOUNCE_MS), "an older record is replaced");
        shell.set(stronger, 1_000);
        assert!(!shell.due(60_000), "nor are foreign settings around it");
    }

    #[test]
    fn option_edits_settle_like_profile_edits() {
        let mut preferences = Preferences::load(&encode(&Record::default()), Source::Current);
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

    fn arbitrary_record(rng: &mut crate::testing::Rng) -> Record {
        let profile =
            TrainingProfile::new(rng.range(50, 600) as u16, rng.range(80, 220) as u16).unwrap();
        let mut sensors = Saved::default();
        for _ in 0..rng.below(12) {
            let any = rng.byte();
            let device_type = rng.pick(&[120, 11, 121, 122, 123, 17, 40, any]);
            sensors.add(device_api::ant::Identity {
                device_type,
                device_number: rng.range(1, 65_535) as u16,
                transmission_type: rng.range(1, 255) as u8,
            });
        }
        let options = Options {
            auto_pause: rng.one_in(2),
            sounds: rng.one_in(2),
            auto_lap: match rng.below(4) {
                0 => AutoLap::Off,
                1 => AutoLap::AtStart,
                _ => AutoLap::Every(rng.range(1, u64::from(AUTO_LAP_MAX_KM)) as u8),
            },
            daylight: rng.pick(&[Daylight::Auto, Daylight::Day, Daylight::Night]),
            theme: rng.pick(&[Theme::Dark, Theme::Light, Theme::Auto]),
            setup: rng.byte(),
            alerts: core::array::from_fn(|_| rng.byte()),
            sunset_alert: rng.one_in(2),
            wheel: Wheel {
                mm: rng.one_in(2).then(|| rng.range(1_500, 2_400) as u16),
                manual: rng.one_in(2),
            },
            deck: Deck::OPTIONAL
                .into_iter()
                .filter(|_| rng.one_in(2))
                .fold(Deck::default(), Deck::toggled),
            main_slots: Slots {
                left: rng.pick(&Reading::LOWER),
                right: rng.pick(&Reading::LOWER),
            },
            mode: DisplayMode::from_byte(rng.below(2) as u8).unwrap(),
            auto_brightness: rng.one_in(2),
            rider: Rider::decode([
                rng.pick(&[0, 30, 72, 200]),
                rng.pick(&[0, 10, 41, 99]),
                rng.pick(&[0, 1, 2]),
            ])
            .unwrap(),
            zone: if rng.one_in(3) {
                Zone::Chile
            } else {
                Zone::Fixed(rng.range(0, 104) as i8 - 48)
            },
        };
        record(profile, sensors, options)
    }

    #[test]
    fn damaged_records_are_never_read_as_other_preferences() {
        for seed in 0..3_000 {
            let mut rng = crate::testing::Rng::new(seed);
            let saved = arbitrary_record(&mut rng);
            let encoded = encode(&saved);
            assert_eq!(decode(&encoded), (saved, Stored::Current), "seed {seed}");
            let payload = rng.mutate(encoded.to_vec());
            let (decoded, stored) = decode(&payload);
            match stored {
                // Every byte of a current record is meaningful, so a damaged
                // one reads only if it is exactly another valid record.
                Stored::Current => assert_eq!(encode(&decoded)[..], payload[..], "seed {seed}"),
                Stored::Outdated | Stored::Foreign => {
                    assert_eq!(decoded, Record::default(), "seed {seed}")
                }
                Stored::Missing => assert!(payload.is_empty(), "seed {seed}"),
            }
        }
    }
}
