use cycling_os::storage::{self, Journal, Sector};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::{FlashStorage, FlashStorageError};

pub const BASE: u32 = 0x00e9_8000;
const INITIAL_RECORD: &[u8] = b"cycling\x01";

pub struct Report {
    pub initialized: bool,
    pub sequence: u32,
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

pub fn init(
    flash: esp_hal::peripherals::FLASH<'static>,
) -> Result<Report, storage::Error<FlashStorageError>> {
    let backend = Backend {
        flash: FlashStorage::new(flash),
    };
    let mut journal = Journal::new(backend);
    let mut payload = [0u8; storage::CAPACITY];
    match journal.load(&mut payload)? {
        Some(record) => Ok(Report {
            initialized: false,
            sequence: record.sequence,
            length: record.length,
        }),
        None => {
            let record = journal.save(INITIAL_RECORD)?;
            Ok(Report {
                initialized: true,
                sequence: record.sequence,
                length: record.length,
            })
        }
    }
}
