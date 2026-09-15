//! Ride layout adapter over an exclusively borrowed byte reservation.

use crate::ride::log::Commit;
use crate::ride::log::Sector;
use crate::ride::log::Slot;
use device_api::storage::AccessError;
use device_api::storage::OwnedFlash;
use device_api::storage::checked_range;
use firmware_services::storage::RegionAccess;

pub trait RideStorage {
    type Error;
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error>;
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), Self::Error>;
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), Self::Error>;
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), Self::Error>;
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), Self::Error>;
}

fn offset<E>(
    index: usize,
    stride: usize,
    within: usize,
    length: usize,
) -> Result<usize, AccessError<E>> {
    let offset = index
        .checked_mul(stride)
        .and_then(|v| v.checked_add(within))
        .ok_or(AccessError::OutOfBounds)?;
    checked_range(crate::ride::log::REGION_SIZE, offset, length, 1)?;
    Ok(offset)
}

impl<B: OwnedFlash> RideStorage for RegionAccess<'_, B> {
    type Error = AccessError<B::Error>;
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
        self.read(
            offset(sector, crate::ride::log::SECTOR_SIZE, 0, output.0.len())?,
            &mut output.0,
        )
        .map_err(AccessError::Device)
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), Self::Error> {
        self.read(
            offset(slot, crate::ride::log::SLOT_SIZE, 0, output.0.len())?,
            &mut output.0,
        )
        .map_err(AccessError::Device)
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
        self.erase(
            offset(
                sector,
                crate::ride::log::SECTOR_SIZE,
                0,
                crate::ride::log::SECTOR_SIZE,
            )?,
            crate::ride::log::SECTOR_SIZE,
        )
        .map_err(AccessError::Device)
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), Self::Error> {
        self.program(
            offset(slot, crate::ride::log::SLOT_SIZE, 0, data.0.len())?,
            &data.0,
        )
        .map_err(AccessError::Device)
    }
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), Self::Error> {
        self.program(
            offset(
                slot,
                crate::ride::log::SLOT_SIZE,
                crate::ride::log::SLOT_SIZE - 4,
                commit.0.len(),
            )?,
            &commit.0,
        )
        .map_err(AccessError::Device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use device_api::storage::Geometry;
    struct Backend;
    impl OwnedFlash for Backend {
        type Error = ();
        fn geometry(&self, _: device_api::storage::Region) -> Geometry {
            Geometry {
                capacity: crate::ride::log::REGION_SIZE,
                program_size: 4,
                erase_size: 4096,
            }
        }
        fn read(
            &mut self,
            _: device_api::storage::Region,
            _: usize,
            _: &mut [u8],
        ) -> Result<(), ()> {
            panic!("out of range read reached media")
        }
        fn program(
            &mut self,
            _: device_api::storage::Region,
            _: usize,
            _: &[u8],
        ) -> Result<(), ()> {
            panic!("out of range program reached media")
        }
        fn erase(&mut self, _: device_api::storage::Region, _: usize, _: usize) -> Result<(), ()> {
            panic!("out of range erase reached media")
        }
    }
    #[test]
    fn invalid_ride_indices_never_reach_the_backend() {
        let mut backend = Backend;
        let mut store = RegionAccess::new(&mut backend, device_api::storage::Region::Data);
        for index in [crate::ride::log::SLOTS, usize::MAX] {
            assert_eq!(
                store.ride_read_slot(index, &mut Slot::default()),
                Err(AccessError::OutOfBounds)
            );
            assert_eq!(
                store.ride_write_slot(index, &Slot::default()),
                Err(AccessError::OutOfBounds)
            );
            assert_eq!(
                store.ride_commit_slot(index, &crate::ride::log::commit_word()),
                Err(AccessError::OutOfBounds)
            );
        }
        for index in [crate::ride::log::SECTORS, usize::MAX] {
            assert_eq!(
                store.ride_read_sector(index, &mut Sector::default()),
                Err(AccessError::OutOfBounds)
            );
            assert_eq!(
                store.ride_erase_sector(index),
                Err(AccessError::OutOfBounds)
            );
        }
    }
}
