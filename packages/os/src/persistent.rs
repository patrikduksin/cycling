use cycling_os::{
    preferences::{self, Settings, Source},
    storage::{self, Journal, Sector},
};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::{FlashStorage, FlashStorageError};

pub const BASE: u32 = 0x00e9_8000;
pub struct Loaded {
    pub settings: Settings,
    pub source: Source,
    pub sequence: Option<u32>,
    pub length: usize,
}

struct Backend<'d> {
    flash: FlashStorage<'d>,
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
}
