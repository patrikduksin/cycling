//! A host-written, read-mostly pack of opaque bytes in one volume.
//!
//! Relative sectors 0 and 1 hold two header copies; data starts at `DATA`. A
//! header names the pack (a 16-byte format tag and a 32-byte digest, both the
//! host's), its length and CRC32, and whether it is still being written or
//! ready. Readers take the valid copy of the newest generation, and new
//! headers always go to the other copy, so a cut leaves the previous header.
//!
//! Writing is a transaction a reader never sees half done: [`begin`] makes a
//! `Writing` header current before any data changes, so no `Ready` header
//! describes bytes being replaced; data sectors are then written in order (each
//! one verified by the medium) while a running CRC follows them; [`Writer::commit`]
//! writes the `Ready` header only when every sector was written or read back
//! and the CRC matches. After a reboot [`Writer::resume`] continues a `Writing`
//! pack: [`Writer::verify`] reads back what is already there, chunk by chunk
//! against the host's CRCs, and writing continues from the first mismatch.

use crate::checksum::{Crc32, crc32};
use crate::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use device_api::bulk::{Error as MediaError, ReadWrite, SECTOR_SIZE};

pub type Sector = [u8; SECTOR_SIZE];

pub const MAGIC: &[u8; 16] = b"CYCLING-PACK\0\0\0\0";
pub const VERSION: u16 = 1;
/// First data sector; the sectors between the headers and it stay unused.
pub const DATA: u64 = 8;
/// Sectors one verify call may read.
pub const MAX_VERIFY: u64 = 256;
const CRC: usize = SECTOR_SIZE - 4;
const SIZE: u64 = SECTOR_SIZE as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Media(MediaError),
    /// A header of another version.
    Foreign,
    /// No pack has been written.
    Empty,
    /// Not the state this operation needs: no pack being written, or none ready.
    State,
    /// Past the pack or the volume, or not the next sector in order.
    Range,
    /// The bytes do not match the header's CRC or a sector's padding is not zero.
    Mismatch,
    /// Another header became current since this writer started.
    Stale,
}

impl From<MediaError> for Error {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Writing,
    Ready,
}

impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Writing => "writing",
            Self::Ready => "ready",
        }
    }
}

/// What the host says the pack is; the device checks length and CRC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub length: u64,
    pub crc: u32,
    pub tag: [u8; 16],
    pub digest: [u8; 32],
    /// Host time of writing, opaque here.
    pub stamp: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub state: State,
    pub generation: u64,
    pub spec: Spec,
}

impl Header {
    /// Data sectors the pack spans.
    pub fn sectors(&self) -> u64 {
        self.spec.length.div_ceil(SIZE)
    }

    fn encode(&self, sector: &mut Sector) {
        sector.fill(0);
        sector[..16].copy_from_slice(MAGIC);
        put_u16(sector, 16, VERSION);
        sector[18] = match self.state {
            State::Writing => 1,
            State::Ready => 2,
        };
        put_u64(sector, 24, self.generation);
        put_u64(sector, 32, self.spec.length);
        put_u32(sector, 40, self.spec.crc);
        sector[48..64].copy_from_slice(&self.spec.tag);
        sector[64..96].copy_from_slice(&self.spec.digest);
        put_u64(sector, 96, self.spec.stamp);
        let checksum = crc32(&sector[..CRC]);
        put_u32(sector, CRC, checksum);
    }
}

enum Replica {
    Ours(Header),
    Foreign,
    Invalid,
}

fn decode(sector: &Sector) -> Replica {
    if sector[..16] != MAGIC[..] || get_u32(sector, CRC) != crc32(&sector[..CRC]) {
        return Replica::Invalid;
    }
    let state = match sector[18] {
        1 => State::Writing,
        2 => State::Ready,
        _ => return Replica::Foreign,
    };
    if get_u16(sector, 16) != VERSION
        || sector[19..24].iter().any(|b| *b != 0)
        || sector[44..48].iter().any(|b| *b != 0)
        || sector[104..CRC].iter().any(|b| *b != 0)
    {
        return Replica::Foreign;
    }
    let mut tag = [0; 16];
    tag.copy_from_slice(&sector[48..64]);
    let mut digest = [0; 32];
    digest.copy_from_slice(&sector[64..96]);
    Replica::Ours(Header {
        state,
        generation: get_u64(sector, 24),
        spec: Spec {
            length: get_u64(sector, 32),
            crc: get_u32(sector, 40),
            tag,
            digest,
            stamp: get_u64(sector, 96),
        },
    })
}

/// The current header and the copy it is in.
fn current(
    media: &mut impl ReadWrite,
    sector: &mut Sector,
) -> Result<Option<(Header, u64)>, Error> {
    let mut best: Option<(Header, u64)> = None;
    for copy in 0..2 {
        media.owned_read(copy, sector)?;
        match decode(sector) {
            Replica::Foreign => return Err(Error::Foreign),
            Replica::Invalid => {}
            Replica::Ours(header) => {
                if best.is_none_or(|(b, _)| header.generation > b.generation) {
                    best = Some((header, copy));
                }
            }
        }
    }
    Ok(best)
}

/// Bytes a pack may hold in a volume of `sectors`.
pub fn capacity(sectors: u64) -> u64 {
    sectors.saturating_sub(DATA) * SIZE
}

/// The current header, if any pack was begun (two sector reads).
pub fn status(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Option<Header>, Error> {
    Ok(current(media, sector)?.map(|(header, _)| header))
}

/// Write `header` into the copy that is not current and return it.
fn publish(
    media: &mut impl ReadWrite,
    state: State,
    spec: Spec,
    sector: &mut Sector,
) -> Result<Header, Error> {
    let now = current(media, sector)?;
    let header = Header {
        state,
        generation: now.map_or(1, |(h, _)| h.generation + 1),
        spec,
    };
    let copy = now.map_or(0, |(_, copy)| 1 - copy);
    header.encode(sector);
    media.owned_write(copy, sector)?;
    Ok(header)
}

/// Start replacing the pack. Once this returns, no reader sees a ready pack
/// until [`Writer::commit`] succeeds. Writes one sector.
pub fn begin(media: &mut impl ReadWrite, spec: Spec, sector: &mut Sector) -> Result<Writer, Error> {
    if spec.length > capacity(media.owned_info()?.sectors) {
        return Err(Error::Range);
    }
    let header = publish(media, State::Writing, spec, sector)?;
    Ok(Writer::new(header))
}

/// Raw CRC32 of `count` data sectors from `start`, for host readback checks.
pub fn crc(
    media: &mut impl ReadWrite,
    start: u64,
    count: u64,
    sector: &mut Sector,
) -> Result<u32, Error> {
    if count > MAX_VERIFY {
        return Err(Error::Range);
    }
    let mut crc = Crc32::new();
    for index in start..start.checked_add(count).ok_or(Error::Range)? {
        media.owned_read(DATA.checked_add(index).ok_or(Error::Range)?, sector)?;
        crc = crc.update(sector);
    }
    Ok(crc.finish())
}

/// Result of a [`Writer::verify`] step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    /// CRC32 of the whole sectors read.
    pub crc: u32,
    /// Whether they matched and the writer moved past them.
    pub advanced: bool,
}

/// An open write: data sectors before `cursor` are known to hold the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Writer {
    header: Header,
    cursor: u64,
    crc: Crc32,
}

impl Writer {
    fn new(header: Header) -> Self {
        Self {
            header,
            cursor: 0,
            crc: Crc32::new(),
        }
    }

    /// Continue the pack being written, from its first sector.
    pub fn resume(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Self, Error> {
        match current(media, sector)? {
            Some((header, _)) if header.state == State::Writing => Ok(Self::new(header)),
            Some(_) => Err(Error::State),
            None => Err(Error::Empty),
        }
    }

    pub fn header(&self) -> Header {
        self.header
    }

    /// Data sectors written or verified so far, in order.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Bytes of the pack in data sector `index`.
    fn bytes(&self, index: u64) -> usize {
        (self.header.spec.length - index * SIZE).min(SIZE) as usize
    }

    fn accept(&mut self, data: &Sector) -> Result<(), Error> {
        let used = self.bytes(self.cursor);
        if data[used..].iter().any(|b| *b != 0) {
            return Err(Error::Mismatch);
        }
        self.crc = self.crc.update(&data[..used]);
        self.cursor += 1;
        Ok(())
    }

    /// Write data sector `index`, which must be the next one. A failed write
    /// leaves the cursor where it was; write that sector again.
    pub fn write(
        &mut self,
        media: &mut impl ReadWrite,
        index: u64,
        data: &Sector,
    ) -> Result<(), Error> {
        if index != self.cursor || index >= self.header.sectors() {
            return Err(Error::Range);
        }
        let used = self.bytes(index);
        if data[used..].iter().any(|b| *b != 0) {
            return Err(Error::Mismatch);
        }
        media.owned_write(DATA + index, data)?;
        self.accept(data)
    }

    /// Read back up to `MAX_VERIFY` sectors from the cursor and move past them
    /// if their whole-sector CRC is `expected`.
    pub fn verify(
        &mut self,
        media: &mut impl ReadWrite,
        count: u64,
        expected: u32,
        sector: &mut Sector,
    ) -> Result<Verified, Error> {
        if count == 0 || count > MAX_VERIFY || self.cursor + count > self.header.sectors() {
            return Err(Error::Range);
        }
        let mut whole = Crc32::new();
        let mut running = *self;
        let mut padded = true;
        for index in self.cursor..self.cursor + count {
            media.owned_read(DATA + index, sector)?;
            whole = whole.update(sector);
            // Bytes past the pack's end that are not zero are not this pack.
            padded = padded && running.accept(sector).is_ok();
        }
        let crc = whole.finish();
        let advanced = crc == expected && padded;
        if advanced {
            *self = running;
        }
        Ok(Verified { crc, advanced })
    }

    /// Make the pack ready: every sector is in place and its CRC matches. Writes
    /// one sector; on failure the pack stays `Writing`.
    pub fn commit(self, media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Header, Error> {
        if self.cursor != self.header.sectors() {
            return Err(Error::Range);
        }
        if self.crc.finish() != self.header.spec.crc {
            return Err(Error::Mismatch);
        }
        match current(media, sector)? {
            Some((now, _)) if now == self.header => {}
            _ => return Err(Error::Stale),
        }
        publish(media, State::Ready, self.header.spec, sector)
    }
}

/// A ready pack, for reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pack {
    header: Header,
}

/// The ready pack (two sector reads).
pub fn open(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Pack, Error> {
    match current(media, sector)? {
        Some((header, _)) if header.state == State::Ready => Ok(Pack { header }),
        Some(_) => Err(Error::State),
        None => Err(Error::Empty),
    }
}

impl Pack {
    pub fn header(&self) -> Header {
        self.header
    }

    pub fn len(&self) -> u64 {
        self.header.spec.length
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copy pack bytes from `offset` into `output`, returning how many (fewer
    /// at the end of the pack). Reads one sector per 512 bytes spanned.
    pub fn read(
        &self,
        media: &mut impl ReadWrite,
        offset: u64,
        output: &mut [u8],
        sector: &mut Sector,
    ) -> Result<usize, Error> {
        let length = self.len();
        if offset > length {
            return Err(Error::Range);
        }
        let wanted = output
            .len()
            .min((length - offset).min(usize::MAX as u64) as usize);
        let mut done = 0;
        while done < wanted {
            let position = offset + done as u64;
            media.owned_read(DATA + position / SIZE, sector)?;
            let within = (position % SIZE) as usize;
            let n = (SECTOR_SIZE - within).min(wanted - done);
            output[done..done + n].copy_from_slice(&sector[within..within + n]);
            done += n;
        }
        Ok(done)
    }

    /// Read the whole pack back and compare its CRC with the header's.
    /// Bounded by the caller: `count` sectors from `*position`, returning
    /// `Some(matched)` after the last one.
    pub fn check(
        &self,
        media: &mut impl ReadWrite,
        state: &mut Check,
        count: u64,
        sector: &mut Sector,
    ) -> Result<Option<bool>, Error> {
        let total = self.header.sectors();
        let end = (state.next + count).min(total);
        for index in state.next..end {
            media.owned_read(DATA + index, sector)?;
            let used = (self.len() - index * SIZE).min(SIZE) as usize;
            state.crc = state.crc.update(&sector[..used]);
        }
        state.next = end;
        Ok((end == total).then(|| state.crc.finish() == self.header.spec.crc))
    }
}

/// Progress of a bounded [`Pack::check`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Check {
    next: u64,
    crc: Crc32,
}

#[cfg(test)]
mod tests;
