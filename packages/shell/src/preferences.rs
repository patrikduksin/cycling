//! Versioned user preferences and wear-conscious save scheduling.

pub const DEFAULT_BRIGHTNESS: u8 = 50;
pub const DEFAULT_DIM_TIMEOUT_SECS: u16 = 30;
pub const DEFAULT_DIM_BRIGHTNESS: u8 = 10;
pub const DEFAULT_TIMEZONE_MINUTES: i16 = 0;
pub const DEBOUNCE_MS: u64 = 1_000;
pub const RETRY_MS: u64 = 5_000;
const PREFIX: &[u8] = b"cycling";
const VERSION: u8 = 5;
const LENGTH: usize = 159;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub brightness: u8,
    pub dim_timeout_secs: u16,
    pub dim_brightness: u8,
    pub timezone_minutes: i16,
    pub wifi: Option<device_api::connectivity::WifiConfig>,
    pub ble: Option<device_api::ble_transport::Selection>,
    pub ble_profile: u8,
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
            settings
        })
    }

    pub fn encode(self) -> [u8; LENGTH] {
        let mut output = [0; LENGTH];
        output[..PREFIX.len()].copy_from_slice(PREFIX);
        output[7] = VERSION;
        output[8] = self.brightness;
        output[9..11].copy_from_slice(&self.dim_timeout_secs.to_le_bytes());
        output[11] = self.dim_brightness;
        output[12..14].copy_from_slice(&self.timezone_minutes.to_le_bytes());
        if let Some(wifi) = self.wifi {
            output[15] = if wifi.wpa3 { 2 } else { 1 };
            output[16] = wifi.ssid.bytes().len() as u8;
            output[17..17 + wifi.ssid.bytes().len()].copy_from_slice(wifi.ssid.bytes());
            output[49] = wifi.password.bytes().len() as u8;
            output[50..50 + wifi.password.bytes().len()].copy_from_slice(wifi.password.bytes());
        }
        if let Some(ble) = self.ble {
            output[113] = 1;
            output[114] = self.ble_profile;
            output[115] = ble.name.bytes().len() as u8;
            output[116..116 + ble.name.bytes().len()].copy_from_slice(ble.name.bytes());
            if let Some(address) = ble.address {
                output[148] = 1;
                output[149..155].copy_from_slice(&address);
            }
            output[155..157].copy_from_slice(&ble.service.to_le_bytes());
            output[157..159].copy_from_slice(&ble.characteristic.to_le_bytes());
        }
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
    let Some(payload) = payload else {
        return (Settings::default(), Source::Missing);
    };
    if payload == b"cycling\x01" {
        return (Settings::default(), Source::LegacyDefaults);
    }
    if !payload.starts_with(PREFIX) || payload.len() < 8 {
        return (Settings::default(), Source::Malformed);
    }
    let settings = match payload[7] {
        2 if payload.len() == 10 && payload[9] == 0 => {
            return match Settings::new(payload[8]) {
                Some(settings) => (settings, Source::Migrated),
                None => (Settings::default(), Source::Malformed),
            };
        }
        3 if payload.len() == 13 && payload[12] == 0 => {
            return match Settings::with_idle(
                payload[8],
                u16::from_le_bytes([payload[9], payload[10]]),
                payload[11],
            ) {
                Some(settings) => (settings, Source::Migrated),
                None => (Settings::default(), Source::Malformed),
            };
        }
        4 | VERSION
            if ((payload[7] == 4 && payload.len() == 15)
                || (payload[7] == VERSION && payload.len() == LENGTH))
                && payload[14] == 0 =>
        {
            Settings::with_values(
                payload[8],
                u16::from_le_bytes([payload[9], payload[10]]),
                payload[11],
                i16::from_le_bytes([payload[12], payload[13]]),
            )
        }
        4 | VERSION => None,
        _ => return (Settings::default(), Source::Unsupported),
    };
    match settings {
        Some(mut settings) => {
            if payload[7] == 4 {
                return (settings, Source::Migrated);
            }
            let malformed = || (Settings::default(), Source::Malformed);
            match payload[15] {
                0 => {
                    if payload[16..113].iter().any(|b| *b != 0) {
                        return malformed();
                    }
                }
                1 | 2 => {
                    let n = usize::from(payload[16]);
                    let m = usize::from(payload[49]);
                    if n > 32
                        || m > 63
                        || payload[17 + n..49]
                            .iter()
                            .chain(payload[50 + m..113].iter())
                            .any(|b| *b != 0)
                    {
                        return malformed();
                    }
                    settings.wifi = device_api::connectivity::WifiConfig::new(
                        &payload[17..17 + n],
                        &payload[50..50 + m],
                        payload[15] == 2,
                    );
                    if settings.wifi.is_none() {
                        return malformed();
                    }
                }
                _ => return malformed(),
            }
            match payload[113] {
                0 => {
                    if payload[114..].iter().any(|b| *b != 0) {
                        return malformed();
                    }
                }
                1 => {
                    let n = usize::from(payload[115]);
                    if n > 32
                        || payload[148] > 1
                        || payload[116 + n..148].iter().any(|b| *b != 0)
                        || (payload[148] == 0 && payload[149..155].iter().any(|b| *b != 0))
                    {
                        return malformed();
                    }
                    let Some(name) = device_api::connectivity::Text::new(&payload[116..116 + n])
                    else {
                        return malformed();
                    };
                    let address =
                        (payload[148] == 1).then(|| payload[149..155].try_into().unwrap());
                    if name.is_empty() && address.is_none() {
                        return malformed();
                    }
                    settings.ble = Some(device_api::ble_transport::Selection {
                        name,
                        address,
                        service: u16::from_le_bytes([payload[155], payload[156]]),
                        characteristic: u16::from_le_bytes([payload[157], payload[158]]),
                    });
                    settings.ble_profile = payload[114];
                }
                _ => return malformed(),
            }
            (settings, Source::Current)
        }
        None => (Settings::default(), Source::Malformed),
    }
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
        if self.pending.map(|(value, _)| value) != Some(current) {
            self.pending = Some((current, now));
        }
        let (_, changed_at) = self.pending.unwrap();
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
mod tests {
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
        assert_eq!(decode(Some(b"cycling\x06\x32\0")).1, Source::Unsupported);
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
        for index in [15, 16, 49, 113, 114, 115, 148, 158] {
            let mut payload = current;
            payload[index] = 255;
            assert_eq!(decode(Some(&payload)).1, Source::Malformed);
        }
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
}
