use super::Runtime;
use super::preferences::Stored;
use crate::ride::log::Status;
use crate::ride::{bests, deleted, facts};
use device_api::bulk::ReadWrite;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, Table, Volume};
use firmware_services::{files, sector_log, store};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    #[default]
    Unread,
    Blank,
    Current,
    Foreign,
    Unusable,
}

impl Health {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Blank => "blank",
            Self::Current => "current",
            Self::Foreign => "foreign",
            Self::Unusable => "unusable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageHealth(pub [(&'static str, Health); 7]);

impl StorageHealth {
    pub fn settled(&self) -> bool {
        self.0.iter().all(|(_, health)| *health != Health::Unread)
    }

    pub fn foreign(&self) -> bool {
        self.0.iter().any(|(_, health)| *health == Health::Foreign)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CardProbe {
    table: Health,
    rides: Health,
    files: Health,
    records: Health,
    traces: Health,
}

impl CardProbe {
    fn all(health: Health) -> Self {
        Self {
            table: health,
            rides: health,
            files: health,
            records: health,
            traces: health,
        }
    }

    #[inline(never)]
    fn read(bulk: &mut impl ReadWrite) -> Self {
        let mut sector = [0u8; 512];
        let table = match volume::read(bulk, &mut sector) {
            Ok(Layout::Table(table)) => table,
            Ok(Layout::Blank) => return Self::all(Health::Blank),
            Err(volume::Error::Foreign) => {
                return Self {
                    table: Health::Foreign,
                    ..Self::all(Health::Unusable)
                };
            }
            Ok(Layout::Lost) | Err(_) => return Self::all(Health::Unusable),
        };
        let (files, records) = files_health(bulk, &table, &mut sector);
        Self {
            table: Health::Current,
            rides: volume_health(bulk, &table, &RIDES, |volume| {
                match sector_log::formatted(volume, crate::ride::bulk::TAG, &mut sector) {
                    Ok(true) => Health::Current,
                    Ok(false) => Health::Blank,
                    Err(sector_log::Error::Foreign) => Health::Foreign,
                    Err(_) => Health::Unusable,
                }
            }),
            files,
            records,
            traces: volume_health(bulk, &table, &STORE, |volume| {
                match store::mount(volume, &mut sector) {
                    Ok(_) => Health::Current,
                    Err(store::Error::Uninitialized) => Health::Blank,
                    Err(store::Error::Foreign) => Health::Foreign,
                    Err(_) => Health::Unusable,
                }
            }),
        }
    }
}

fn volume_health<B: ReadWrite>(
    bulk: &mut B,
    table: &Table,
    name: &volume::Name,
    read: impl FnOnce(&mut Volume<'_, B>) -> Health,
) -> Health {
    match table.find(name) {
        Some(extent) => read(&mut Volume::new(bulk, extent)),
        None => Health::Blank,
    }
}

fn files_health(
    bulk: &mut impl ReadWrite,
    table: &Table,
    sector: &mut files::Sector,
) -> (Health, Health) {
    let Some(extent) = table.find(&FILES) else {
        return (Health::Blank, Health::Blank);
    };
    let volume = &mut Volume::new(bulk, extent);
    let dir = match files::mount(volume, sector) {
        Ok(dir) => dir,
        Err(files::Error::Uninitialized) => return (Health::Blank, Health::Blank),
        Err(files::Error::Foreign) => return (Health::Foreign, Health::Unusable),
        Err(_) => return (Health::Unusable, Health::Unusable),
    };
    let tables = [
        (facts::TABLE, facts::STAMP),
        (deleted::TABLE, deleted::STAMP),
        (bests::FILE, bests::STAMP),
    ];
    let mut records = Health::Current;
    for index in 0..files::DIR_SECTORS {
        let Ok(entries) = dir.entries(volume, index, sector) else {
            return (Health::Current, Health::Unusable);
        };
        for entry in entries.iter().flatten() {
            let name = entry.name.as_str();
            let stamp = tables
                .iter()
                .find(|(file, _)| *file == name)
                .map(|(_, stamp)| *stamp)
                .or_else(|| crate::workout::history::stamp(name));
            let Some(stamp) = stamp else {
                continue;
            };
            let mut head = [0u8; 5];
            match dir.read(volume, entry, 0, &mut head, sector) {
                Ok(5) if stamp.another_version(&head) => {
                    return (Health::Current, Health::Foreign);
                }
                Ok(_) => {}
                Err(_) => records = Health::Unusable,
            }
        }
    }
    (Health::Current, records)
}

impl Runtime {
    pub(super) fn probe_storage(&mut self, bulk: &mut impl ReadWrite) {
        if self.card_probe.is_some() {
            return;
        }
        self.card_probe = match self.ride.bulk.status() {
            Status::Scanning => None,
            Status::Unsupported | Status::Unavailable | Status::Error => {
                Some(CardProbe::all(Health::Unusable))
            }
            _ => Some(CardProbe::read(bulk)),
        };
    }

    pub fn storage_health(&self) -> StorageHealth {
        let settings = match self.preferences.stored {
            Stored::Missing => Health::Blank,
            Stored::Current => Health::Current,
            Stored::Foreign => Health::Foreign,
        };
        let internal = match self.ride.internal.status() {
            Status::Scanning => Health::Unread,
            _ if self.ride.internal.holds_another_version() => Health::Foreign,
            Status::NeedsInit
            | Status::Error
            | Status::Unsupported
            | Status::Unavailable
            | Status::Corrupt => Health::Unusable,
            _ => Health::Current,
        };
        let probe = self.card_probe.unwrap_or_default();
        StorageHealth([
            ("settings", settings),
            ("internal", internal),
            ("table", probe.table),
            ("rides", probe.rides),
            ("files", probe.files),
            ("records", probe.records),
            ("traces", probe.traces),
        ])
    }
}
