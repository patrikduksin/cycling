//! Versioned user preferences and wear-conscious save scheduling.

pub const DEFAULT_BRIGHTNESS: u8 = 50;
pub const DEBOUNCE_MS: u64 = 1_000;
pub const RETRY_MS: u64 = 5_000;
const PREFIX: &[u8] = b"cycling";
const VERSION: u8 = 2;
const LENGTH: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    pub brightness: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brightness: DEFAULT_BRIGHTNESS,
        }
    }
}

impl Settings {
    pub fn new(brightness: u8) -> Option<Self> {
        (5..=100)
            .contains(&brightness)
            .then_some(Self { brightness })
    }

    pub fn encode(self) -> [u8; LENGTH] {
        let mut output = [0; LENGTH];
        output[..PREFIX.len()].copy_from_slice(PREFIX);
        output[7] = VERSION;
        output[8] = self.brightness;
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Current,
    LegacyDefaults,
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
    if payload[7] != VERSION {
        return (Settings::default(), Source::Unsupported);
    }
    if payload.len() != LENGTH || payload[9] != 0 {
        return (Settings::default(), Source::Malformed);
    }
    match Settings::new(payload[8]) {
        Some(settings) => (settings, Source::Current),
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
        assert_eq!(decode(Some(b"cycling\x03\x32\0")).1, Source::Unsupported);
        assert_eq!(decode(Some(b"cycling\x02\x00\0")).1, Source::Malformed);
        assert_eq!(decode(Some(b"cycling\x02\x65\0")).1, Source::Malformed);
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
