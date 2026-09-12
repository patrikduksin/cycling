use cycling_os::{
    preferences::{self, Settings, Source},
    storage::{self, Journal, Sector},
};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::{FlashStorage, FlashStorageError};

pub use crate::device::storage::{RIDE_BASE, SETTINGS_BASE as BASE};
pub struct Loaded {
    pub settings: Settings,
    pub source: Source,
    pub sequence: Option<u32>,
    pub length: usize,
}

struct Backend<'d> {
    flash: FlashStorage<'d>,
}

impl Backend<'_> {
    fn ride_address(offset: usize, length: usize) -> Option<u32> {
        offset
            .checked_add(length)
            .filter(|end| *end <= cycling_os::ride_log::REGION_SIZE)
            .and_then(|_| u32::try_from(offset).ok())
            .and_then(|offset| RIDE_BASE.checked_add(offset))
    }

    fn ride_read_sector(
        &mut self,
        sector: usize,
        output: &mut cycling_os::ride_log::Sector,
    ) -> Result<(), FlashStorageError> {
        let offset = sector
            .checked_mul(cycling_os::ride_log::SECTOR_SIZE)
            .and_then(|offset| Self::ride_address(offset, output.0.len()))
            .ok_or(FlashStorageError::OutOfBounds)?;
        self.flash.read(offset, &mut output.0)
    }

    fn ride_read_slot(
        &mut self,
        slot: usize,
        output: &mut cycling_os::ride_log::Slot,
    ) -> Result<(), FlashStorageError> {
        let offset = slot
            .checked_mul(cycling_os::ride_log::SLOT_SIZE)
            .and_then(|offset| Self::ride_address(offset, output.0.len()))
            .ok_or(FlashStorageError::OutOfBounds)?;
        self.flash.read(offset, &mut output.0)
    }

    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), FlashStorageError> {
        let from = sector
            .checked_mul(cycling_os::ride_log::SECTOR_SIZE)
            .and_then(|offset| Self::ride_address(offset, cycling_os::ride_log::SECTOR_SIZE))
            .ok_or(FlashStorageError::OutOfBounds)?;
        self.flash
            .erase(from, from + cycling_os::ride_log::SECTOR_SIZE as u32)
    }

    fn ride_write_slot(
        &mut self,
        slot: usize,
        data: &cycling_os::ride_log::Slot,
    ) -> Result<(), FlashStorageError> {
        let offset = slot
            .checked_mul(cycling_os::ride_log::SLOT_SIZE)
            .and_then(|offset| Self::ride_address(offset, data.0.len()))
            .ok_or(FlashStorageError::OutOfBounds)?;
        self.flash.write(offset, &data.0)
    }

    fn ride_commit_slot(
        &mut self,
        slot: usize,
        commit: &cycling_os::ride_log::Commit,
    ) -> Result<(), FlashStorageError> {
        let relative = slot
            .checked_mul(cycling_os::ride_log::SLOT_SIZE)
            .and_then(|offset| offset.checked_add(cycling_os::ride_log::SLOT_SIZE - 4))
            .and_then(|offset| Self::ride_address(offset, commit.0.len()))
            .ok_or(FlashStorageError::OutOfBounds)?;
        self.flash.write(relative, &commit.0)
    }
}

impl storage::Flash for Backend<'_> {
    type Error = FlashStorageError;

    fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
        self.flash
            .read(BASE + (sector * storage::SECTOR_SIZE) as u32, &mut output.0)
    }

    fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
        let from = BASE + (sector * storage::SECTOR_SIZE) as u32;
        self.flash.erase(from, from + storage::SECTOR_SIZE as u32)
    }

    fn write_sector(&mut self, sector: usize, data: &Sector) -> Result<(), Self::Error> {
        self.flash
            .write(BASE + (sector * storage::SECTOR_SIZE) as u32, &data.0)
    }

    fn commit_sector(&mut self, sector: usize, commit: &[u8; 4]) -> Result<(), Self::Error> {
        self.flash
            .write(BASE + (sector * storage::SECTOR_SIZE + 20) as u32, commit)
    }
}

pub struct Store {
    journal: Journal<Backend<'static>>,
}

impl Store {
    pub fn open(
        flash: esp_hal::peripherals::FLASH<'static>,
    ) -> (Self, Result<Loaded, storage::Error<FlashStorageError>>) {
        let mut store = Self {
            journal: Journal::new(Backend {
                flash: FlashStorage::new(flash),
            }),
        };
        let loaded = store.load();
        (store, loaded)
    }

    pub fn load(&mut self) -> Result<Loaded, storage::Error<FlashStorageError>> {
        let mut payload = [0u8; storage::CAPACITY];
        let record = self.journal.load(&mut payload)?;
        let bytes = record.map(|record| &payload[..record.length]);
        let (settings, source) = preferences::decode(bytes);
        Ok(Loaded {
            settings,
            source,
            sequence: record.map(|record| record.sequence),
            length: record.map(|record| record.length).unwrap_or(0),
        })
    }

    pub fn save(
        &mut self,
        settings: Settings,
    ) -> Result<storage::Record, storage::Error<FlashStorageError>> {
        match self.journal.save(&settings.encode()) {
            Ok(record) => Ok(record),
            Err(error) => match self.load() {
                Ok(loaded) if loaded.source == Source::Current && loaded.settings == settings => {
                    Ok(storage::Record {
                        sequence: loaded.sequence.unwrap(),
                        length: loaded.length,
                    })
                }
                _ => Err(error),
            },
        }
    }

    pub fn ride_read_sector(
        &mut self,
        sector: usize,
        output: &mut cycling_os::ride_log::Sector,
    ) -> Result<(), FlashStorageError> {
        self.journal.flash_mut().ride_read_sector(sector, output)
    }

    pub fn ride_read_slot(
        &mut self,
        slot: usize,
        output: &mut cycling_os::ride_log::Slot,
    ) -> Result<(), FlashStorageError> {
        self.journal.flash_mut().ride_read_slot(slot, output)
    }

    pub fn ride_erase_sector(&mut self, sector: usize) -> Result<(), FlashStorageError> {
        self.journal.flash_mut().ride_erase_sector(sector)
    }

    pub fn ride_write_slot(
        &mut self,
        slot: usize,
        data: &cycling_os::ride_log::Slot,
    ) -> Result<(), FlashStorageError> {
        self.journal.flash_mut().ride_write_slot(slot, data)
    }

    pub fn ride_commit_slot(
        &mut self,
        slot: usize,
        commit: &cycling_os::ride_log::Commit,
    ) -> Result<(), FlashStorageError> {
        self.journal.flash_mut().ride_commit_slot(slot, commit)
    }
}
