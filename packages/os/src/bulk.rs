//! Read-only block media. This does not grant application write ownership.

pub const SECTOR_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Unavailable,
    Range,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub sectors: u64,
    pub sector_size: u16,
    pub clock_hz: u32,
    pub bus_width: u8,
    pub reads: u32,
    pub failures: u32,
    pub last_read_us: u64,
}

/// Reads complete synchronously, including DMA cleanup on failure. Failed reads
/// never publish partial bytes. No write, erase, format or partition switch exists.
pub trait Read {
    fn info(&self) -> Result<Info, Error>;
    fn read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error>;
    fn clock(&mut self, hz: u32) -> Result<(), Error>;
    fn recover(&mut self) -> Result<(), Error>;
}

pub fn address(sector: u64, sectors: u64, high_capacity: bool) -> Result<u32, Error> {
    if sector >= sectors {
        return Err(Error::Range);
    }
    let address = if high_capacity {
        sector
    } else {
        sector.checked_mul(SECTOR_SIZE as u64).ok_or(Error::Range)?
    };
    address.try_into().map_err(|_| Error::Range)
}

/// Validate a bounded USB chunk before any device read.
pub fn chunk(offset: usize, length: usize) -> Result<core::ops::Range<usize>, Error> {
    if length == 0 || length > 256 {
        return Err(Error::Range);
    }
    let end = offset.checked_add(length).ok_or(Error::Range)?;
    if end > SECTOR_SIZE {
        return Err(Error::Range);
    }
    Ok(offset..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_wrap_or_access_past_media() {
        assert_eq!(address(0, 0, true), Err(Error::Range));
        assert_eq!(address(7, 8, true), Ok(7));
        assert_eq!(address(8, 8, true), Err(Error::Range));
        assert_eq!(address(u64::MAX - 1, u64::MAX, false), Err(Error::Range));
        assert_eq!(
            address(u32::MAX as u64 + 1, u64::MAX, true),
            Err(Error::Range)
        );
        assert_eq!(address(7, 8, false), Ok(3584));
        assert_eq!(chunk(256, 256), Ok(256..512));
        for (offset, length) in [(0, 0), (0, 257), (511, 2), (usize::MAX, 2)] {
            assert_eq!(chunk(offset, length), Err(Error::Range));
        }
    }
}
