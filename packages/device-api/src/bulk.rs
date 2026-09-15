//! Raw read-only media and explicitly marked application-owned block storage.

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

/// Relative application data region. Its reservation marker is excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnedInfo {
    pub start_sector: u64,
    pub sectors: u64,
    pub sector_size: u16,
}
impl OwnedInfo {
    pub fn address(self, relative: u64) -> Result<u64, Error> {
        if relative >= self.sectors {
            return Err(Error::Range);
        }
        self.start_sector.checked_add(relative).ok_or(Error::Range)
    }
}

/// No implicit initialization or formatting. Implementations validate ownership
/// before writes; completion means programming finished and readback matched.
/// A failed write may have changed the sector and must never be retried blindly.
pub trait ReadWrite {
    fn owned_info(&mut self) -> Result<OwnedInfo, Error>;
    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error>;
    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error>;
}
