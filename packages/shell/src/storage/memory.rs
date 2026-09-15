//! Flash semantics for shell persistence tests.
use device_api::storage::{Geometry, OwnedFlash, Region, checked_range};
use firmware_services::storage::{SECTOR_COUNT, SECTOR_SIZE};

pub(super) struct Memory {
    pub(super) bytes: [[u8; SECTOR_SIZE]; SECTOR_COUNT],
    pub(super) operations: usize,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            bytes: [[0xff; SECTOR_SIZE]; SECTOR_COUNT],
            operations: 0,
        }
    }
}
impl OwnedFlash for Memory {
    type Error = ();
    fn geometry(&self, region: Region) -> Geometry {
        Geometry {
            capacity: if region == Region::Configuration {
                SECTOR_SIZE * SECTOR_COUNT
            } else {
                0
            },
            program_size: 4,
            erase_size: SECTOR_SIZE,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
        checked_range::<()>(self.geometry(region).capacity, offset, output.len(), 1)
            .map_err(|_| ())?;
        self.operations += 1;
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = self.bytes[(offset + index) / SECTOR_SIZE][(offset + index) % SECTOR_SIZE];
        }
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, input: &[u8]) -> Result<(), ()> {
        checked_range::<()>(self.geometry(region).capacity, offset, input.len(), 4)
            .map_err(|_| ())?;
        self.operations += 1;
        for (index, byte) in input.iter().enumerate() {
            self.bytes[(offset + index) / SECTOR_SIZE][(offset + index) % SECTOR_SIZE] &= *byte;
        }
        Ok(())
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
        checked_range::<()>(self.geometry(region).capacity, offset, length, SECTOR_SIZE)
            .map_err(|_| ())?;
        self.operations += 1;
        for index in offset..offset + length {
            self.bytes[index / SECTOR_SIZE][index % SECTOR_SIZE] = 0xff;
        }
        Ok(())
    }
}
