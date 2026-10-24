//! Versioned user preferences and wear-conscious save scheduling.
use firmware_services::le::{get_u16, put_u16};

pub const DEFAULT_BRIGHTNESS: u8 = 50;
pub const DEFAULT_DIM_TIMEOUT_SECS: u16 = 30;
pub const DEFAULT_DIM_BRIGHTNESS: u8 = 10;
pub const DEFAULT_TIMEZONE_MINUTES: i16 = 0;
pub const DEBOUNCE_MS: u64 = 1_000;
pub const RETRY_MS: u64 = 5_000;
const PREFIX: &[u8] = b"cycling";
const VERSION: u8 = 6;
const LENGTH: usize = layout::APP + APP_CAPACITY;
/// Bytes reserved for the application's own versioned preference record.
pub const APP_CAPACITY: usize = 96;

/// Byte offsets of the persisted settings record. Older versions are prefixes
/// of this layout; text fields are length-prefixed and zero-padded, and every
/// unused byte must stay zero. Multi-byte fields are little-endian.
mod layout {
    pub const VERSION_BYTE: usize = 7;
    pub const BRIGHTNESS: usize = 8;
    pub const DIM_TIMEOUT: usize = 9;
    pub const DIM_BRIGHTNESS: usize = 11;
    pub const TIMEZONE: usize = 12;
    /// Always zero; terminates version 4 records.
    pub const RESERVED: usize = 14;
    pub const WIFI: usize = 15;
    pub const SSID_LEN: usize = 16;
    pub const SSID: usize = 17;
    pub const PASSWORD_LEN: usize = SSID + 32;
    pub const PASSWORD: usize = PASSWORD_LEN + 1;
    pub const BLE: usize = PASSWORD + 63;
    pub const BLE_PROFILE: usize = BLE + 1;
    pub const BLE_NAME_LEN: usize = BLE_PROFILE + 1;
    pub const BLE_NAME: usize = BLE_NAME_LEN + 1;
    pub const BLE_ADDRESS_KIND: usize = BLE_NAME + 32;
    pub const BLE_ADDRESS: usize = BLE_ADDRESS_KIND + 1;
    pub const BLE_SERVICE: usize = BLE_ADDRESS + 6;
    pub const BLE_CHARACTERISTIC: usize = BLE_SERVICE + 2;
    /// Terminates version 5 records.
    pub const APP_LEN: usize = BLE_CHARACTERISTIC + 2;
    pub const APP: usize = APP_LEN + 1;
}
const V2_LENGTH: usize = 10;
const V3_LENGTH: usize = 13;
const V4_LENGTH: usize = layout::RESERVED + 1;
const V5_LENGTH: usize = layout::APP_LEN;

/// Application-owned preference bytes the shell stores without interpreting.
/// The application chooses their schema, versioning and recovery; the shell
/// only bounds their length and carries them through every settings save.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppPreferences {
    length: u8,
    bytes: [u8; APP_CAPACITY],
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl AppPreferences {
    pub const EMPTY: Self = Self {
        length: 0,
        bytes: [0; APP_CAPACITY],
    };

    pub fn new(value: &[u8]) -> Option<Self> {
        let mut bytes = [0; APP_CAPACITY];
        bytes.get_mut(..value.len())?.copy_from_slice(value);
        Some(Self {
            length: value.len() as u8,
            bytes,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub brightness: u8,
    pub dim_timeout_secs: u16,
    pub dim_brightness: u8,
    pub timezone_minutes: i16,
    pub wifi: Option<device_api::connectivity::WifiConfig>,
    pub ble: Option<device_api::ble_transport::Selection>,
    pub ble_profile: u8,
    /// Application preferences, opaque to the shell.
    pub app: AppPreferences,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brightness: DEFAULT_BRIGHTNESS,
            dim_timeout_secs: DEFAULT_DIM_TIMEOUT_SECS,
            dim_brightness: DEFAULT_DIM_BRIGHTNESS,
            timezone_minutes: DEFAULT_TIMEZONE_MINUTES,
            wifi: None,
            ble: None,
            ble_profile: 0,
            app: AppPreferences::EMPTY,
        }
    }
}

impl Settings {
    pub fn new(brightness: u8) -> Option<Self> {
        Self::with_idle(brightness, DEFAULT_DIM_TIMEOUT_SECS, DEFAULT_DIM_BRIGHTNESS)
    }

    pub fn with_idle(brightness: u8, dim_timeout_secs: u16, dim_brightness: u8) -> Option<Self> {
        Self::with_values(
            brightness,
            dim_timeout_secs,
            dim_brightness,
            DEFAULT_TIMEZONE_MINUTES,
        )
    }

    pub fn with_values(
        brightness: u8,
        dim_timeout_secs: u16,
        dim_brightness: u8,
        timezone_minutes: i16,
    ) -> Option<Self> {
        ((5..=100).contains(&brightness)
            && dim_timeout_secs <= 3_600
            && (5..=100).contains(&dim_brightness)
            && (-720..=840).contains(&timezone_minutes)
            && timezone_minutes % 30 == 0)
            .then_some(Self {
                brightness,
                dim_timeout_secs,
                dim_brightness,
                timezone_minutes,
                wifi: None,
                ble: None,
                ble_profile: 0,
                app: AppPreferences::EMPTY,
            })
    }

    pub fn with_brightness(self, brightness: u8) -> Option<Self> {
        Self::with_values(
            brightness,
            self.dim_timeout_secs,
            self.dim_brightness,
            self.timezone_minutes,
        )
        .map(|mut settings| {
            settings.wifi = self.wifi;
            settings.ble = self.ble;
            settings.ble_profile = self.ble_profile;
            settings.app = self.app;
            settings
        })
    }

    pub fn with_idle_preferences(self, dim_timeout_secs: u16, dim_brightness: u8) -> Option<Self> {
        Self::with_values(
            self.brightness,
            dim_timeout_secs,
            dim_brightness,
            self.timezone_minutes,
        )
        .map(|mut settings| {
            settings.wifi = self.wifi;
            settings.ble = self.ble;
            settings.ble_profile = self.ble_profile;
            settings.app = self.app;
            settings
        })
    }

    pub fn with_timezone(self, timezone_minutes: i16) -> Option<Self> {
        Self::with_values(
            self.brightness,
            self.dim_timeout_secs,
            self.dim_brightness,
            timezone_minutes,
        )
        .map(|mut settings| {
            settings.wifi = self.wifi;
            settings.ble = self.ble;
            settings.ble_profile = self.ble_profile;
            settings.app = self.app;
            settings
        })
    }

    pub fn encode(self) -> [u8; LENGTH] {
        use layout::*;
        let mut output = [0; LENGTH];
        output[..PREFIX.len()].copy_from_slice(PREFIX);
        output[VERSION_BYTE] = VERSION;
        output[BRIGHTNESS] = self.brightness;
        put_u16(&mut output, DIM_TIMEOUT, self.dim_timeout_secs);
        output[DIM_BRIGHTNESS] = self.dim_brightness;
        put_u16(&mut output, TIMEZONE, self.timezone_minutes as u16);
        if let Some(wifi) = self.wifi {
            output[WIFI] = if wifi.wpa3 { 2 } else { 1 };
            put_text(&mut output, SSID_LEN, wifi.ssid.bytes());
            put_text(&mut output, PASSWORD_LEN, wifi.password.bytes());
        }
        if let Some(ble) = self.ble {
            output[BLE] = 1;
            output[BLE_PROFILE] = self.ble_profile;
            put_text(&mut output, BLE_NAME_LEN, ble.name.bytes());
            if let Some(address) = ble.address {
                output[BLE_ADDRESS_KIND] = 1;
                output[BLE_ADDRESS..BLE_SERVICE].copy_from_slice(&address);
            }
            put_u16(&mut output, BLE_SERVICE, ble.service);
            put_u16(&mut output, BLE_CHARACTERISTIC, ble.characteristic);
        }
        put_text(&mut output, APP_LEN, self.app.bytes());
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Current,
    LegacyDefaults,
    Migrated,
    Missing,
    Malformed,
    Unsupported,
}

pub fn decode(payload: Option<&[u8]>) -> (Settings, Source) {
    use layout::*;
    let Some(payload) = payload else {
        return (Settings::default(), Source::Missing);
    };
    if payload == b"cycling\x01" {
        return (Settings::default(), Source::LegacyDefaults);
    }
    if !payload.starts_with(PREFIX) || payload.len() < 8 {
        return (Settings::default(), Source::Malformed);
    }
    let version = payload[VERSION_BYTE];
    let length = payload.len();
    let settings = match version {
        2 if length == V2_LENGTH && payload[V2_LENGTH - 1] == 0 => {
            return match Settings::new(payload[BRIGHTNESS]) {
                Some(settings) => (settings, Source::Migrated),
                None => (Settings::default(), Source::Malformed),
            };
        }
        3 if length == V3_LENGTH && payload[V3_LENGTH - 1] == 0 => {
            return match Settings::with_idle(
                payload[BRIGHTNESS],
                get_u16(payload, DIM_TIMEOUT),
                payload[DIM_BRIGHTNESS],
            ) {
                Some(settings) => (settings, Source::Migrated),
                None => (Settings::default(), Source::Malformed),
            };
        }
        4 | 5 | VERSION
            if ((version == 4 && length == V4_LENGTH)
                || (version == 5 && length == V5_LENGTH)
                || (version == VERSION && length == LENGTH))
                && payload[RESERVED] == 0 =>
        {
            Settings::with_values(
                payload[BRIGHTNESS],
                get_u16(payload, DIM_TIMEOUT),
                payload[DIM_BRIGHTNESS],
                get_u16(payload, TIMEZONE) as i16,
            )
        }
        4 | 5 | VERSION => None,
        _ => return (Settings::default(), Source::Unsupported),
    };
    match settings {
        Some(mut settings) => {
            if version == 4 {
                return (settings, Source::Migrated);
            }
            let malformed = || (Settings::default(), Source::Malformed);
            match payload[WIFI] {
                0 => {
                    if payload[SSID_LEN..BLE].iter().any(|b| *b != 0) {
                        return malformed();
                    }
                }
                kind @ (1 | 2) => {
                    let (Some(ssid), Some(password)) = (
                        text(payload, SSID_LEN, PASSWORD_LEN),
                        text(payload, PASSWORD_LEN, BLE),
                    ) else {
                        return malformed();
                    };
                    settings.wifi =
                        device_api::connectivity::WifiConfig::new(ssid, password, kind == 2);
                    if settings.wifi.is_none() {
                        return malformed();
                    }
                }
                _ => return malformed(),
            }
            match payload[BLE] {
                0 => {
                    if payload[BLE_PROFILE..APP_LEN].iter().any(|b| *b != 0) {
                        return malformed();
                    }
                }
                1 => {
                    let mut address = [0; 6];
                    address.copy_from_slice(&payload[BLE_ADDRESS..BLE_SERVICE]);
                    let address = match payload[BLE_ADDRESS_KIND] {
                        0 if address == [0; 6] => None,
                        1 => Some(address),
                        _ => return malformed(),
                    };
                    let Some(name) = text(payload, BLE_NAME_LEN, BLE_ADDRESS_KIND)
                        .and_then(device_api::connectivity::Text::new)
                    else {
                        return malformed();
                    };
                    if name.is_empty() && address.is_none() {
                        return malformed();
                    }
                    settings.ble = Some(device_api::ble_transport::Selection {
                        name,
                        address,
                        service: get_u16(payload, BLE_SERVICE),
                        characteristic: get_u16(payload, BLE_CHARACTERISTIC),
                    });
                    settings.ble_profile = payload[BLE_PROFILE];
                }
                _ => return malformed(),
            }
            if version == 5 {
                return (settings, Source::Migrated);
            }
            let Some(app) = text(payload, APP_LEN, LENGTH).and_then(AppPreferences::new) else {
                return malformed();
            };
            settings.app = app;
            (settings, Source::Current)
        }
        None => (Settings::default(), Source::Malformed),
    }
}

/// A length byte at `at`, then text that must fit before `end` with zero padding.
fn text(payload: &[u8], at: usize, end: usize) -> Option<&[u8]> {
    let field = &payload[at + 1..end];
    let length = usize::from(payload[at]);
    (length <= field.len() && field[length..].iter().all(|b| *b == 0)).then(|| &field[..length])
}

fn put_text(output: &mut [u8], at: usize, value: &[u8]) {
    output[at] = value.len() as u8;
    output[at + 1..at + 1 + value.len()].copy_from_slice(value);
}

pub struct Saver {
    saved: Settings,
    pending: Option<(Settings, u64)>,
    retry_at: u64,
}

impl Saver {
    pub fn new(saved: Settings) -> Self {
        Self {
            saved,
            pending: None,
            retry_at: 0,
        }
    }

    pub fn ready(
        &mut self,
        now: u64,
        current: Settings,
        temporary: bool,
        gesture_active: bool,
    ) -> bool {
        if temporary || gesture_active {
            return false;
        }
        if current == self.saved {
            self.pending = None;
            return false;
        }
        let changed_at = match self.pending {
            Some((value, changed_at)) if value == current => changed_at,
            _ => {
                self.pending = Some((current, now));
                now
            }
        };
        now >= changed_at + DEBOUNCE_MS && now >= self.retry_at
    }

    pub fn saved(&mut self, settings: Settings) {
        self.saved = settings;
        self.pending = None;
        self.retry_at = 0;
    }

    pub fn failed(&mut self, now: u64) {
        self.retry_at = now + RETRY_MS;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn current_legacy_missing_malformed_and_unsupported_are_safe() {
        let settings = Settings::new(75).unwrap();
        assert_eq!(
            decode(Some(&settings.encode())),
            (settings, Source::Current)
        );
        assert_eq!(decode(Some(b"cycling\x01")).1, Source::LegacyDefaults);
        assert_eq!(decode(None).1, Source::Missing);
        assert_eq!(decode(Some(b"garbage")).1, Source::Malformed);
        assert_eq!(decode(Some(b"cycling\x07\x32\0")).1, Source::Unsupported);
        assert_eq!(decode(Some(b"cycling\x02\x00\0")).1, Source::Malformed);
        assert_eq!(decode(Some(b"cycling\x02\x65\0")).1, Source::Malformed);
        assert_eq!(decode(Some(b"cycling\x02\x4b\0")).1, Source::Migrated);
        let mut version_three = [0u8; 13];
        version_three[..7].copy_from_slice(b"cycling");
        version_three[7] = 3;
        version_three[8] = 75;
        version_three[9..11].copy_from_slice(&30u16.to_le_bytes());
        version_three[11] = 10;
        assert_eq!(decode(Some(&version_three)), (settings, Source::Migrated));
        assert!(settings.with_timezone(330).is_some());
        assert!(settings.with_timezone(331).is_none());
    }

    #[test]
    fn connectivity_round_trips_and_all_preference_updates_preserve_it() {
        let mut settings = Settings::with_values(77, 45, 12, -180).unwrap();
        settings.wifi = Some(
            device_api::connectivity::WifiConfig::new(b"Owned test", b"test-password", true)
                .unwrap(),
        );
        settings.ble = Some(device_api::ble_transport::Selection {
            name: device_api::connectivity::Text::new(b"Owned fixture").unwrap(),
            address: Some([1, 2, 3, 4, 5, 6]),
            service: 0x180d,
            characteristic: 0x2a37,
        });
        settings.ble_profile = 1;
        assert_eq!(
            decode(Some(&settings.encode())),
            (settings, Source::Current)
        );
        for updated in [
            settings.with_brightness(62).unwrap(),
            settings.with_idle_preferences(90, 20).unwrap(),
            settings.with_timezone(330).unwrap(),
        ] {
            assert_eq!(updated.wifi, settings.wifi);
            assert_eq!(updated.ble, settings.ble);
            assert_eq!(updated.ble_profile, 1);
            assert_eq!(decode(Some(&updated.encode())).0, updated);
        }
        let mut old = [0; 15];
        old[..7].copy_from_slice(b"cycling");
        old[7] = 4;
        old[8] = 77;
        old[9..11].copy_from_slice(&45u16.to_le_bytes());
        old[11] = 12;
        old[12..14].copy_from_slice(&(-180i16).to_le_bytes());
        let (migrated, source) = decode(Some(&old));
        assert_eq!(source, Source::Migrated);
        assert_eq!(migrated, Settings::with_values(77, 45, 12, -180).unwrap());
    }

    #[test]
    fn corrupt_connectivity_lengths_and_reserved_bytes_are_preserved_as_malformed() {
        let current = Settings::default().encode();
        for index in [15, 16, 49, 113, 114, 115, 148, 158, 159, 160, 255] {
            let mut payload = current;
            payload[index] = 255;
            assert_eq!(decode(Some(&payload)).1, Source::Malformed);
        }
    }

    #[test]
    fn version_five_and_six_layouts_are_pinned_and_padding_must_be_zero() {
        let mut settings = Settings::with_values(77, 0x0102, 12, -180).unwrap();
        settings.wifi =
            Some(device_api::connectivity::WifiConfig::new(b"ssid", b"password", false).unwrap());
        settings.ble = Some(device_api::ble_transport::Selection {
            name: device_api::connectivity::Text::new(b"hr").unwrap(),
            address: Some([1, 2, 3, 4, 5, 6]),
            service: 0x180d,
            characteristic: 0x2a37,
        });
        settings.ble_profile = 3;
        let mut expected = [0u8; 159];
        expected[..8].copy_from_slice(b"cycling\x05");
        expected[8] = 77;
        expected[9..11].copy_from_slice(&[2, 1]);
        expected[11] = 12;
        expected[12..14].copy_from_slice(&(-180i16).to_le_bytes());
        expected[15] = 1;
        expected[16] = 4;
        expected[17..21].copy_from_slice(b"ssid");
        expected[49] = 8;
        expected[50..58].copy_from_slice(b"password");
        expected[113] = 1;
        expected[114] = 3;
        expected[115] = 2;
        expected[116..118].copy_from_slice(b"hr");
        expected[148] = 1;
        expected[149..155].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        expected[155..157].copy_from_slice(&[0x0d, 0x18]);
        expected[157..159].copy_from_slice(&[0x37, 0x2a]);
        // Version 5 records stay readable, with no application preferences.
        assert_eq!(decode(Some(&expected)), (settings, Source::Migrated));
        let version_five = expected;
        let mut expected = [0u8; 256];
        expected[..159].copy_from_slice(&version_five);
        expected[7] = 6;
        expected[159] = 3;
        expected[160..163].copy_from_slice(&[1, 0xd7, 0]);
        settings.app = AppPreferences::new(&[1, 0xd7, 0]).unwrap();
        assert_eq!(settings.encode(), expected);
        assert_eq!(decode(Some(&expected)), (settings, Source::Current));
        for (index, value) in [
            (14, 1),
            (21, 1),
            (58, 1),
            (118, 1),
            (148, 2),
            (159, 97),
            (163, 1),
            (255, 1),
        ] {
            let mut payload = expected;
            payload[index] = value;
            assert_eq!(decode(Some(&payload)).1, Source::Malformed, "{index}");
        }
        let mut anonymous = expected;
        anonymous[148] = 0;
        assert_eq!(decode(Some(&anonymous)).1, Source::Malformed);
        anonymous[149..155].fill(0);
        assert_eq!(decode(Some(&anonymous)).0.ble.unwrap().address, None);
        let mut unnamed = expected;
        unnamed[115] = 0;
        unnamed[116..118].fill(0);
        assert_eq!(decode(Some(&unnamed)).1, Source::Current);
        unnamed[148] = 0;
        unnamed[149..155].fill(0);
        assert_eq!(decode(Some(&unnamed)).1, Source::Malformed);
    }

    #[test]
    fn application_preferences_survive_every_shell_update() {
        let app = AppPreferences::new(b"vana").unwrap();
        assert_eq!(app.bytes(), b"vana");
        assert!(AppPreferences::new(&[0; APP_CAPACITY]).is_some());
        assert!(AppPreferences::new(&[0; APP_CAPACITY + 1]).is_none());
        let mut settings = Settings {
            app,
            ..Settings::default()
        };
        for updated in [
            settings.with_brightness(62).unwrap(),
            settings.with_idle_preferences(90, 20).unwrap(),
            settings.with_timezone(330).unwrap(),
        ] {
            assert_eq!(updated.app, app);
            assert_eq!(decode(Some(&updated.encode())), (updated, Source::Current));
        }
        // Full-capacity application bytes need no padding.
        settings.app = AppPreferences::new(&[0xa5; APP_CAPACITY]).unwrap();
        assert_eq!(
            decode(Some(&settings.encode())),
            (settings, Source::Current)
        );
    }

    #[test]
    fn debounce_coalesces_and_pauses_temporary_or_active_changes() {
        let mut saver = Saver::new(Settings::default());
        let sixty = Settings::new(60).unwrap();
        assert!(!saver.ready(0, sixty, false, false));
        assert!(!saver.ready(900, Settings::new(70).unwrap(), false, false));
        assert!(!saver.ready(1_500, Settings::new(70).unwrap(), true, false));
        assert!(!saver.ready(1_900, Settings::new(70).unwrap(), false, true));
        assert!(saver.ready(2_000, Settings::new(70).unwrap(), false, false));
    }

    #[test]
    fn failed_save_retries_later_and_success_stops_writes() {
        let mut saver = Saver::new(Settings::default());
        let settings = Settings::new(55).unwrap();
        assert!(!saver.ready(0, settings, false, false));
        assert!(saver.ready(1_000, settings, false, false));
        saver.failed(1_000);
        assert!(!saver.ready(5_999, settings, false, false));
        assert!(saver.ready(6_000, settings, false, false));
        saver.saved(settings);
        assert!(!saver.ready(20_000, settings, false, false));
    }

    pub(crate) use firmware_services::testing::Rng;

    fn text(rng: &mut Rng, max: u64) -> std::vec::Vec<u8> {
        (0..rng.below(max + 1))
            .map(|_| b'!' + rng.below(94) as u8)
            .collect()
    }

    pub(crate) fn arbitrary_settings(rng: &mut Rng) -> Settings {
        let mut settings = Settings::with_values(
            5 + rng.below(96) as u8,
            rng.below(3_601) as u16,
            5 + rng.below(96) as u8,
            (rng.below(53) as i16 - 24) * 30,
        )
        .unwrap();
        if rng.below(2) == 0 {
            let ssid = [&b"a"[..], &text(rng, 31)].concat();
            let password = [&b"password"[..], &text(rng, 55)].concat();
            settings.wifi =
                device_api::connectivity::WifiConfig::new(&ssid, &password, rng.below(2) == 0);
        }
        if rng.below(2) == 0 {
            settings.ble = Some(device_api::ble_transport::Selection {
                name: device_api::connectivity::Text::new(&[&b"b"[..], &text(rng, 31)].concat())
                    .unwrap(),
                address: (rng.below(2) == 0).then(|| [rng.below(256) as u8; 6]),
                service: rng.below(65_536) as u16,
                characteristic: rng.below(65_536) as u16,
            });
            settings.ble_profile = rng.below(256) as u8;
        }
        settings.app = AppPreferences::new(&text(rng, APP_CAPACITY as u64)).unwrap();
        settings
    }

    pub(crate) fn damage(rng: &mut Rng, mut payload: std::vec::Vec<u8>) -> std::vec::Vec<u8> {
        for _ in 0..1 + rng.below(3) {
            match rng.below(4) {
                0 if !payload.is_empty() => {
                    let at = rng.below(payload.len() as u64) as usize;
                    payload[at] ^= 1 << rng.below(8);
                }
                1 if !payload.is_empty() => {
                    let at = rng.below(payload.len() as u64) as usize;
                    payload[at] = rng.below(256) as u8;
                }
                2 => payload.truncate(rng.below(payload.len() as u64 + 1) as usize),
                _ => payload.push(rng.below(256) as u8),
            }
        }
        payload
    }

    #[test]
    fn damaged_records_are_never_read_as_other_settings() {
        for seed in 0..3_000 {
            let mut rng = Rng::new(seed);
            let settings = arbitrary_settings(&mut rng);
            let encoded = settings.encode();
            assert_eq!(
                decode(Some(&encoded)),
                (settings, Source::Current),
                "seed {seed}"
            );
            let payload = damage(&mut rng, encoded.to_vec());
            let (decoded, source) = decode(Some(&payload));
            match source {
                // Every byte of a current record is meaningful, so a damaged
                // one reads only if it is exactly another valid record.
                Source::Current => assert_eq!(decoded.encode()[..], payload[..], "seed {seed}"),
                Source::Migrated => assert_eq!(
                    decode(Some(&decoded.encode())),
                    (decoded, Source::Current),
                    "seed {seed}"
                ),
                Source::Malformed | Source::Unsupported => {
                    assert_eq!(decoded, Settings::default(), "seed {seed}")
                }
                Source::Missing | Source::LegacyDefaults => panic!("seed {seed}: {source:?}"),
            }
        }
    }
}
