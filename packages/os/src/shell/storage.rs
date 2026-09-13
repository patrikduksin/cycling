use crate::storage::*;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Journal(crate::storage::Error<E>),
    /// The journal is valid but the preference schema cannot safely be replaced.
    UnrecognizedSettings,
}
impl<E> From<crate::storage::Error<E>> for Error<E> {
    fn from(error: crate::storage::Error<E>) -> Self {
        Self::Journal(error)
    }
}
pub struct Loaded {
    pub settings: crate::shell::preferences::Settings,
    pub source: crate::shell::preferences::Source,
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
        let (settings, source) = crate::shell::preferences::decode(bytes);
        Ok(Loaded {
            settings,
            source,
            sequence: record.map(|r| r.sequence),
            length: record.map(|r| r.length).unwrap_or(0),
        })
    }

    pub fn save(
        &mut self,
        settings: crate::shell::preferences::Settings,
    ) -> Result<Record, Error<B::Error>> {
        if matches!(
            self.load()?.source,
            crate::shell::preferences::Source::Unsupported
                | crate::shell::preferences::Source::Malformed
        ) {
            return Err(Error::UnrecognizedSettings);
        }
        match self.journal.save(&settings.encode()) {
            Ok(record) => Ok(record),
            Err(error) => match self.load() {
                Ok(loaded)
                    if loaded.source == crate::shell::preferences::Source::Current
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

    pub fn data(&mut self) -> RegionAccess<'_, B> {
        RegionAccess::new(self.journal.flash_mut(), Region::Data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::Memory;
    #[test]
    fn unsupported_and_malformed_settings_are_readable_but_cannot_be_overwritten() {
        use crate::shell::preferences::{Settings, Source};
        for (payload, source) in [
            (b"cycling\x05".as_slice(), Source::Unsupported),
            (b"unrecognized".as_slice(), Source::Malformed),
            (b"cycling\x04".as_slice(), Source::Malformed),
        ] {
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            journal.save(&Settings::default().encode()).unwrap();
            journal.save(payload).unwrap();
            let mut store = Store { journal };
            assert_eq!(store.load().unwrap().source, source);
            let before = store.journal.flash.bytes;
            let operations = store.journal.flash.operations;
            assert_eq!(
                store.save(Settings::default()),
                Err(Error::UnrecognizedSettings)
            );
            assert_eq!(store.journal.flash.bytes, before);
            // Two scan reads plus a verified load; no mutation operation.
            assert_eq!(store.journal.flash.operations - operations, 3);
        }
    }

    #[test]
    fn supported_settings_versions_and_missing_journal_remain_writable() {
        use crate::shell::preferences::{Settings, Source};
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
