//! A small power-loss-safe store of named files in one volume.
//!
//! Two directory copies, A at relative sector 0 and B at `COPY_B`, each a
//! header followed by `DIR_SECTORS` entry sectors (`CAPACITY` entries). Each
//! file is one contiguous extent of data sectors from `DATA`. The header holds
//! the store id, a generation, an allocation cursor and the CRC32 of every
//! entry sector; its own CRC seals the copy. The current directory is the
//! valid header of the newest generation.
//!
//! Every change is copy-on-write and commits with one sector write:
//! - a file's bytes go to sectors no entry of the current directory uses, so a
//!   cut while writing them leaves only unreferenced sectors;
//! - the new directory goes to the other copy, entry sectors first (skipping
//!   those that already hold the new bytes) and its header last. A cut before
//!   the header leaves the old header current; a torn header does not validate.
//!
//! So after any cut the store holds exactly the files of the last committed
//! directory: an interrupted write, replace or delete has either happened
//! completely or not at all. Nothing is cached between calls, so another
//! writer on the same medium (the host, through the console) cannot leave a
//! stale view; a commit whose directory changed since its writer began fails
//! with [`Error::Stale`] instead of overwriting the change.

use crate::checksum::{Crc32, crc32};
use crate::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use device_api::bulk::{Error as MediaError, ReadWrite, SECTOR_SIZE};

pub type Sector = [u8; SECTOR_SIZE];

pub const MAGIC: &[u8; 16] = b"CYCLING-FILES\0\0\0";
pub const VERSION: u16 = 1;
pub const NAME: usize = 40;
const ENTRY: usize = 64;
const PER_SECTOR: usize = SECTOR_SIZE / ENTRY;
pub const DIR_SECTORS: usize = 16;
/// Files the store can hold.
pub const CAPACITY: usize = DIR_SECTORS * PER_SECTOR;
/// Largest file in bytes.
pub const MAX_FILE: u32 = 1 << 20;
pub const COPY_B: u64 = 32;
/// First data sector.
pub const DATA: u64 = 64;
const CRC: usize = SECTOR_SIZE - 4;
const CRCS: usize = 64;
const SIZE: u64 = SECTOR_SIZE as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Media(MediaError),
    /// No directory: the volume was never formatted as a file store.
    Uninitialized,
    /// A directory of another version.
    Foreign,
    /// The current directory's entries do not match its header.
    Corrupt,
    /// Empty, too long, or with characters outside `A-Z a-z 0-9 . _ - /`.
    Name,
    TooLarge,
    /// No free entry or no free extent large enough.
    Full,
    NotFound,
    /// The directory changed since this writer began; nothing was committed.
    Stale,
    /// Writes past the declared length, or a commit before all bytes arrived.
    Range,
    /// The file's bytes do not match its CRC.
    Mismatch,
}

impl From<MediaError> for Error {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

/// A file name: 1 to `NAME` bytes of `A-Z a-z 0-9 . _ - /`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME],
    length: u8,
}

impl Name {
    pub fn new(name: &str) -> Result<Self, Error> {
        let raw = name.as_bytes();
        if raw.is_empty()
            || raw.len() > NAME
            || !raw
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        {
            return Err(Error::Name);
        }
        let mut bytes = [0; NAME];
        bytes[..raw.len()].copy_from_slice(raw);
        Ok(Self {
            bytes,
            length: raw.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        // Only validated ASCII is ever stored.
        core::str::from_utf8(&self.bytes[..usize::from(self.length)]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: Name,
    pub length: u32,
    pub crc: u32,
    /// Low bits of the directory generation that committed this content.
    pub revision: u32,
    start: u32,
}

impl Entry {
    fn sectors(&self) -> u64 {
        u64::from(self.length).div_ceil(SIZE)
    }

    fn encode(&self, bytes: &mut [u8]) {
        bytes.fill(0);
        bytes[..NAME].copy_from_slice(&self.name.bytes);
        put_u32(bytes, 40, self.length);
        put_u32(bytes, 44, self.start);
        put_u32(bytes, 48, self.crc);
        put_u32(bytes, 52, self.revision);
    }

    fn decode(bytes: &[u8]) -> Result<Option<Self>, Error> {
        if bytes.iter().all(|b| *b == 0) {
            return Ok(None);
        }
        let length = bytes[..NAME].iter().position(|b| *b == 0).unwrap_or(NAME);
        let name = core::str::from_utf8(&bytes[..length]).map_err(|_| Error::Corrupt)?;
        let name = Name::new(name).map_err(|_| Error::Corrupt)?;
        if bytes[length..NAME].iter().any(|b| *b != 0) || bytes[56..].iter().any(|b| *b != 0) {
            return Err(Error::Corrupt);
        }
        let entry = Self {
            name,
            length: get_u32(bytes, 40),
            start: get_u32(bytes, 44),
            crc: get_u32(bytes, 48),
            revision: get_u32(bytes, 52),
        };
        if entry.length > MAX_FILE {
            return Err(Error::Corrupt);
        }
        Ok(Some(entry))
    }
}

/// The current directory's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dir {
    copy: u64,
    id: u64,
    generation: u64,
    cursor: u32,
    count: u16,
    crcs: [u32; DIR_SECTORS],
    data: u64,
}

fn base(copy: u64) -> u64 {
    if copy == 0 { 0 } else { COPY_B }
}

enum Replica {
    Ours(Dir),
    Foreign,
    Invalid,
}

fn decode_header(sector: &Sector, copy: u64, data: u64) -> Replica {
    if sector[..16] != MAGIC[..] || get_u32(sector, CRC) != crc32(&sector[..CRC]) {
        return Replica::Invalid;
    }
    if get_u16(sector, 16) != VERSION
        || sector[20..24].iter().any(|b| *b != 0)
        || sector[44..CRCS].iter().any(|b| *b != 0)
        || sector[CRCS + 4 * DIR_SECTORS..CRC].iter().any(|b| *b != 0)
        || usize::from(get_u16(sector, 18)) > CAPACITY
    {
        return Replica::Foreign;
    }
    let mut crcs = [0; DIR_SECTORS];
    for (index, crc) in crcs.iter_mut().enumerate() {
        *crc = get_u32(sector, CRCS + 4 * index);
    }
    Replica::Ours(Dir {
        copy,
        id: get_u64(sector, 24),
        generation: get_u64(sector, 32),
        cursor: get_u32(sector, 40),
        count: get_u16(sector, 18),
        crcs,
        data,
    })
}

fn headers(
    media: &mut impl ReadWrite,
    sector: &mut Sector,
) -> Result<([Option<Dir>; 2], u64), Error> {
    let sectors = media.owned_info()?.sectors;
    let data = sectors.checked_sub(DATA).ok_or(Error::Range)?;
    let mut copies = [None, None];
    for (copy, slot) in copies.iter_mut().enumerate() {
        media.owned_read(base(copy as u64), sector)?;
        match decode_header(sector, copy as u64, data) {
            Replica::Ours(dir) => *slot = Some(dir),
            Replica::Foreign => return Err(Error::Foreign),
            Replica::Invalid => {}
        }
    }
    Ok((copies, data))
}

/// The current directory (two sector reads).
pub fn mount(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Dir, Error> {
    let (copies, _) = headers(media, sector)?;
    copies
        .into_iter()
        .flatten()
        .max_by_key(|d| d.generation)
        .ok_or(Error::Uninitialized)
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Start an empty store, superseding any directory there. Writes the empty
/// directory into the copy that is not current: a cut leaves the old store.
/// A foreign directory (another format version) is refused without writing:
/// formatting one copy would still leave the other blocking every mount.
pub fn format(media: &mut impl ReadWrite, seed: u64, sector: &mut Sector) -> Result<Dir, Error> {
    let (copies, data) = headers(media, sector)?;
    if data == 0 {
        return Err(Error::Range);
    }
    let newest = copies
        .iter()
        .flatten()
        .max_by_key(|d| d.generation)
        .copied();
    let mut id = mix(seed ^ media.entropy());
    while id == 0 || copies.iter().flatten().any(|d| d.id == id) {
        id = mix(id);
    }
    let empty = [0u8; SECTOR_SIZE];
    let dir = Dir {
        copy: newest.map_or(0, |d| 1 - d.copy),
        id,
        generation: newest.map_or(0, |d| d.generation) + 1,
        cursor: 0,
        count: 0,
        crcs: [crc32(&empty); DIR_SECTORS],
        data,
    };
    for index in 0..DIR_SECTORS as u64 {
        media.owned_write(base(dir.copy) + 1 + index, &empty)?;
    }
    write_header(media, &dir, sector)?;
    Ok(dir)
}

fn write_header(media: &mut impl ReadWrite, dir: &Dir, sector: &mut Sector) -> Result<(), Error> {
    sector.fill(0);
    sector[..16].copy_from_slice(MAGIC);
    put_u16(sector, 16, VERSION);
    put_u16(sector, 18, dir.count);
    put_u64(sector, 24, dir.id);
    put_u64(sector, 32, dir.generation);
    put_u32(sector, 40, dir.cursor);
    for (index, crc) in dir.crcs.iter().enumerate() {
        put_u32(sector, CRCS + 4 * index, *crc);
    }
    let checksum = crc32(&sector[..CRC]);
    put_u32(sector, CRC, checksum);
    media.owned_write(base(dir.copy), sector)?;
    Ok(())
}

impl Dir {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn store_id(&self) -> u64 {
        self.id
    }

    /// Files in the store.
    pub fn len(&self) -> usize {
        usize::from(self.count)
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Data sectors of the volume.
    pub fn data_sectors(&self) -> u64 {
        self.data
    }

    /// Entry sector `index` of this directory, verified against the header.
    fn entries(
        &self,
        media: &mut impl ReadWrite,
        index: usize,
        sector: &mut Sector,
    ) -> Result<[Option<Entry>; PER_SECTOR], Error> {
        media.owned_read(base(self.copy) + 1 + index as u64, sector)?;
        if crc32(sector) != self.crcs[index] {
            return Err(Error::Corrupt);
        }
        let mut entries = [None; PER_SECTOR];
        for (slot, entry) in entries.iter_mut().enumerate() {
            *entry = Entry::decode(&sector[slot * ENTRY..][..ENTRY])?;
            if let Some(e) = entry
                && u64::from(e.start) + e.sectors() > self.data
            {
                return Err(Error::Corrupt);
            }
        }
        Ok(entries)
    }

    /// Entry slot `index` (`0..CAPACITY`), one sector read. Slots are not
    /// ordered; list a store by visiting every slot.
    pub fn entry(
        &self,
        media: &mut impl ReadWrite,
        index: usize,
        sector: &mut Sector,
    ) -> Result<Option<Entry>, Error> {
        if index >= CAPACITY {
            return Err(Error::Range);
        }
        Ok(self.entries(media, index / PER_SECTOR, sector)?[index % PER_SECTOR])
    }

    /// Visit every file in slot order: up to `DIR_SECTORS` reads.
    pub fn list(
        &self,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
        mut visit: impl FnMut(&Entry),
    ) -> Result<(), Error> {
        for index in 0..DIR_SECTORS {
            for entry in self.entries(media, index, sector)?.iter().flatten() {
                visit(entry);
            }
        }
        Ok(())
    }

    pub fn find(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        sector: &mut Sector,
    ) -> Result<Option<Entry>, Error> {
        for index in 0..DIR_SECTORS {
            if let Some(entry) = self
                .entries(media, index, sector)?
                .into_iter()
                .flatten()
                .find(|e| e.name == *name)
            {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// Copy bytes of `entry` from `offset`, returning how many.
    pub fn read(
        &self,
        media: &mut impl ReadWrite,
        entry: &Entry,
        offset: u32,
        output: &mut [u8],
        sector: &mut Sector,
    ) -> Result<usize, Error> {
        if offset > entry.length {
            return Err(Error::Range);
        }
        let wanted = output.len().min((entry.length - offset) as usize);
        let mut done = 0;
        while done < wanted {
            let position = u64::from(offset) + done as u64;
            media.owned_read(DATA + u64::from(entry.start) + position / SIZE, sector)?;
            let within = (position % SIZE) as usize;
            let n = (SECTOR_SIZE - within).min(wanted - done);
            output[done..done + n].copy_from_slice(&sector[within..within + n]);
            done += n;
        }
        Ok(done)
    }

    /// Read `entry` completely and compare its CRC.
    pub fn verify(
        &self,
        media: &mut impl ReadWrite,
        entry: &Entry,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        let mut crc = Crc32::new();
        let mut left = u64::from(entry.length);
        for index in 0..entry.sectors() {
            media.owned_read(DATA + u64::from(entry.start) + index, sector)?;
            let n = left.min(SIZE) as usize;
            crc = crc.update(&sector[..n]);
            left -= n as u64;
        }
        if crc.finish() == entry.crc {
            Ok(())
        } else {
            Err(Error::Mismatch)
        }
    }

    /// Start writing `length` bytes as `name`, replacing any file of that name
    /// when committed. Allocates sectors no current file uses.
    pub fn begin(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        length: u32,
        sector: &mut Sector,
    ) -> Result<Writer, Error> {
        if length > MAX_FILE {
            return Err(Error::TooLarge);
        }
        let mut used = [(0u32, 0u32); CAPACITY];
        let mut count = 0;
        let mut exists = false;
        for index in 0..DIR_SECTORS {
            for entry in self.entries(media, index, sector)?.iter().flatten() {
                exists |= entry.name == *name;
                if entry.length > 0 {
                    used[count] = (entry.start, entry.sectors() as u32);
                    count += 1;
                }
            }
        }
        if !exists && usize::from(self.count) >= CAPACITY {
            return Err(Error::Full);
        }
        let need = u64::from(length).div_ceil(SIZE);
        let start = if need == 0 {
            0
        } else {
            allocate(&used[..count], u64::from(self.cursor), need, self.data).ok_or(Error::Full)?
        };
        Ok(Writer {
            name: *name,
            length,
            start: start as u32,
            written: 0,
            crc: Crc32::new(),
            base: (self.id, self.generation),
        })
    }

    /// Remove `name`. Returns the new directory.
    pub fn delete(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        sector: &mut Sector,
    ) -> Result<Dir, Error> {
        self.commit(media, name, None, self.cursor, sector)
    }

    /// Write the directory with `name` set to `entry` (or removed) into the
    /// other copy. Fails with `Stale`, writing nothing, unless `self` is still
    /// the current directory: a stale snapshot would drop changes made since.
    fn commit(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        entry: Option<Entry>,
        cursor: u32,
        sector: &mut Sector,
    ) -> Result<Dir, Error> {
        let current = mount(media, sector)?;
        if (current.id, current.generation, current.copy) != (self.id, self.generation, self.copy) {
            return Err(Error::Stale);
        }
        let mut next = Dir {
            copy: 1 - self.copy,
            generation: self.generation + 1,
            cursor,
            ..*self
        };
        // Where the entry goes: the slot holding `name`, else the first free.
        let mut target = None;
        let mut free = None;
        for index in 0..DIR_SECTORS {
            for (slot, e) in self.entries(media, index, sector)?.iter().enumerate() {
                let at = index * PER_SECTOR + slot;
                match e {
                    Some(e) if e.name == *name => target = Some(at),
                    None if free.is_none() => free = Some(at),
                    _ => {}
                }
            }
        }
        let at = match (target, entry) {
            (Some(at), _) => at,
            (None, Some(_)) => free.ok_or(Error::Full)?,
            (None, None) => return Err(Error::NotFound),
        };
        let mut count = usize::from(self.count);
        if target.is_none() {
            count += 1;
        }
        if entry.is_none() {
            count = count.checked_sub(1).ok_or(Error::Corrupt)?;
        }
        next.count = count as u16;
        let mut other = [0u8; SECTOR_SIZE];
        for index in 0..DIR_SECTORS {
            // Re-read and re-verify this copy's sector, then change one slot.
            self.entries(media, index, sector)?;
            if index == at / PER_SECTOR {
                let bytes = &mut sector[(at % PER_SECTOR) * ENTRY..][..ENTRY];
                match &entry {
                    Some(e) => e.encode(bytes),
                    None => bytes.fill(0),
                }
            }
            next.crcs[index] = crc32(sector);
            let address = base(next.copy) + 1 + index as u64;
            media.owned_read(address, &mut other)?;
            if other != *sector {
                media.owned_write(address, sector)?;
            }
        }
        write_header(media, &next, sector)?;
        Ok(next)
    }
}

/// Next fit: the first gap of `need` sectors at or after `cursor`, wrapping.
fn allocate(used: &[(u32, u32)], cursor: u64, need: u64, data: u64) -> Option<u64> {
    let fits = |at: u64| {
        at + need <= data
            && used.iter().all(|&(s, n)| {
                let (s, n) = (u64::from(s), u64::from(n));
                at + need <= s || s + n <= at
            })
    };
    let candidates =
        || core::iter::once(0).chain(used.iter().map(|&(s, n)| u64::from(s) + u64::from(n)));
    let cursor = if cursor >= data { 0 } else { cursor };
    core::iter::once(cursor)
        .chain(candidates().filter(|c| *c >= cursor))
        .chain(candidates())
        .filter(|c| fits(*c))
        .min_by_key(|c| if *c >= cursor { *c - cursor } else { *c + data })
}

/// An open file write. Data sectors go straight to their extent; the file
/// appears only when [`Writer::commit`] writes the directory.
#[derive(Clone, Copy, Debug)]
pub struct Writer {
    name: Name,
    length: u32,
    start: u32,
    written: u32,
    crc: Crc32,
    base: (u64, u64),
}

impl Writer {
    pub fn name(&self) -> Name {
        self.name
    }

    pub fn length(&self) -> u32 {
        self.length
    }

    pub fn written(&self) -> u32 {
        self.written
    }

    /// Append `bytes`: whole sectors, except a final chunk that ends the file.
    /// Fails with `Stale`, writing nothing, once another change committed: its
    /// files may now use this writer's sectors.
    pub fn write(&mut self, media: &mut impl ReadWrite, bytes: &[u8]) -> Result<(), Error> {
        let end = u64::from(self.written) + bytes.len() as u64;
        if end > u64::from(self.length)
            || (end < u64::from(self.length) && !bytes.len().is_multiple_of(SECTOR_SIZE))
        {
            return Err(Error::Range);
        }
        let mut sector = [0u8; SECTOR_SIZE];
        let dir = mount(media, &mut sector)?;
        if (dir.id, dir.generation) != self.base {
            return Err(Error::Stale);
        }
        let first = DATA + u64::from(self.start) + u64::from(self.written) / SIZE;
        for (index, chunk) in bytes.chunks(SECTOR_SIZE).enumerate() {
            sector.fill(0);
            sector[..chunk.len()].copy_from_slice(chunk);
            media.owned_write(first + index as u64, &sector)?;
            self.crc = self.crc.update(chunk);
            self.written += chunk.len() as u32;
        }
        Ok(())
    }

    /// Publish the file. `crc`, the host's CRC32 of the whole file, must match
    /// the bytes written and the bytes read back from the extent (another open
    /// writer may share it). Fails with `Stale` if the directory changed.
    pub fn commit(
        self,
        media: &mut impl ReadWrite,
        crc: u32,
        sector: &mut Sector,
    ) -> Result<Entry, Error> {
        if self.written != self.length {
            return Err(Error::Range);
        }
        if self.crc.finish() != crc {
            return Err(Error::Mismatch);
        }
        let dir = mount(media, sector)?;
        if (dir.id, dir.generation) != self.base {
            return Err(Error::Stale);
        }
        let entry = Entry {
            name: self.name,
            length: self.length,
            crc,
            revision: (dir.generation + 1) as u32,
            start: self.start,
        };
        // Another uncommitted writer from the same directory may have staged
        // its bytes over this extent: publish only what is really there.
        dir.verify(media, &entry, sector)?;
        let sectors = entry.sectors();
        let cursor = if sectors == 0 {
            dir.cursor
        } else {
            (u64::from(self.start) + sectors) as u32
        };
        dir.commit(media, &self.name, Some(entry), cursor, sector)?;
        Ok(entry)
    }
}

/// Write a whole file held in memory.
pub fn write(
    media: &mut impl ReadWrite,
    name: &Name,
    bytes: &[u8],
    sector: &mut Sector,
) -> Result<Entry, Error> {
    let length = u32::try_from(bytes.len()).map_err(|_| Error::TooLarge)?;
    let mut writer = mount(media, sector)?.begin(media, name, length, sector)?;
    writer.write(media, bytes)?;
    writer.commit(media, crc32(bytes), sector)
}

/// Remove a file.
pub fn delete(media: &mut impl ReadWrite, name: &Name, sector: &mut Sector) -> Result<(), Error> {
    mount(media, sector)?.delete(media, name, sector)?;
    Ok(())
}

#[cfg(test)]
mod tests;
