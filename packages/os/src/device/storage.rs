//! Sole physical flash owner. Reservations are fixed, never caller-selected.
//! No access to stock, boot metadata, eFuses or the vendor filesystem is exposed.

use cycling_os::storage::{self, AccessError, Geometry, OwnedFlash, Region};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::{FlashStorage, FlashStorageError};

pub const SETTINGS_BASE: u32 = 0x00e9_8000;
pub const DATA_BASE: u32 = 0x00d9_8000;
const DATA_SIZE: usize = 0x10_0000;
const SETTINGS_SIZE: usize = 2 * 4096;
pub type Error = AccessError<FlashStorageError>;

pub struct Backend<'d> {
    flash: FlashStorage<'d>,
}

impl<'d> Backend<'d> {
    pub fn new(flash: esp_hal::peripherals::FLASH<'d>) -> Self {
        Self {
            flash: FlashStorage::new(flash),
        }
    }
    fn address(
        base: u32,
        capacity: usize,
        offset: usize,
        length: usize,
        alignment: usize,
    ) -> Result<u32, Error> {
        storage::checked_range(capacity, offset, length, alignment)?;
        let relative = u32::try_from(offset).map_err(|_| AccessError::OutOfBounds)?;
        base.checked_add(relative).ok_or(AccessError::OutOfBounds)
    }
    fn region(region: Region) -> (u32, usize) {
        match region {
            Region::Configuration => (SETTINGS_BASE, SETTINGS_SIZE),
            Region::Data => (DATA_BASE, DATA_SIZE),
        }
    }
}

impl OwnedFlash for Backend<'_> {
    type Error = Error;
    fn availability(&self, _region: Region) -> cycling_os::capabilities::Availability {
        if super::services::power::ACCESS.closed() {
            cycling_os::capabilities::Availability::Initializing
        } else {
            cycling_os::capabilities::Availability::Ready
        }
    }
    fn geometry(&self, region: Region) -> Geometry {
        Geometry {
            capacity: Self::region(region).1,
            program_size: FlashStorage::WRITE_SIZE,
            erase_size: FlashStorage::ERASE_SIZE,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| AccessError::Unavailable)?;
        let (base, capacity) = Self::region(region);
        let address = Self::address(base, capacity, offset, output.len(), 1)?;
        // Adapt arbitrary byte reads to the physical four-byte read granularity.
        // Every aligned word remains inside this sector-aligned reservation.
        let mut address = address;
        let mut output = output;
        if output.is_empty() {
            return Ok(());
        }
        #[repr(align(4))]
        struct Word([u8; 4]);
        let mut word = Word([0; 4]);
        let head = address as usize % 4;
        if head != 0 {
            self.flash
                .read(address - head as u32, &mut word.0)
                .map_err(AccessError::Device)?;
            let count = output.len().min(4 - head);
            output[..count].copy_from_slice(&word.0[head..head + count]);
            address += count as u32;
            output = &mut output[count..];
        }
        let middle = output.len() / 4 * 4;
        if middle != 0 {
            self.flash
                .read(address, &mut output[..middle])
                .map_err(AccessError::Device)?;
            address += middle as u32;
            output = &mut output[middle..];
        }
        if !output.is_empty() {
            self.flash
                .read(address, &mut word.0)
                .map_err(AccessError::Device)?;
            output.copy_from_slice(&word.0[..output.len()]);
        }
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| AccessError::Unavailable)?;
        let (base, capacity) = Self::region(region);
        let address = Self::address(
            base,
            capacity,
            offset,
            bytes.len(),
            FlashStorage::WRITE_SIZE,
        )?;
        self.flash
            .write(address, bytes)
            .map_err(AccessError::Device)
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| AccessError::Unavailable)?;
        let (base, capacity) = Self::region(region);
        let address = Self::address(base, capacity, offset, length, FlashStorage::ERASE_SIZE)?;
        self.flash
            .erase(address, address + length as u32)
            .map_err(AccessError::Device)
    }
}
