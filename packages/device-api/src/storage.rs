use crate::capability::capability;

/// Physical geometry of an exclusively owned byte reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub capacity: usize,
    pub program_size: usize,
    pub erase_size: usize,
    /// The bytes survive power-off. A memory-backed reservation holds the
    /// same format but starts erased on every boot.
    pub durable: bool,
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
    /// The rider's screen layouts, apart from every other store, so a bad
    /// record resets only the screens.
    Screens,
}

impl Region {
    pub const ALL: [Region; 5] = [
        Self::Configuration,
        Self::Data,
        Self::Crash,
        Self::Intent,
        Self::Screens,
    ];
}

/// Sole physical owner of independently bounded reservations. Every operation
/// uses relative byte offsets; no caller can select a physical flash address.
/// Return means completed hardware access, including on failure. A failed
/// mutation may have changed bytes: inspect/rescan before deciding to retry.
#[capability(id = 17)]
pub trait OwnedFlash {
    type Error;
    #[observe(keys = Region::ALL)]
    fn availability(&self, region: Region) -> crate::observation::Availability;
    #[observe(keys = Region::ALL)]
    fn geometry(&self, region: Region) -> Geometry;
    #[media(read)]
    fn read(
        &mut self,
        #[at] region: Region,
        #[at] offset: usize,
        #[out] output: &mut [u8],
    ) -> Result<(), Self::Error>;
    #[media(write)]
    fn program(
        &mut self,
        #[at] region: Region,
        #[at] offset: usize,
        #[input] bytes: &[u8],
    ) -> Result<(), Self::Error>;
    #[media(erase)]
    fn erase(
        &mut self,
        #[at] region: Region,
        #[at] offset: usize,
        #[len] length: usize,
    ) -> Result<(), Self::Error>;
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
