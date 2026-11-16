//! What each of VANA's stores holds: the one answer the FORMAT offer, the
//! ready line and `FORMAT` read.
//!
//! The settings and the internal journal are read from their owners. The
//! card's volumes are read once, after the bulk recorder's scan, by
//! [`Probe`]: the table, then the head of each volume, then the head of the
//! tables VANA writes into FILES. Files the host writes (course, turns,
//! workouts) are the host's to replace and never raise the offer; nor does
//! the terrain pack.
use super::Runtime;
use super::preferences::Stored;
use crate::ride::log::Status;
use device_api::bulk::ReadWrite;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, Table, Volume};
use firmware_services::{files, sector_log, store};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    /// Not read yet this boot.
    #[default]
    Unread,
    Blank,
    Current,
    /// Written by another version: kept, and the device offers FORMAT.
    Foreign,
    /// Damaged, unreachable or absent. Not another version's, so no offer.
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

/// Every store's health, by the name `FORMAT` reports it under.
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

/// What the card's volumes hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Probe {
    table: Health,
    rides: Health,
    files: Health,
    /// The tables VANA writes into FILES.
    records: Health,
    traces: Health,
}

impl Probe {
    fn all(health: Health) -> Self {
        Self {
            table: health,
            rides: health,
            files: health,
            records: health,
            traces: health,
        }
    }

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
            rides: in_volume(bulk, &table, &RIDES, |volume| {
                match sector_log::formatted(volume, crate::ride::bulk::TAG, &mut sector) {
                    Ok(true) => Health::Current,
                    Ok(false) => Health::Blank,
                    Err(sector_log::Error::Foreign) => Health::Foreign,
                    Err(_) => Health::Unusable,
                }
            }),
            files,
            records,
            traces: in_volume(bulk, &table, &STORE, |volume| {
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

/// A volume the plan leaves out, as small media do, is no store to format.
fn in_volume<B: ReadWrite>(
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
    let mut records = Health::Current;
    for (name, magic, version) in [
        crate::ride::facts::STAMP,
        crate::ride::deleted::STAMP,
        crate::ride::bests::STAMP,
    ] {
        let mut head = [0u8; 5];
        let read = files::Name::new(name)
            .and_then(|name| dir.find(volume, &name, sector))
            .and_then(|entry| match entry {
                Some(entry) => dir.read(volume, &entry, 0, &mut head, sector),
                None => Ok(0),
            });
        match read {
            Ok(5) if head[..4] == magic[..] && head[4] != version => return (Health::Current, Health::Foreign),
            Ok(_) => {}
            Err(_) => records = Health::Unusable,
        }
    }
    (Health::Current, records)
}

impl Runtime {
    /// Read the card's volumes once the bulk recorder has scanned them. A
    /// card that is absent or failing is not read again.
    pub(super) fn probe_storage(&mut self, bulk: &mut impl ReadWrite) {
        if self.storage.is_some() {
            return;
        }
        self.storage = match self.ride.bulk.status() {
            Status::Scanning => None,
            Status::Unsupported | Status::Unavailable | Status::Error => {
                Some(Probe::all(Health::Unusable))
            }
            _ => Some(Probe::read(bulk)),
        };
    }

    pub fn storage_health(&self) -> StorageHealth {
        let settings = match self.preferences.stored {
            Stored::Missing => Health::Blank,
            Stored::Current => Health::Current,
            Stored::Foreign => Health::Foreign,
        };
        // The internal journal never reads blank: `NeedsInit` there is
        // occupied data no slot of this version explains.
        let internal = match self.ride.internal.status() {
            Status::Scanning => Health::Unread,
            Status::NeedsInit => Health::Foreign,
            Status::Error | Status::Unsupported | Status::Unavailable | Status::Corrupt => {
                Health::Unusable
            }
            _ => Health::Current,
        };
        let probe = self.storage.unwrap_or_default();
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
