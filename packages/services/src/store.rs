//! A power-cut-safe store of growable, append-only files in one volume, with
//! directories as name prefixes: `traces/0001` is in the directory `traces/`.
//!
//! Layout, in sectors from the start of the volume:
//! - Metadata copy A at 0 and copy B at `COPY_B`. Each is a header, then
//!   `DIR_SECTORS` entry sectors (`CAPACITY` files: name and id), then the
//!   chunk table: 4 bytes per chunk, the owning entry slot + 1 (0 when free)
//!   and the chunk's position in its file. The header holds the store id, a
//!   generation, the next file id, an allocation cursor and the CRC32 of
//!   every body sector; its own CRC seals the copy. The current copy is the
//!   valid header of the newest generation.
//! - Data from `DATA` in chunks of `CHUNK` sectors (256 KiB). A file holds at
//!   least one chunk; every chunk but its last is full.
//!
//! Each data sector is a self-checking record: store id, file id, record
//! number, payload length and a CRC over all of it. Records are written once,
//! in order, so the valid records of a file's last chunk are a prefix of it
//! and its end is found by binary search. An append writes record sectors
//! and no metadata; a full chunk takes one metadata commit to link the next.
//! [`Appender::sync`] writes the partly filled record, so a cut loses at most
//! the bytes appended since the last sync or full record.
//!
//! Metadata changes (create, a new chunk, delete) are copy-on-write into the
//! copy that is not current: its header is invalidated first, then the body
//! sectors that change or differ from the current copy are written, then its
//! header. A valid header therefore always describes its own body, a cut
//! leaves the old or the new metadata, and the next commit can trust the
//! other copy's CRCs to skip the sectors it already holds.
//!
//! Another writer (the host's transfer session) may change the medium
//! between calls. A [`Store`] whose metadata is no longer current commits
//! nothing and fails with [`Error::Stale`]. Appends do not read metadata, so
//! after a transfer session [`mount`] again and compare [`Store::generation`]:
//! when it changed, reopen held files with [`Store::open_append`] (the host
//! may have deleted them). Only one [`Appender`] per store writes at a time;
//! opening another one makes the first fail with `Stale`.
//!
//! Rollover: an appender or a create given a directory deletes the oldest
//! closed file in it (lowest id, never the open one) when the store is full.
//!
//! Every operation does a bounded number of sector operations, whatever the
//! volume size: with `D = DIR_SECTORS` (8), `T` table sectors (at most
//! `MAX_TABLE`, 9 on the C606 STORE volume) and `B = ⌊log2 CHUNK⌋ + 1` (10),
//! a metadata commit reads at most `D + T + 2` and writes at most `D + T + 2`
//! sectors, and
//!
//! | operation | reads | writes | C606 |
//! | --- | --- | --- | --- |
//! | [`mount`] | 2 | 0 | 2 / 0 |
//! | [`format`] | 2 | `D + T + 2` | 2 / 19 |
//! | [`Store::list`], [`Store::reader`] | `D + T` | 0 | 17 / 0 |
//! | [`Store::open_append`] | `D + T + B` | 0 | 27 / 0 |
//! | [`Store::delete`], [`Store::delete_oldest`] | `2D + T + 2` | `D + T + 2` | 27 / 19 |
//! | [`Store::create`] | `2D + 2T + 2` | `D + T + 2` | 36 / 19 |
//! | [`Store::create`] that rolls over | `5D + 4T + 4` | `2D + 2T + 4` | 80 / 38 |
//! | [`Appender::append`] of up to `PAYLOAD` bytes, [`Appender::sync`] | `3D + 4T + 4` | `2D + 2T + 5` | 64 / 39 |
//! | [`Reader::next`] | `T + 1` | 0 | 10 / 0 |
//! | [`Reader::seek`] | `T` | 0 | 9 / 0 |
//!
//! Appends are 1 write per `PAYLOAD` bytes; the full costs come from linking
//! a chunk (once per `CHUNK` records), rolling over, and a commit after a cut
//! (when the other copy holds nothing it can vouch for).
//! `tests::operations_have_the_documented_worst_case_costs` measures them.

use crate::checksum::crc32;
use crate::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use device_api::bulk::{Error as MediaError, ReadWrite, SECTOR_SIZE};

pub type Sector = [u8; SECTOR_SIZE];

pub const MAGIC: &[u8; 16] = b"CYCLING-STORE\0\0\0";
pub const VERSION: u16 = 1;
/// Longest name or directory in bytes.
pub const NAME: usize = 40;
const ENTRY: usize = 64;
const PER_SECTOR: usize = SECTOR_SIZE / ENTRY;
pub const DIR_SECTORS: usize = 8;
/// Files the store can hold.
pub const CAPACITY: usize = DIR_SECTORS * PER_SECTOR;
/// Chunk table entries per sector.
const LINKS: usize = SECTOR_SIZE / 4;
/// Table sectors at most, whatever the volume size.
pub const MAX_TABLE: usize = 16;
/// Chunks at most: 512 MiB of data; a larger volume leaves the rest unused.
pub const MAX_CHUNKS: usize = MAX_TABLE * LINKS;
/// Sectors per data chunk: 256 KiB.
pub const CHUNK: u32 = 512;
const BODY: usize = DIR_SECTORS + MAX_TABLE;
pub const COPY_B: u64 = 32;
/// First data sector.
pub const DATA: u64 = 64;
const CRC: usize = SECTOR_SIZE - 4;
const CRCS: usize = 64;
const RECORD: &[u8; 4] = b"CSRD";
const RECORD_HEADER: usize = 22;
/// Payload bytes in one record sector.
pub const PAYLOAD: usize = CRC - RECORD_HEADER;

const _: () = assert!((BODY as u64) < COPY_B && COPY_B + (BODY as u64) < DATA);
const _: () = assert!(CRCS + 4 * BODY <= CRC && CAPACITY < u16::MAX as usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Media(MediaError),
    /// No metadata: the volume was never formatted as a store.
    Uninitialized,
    /// Metadata of another version or geometry. Never overwritten.
    Foreign,
    /// The metadata or a full chunk does not match its checks.
    Corrupt,
    /// Not a name or directory: see [`Name`] and [`Dir`].
    Name,
    Exists,
    NotFound,
    /// No free entry or chunk, and nothing to roll over.
    Full,
    /// The store changed since this value was read, or another appender
    /// opened; nothing was written.
    Stale,
    /// The volume is too small for a store.
    Range,
}

impl From<MediaError> for Error {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

/// Up to `NAME` bytes of `A-Z a-z 0-9 . _ - /`, as `/`-separated segments
/// that are not empty. A directory is empty (the root) or ends in `/`.
fn path(text: &str, dir: bool) -> Result<([u8; NAME], u8), Error> {
    let raw = text.as_bytes();
    let segments = match raw.split_last() {
        None if dir => &[][..],
        Some((b'/', head)) if dir => head,
        _ if dir => return Err(Error::Name),
        _ => raw,
    };
    if raw.len() > NAME
        || (!dir && raw.is_empty())
        || (!raw.is_empty() && segments.split(|b| *b == b'/').any(<[u8]>::is_empty))
        || !raw
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(Error::Name);
    }
    let mut bytes = [0; NAME];
    bytes[..raw.len()].copy_from_slice(raw);
    Ok((bytes, raw.len() as u8))
}

/// A file name, such as `traces/0001`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME],
    length: u8,
}

impl Name {
    pub fn new(text: &str) -> Result<Self, Error> {
        let (bytes, length) = path(text, false)?;
        Ok(Self { bytes, length })
    }

    pub fn as_str(&self) -> &str {
        // Only validated ASCII is ever stored.
        core::str::from_utf8(&self.bytes[..usize::from(self.length)]).unwrap_or("")
    }

    /// In `dir` or one of its subdirectories.
    pub fn is_in(&self, dir: &Dir) -> bool {
        self.as_str().starts_with(dir.as_str())
    }
}

/// A directory: empty for the root, else ending in `/`, such as `traces/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dir {
    bytes: [u8; NAME],
    length: u8,
}

impl Dir {
    pub const ROOT: Self = Self {
        bytes: [0; NAME],
        length: 0,
    };

    pub fn new(text: &str) -> Result<Self, Error> {
        let (bytes, length) = path(text, true)?;
        Ok(Self { bytes, length })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..usize::from(self.length)]).unwrap_or("")
    }
}

/// A file as [`Store::list`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: Name,
    /// Unique in the store and increasing: a larger id is a newer file.
    pub id: u32,
    /// Chunks it holds; every one but the last is full.
    pub chunks: u32,
}

fn base(copy: u64) -> u64 {
    if copy == 0 { 0 } else { COPY_B }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    id: u64,
    generation: u64,
    next_id: u32,
    cursor: u32,
    chunks: u32,
    chunk: u32,
    crcs: [u32; BODY],
}

impl Header {
    fn table(&self) -> usize {
        (self.chunks as usize).div_ceil(LINKS)
    }

    fn body(&self) -> usize {
        DIR_SECTORS + self.table()
    }

    fn encode(&self, sector: &mut Sector) {
        sector.fill(0);
        sector[..16].copy_from_slice(MAGIC);
        put_u16(sector, 16, VERSION);
        put_u16(sector, 18, DIR_SECTORS as u16);
        put_u16(sector, 20, self.table() as u16);
        put_u64(sector, 24, self.id);
        put_u64(sector, 32, self.generation);
        put_u32(sector, 40, self.next_id);
        put_u32(sector, 44, self.cursor);
        put_u32(sector, 48, self.chunks);
        put_u32(sector, 52, self.chunk);
        for (k, crc) in self.crcs[..self.body()].iter().enumerate() {
            put_u32(sector, CRCS + 4 * k, *crc);
        }
        let checksum = crc32(&sector[..CRC]);
        put_u32(sector, CRC, checksum);
    }
}

enum Replica {
    Ours(Header),
    Foreign,
    Invalid,
}

/// A header copy, for a volume with `data` sectors after `DATA`.
fn decode(sector: &Sector, data: u64) -> Replica {
    if sector[..16] != MAGIC[..] || get_u32(sector, CRC) != crc32(&sector[..CRC]) {
        return Replica::Invalid;
    }
    let mut head = Header {
        id: get_u64(sector, 24),
        generation: get_u64(sector, 32),
        next_id: get_u32(sector, 40),
        cursor: get_u32(sector, 44),
        chunks: get_u32(sector, 48),
        chunk: get_u32(sector, 52),
        crcs: [0; BODY],
    };
    let fits = (1..=CHUNK).contains(&head.chunk)
        && (1..=MAX_CHUNKS as u32).contains(&head.chunks)
        && u64::from(head.chunks) * u64::from(head.chunk) <= data;
    if !fits
        || get_u16(sector, 16) != VERSION
        || usize::from(get_u16(sector, 18)) != DIR_SECTORS
        || usize::from(get_u16(sector, 20)) != head.table()
        || head.id == 0
        || head.next_id == 0
        || head.cursor >= head.chunks
        || sector[22..24].iter().any(|b| *b != 0)
        || sector[56..CRCS].iter().any(|b| *b != 0)
        || sector[CRCS + 4 * head.body()..CRC].iter().any(|b| *b != 0)
    {
        return Replica::Foreign;
    }
    for k in 0..head.body() {
        head.crcs[k] = get_u32(sector, CRCS + 4 * k);
    }
    Replica::Ours(head)
}

fn headers(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<[Option<Header>; 2], Error> {
    let data = media
        .owned_info()?
        .sectors
        .checked_sub(DATA)
        .ok_or(Error::Range)?;
    let mut copies = [None, None];
    for (copy, slot) in copies.iter_mut().enumerate() {
        media.owned_read(base(copy as u64), sector)?;
        match decode(sector, data) {
            Replica::Ours(head) => *slot = Some(head),
            Replica::Foreign => return Err(Error::Foreign),
            Replica::Invalid => {}
        }
    }
    Ok(copies)
}

/// The current metadata of the store in `media`.
pub fn mount(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Store, Error> {
    let [a, b] = headers(media, sector)?;
    let (copy, head, other) = match (a, b) {
        (Some(a), Some(b)) if b.generation > a.generation => (1, b, Some(a)),
        (Some(a), b) => (0, a, b),
        (None, Some(b)) => (1, b, None),
        (None, None) => return Err(Error::Uninitialized),
    };
    let other = other
        .filter(|o| (o.id, o.chunks, o.chunk) == (head.id, head.chunks, head.chunk))
        .map(|o| o.crcs);
    Ok(Store {
        copy,
        head,
        other,
        writer: 0,
    })
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Start an empty store with `CHUNK`-sector chunks, superseding any store
/// there. It goes to the copy that is not current, so a cut leaves the old
/// store; a foreign one is refused without writing.
pub fn format(media: &mut impl ReadWrite, seed: u64, sector: &mut Sector) -> Result<Store, Error> {
    format_chunks(media, seed, CHUNK, sector)
}

fn format_chunks(
    media: &mut impl ReadWrite,
    seed: u64,
    chunk: u32,
    sector: &mut Sector,
) -> Result<Store, Error> {
    let copies = headers(media, sector)?;
    let data = media.owned_info()?.sectors - DATA;
    let chunks = (data / u64::from(chunk)).min(MAX_CHUNKS as u64) as u32;
    if chunks == 0 {
        return Err(Error::Range);
    }
    let newest = (0..2)
        .filter_map(|copy| copies[copy].map(|head| (copy as u64, head.generation)))
        .max_by_key(|(_, generation)| *generation);
    let copy = newest.map_or(0, |(copy, _)| 1 - copy);
    let mut id = mix(seed ^ media.entropy());
    while id == 0 || copies.iter().flatten().any(|head| head.id == id) {
        id = mix(id);
    }
    let mut head = Header {
        id,
        generation: newest.map_or(0, |(_, generation)| generation) + 1,
        next_id: 1,
        cursor: 0,
        chunks,
        chunk,
        crcs: [0; BODY],
    };
    sector.fill(0);
    media.owned_write(base(copy), sector)?;
    for k in 0..head.body() {
        media.owned_write(base(copy) + 1 + k as u64, sector)?;
        head.crcs[k] = crc32(sector);
    }
    head.encode(sector);
    media.owned_write(base(copy), sector)?;
    Ok(Store {
        copy,
        head,
        other: None,
        writer: 0,
    })
}

fn decode_entry(bytes: &[u8]) -> Result<Option<(Name, u32)>, Error> {
    if bytes.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    let length = bytes[..NAME].iter().position(|b| *b == 0).unwrap_or(NAME);
    let text = core::str::from_utf8(&bytes[..length]).map_err(|_| Error::Corrupt)?;
    let name = Name::new(text).map_err(|_| Error::Corrupt)?;
    let id = get_u32(bytes, NAME);
    if id == 0
        || bytes[length..NAME]
            .iter()
            .chain(&bytes[NAME + 4..])
            .any(|b| *b != 0)
    {
        return Err(Error::Corrupt);
    }
    Ok(Some((name, id)))
}

/// A chunk table entry: the owning slot and the position in its file.
type Link = Option<(usize, u32)>;

fn put_link(sector: &mut Sector, chunk: u32, link: Link) {
    let at = 4 * (chunk as usize % LINKS);
    let (owner, position) = link.map_or((0, 0), |(slot, position)| (slot + 1, position));
    put_u16(sector, at, owner as u16);
    put_u16(sector, at + 2, position as u16);
}

/// Where a file's chunks are: how many, the first and the last.
struct Chain {
    count: u32,
    first: u32,
    last: u32,
}

/// The mounted metadata: the current header, what the other copy holds and
/// the open appender's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Store {
    copy: u64,
    head: Header,
    /// Body CRCs the other copy's valid header vouches for.
    other: Option<[u32; BODY]>,
    /// File id of the open appender, 0 for none.
    writer: u32,
}

impl Store {
    /// Changes with every metadata commit, by any writer.
    pub fn generation(&self) -> u64 {
        self.head.generation
    }

    pub fn store_id(&self) -> u64 {
        self.head.id
    }

    /// Data chunks in the volume.
    pub fn chunks(&self) -> u32 {
        self.head.chunks
    }

    /// Record sectors per chunk.
    pub fn chunk_sectors(&self) -> u32 {
        self.head.chunk
    }

    /// Body sector `k` of the current copy, verified against its header.
    fn body(&self, media: &mut impl ReadWrite, k: usize, sector: &mut Sector) -> Result<(), Error> {
        media.owned_read(base(self.copy) + 1 + k as u64, sector)?;
        if crc32(sector) != self.head.crcs[k] {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    /// Visit every entry slot with its file until `visit` returns true:
    /// at most `D` reads.
    fn scan(
        &self,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
        mut visit: impl FnMut(usize, Option<(Name, u32)>) -> bool,
    ) -> Result<(), Error> {
        for k in 0..DIR_SECTORS {
            self.body(media, k, sector)?;
            for slot in 0..PER_SECTOR {
                let file = decode_entry(&sector[slot * ENTRY..][..ENTRY])?;
                if visit(k * PER_SECTOR + slot, file) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The slot and id of `name`: at most `D` reads.
    fn find(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        sector: &mut Sector,
    ) -> Result<(usize, u32), Error> {
        let mut found = None;
        self.scan(media, sector, |slot, file| {
            if let Some((n, id)) = file
                && n == *name
            {
                found = Some((slot, id));
            }
            found.is_some()
        })?;
        found.ok_or(Error::NotFound)
    }

    /// Visit every chunk's table entry: `T` reads.
    fn table(
        &self,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
        mut visit: impl FnMut(u32, Link),
    ) -> Result<(), Error> {
        for t in 0..self.head.table() {
            self.body(media, DIR_SECTORS + t, sector)?;
            for i in 0..LINKS {
                let chunk = (t * LINKS + i) as u32;
                let owner = usize::from(get_u16(sector, 4 * i));
                let position = u32::from(get_u16(sector, 4 * i + 2));
                match owner {
                    0 if position == 0 => {
                        if chunk < self.head.chunks {
                            visit(chunk, None);
                        }
                    }
                    1..=CAPACITY if chunk < self.head.chunks => {
                        visit(chunk, Some((owner - 1, position)));
                    }
                    _ => return Err(Error::Corrupt),
                }
            }
        }
        Ok(())
    }

    /// The free chunk nearest after the cursor, wrapping: `T` reads.
    fn free_chunk(
        &self,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
    ) -> Result<Option<u32>, Error> {
        let (cursor, chunks) = (self.head.cursor, self.head.chunks);
        let distance = |chunk: u32| (chunk + chunks - cursor) % chunks;
        let mut best: Option<u32> = None;
        self.table(media, sector, |chunk, link| {
            if link.is_none() && best.is_none_or(|b| distance(chunk) < distance(b)) {
                best = Some(chunk);
            }
        })?;
        Ok(best)
    }

    /// The chunks of the file in `slot`: `T` reads.
    fn chain(
        &self,
        media: &mut impl ReadWrite,
        slot: usize,
        sector: &mut Sector,
    ) -> Result<Chain, Error> {
        let (mut count, mut first, mut last) = (0, None, None);
        self.table(media, sector, |chunk, link| {
            if let Some((owner, position)) = link
                && owner == slot
            {
                count += 1;
                if position == 0 {
                    first = Some(chunk);
                }
                if last.is_none_or(|(p, _)| position > p) {
                    last = Some((position, chunk));
                }
            }
        })?;
        match (first, last) {
            (Some(first), Some((position, last))) if position + 1 == count => {
                Ok(Chain { count, first, last })
            }
            _ => Err(Error::Corrupt),
        }
    }

    /// The chunk at `position` of the file in `slot`: `T` reads.
    fn locate(
        &self,
        media: &mut impl ReadWrite,
        slot: usize,
        position: u32,
        sector: &mut Sector,
    ) -> Result<u32, Error> {
        let mut found = None;
        self.table(media, sector, |chunk, link| {
            if link == Some((slot, position)) {
                found = Some(chunk);
            }
        })?;
        found.ok_or(Error::Corrupt)
    }

    /// The payload length of `chunk`'s record `index` when it is record
    /// `number` of file `id`: one read.
    fn record(
        &self,
        media: &mut impl ReadWrite,
        id: u32,
        chunk: u32,
        index: u32,
        number: u32,
        sector: &mut Sector,
    ) -> Result<Option<usize>, Error> {
        let at = DATA + u64::from(chunk) * u64::from(self.head.chunk) + u64::from(index);
        media.owned_read(at, sector)?;
        let length = usize::from(get_u16(sector, 20));
        let valid = sector[..4] == RECORD[..]
            && get_u64(sector, 4) == self.head.id
            && get_u32(sector, 12) == id
            && get_u32(sector, 16) == number
            && (1..=PAYLOAD).contains(&length)
            && get_u32(sector, CRC) == crc32(&sector[..CRC]);
        Ok(valid.then_some(length))
    }

    /// Copy-on-write commit into the other copy: the body sectors in
    /// `change` pass through `edit`, the others are copied where that copy
    /// does not already hold them. Fails with `Stale`, writing nothing,
    /// unless this metadata is still current.
    fn commit(
        &mut self,
        media: &mut impl ReadWrite,
        change: u32,
        next_id: u32,
        cursor: u32,
        sector: &mut Sector,
        mut edit: impl FnMut(usize, &mut Sector),
    ) -> Result<(), Error> {
        let current = mount(media, sector)?;
        if (current.copy, current.head) != (self.copy, self.head) {
            return Err(Error::Stale);
        }
        // Even a header this value does not trust may still be valid on the
        // medium (a failed earlier commit, another store's), and it must not
        // vouch for a body being rewritten.
        let target = 1 - self.copy;
        let held = self.other.take();
        sector.fill(0);
        media.owned_write(base(target), sector)?;
        let mut head = self.head;
        for k in 0..head.body() {
            let changing = change & (1 << k) != 0;
            let holds = held.map(|crcs| crcs[k]);
            if !changing && holds == Some(self.head.crcs[k]) {
                continue;
            }
            self.body(media, k, sector)?;
            if changing {
                edit(k, sector);
            }
            head.crcs[k] = crc32(sector);
            if holds != Some(head.crcs[k]) {
                media.owned_write(base(target) + 1 + k as u64, sector)?;
            }
        }
        head.generation += 1;
        head.next_id = next_id;
        head.cursor = cursor;
        head.encode(sector);
        media.owned_write(base(target), sector)?;
        self.other = Some(self.head.crcs);
        self.head = head;
        self.copy = target;
        Ok(())
    }

    /// Visit the files in `dir` and its subdirectories, in slot order.
    pub fn list(
        &self,
        media: &mut impl ReadWrite,
        dir: &Dir,
        sector: &mut Sector,
        mut visit: impl FnMut(&Entry),
    ) -> Result<(), Error> {
        let mut chunks = [0u32; CAPACITY];
        self.table(media, sector, |_, link| {
            if let Some((slot, _)) = link {
                chunks[slot] += 1;
            }
        })?;
        self.scan(media, sector, |slot, file| {
            if let Some((name, id)) = file
                && name.is_in(dir)
            {
                visit(&Entry {
                    name,
                    id,
                    chunks: chunks[slot],
                });
            }
            false
        })
    }

    /// Create `name` empty, holding one chunk, and open it for appending in
    /// place of any open appender. With `rollover`, a full store first
    /// deletes the oldest file in that directory.
    pub fn create(
        &mut self,
        media: &mut impl ReadWrite,
        name: &Name,
        rollover: Option<Dir>,
        sector: &mut Sector,
    ) -> Result<Appender, Error> {
        self.writer = 0;
        for _ in 0..2 {
            let (mut exists, mut free) = (false, None);
            self.scan(media, sector, |slot, file| {
                match file {
                    Some((n, _)) => exists |= n == *name,
                    None => {
                        free.get_or_insert(slot);
                    }
                }
                false
            })?;
            if exists {
                return Err(Error::Exists);
            }
            let chunk = match free {
                Some(_) => self.free_chunk(media, sector)?,
                None => None,
            };
            if let (Some(slot), Some(chunk)) = (free, chunk) {
                let id = self.head.next_id;
                let (entry_k, link_k) = (slot / PER_SECTOR, DIR_SECTORS + chunk as usize / LINKS);
                let cursor = (chunk + 1) % self.head.chunks;
                self.commit(
                    media,
                    (1 << entry_k) | (1 << link_k),
                    id + 1,
                    cursor,
                    sector,
                    |k, s| {
                        if k == entry_k {
                            let bytes = &mut s[(slot % PER_SECTOR) * ENTRY..][..ENTRY];
                            bytes.fill(0);
                            bytes[..name.bytes.len()].copy_from_slice(&name.bytes);
                            put_u32(bytes, NAME, id);
                        }
                        if k == link_k {
                            put_link(s, chunk, Some((slot, 0)));
                        }
                    },
                )?;
                self.writer = id;
                return Ok(Appender::new(slot, id, chunk, 0, 0, rollover));
            }
            match rollover {
                Some(dir) if self.delete_oldest(media, &dir, sector)?.is_some() => {}
                _ => return Err(Error::Full),
            }
        }
        // A deleted file always frees an entry and a chunk.
        Err(Error::Corrupt)
    }

    /// Open `name` for appending after its last record, in place of any
    /// open appender.
    pub fn open_append(
        &mut self,
        media: &mut impl ReadWrite,
        name: &Name,
        rollover: Option<Dir>,
        sector: &mut Sector,
    ) -> Result<Appender, Error> {
        self.writer = 0;
        let (slot, id) = self.find(media, name, sector)?;
        let chain = self.chain(media, slot, sector)?;
        let position = chain.count - 1;
        let (mut low, mut high) = (0, self.head.chunk);
        while low < high {
            let middle = (low + high) / 2;
            let number = position * self.head.chunk + middle;
            if self
                .record(media, id, chain.last, middle, number, sector)?
                .is_some()
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        self.writer = id;
        Ok(Appender::new(slot, id, chain.last, position, low, rollover))
    }

    /// A reader of `name` from its first record.
    pub fn reader(
        &self,
        media: &mut impl ReadWrite,
        name: &Name,
        sector: &mut Sector,
    ) -> Result<Reader, Error> {
        let (slot, id) = self.find(media, name, sector)?;
        let chain = self.chain(media, slot, sector)?;
        Ok(Reader {
            store: self.head.id,
            generation: self.head.generation,
            slot,
            id,
            chunks: chain.count,
            chunk: chain.first,
            position: 0,
            index: 0,
        })
    }

    /// Delete `name`; an appender writing it fails with `Stale` from now on.
    pub fn delete(
        &mut self,
        media: &mut impl ReadWrite,
        name: &Name,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        let (slot, id) = self.find(media, name, sector)?;
        self.remove(media, slot, sector)?;
        if id == self.writer {
            self.writer = 0;
        }
        Ok(())
    }

    /// Delete the oldest file in `dir` and its subdirectories other than the
    /// open appender's, and return its name; `None` when there is none.
    pub fn delete_oldest(
        &mut self,
        media: &mut impl ReadWrite,
        dir: &Dir,
        sector: &mut Sector,
    ) -> Result<Option<Name>, Error> {
        let mut oldest: Option<(usize, Name, u32)> = None;
        self.scan(media, sector, |slot, file| {
            if let Some((name, id)) = file
                && id != self.writer
                && name.is_in(dir)
                && oldest.is_none_or(|(_, _, o)| id < o)
            {
                oldest = Some((slot, name, id));
            }
            false
        })?;
        let Some((slot, name, _)) = oldest else {
            return Ok(None);
        };
        self.remove(media, slot, sector)?;
        Ok(Some(name))
    }

    /// Clear the entry in `slot` and free its chunks in one commit.
    fn remove(
        &mut self,
        media: &mut impl ReadWrite,
        slot: usize,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        let tables = ((1u32 << self.head.table()) - 1) << DIR_SECTORS;
        let entry_k = slot / PER_SECTOR;
        let (next_id, cursor) = (self.head.next_id, self.head.cursor);
        self.commit(
            media,
            tables | (1 << entry_k),
            next_id,
            cursor,
            sector,
            |k, s| {
                if k == entry_k {
                    s[(slot % PER_SECTOR) * ENTRY..][..ENTRY].fill(0);
                }
                if k >= DIR_SECTORS {
                    for i in 0..LINKS {
                        if usize::from(get_u16(s, 4 * i)) == slot + 1 {
                            s[4 * i..4 * i + 4].fill(0);
                        }
                    }
                }
            },
        )
    }
}

/// The open file of a store. Full records are written as they fill;
/// [`Appender::sync`] writes the partly filled one.
#[derive(Debug)]
pub struct Appender {
    slot: usize,
    id: u32,
    chunk: u32,
    position: u32,
    index: u32,
    fill: usize,
    buffer: [u8; PAYLOAD],
    rollover: Option<Dir>,
}

impl Appender {
    fn new(
        slot: usize,
        id: u32,
        chunk: u32,
        position: u32,
        index: u32,
        rollover: Option<Dir>,
    ) -> Self {
        Self {
            slot,
            id,
            chunk,
            position,
            index,
            fill: 0,
            buffer: [0; PAYLOAD],
            rollover,
        }
    }

    /// Append `bytes`; each `PAYLOAD` of them writes a record. After an
    /// error, open the file again: it ends after its last written record.
    pub fn append(
        &mut self,
        store: &mut Store,
        media: &mut impl ReadWrite,
        mut bytes: &[u8],
        sector: &mut Sector,
    ) -> Result<(), Error> {
        if store.writer != self.id {
            return Err(Error::Stale);
        }
        while !bytes.is_empty() {
            let n = (PAYLOAD - self.fill).min(bytes.len());
            self.buffer[self.fill..self.fill + n].copy_from_slice(&bytes[..n]);
            self.fill += n;
            bytes = &bytes[n..];
            if self.fill == PAYLOAD {
                self.flush(store, media, sector)?;
            }
        }
        Ok(())
    }

    /// Make every appended byte durable.
    pub fn sync(
        &mut self,
        store: &mut Store,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        if store.writer != self.id {
            return Err(Error::Stale);
        }
        if self.fill > 0 {
            self.flush(store, media, sector)?;
        }
        Ok(())
    }

    fn flush(
        &mut self,
        store: &mut Store,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        let chunk = store.head.chunk;
        if self.index == chunk {
            self.extend(store, media, sector)?;
        }
        sector.fill(0);
        sector[..4].copy_from_slice(RECORD);
        put_u64(sector, 4, store.head.id);
        put_u32(sector, 12, self.id);
        put_u32(sector, 16, self.position * chunk + self.index);
        put_u16(sector, 20, self.fill as u16);
        sector[RECORD_HEADER..RECORD_HEADER + self.fill].copy_from_slice(&self.buffer[..self.fill]);
        let checksum = crc32(&sector[..CRC]);
        put_u32(sector, CRC, checksum);
        let at = DATA + u64::from(self.chunk) * u64::from(chunk) + u64::from(self.index);
        media.owned_write(at, sector)?;
        self.index += 1;
        self.fill = 0;
        Ok(())
    }

    /// Give the file another chunk, rolling over if the store is full.
    fn extend(
        &mut self,
        store: &mut Store,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        let next = match store.free_chunk(media, sector)? {
            Some(chunk) => chunk,
            None => {
                let dir = self.rollover.ok_or(Error::Full)?;
                if store.delete_oldest(media, &dir, sector)?.is_none() {
                    return Err(Error::Full);
                }
                store.free_chunk(media, sector)?.ok_or(Error::Corrupt)?
            }
        };
        let link_k = DIR_SECTORS + next as usize / LINKS;
        let (slot, position) = (self.slot, self.position + 1);
        let (next_id, cursor) = (store.head.next_id, (next + 1) % store.head.chunks);
        store.commit(media, 1 << link_k, next_id, cursor, sector, |_, s| {
            put_link(s, next, Some((slot, position)));
        })?;
        self.chunk = next;
        self.position = position;
        self.index = 0;
        Ok(())
    }
}

/// A sequential reader of one file's records, valid while the store's
/// metadata does not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reader {
    store: u64,
    generation: u64,
    slot: usize,
    id: u32,
    chunks: u32,
    chunk: u32,
    position: u32,
    index: u32,
}

impl Reader {
    /// The record [`Reader::next`] reads.
    pub fn record(&self, store: &Store) -> u32 {
        self.position * store.head.chunk + self.index
    }

    fn check(&self, store: &Store) -> Result<(), Error> {
        if (self.store, self.generation) != (store.head.id, store.head.generation) {
            return Err(Error::Stale);
        }
        Ok(())
    }

    /// Continue at record `record`; past the end, `next` finds none.
    pub fn seek(
        &mut self,
        store: &Store,
        media: &mut impl ReadWrite,
        record: u32,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        self.check(store)?;
        let position = record / store.head.chunk;
        if position >= self.chunks {
            (self.position, self.index) = (self.chunks, 0);
            return Ok(());
        }
        if position != self.position {
            self.chunk = store.locate(media, self.slot, position, sector)?;
        }
        (self.position, self.index) = (position, record % store.head.chunk);
        Ok(())
    }

    /// Copy the next record's payload to `out` and return its length; `None`
    /// at the end of the file.
    pub fn next(
        &mut self,
        store: &Store,
        media: &mut impl ReadWrite,
        out: &mut [u8; PAYLOAD],
        sector: &mut Sector,
    ) -> Result<Option<usize>, Error> {
        self.check(store)?;
        let chunk = store.head.chunk;
        if self.index == chunk && self.position + 1 < self.chunks {
            self.chunk = store.locate(media, self.slot, self.position + 1, sector)?;
            (self.position, self.index) = (self.position + 1, 0);
        }
        if self.position >= self.chunks || self.index == chunk {
            return Ok(None);
        }
        let number = self.position * chunk + self.index;
        match store.record(media, self.id, self.chunk, self.index, number, sector)? {
            Some(length) => {
                out[..length].copy_from_slice(&sector[RECORD_HEADER..RECORD_HEADER + length]);
                self.index += 1;
                Ok(Some(length))
            }
            None if self.position + 1 == self.chunks => Ok(None),
            None => Err(Error::Corrupt),
        }
    }
}

#[cfg(test)]
mod tests;
