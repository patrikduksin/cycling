//! A two-sector, power-loss-tolerant journal for small records.
use crate::checksum::Crc32;
use crate::le::{get_u32, put_u16, put_u32};
use device_api::storage::*;

pub const SECTOR_SIZE: usize = 4096;
pub const SECTOR_COUNT: usize = 2;
pub const HEADER_SIZE: usize = 24;
pub const CAPACITY: usize = SECTOR_SIZE - HEADER_SIZE;

const MAGIC: [u8; 4] = *b"C606";
const VERSION: u8 = 2;
const VERSION_AT: usize = 4;
const HEADER_SIZE_AT: usize = 6;
const SEQUENCE_AT: usize = 8;
const LENGTH_AT: usize = 12;
const CHECKSUM_AT: usize = 16;
const HEAD: usize = LENGTH_AT;
const COMMITTED: u32 = 0x434f_4d54;
const COMMIT_OFFSET: usize = 20;
/// Flash programs a page's bits in parallel, and pages in address order.
const PAGE_SIZE: usize = 256;

#[repr(C, align(4))]
pub struct Sector(pub [u8; SECTOR_SIZE]);

impl Default for Sector {
    fn default() -> Self {
        Self([0; SECTOR_SIZE])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub sequence: u32,
    pub length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Flash(E),
    PayloadTooLarge,
    OutputTooSmall,
    ReadbackFailed,
    /// No committed supported record exists, and the occupied sectors are not
    /// one torn first save beside a blank sector.
    /// Preserve the bytes for inspection instead of treating them as unused flash.
    UnrecognizedJournal,
    UnsupportedGeometry,
    /// The sector holds no record whose body verifies under a commit word
    /// this journal began.
    NoRecord,
}

/// What one of the journal's sectors holds, read on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Held {
    Blank,
    Committed(Record),
    /// Anything else: a torn save, another format, or damage.
    Other,
}

/// Where the next save goes: after the newest committed record, or into a
/// free sector with no committed record to follow.
enum Scan {
    Newest(usize, Record),
    Free { sector: usize, sequence: u32 },
}

pub struct Journal<F> {
    pub(crate) flash: F,
    scratch: Sector,
    region: Region,
}

impl<F: OwnedFlash> Journal<F> {
    pub fn new(flash: F, region: Region) -> Self {
        Self {
            flash,
            scratch: Sector::default(),
            region,
        }
    }

    /// Lend the sole backend for a disjoint, bounds-checked storage operation.
    pub fn flash_mut(&mut self) -> &mut F {
        &mut self.flash
    }

    pub fn load(&mut self, output: &mut [u8]) -> Result<Option<Record>, Error<F::Error>> {
        let Scan::Newest(sector, record) = self.scan()? else {
            return Ok(None);
        };
        if output.len() < record.length {
            return Err(Error::OutputTooSmall);
        }
        self.flash
            .read(self.region, sector * SECTOR_SIZE, &mut self.scratch.0)
            .map_err(Error::Flash)?;
        let verified = decode(&self.scratch).ok_or(Error::ReadbackFailed)?;
        if verified != record {
            return Err(Error::ReadbackFailed);
        }
        output[..verified.length]
            .copy_from_slice(&self.scratch.0[HEADER_SIZE..HEADER_SIZE + verified.length]);
        Ok(Some(verified))
    }

    pub fn save(&mut self, payload: &[u8]) -> Result<Record, Error<F::Error>> {
        if payload.len() > CAPACITY {
            return Err(Error::PayloadTooLarge);
        }
        let (target, sequence) = match self.scan()? {
            Scan::Newest(sector, record) => (1 - sector, record.sequence.wrapping_add(1)),
            Scan::Free { sector, sequence } => (sector, sequence),
        };

        self.scratch.0.fill(0xff);
        self.scratch.0[..HEAD].copy_from_slice(&head(sequence));
        put_u32(&mut self.scratch.0, LENGTH_AT, payload.len() as u32);
        self.scratch.0[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
        let checksum = checksum(sequence, payload);
        put_u32(&mut self.scratch.0, CHECKSUM_AT, checksum);

        self.flash
            .erase(self.region, target * SECTOR_SIZE, SECTOR_SIZE)
            .map_err(Error::Flash)?;
        self.flash
            .program(self.region, target * SECTOR_SIZE, &self.scratch.0)
            .map_err(Error::Flash)?;
        self.flash
            .program(
                self.region,
                target * SECTOR_SIZE + COMMIT_OFFSET,
                &COMMITTED.to_le_bytes(),
            )
            .map_err(Error::Flash)?;

        self.flash
            .read(self.region, target * SECTOR_SIZE, &mut self.scratch.0)
            .map_err(Error::Flash)?;
        let record = decode(&self.scratch).ok_or(Error::ReadbackFailed)?;
        if record.sequence != sequence
            || record.length != payload.len()
            || self.scratch.0[HEADER_SIZE..HEADER_SIZE + payload.len()] != *payload
        {
            return Err(Error::ReadbackFailed);
        }
        Ok(record)
    }

    /// Erase both sectors, whatever they hold. The newest record goes last,
    /// so a cut never leaves an older record as the newest.
    pub fn erase(&mut self) -> Result<(), Error<F::Error>> {
        let last = match self.scan() {
            Ok(Scan::Newest(sector, _)) => sector,
            Ok(Scan::Free { .. }) | Err(Error::UnrecognizedJournal) => 1,
            Err(error) => return Err(error),
        };
        for sector in [1 - last, last] {
            self.flash
                .erase(self.region, sector * SECTOR_SIZE, SECTOR_SIZE)
                .map_err(Error::Flash)?;
        }
        Ok(())
    }

    /// Program the commit word of the record in `sector` again and read it
    /// back. Programming the same word again completes a commit a cut left
    /// marginal, and NOR programming only clears bits, so nothing else
    /// changes. A part with internal ECC, or flash encryption, would forbid
    /// it; the C606's boot flash is a plain NOR part (JEDEC `c8`/`4018`) with
    /// encryption disabled (`devices/c606/research`). Refuses with no write
    /// unless the sector's body verifies
    /// and its commit word could be a cut program of this journal's.
    pub fn recommit(&mut self, sector: usize) -> Result<Record, Error<F::Error>> {
        self.check_geometry()?;
        let at = (sector % SECTOR_COUNT) * SECTOR_SIZE;
        self.flash
            .read(self.region, at, &mut self.scratch.0)
            .map_err(Error::Flash)?;
        let bytes = &self.scratch.0;
        let begun = bytes[..SEQUENCE_AT] == head(0)[..SEQUENCE_AT]
            && get_u32(bytes, COMMIT_OFFSET) & COMMITTED == COMMITTED;
        let record = whole(bytes).filter(|_| begun).ok_or(Error::NoRecord)?;
        self.flash
            .program(self.region, at + COMMIT_OFFSET, &COMMITTED.to_le_bytes())
            .map_err(Error::Flash)?;
        self.flash
            .read(self.region, at, &mut self.scratch.0)
            .map_err(Error::Flash)?;
        if decode(&self.scratch) != Some(record) {
            return Err(Error::ReadbackFailed);
        }
        Ok(record)
    }

    /// Sector `sector` on its own, whatever the other one holds. A committed
    /// record's payload goes to the start of `output`.
    pub fn held(&mut self, sector: usize, output: &mut [u8]) -> Result<Held, Error<F::Error>> {
        self.check_geometry()?;
        self.flash
            .read(
                self.region,
                (sector % SECTOR_COUNT) * SECTOR_SIZE,
                &mut self.scratch.0,
            )
            .map_err(Error::Flash)?;
        if self.scratch.0.iter().all(|byte| *byte == 0xff) {
            return Ok(Held::Blank);
        }
        let Some(record) = decode(&self.scratch) else {
            return Ok(Held::Other);
        };
        let payload = output
            .get_mut(..record.length)
            .ok_or(Error::OutputTooSmall)?;
        payload.copy_from_slice(&self.scratch.0[HEADER_SIZE..HEADER_SIZE + record.length]);
        Ok(Held::Committed(record))
    }

    fn check_geometry(&self) -> Result<(), Error<F::Error>> {
        let geometry = self.flash.geometry(self.region);
        if geometry.capacity < SECTOR_COUNT * SECTOR_SIZE
            || geometry.program_size == 0
            || !4usize.is_multiple_of(geometry.program_size)
            || geometry.erase_size == 0
            || !SECTOR_SIZE.is_multiple_of(geometry.erase_size)
        {
            return Err(Error::UnsupportedGeometry);
        }
        Ok(())
    }

    fn scan(&mut self) -> Result<Scan, Error<F::Error>> {
        self.check_geometry()?;
        let mut newest = None;
        let mut blank = [false; SECTOR_COUNT];
        let mut torn = [false; SECTOR_COUNT];
        for sector in 0..SECTOR_COUNT {
            self.flash
                .read(self.region, sector * SECTOR_SIZE, &mut self.scratch.0)
                .map_err(Error::Flash)?;
            blank[sector] = self.scratch.0.iter().all(|byte| *byte == 0xff);
            torn[sector] = torn_first_save(&self.scratch);
            if let Some(record) = decode(&self.scratch)
                && newest
                    .map(|(_, current): (usize, Record)| newer(record.sequence, current.sequence))
                    .unwrap_or(true)
            {
                newest = Some((sector, record));
            }
        }
        if let Some((sector, record)) = newest {
            return Ok(Scan::Newest(sector, record));
        }
        // A first save cut before its commit word stays in place for
        // inspection, and the next save goes into the other, blank sector.
        // A commit word cut mid-program can read whole on a later boot, so
        // that save outranks the torn sequence 1.
        let free = |sector| {
            Ok(Scan::Free {
                sector,
                sequence: 2,
            })
        };
        match (blank, torn) {
            ([true, true], _) => Ok(Scan::Free {
                sector: 0,
                sequence: 1,
            }),
            ([false, true], [true, false]) => free(1),
            ([true, false], [false, true]) => free(0),
            _ => Err(Error::UnrecognizedJournal),
        }
    }
}

fn decode(sector: &Sector) -> Option<Record> {
    let bytes = &sector.0;
    if bytes[..SEQUENCE_AT] != head(0)[..SEQUENCE_AT] || get_u32(bytes, COMMIT_OFFSET) != COMMITTED
    {
        return None;
    }
    whole(bytes)
}

/// The record whose length and checksum verify, committed or not.
fn whole(bytes: &[u8; SECTOR_SIZE]) -> Option<Record> {
    let length = get_u32(bytes, LENGTH_AT) as usize;
    if length > CAPACITY {
        return None;
    }
    let sequence = get_u32(bytes, SEQUENCE_AT);
    let payload = &bytes[HEADER_SIZE..HEADER_SIZE + length];
    (get_u32(bytes, CHECKSUM_AT) == checksum(sequence, payload))
        .then_some(Record { sequence, length })
}

/// Whether a sector holds a partial first save. A cut inside the first page
/// leaves each header byte with some of its target's bits programmed and no
/// later page touched. Once the header is whole, only the commit word may be
/// partial. A partial header whose checksum still verifies belongs to a whole
/// later save, such as a sequence 3 cut before its commit word.
fn torn_first_save(sector: &Sector) -> bool {
    let header = head(1);
    let bytes = &sector.0;
    let commit = get_u32(bytes, COMMIT_OFFSET);
    if commit == COMMITTED || commit & COMMITTED != COMMITTED {
        return false;
    }
    bytes[..12] == header
        || (bytes[..12]
            .iter()
            .zip(header)
            .all(|(byte, target)| byte & target == target)
            && bytes[..12].iter().any(|byte| *byte != 0xff)
            && bytes[PAGE_SIZE..].iter().all(|byte| *byte == 0xff)
            && whole(bytes).is_none())
}

fn head(sequence: u32) -> [u8; HEAD] {
    let mut header = [0; HEAD];
    header[..4].copy_from_slice(&MAGIC);
    header[VERSION_AT] = VERSION;
    put_u16(&mut header, HEADER_SIZE_AT, HEADER_SIZE as u16);
    put_u32(&mut header, SEQUENCE_AT, sequence);
    header
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

fn checksum(sequence: u32, payload: &[u8]) -> u32 {
    Crc32::new()
        .update(&sequence.to_le_bytes())
        .update(&(payload.len() as u32).to_le_bytes())
        .update(payload)
        .finish()
}

/// An exclusive borrow of one reservation. Journals and domain adapters can
/// use different reservations while the backend serializes physical access.
/// As `OwnedFlash` it reaches only its own region.
pub struct RegionAccess<'a, B> {
    backend: &'a mut B,
    region: Region,
}
impl<'a, B: OwnedFlash> RegionAccess<'a, B> {
    pub fn new(backend: &'a mut B, region: Region) -> Self {
        Self { backend, region }
    }
    pub fn geometry(&self) -> Geometry {
        self.backend.geometry(self.region)
    }
    pub fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), B::Error> {
        self.backend.read(self.region, offset, output)
    }
    pub fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), B::Error> {
        self.backend.program(self.region, offset, bytes)
    }
    pub fn erase(&mut self, offset: usize, length: usize) -> Result<(), B::Error> {
        self.backend.erase(self.region, offset, length)
    }
    fn own(&self, region: Region) -> Result<(), AccessError<B::Error>> {
        (region == self.region)
            .then_some(())
            .ok_or(AccessError::OutOfBounds)
    }
}

impl<B: OwnedFlash> OwnedFlash for RegionAccess<'_, B> {
    type Error = AccessError<B::Error>;
    fn availability(&self, region: Region) -> device_api::observation::Availability {
        match self.own(region) {
            Ok(()) => self.backend.availability(region),
            Err(_) => device_api::observation::Availability::Unsupported,
        }
    }
    fn geometry(&self, region: Region) -> Geometry {
        let geometry = self.backend.geometry(self.region);
        Geometry {
            capacity: if region == self.region {
                geometry.capacity
            } else {
                0
            },
            ..geometry
        }
    }
    fn read(
        &mut self,
        region: Region,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.own(region)?;
        RegionAccess::read(self, offset, output).map_err(AccessError::Device)
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Self::Error> {
        self.own(region)?;
        RegionAccess::program(self, offset, bytes).map_err(AccessError::Device)
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Self::Error> {
        self.own(region)?;
        RegionAccess::erase(self, offset, length).map_err(AccessError::Device)
    }
}

/// The application's reservations under one borrow: its ride journal and,
/// outside it, the intent that records a wipe of its stores in progress.
pub struct AppStorage<'a, B> {
    backend: &'a mut B,
}
impl<'a, B: OwnedFlash> AppStorage<'a, B> {
    pub fn new(backend: &'a mut B) -> Self {
        Self { backend }
    }
    pub fn data(&mut self) -> RegionAccess<'_, B> {
        RegionAccess::new(self.backend, Region::Data)
    }
    pub fn intent(&mut self) -> RegionAccess<'_, B> {
        RegionAccess::new(self.backend, Region::Intent)
    }
}

#[cfg(test)]
pub(crate) mod tests;
