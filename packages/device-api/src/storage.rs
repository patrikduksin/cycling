/// Physical geometry of an exclusively owned byte reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub capacity: usize,
    pub program_size: usize,
    pub erase_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessError<E> {
    Unavailable,
    OutOfBounds,
    Unaligned,
    Device(E),
}

/// Validate relative ranges before translating them to physical addresses.
pub fn checked_range<E>(
    capacity: usize,
    offset: usize,
    length: usize,
    alignment: usize,
) -> Result<(), AccessError<E>> {
    if offset.checked_add(length).is_none_or(|end| end > capacity) {
        return Err(AccessError::OutOfBounds);
    }
    if alignment == 0 || !offset.is_multiple_of(alignment) || !length.is_multiple_of(alignment) {
        return Err(AccessError::Unaligned);
    }
    Ok(())
}

/// Device-owned reservations. These names select disjoint bounds, not schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    Configuration,
    Data,
    /// The device's copy of its last panic record, kept across power-off.
    Crash,
    /// Where the application records a wipe of its stores before the first
    /// write, outside every store, so a wipe cut by power is finished at boot.
    Intent,
}

/// Sole physical owner of independently bounded reservations. Every operation
/// uses relative byte offsets; no caller can select a physical flash address.
/// Return means completed hardware access, including on failure. A failed
/// mutation may have changed bytes: inspect/rescan before deciding to retry.
pub trait OwnedFlash {
    type Error;
    fn availability(&self, region: Region) -> crate::observation::Availability;
    fn geometry(&self, region: Region) -> Geometry;
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8])
    -> Result<(), Self::Error>;
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Self::Error>;
}

impl<T: OwnedFlash + ?Sized> OwnedFlash for &mut T {
    type Error = T::Error;
    fn availability(&self, region: Region) -> crate::observation::Availability {
        (**self).availability(region)
    }
    fn geometry(&self, region: Region) -> Geometry {
        (**self).geometry(region)
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), T::Error> {
        (**self).read(region, offset, output)
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), T::Error> {
        (**self).program(region, offset, bytes)
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), T::Error> {
        (**self).erase(region, offset, length)
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    #[test]
    fn reservations_reject_overflow_boundary_crossing_and_alignment() {
        assert_eq!(checked_range::<()>(8192, 8192, 0, 1), Ok(()));
        assert_eq!(checked_range::<()>(8192, 8191, 1, 1), Ok(()));
        assert_eq!(
            checked_range::<()>(8192, 8192, 1, 1),
            Err(AccessError::OutOfBounds)
        );
        assert_eq!(
            checked_range::<()>(8192, usize::MAX, 2, 1),
            Err(AccessError::OutOfBounds)
        );
        assert_eq!(
            checked_range::<()>(8192, 1, 4, 4),
            Err(AccessError::Unaligned)
        );
        assert_eq!(
            checked_range::<()>(8192, 0, 4095, 4096),
            Err(AccessError::Unaligned)
        );
        assert_eq!(checked_range::<()>(8192, 4096, 4096, 4096), Ok(()));
    }
}
