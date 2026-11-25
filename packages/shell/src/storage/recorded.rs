//! How the settings capability reaches a trace: the encoding of what its
//! methods take and give, and the hook that replaces the saved Wi-Fi
//! password and the pairing in a redacting log.
use super::{Error, Loaded, SettingsRedact};
use crate::preferences::{AppPreferences, Settings, Source};
use core::ops::DerefMut;
use device_api::journal::Put;
use firmware_services::storage::{Error as Journal, Record};
use firmware_services::trace::log::Log;
use firmware_services::trace::media::Memory;
use firmware_services::trace::recorder::Sink;
use firmware_services::trace::redact;
use firmware_services::trace::wire::{Reader, Wire, Writer};

impl Wire<'_> for Error {
    fn put(&self, w: &mut Writer) {
        w.byte(match self {
            Self::Journal(Journal::Flash(())) => 0,
            Self::Journal(Journal::PayloadTooLarge) => 1,
            Self::Journal(Journal::OutputTooSmall) => 2,
            Self::Journal(Journal::ReadbackFailed) => 3,
            Self::Journal(Journal::UnrecognizedJournal) => 4,
            Self::Journal(Journal::UnsupportedGeometry) => 5,
            Self::Journal(Journal::NoRecord) => 6,
            Self::UnrecognizedSettings => 7,
        });
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self::Journal(match r.byte() {
            0 => Journal::Flash(()),
            1 => Journal::PayloadTooLarge,
            2 => Journal::OutputTooSmall,
            3 => Journal::ReadbackFailed,
            4 => Journal::UnrecognizedJournal,
            5 => Journal::UnsupportedGeometry,
            6 => Journal::NoRecord,
            7 => return Self::UnrecognizedSettings,
            _ => {
                r.fail();
                Journal::NoRecord
            }
        })
    }
}

impl Wire<'_> for Source {
    fn put(&self, w: &mut Writer) {
        w.byte(match self {
            Self::Current => 0,
            Self::Missing => 1,
            Self::Foreign => 2,
            Self::Failed => 3,
            Self::Unavailable => 4,
        });
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Current,
            1 => Self::Missing,
            2 => Self::Foreign,
            3 => Self::Failed,
            4 => Self::Unavailable,
            _ => {
                r.fail();
                Self::Failed
            }
        }
    }
}

impl Wire<'_> for AppPreferences {
    fn put(&self, w: &mut Writer) {
        self.bytes().put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self::new(<&[u8]>::take(r)).unwrap_or_else(|| {
            r.fail();
            Self::EMPTY
        })
    }
}

/// The settings record as the shell persists it, its Wi-Fi password and
/// pairing replaced when `redacting`.
pub fn encoded(settings: &Settings, redacting: bool) -> [u8; crate::preferences::LENGTH] {
    let mut settings = *settings;
    if redacting && let Some(wifi) = &mut settings.wifi {
        wifi.password = redact::wifi_password(wifi.password);
    }
    if redacting && let Some(pairing) = &mut settings.pairing {
        *pairing = redact::PAIRING;
    }
    settings.encode()
}

impl<B, S, M> SettingsRedact for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn load(&mut self, value: &Result<Loaded, Error>) {
        match value {
            Ok(loaded) => {
                self.put(&0u8);
                Put::<[u8]>::put(self, &encoded(&loaded.settings, self.redacts()));
                self.put(&loaded.source);
                self.put(&loaded.sequence);
                self.put(&loaded.length);
            }
            Err(error) => {
                self.put(&1u8);
                self.put(error);
            }
        }
    }

    fn save(&mut self, settings: &Settings, value: &Result<Record, Error>) {
        Put::<[u8]>::put(self, &encoded(settings, self.redacts()));
        self.put(value);
    }

    fn save_connectivity(&mut self, current: &Settings, value: &Result<Record, Error>) {
        Put::<[u8]>::put(self, &encoded(current, self.redacts()));
        self.put(value);
    }
}
