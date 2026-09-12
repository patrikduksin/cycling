//! Ride layout adapter over core-owned bytes. Core never interprets these records.
use super::ride_log::{self, Commit, Sector, Slot};
use crate::storage::{AccessError, OwnedFlash, Store, checked_range};

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
    checked_range(ride_log::REGION_SIZE, offset, length, 1)?;
    Ok(offset)
}

impl<B: OwnedFlash> RideStorage for Store<B> {
    type Error = AccessError<B::Error>;
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
        self.read(
            offset(sector, ride_log::SECTOR_SIZE, 0, output.0.len())?,
            &mut output.0,
        )
        .map_err(AccessError::Device)
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), Self::Error> {
        self.read(
            offset(slot, ride_log::SLOT_SIZE, 0, output.0.len())?,
            &mut output.0,
        )
        .map_err(AccessError::Device)
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
        self.erase(
            offset(sector, ride_log::SECTOR_SIZE, 0, ride_log::SECTOR_SIZE)?,
            ride_log::SECTOR_SIZE,
        )
        .map_err(AccessError::Device)
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), Self::Error> {
        self.program(offset(slot, ride_log::SLOT_SIZE, 0, data.0.len())?, &data.0)
            .map_err(AccessError::Device)
    }
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), Self::Error> {
        self.program(
            offset(
                slot,
                ride_log::SLOT_SIZE,
                ride_log::SLOT_SIZE - 4,
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
    use crate::storage::{self, Geometry};
    struct Backend;
    impl storage::Flash for Backend {
        type Error = ();
        fn read_sector(&mut self, _: usize, _: &mut storage::Sector) -> Result<(), ()> {
            panic!("unexpected settings read")
        }
        fn erase_sector(&mut self, _: usize) -> Result<(), ()> {
            panic!("unexpected settings erase")
        }
        fn write_sector(&mut self, _: usize, _: &storage::Sector) -> Result<(), ()> {
            panic!("unexpected settings write")
        }
        fn commit_sector(&mut self, _: usize, _: &[u8; 4]) -> Result<(), ()> {
            panic!("unexpected settings commit")
        }
    }
    impl OwnedFlash for Backend {
        fn geometry(&self) -> Geometry {
            Geometry {
                capacity: ride_log::REGION_SIZE,
                program_size: 4,
                erase_size: 4096,
            }
        }
        fn read_data(&mut self, _: usize, _: &mut [u8]) -> Result<(), ()> {
            panic!("out of range read reached media")
        }
        fn program_data(&mut self, _: usize, _: &[u8]) -> Result<(), ()> {
            panic!("out of range program reached media")
        }
        fn erase_data(&mut self, _: usize, _: usize) -> Result<(), ()> {
            panic!("out of range erase reached media")
        }
    }
    #[test]
    fn invalid_ride_indices_never_reach_the_backend() {
        let mut store = Store::new(Backend);
        for index in [ride_log::SLOTS, usize::MAX] {
            assert_eq!(
                store.ride_read_slot(index, &mut Slot::default()),
                Err(AccessError::OutOfBounds)
            );
            assert_eq!(
                store.ride_write_slot(index, &Slot::default()),
                Err(AccessError::OutOfBounds)
            );
            assert_eq!(
                store.ride_commit_slot(index, &ride_log::commit_word()),
                Err(AccessError::OutOfBounds)
            );
        }
        for index in [ride_log::SECTORS, usize::MAX] {
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
