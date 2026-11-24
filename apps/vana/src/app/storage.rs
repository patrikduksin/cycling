use super::Runtime;
use super::preferences::Stored;
use crate::ride::Contents;
use crate::ride::log::Status;
use crate::workout::history;
use device_api::bulk::ReadWrite;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, Table, Volume};
use firmware_services::{files, store};

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

/// The discriminants are persisted: each is a store's bit in the intent
/// journal's `Scope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    Table = 0,
    Rides = 1,
    Files = 2,
    Records = 3,
    /// The STORE volume.
    Traces = 4,
    Internal = 5,
    Settings = 6,
}

impl Store {
    pub const ALL: [Self; 7] = [
        Self::Table,
        Self::Rides,
        Self::Files,
        Self::Records,
        Self::Traces,
        Self::Internal,
        Self::Settings,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Rides => "rides",
            Self::Files => "files",
            Self::Records => "records",
            Self::Traces => "traces",
            Self::Internal => "internal",
            Self::Settings => "settings",
        }
    }

    pub(super) const fn on_card(self) -> bool {
        !matches!(self, Self::Internal | Self::Settings)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StorageHealth([Health; Store::ALL.len()]);

impl StorageHealth {
    pub fn of(&self, store: Store) -> Health {
        self.0[store as usize]
    }

    fn set(&mut self, store: Store, health: Health) {
        self.0[store as usize] = health;
    }

    pub fn iter(&self) -> impl Iterator<Item = (Store, Health)> + '_ {
        Store::ALL.into_iter().map(|store| (store, self.of(store)))
    }

    pub fn settled(&self) -> bool {
        !self.0.contains(&Health::Unread)
    }

    pub fn foreign(&self) -> bool {
        self.0.contains(&Health::Foreign)
    }

    fn card(health: Health) -> Self {
        let mut all = Self::default();
        for store in Store::ALL.into_iter().filter(|store| store.on_card()) {
            all.set(store, health);
        }
        all
    }

    #[inline(never)]
    fn read_card(bulk: &mut impl ReadWrite) -> Self {
        let mut sector = [0u8; 512];
        let table = match volume::read(bulk, &mut sector) {
            Ok(Layout::Table(table)) => table,
            Ok(Layout::Blank) => return Self::card(Health::Blank),
            Err(volume::Error::Foreign) => {
                let mut health = Self::card(Health::Unusable);
                health.set(Store::Table, Health::Foreign);
                return health;
            }
            Ok(Layout::Lost) | Err(_) => return Self::card(Health::Unusable),
        };
        let mut health = Self::default();
        for store in Store::ALL {
            let held = match store {
                Store::Table => Health::Current,
                Store::Rides => {
                    in_volume(
                        bulk,
                        &table,
                        &RIDES,
                        |rides| match crate::ride::bulk::formatted(rides, &mut sector) {
                            Ok(true) => Health::Current,
                            Ok(false) => Health::Blank,
                            Err(firmware_services::sector_log::Error::Foreign) => Health::Foreign,
                            Err(_) => Health::Unusable,
                        },
                    )
                }
                Store::Files => in_volume(bulk, &table, &FILES, |files| {
                    match files::mount(files, &mut sector) {
                        Ok(_) => Health::Current,
                        Err(files::Error::Uninitialized) => Health::Blank,
                        Err(files::Error::Foreign) => Health::Foreign,
                        Err(_) => Health::Unusable,
                    }
                }),
                Store::Records => match health.of(Store::Files) {
                    Health::Current => records(bulk),
                    Health::Blank => Health::Blank,
                    _ => Health::Unusable,
                },
                Store::Traces => in_volume(bulk, &table, &STORE, |traces| {
                    match store::mount(traces, &mut sector) {
                        Ok(_) => Health::Current,
                        Err(store::Error::Uninitialized) => Health::Blank,
                        Err(store::Error::Foreign) => Health::Foreign,
                        Err(_) => Health::Unusable,
                    }
                }),
                Store::Internal | Store::Settings => continue,
            };
            health.set(store, held);
        }
        health
    }
}

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

#[inline(never)]
fn records(bulk: &mut impl ReadWrite) -> Health {
    let held = [history::contents(bulk).ok()];
    if held.contains(&Some(Contents::Foreign)) {
        Health::Foreign
    } else if held.contains(&None) {
        Health::Unusable
    } else {
        Health::Current
    }
}

impl Runtime {
    pub(super) fn probe_storage(&mut self, bulk: &mut impl ReadWrite) {
        if self.card_probe.is_some() {
            return;
        }
        self.card_probe = match self.ride.bulk().status() {
            Status::Scanning => None,
            Status::Unsupported | Status::Unavailable | Status::Error => {
                Some(StorageHealth::card(Health::Unusable))
            }
            _ => Some(StorageHealth::read_card(bulk)),
        };
    }

    pub fn storage_health(&self) -> StorageHealth {
        let mut health = self.card_probe.unwrap_or_default();
        health.set(
            Store::Internal,
            match self.ride.internal().status() {
                Status::Scanning => Health::Unread,
                Status::NeedsInit => Health::Blank,
                Status::Foreign => Health::Foreign,
                Status::Error | Status::Unsupported | Status::Unavailable | Status::Corrupt => {
                    Health::Unusable
                }
                _ => Health::Current,
            },
        );
        health.set(
            Store::Settings,
            match self.preferences.stored {
                Stored::Missing | Stored::Outdated => Health::Blank,
                Stored::Current => Health::Current,
                Stored::Foreign => Health::Foreign,
            },
        );
        health
    }
}
