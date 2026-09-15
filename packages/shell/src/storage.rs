use device_api::storage::{OwnedFlash, Region};
use firmware_services::storage::{CAPACITY, Journal, Record, RegionAccess};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Journal(firmware_services::storage::Error<E>),
    /// The journal is valid but the preference schema cannot safely be replaced.
    UnrecognizedSettings,
}
impl<E> From<firmware_services::storage::Error<E>> for Error<E> {
    fn from(error: firmware_services::storage::Error<E>) -> Self {
        Self::Journal(error)
    }
}
pub struct Loaded {
    pub settings: crate::preferences::Settings,
    pub source: crate::preferences::Source,
    pub sequence: Option<u32>,
    pub length: usize,
}

/// Settings journal and generic owned bytes. No application format or reclaim policy.
/// Calls are synchronous: the caller yields between bounded media operations.
/// Media programming/erase can suspend interrupts and acquisition. Callers
/// must retain transport loss detection and reset parsing after detected gaps.
/// A failed mutation is ambiguous; reconcile by reading, never blindly retry it.
pub struct Store<B> {
    journal: Journal<B>,
}

impl<B: OwnedFlash> Store<B> {
    pub fn new(backend: B) -> Self {
        Self {
            journal: Journal::new(backend, Region::Configuration),
        }
    }

    pub fn load(&mut self) -> Result<Loaded, Error<B::Error>> {
        let mut payload = [0u8; CAPACITY];
        let record = self.journal.load(&mut payload)?;
        let bytes = record.map(|record| &payload[..record.length]);
        let (settings, source) = crate::preferences::decode(bytes);
        Ok(Loaded {
            settings,
            source,
            sequence: record.map(|r| r.sequence),
            length: record.map(|r| r.length).unwrap_or(0),
        })
    }

    pub fn save(
        &mut self,
        settings: crate::preferences::Settings,
    ) -> Result<Record, Error<B::Error>> {
        if matches!(
            self.load()?.source,
            crate::preferences::Source::Unsupported | crate::preferences::Source::Malformed
        ) {
            return Err(Error::UnrecognizedSettings);
        }
        match self.journal.save(&settings.encode()) {
            Ok(record) => Ok(record),
            Err(error) => match self.load() {
                Ok(loaded)
                    if loaded.source == crate::preferences::Source::Current
                        && loaded.settings == settings =>
                {
                    Ok(Record {
                        sequence: loaded.sequence.unwrap(),
                        length: loaded.length,
                    })
                }
                _ => Err(error.into()),
            },
        }
    }

    /// Connectivity provisioning must not accidentally persist temporary display preferences.
    pub fn save_connectivity(
        &mut self,
        current: crate::preferences::Settings,
    ) -> Result<Record, Error<B::Error>> {
        let mut settings = self.load()?.settings;
        settings.wifi = current.wifi;
        settings.ble = current.ble;
        settings.ble_profile = current.ble_profile;
        self.save(settings)
    }

    pub fn data(&mut self) -> RegionAccess<'_, B> {
        RegionAccess::new(self.journal.flash_mut(), Region::Data)
    }
}

#[cfg(test)]
mod tests {
    use super::memory::Memory;
    use super::*;
    #[test]
    fn unsupported_and_malformed_settings_are_readable_but_cannot_be_overwritten() {
        use crate::preferences::Settings;
        use crate::preferences::Source;
        for (payload, source) in [
            (b"cycling\x06".as_slice(), Source::Unsupported),
            (b"unrecognized".as_slice(), Source::Malformed),
            (b"cycling\x04".as_slice(), Source::Malformed),
        ] {
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            journal.save(&Settings::default().encode()).unwrap();
            journal.save(payload).unwrap();
            let mut store = Store { journal };
            assert_eq!(store.load().unwrap().source, source);
            let before = store.journal.flash_mut().bytes;
            let operations = store.journal.flash_mut().operations;
            assert_eq!(
                store.save(Settings::default()),
                Err(Error::UnrecognizedSettings)
            );
            assert_eq!(store.journal.flash_mut().bytes, before);
            // Two scan reads plus a verified load; no mutation operation.
            assert_eq!(store.journal.flash_mut().operations - operations, 3);
        }
    }

    #[test]
    fn migration_and_connectivity_save_keep_preferences() {
        use crate::preferences::Settings;
        use crate::preferences::Source;
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
        let old = [
            b'c', b'y', b'c', b'l', b'i', b'n', b'g', 4, 75, 30, 0, 10, 0, 0, 0,
        ];
        journal.save(&old).unwrap();
        let mut store = Store { journal };
        let loaded = store.load().unwrap();
        assert_eq!(loaded.source, Source::Migrated);
        assert_eq!(loaded.settings.brightness, 75);
        let mut settings = loaded.settings;
        settings.wifi = Some(
            device_api::connectivity::WifiConfig::new(b"Owned test", b"test-password", false)
                .unwrap(),
        );
        store.save(settings).unwrap();
        assert_eq!(store.load().unwrap().settings, settings);
        let update = settings.with_timezone(330).unwrap();
        store.save(update).unwrap();
        assert_eq!(store.load().unwrap().settings.wifi, settings.wifi);
        assert_eq!(store.load().unwrap().settings.brightness, 75);
        let _ = Settings::default();
    }

    #[test]
    fn supported_settings_versions_and_missing_journal_remain_writable() {
        use crate::preferences::Settings;
        use crate::preferences::Source;
        for payload in [
            None,
            Some(b"cycling\x01".as_slice()),
            Some(b"cycling\x02\x32\x00".as_slice()),
            Some(Settings::default().encode().as_slice()),
        ] {
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            if let Some(payload) = payload {
                journal.save(payload).unwrap();
            }
            let mut store = Store { journal };
            let settings = Settings::new(75).unwrap();
            store.save(settings).unwrap();
            let loaded = store.load().unwrap();
            assert_eq!(loaded.source, Source::Current);
            assert_eq!(loaded.settings, settings);
        }
    }
}

#[cfg(test)]
mod memory;
