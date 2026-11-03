//! Saved rides the rider discarded or deleted, kept on the FILES volume in one
//! table, `rides/deleted.tbl`. Nothing is erased: the journals keep every
//! ride, and the list, the totals and the phone leave out the rides this
//! table names. Removing a record, or the file, brings a ride back.
//!
//! The table, little-endian, is a header slot and then one slot per hidden
//! ride. Every slot is [`SLOT`] bytes and ends in the CRC32 of the bytes
//! before it, so a damaged slot costs only its own ride.
//!
//! | Header bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VDEL` |
//! | 4 | table version, 1 |
//! | 6..8 | stride, 32 |
//!
//! | Slot bytes | Field |
//! | --- | --- |
//! | 0 | record version, 1 |
//! | 1 | why: 1 discarded at the end, 2 deleted on VANA |
//! | 2 | journal: `i` internal, `b` bulk |
//! | 4..8 | ride id in that journal |
//! | 8..12 | ride start, UTC seconds; 0 unknown |
//! | 12..16 | ride time, seconds |
//! | 16..24 | the bulk log's store id; 0 for the internal journal |
//! | 24..28 | when it was hidden, UTC seconds; 0 unknown |
//! | 28..32 | CRC32 of bytes 0..28 |
//!
//! A record hides the ride whose journal, store, id, start second and ride
//! time all match, so an undated ride is hidden too, and a stale record left
//! after older firmware cleared a journal hides nothing new in practice.
//!
//! A save writes the whole table again, copy-on-write, its slots byte for
//! byte and in their order with the new one last, so a table that only gains
//! rides only grows at its end and a ride trace can take it back to its
//! length at power-on. The commit is one directory write, so a cut leaves the
//! old table or the new. Saving a ride twice changes nothing.
//! The table keeps up to [`KEEP`] rides and never drops one on its own.
//! A table this version would not write, such as one with another stride
//! from a later version, hides nothing and is never written over. Older firmware lists only `.workout` and `.facts` files under
//! `rides/`, so it never opens or deletes this one.
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::{Crc32, crc32};
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::{files, volume};

pub const TABLE: &str = "rides/deleted.tbl";
const MAGIC: &[u8; 4] = b"VDEL";
const TABLE_VERSION: u8 = 1;
const VERSION: u8 = 1;
pub const SLOT: usize = 32;
/// Rides the table can hide, in 8 KB.
pub const KEEP: usize = 256;
/// Bytes the composition lends to hold the table in memory.
pub const SHELF_BYTES: usize = KEEP * SLOT;
/// The largest table: the header and [`KEEP`] slots.
const MAX_BYTES: usize = (KEEP + 1) * SLOT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Discarded on END RIDE?, right after it was saved.
    Discarded,
    /// Deleted from the rides list.
    Deleted,
}

impl Reason {
    fn byte(self) -> u8 {
        match self {
            Self::Discarded => 1,
            Self::Deleted => 2,
        }
    }
}

/// Which saved ride a record hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ride {
    /// `b'i'` for the internal journal, `b'b'` for the bulk log.
    pub journal: u8,
    pub ride_id: u32,
    /// The bulk log's store id, which a new log on the card changes.
    pub store: u64,
    /// UTC seconds; 0 when the clock was unknown.
    pub started_utc_s: u32,
    pub ride_s: u32,
}

impl Ride {
    pub fn new(
        journal: u8,
        store: u64,
        ride_id: u32,
        started_utc_ms: Option<u64>,
        active_ms: u64,
    ) -> Self {
        let seconds = |ms: u64| u32::try_from(ms / 1000).unwrap_or(u32::MAX);
        Self {
            journal,
            ride_id,
            store,
            started_utc_s: started_utc_ms.map_or(0, seconds),
            ride_s: seconds(active_ms),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub ride: Ride,
    pub reason: Reason,
    /// UTC seconds; 0 unknown.
    pub hidden_utc_s: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Volume(volume::Error),
    Files(files::Error),
    /// The file is not a table this version writes: no magic, a damaged
    /// header or another stride. It is left as it is.
    Table,
    /// The table holds [`KEEP`] rides.
    Full,
}

impl From<volume::Error> for Error {
    fn from(error: volume::Error) -> Self {
        Self::Volume(error)
    }
}

impl From<files::Error> for Error {
    fn from(error: files::Error) -> Self {
        Self::Files(error)
    }
}

fn table_name() -> files::Name {
    files::Name::new(TABLE).unwrap()
}

fn seal(slot: &mut [u8]) {
    let end = slot.len() - 4;
    let crc = crc32(&slot[..end]);
    put_u32(slot, end, crc);
}

fn sound(slot: &[u8]) -> bool {
    let end = slot.len() - 4;
    crc32(&slot[..end]) == get_u32(slot, end)
}

fn header() -> [u8; SLOT] {
    let mut slot = [0; SLOT];
    slot[..4].copy_from_slice(MAGIC);
    slot[4] = TABLE_VERSION;
    put_u16(&mut slot, 6, SLOT as u16);
    seal(&mut slot);
    slot
}

pub fn encode(record: &Record) -> [u8; SLOT] {
    let mut slot = [0; SLOT];
    let ride = &record.ride;
    slot[0] = VERSION;
    slot[1] = record.reason.byte();
    slot[2] = ride.journal;
    put_u32(&mut slot, 4, ride.ride_id);
    put_u32(&mut slot, 8, ride.started_utc_s);
    put_u32(&mut slot, 12, ride.ride_s);
    put_u64(&mut slot, 16, ride.store);
    put_u32(&mut slot, 24, record.hidden_utc_s);
    seal(&mut slot);
    slot
}

/// A slot of any record version from 1 on, or `None` if damaged, empty or of
/// a reason this version does not know.
pub fn decode(slot: &[u8]) -> Option<Record> {
    if slot.len() < SLOT || slot[0] < 1 || !sound(slot) {
        return None;
    }
    let reason = match slot[1] {
        1 => Reason::Discarded,
        2 => Reason::Deleted,
        _ => return None,
    };
    Some(Record {
        ride: Ride {
            journal: slot[2],
            ride_id: get_u32(slot, 4),
            store: get_u64(slot, 16),
            started_utc_s: get_u32(slot, 8),
            ride_s: get_u32(slot, 12),
        },
        reason,
        hidden_utc_s: get_u32(slot, 24),
    })
}

/// Whether `bytes` start with a header this version writes over.
fn ours(bytes: &[u8]) -> bool {
    bytes.len() >= SLOT
        && bytes[..4] == MAGIC[..]
        && bytes[4] >= 1
        && usize::from(get_u16(bytes, 6)) == SLOT
        && sound(&bytes[..SLOT])
}

/// Whether a table file of `length` bytes that starts with `head` is one
/// this version reads and writes.
fn writable_table(head: &[u8], length: usize) -> bool {
    ours(head) && length.is_multiple_of(SLOT) && length <= MAX_BYTES
}

/// The rides a whole table held in `bytes` hides; none for a table this
/// version would not read.
pub fn decode_table(bytes: &[u8]) -> impl Iterator<Item = Record> + '_ {
    let slots = if ours(bytes) { bytes } else { &[] };
    slots
        .as_chunks::<SLOT>()
        .0
        .iter()
        .skip(1)
        .filter_map(|slot| decode(slot))
}

/// The table in memory, ordered by ride for lookup, in storage the
/// composition lends: [`SLOT`] bytes a record, each the slot as stored.
pub struct Shelf {
    slots: &'static mut [u8],
    len: usize,
}

impl Shelf {
    /// An empty shelf in `slots` (a multiple of [`SLOT`] bytes).
    pub fn new(slots: &'static mut [u8]) -> Self {
        Self { slots, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.slots.len() / SLOT
    }

    /// Whether the next ride would not fit.
    pub fn full(&self) -> bool {
        self.len >= self.capacity().min(KEEP)
    }

    fn slot(&self, index: usize) -> &[u8] {
        &self.slots[index * SLOT..][..SLOT]
    }

    /// The ride of a slot `put` checked, without its CRC again.
    fn ride(&self, index: usize) -> Ride {
        let slot = self.slot(index);
        Ride {
            journal: slot[2],
            ride_id: get_u32(slot, 4),
            store: get_u64(slot, 16),
            started_utc_s: get_u32(slot, 8),
            ride_s: get_u32(slot, 12),
        }
    }

    fn search(&self, ride: &Ride) -> Result<usize, usize> {
        let (mut low, mut high) = (0, self.len);
        while low < high {
            let middle = low + (high - low) / 2;
            match self.ride(middle).cmp(ride) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Ok(middle),
            }
        }
        Err(low)
    }

    pub fn hides(&self, ride: &Ride) -> bool {
        self.search(ride).is_ok()
    }

    /// Whether a record names the ride `ride_id` of `journal` and `store`,
    /// whatever its start and ride time: for a reader that knows only the id.
    pub fn names(&self, journal: u8, store: u64, ride_id: u32) -> bool {
        let first = Ride {
            journal,
            ride_id,
            store,
            started_utc_s: 0,
            ride_s: 0,
        };
        let at = self.search(&first).unwrap_or_else(|at| at);
        (at..self.len)
            .map(|index| self.ride(index))
            .take_while(|ride| {
                (ride.journal, ride.ride_id, ride.store) == (journal, ride_id, store)
            })
            .next()
            .is_some()
    }

    /// Keep the slot of a record read from the table, or saved; returns
    /// whether it was new.
    fn put(&mut self, slot: &[u8]) -> bool {
        let Some(record) = decode(slot) else {
            return false;
        };
        let Err(index) = self.search(&record.ride) else {
            return false;
        };
        if self.len == self.capacity() {
            return false;
        }
        self.slots
            .copy_within(index * SLOT..self.len * SLOT, (index + 1) * SLOT);
        self.slots[index * SLOT..][..SLOT].copy_from_slice(&slot[..SLOT]);
        self.len += 1;
        true
    }

    /// Keep only the rides `keep` accepts.
    fn retain(&mut self, keep: impl Fn(&Ride) -> bool) {
        let mut kept = 0;
        for index in 0..self.len {
            if keep(&self.ride(index)) {
                self.slots
                    .copy_within(index * SLOT..(index + 1) * SLOT, kept * SLOT);
                kept += 1;
            }
        }
        self.len = kept;
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Every hidden ride, in key order.
    pub fn records(&self) -> impl Iterator<Item = Record> + '_ {
        (0..self.len).filter_map(|index| decode(self.slot(index)))
    }
}

/// Read the whole table onto `shelf`, which it clears first. Returns whether
/// a table was found. The shelf keeps the table's sound slots byte for byte;
/// one past its capacity is left out, as the table never holds more.
pub fn read(media: &mut impl ReadWrite, shelf: &mut Shelf) -> Result<bool, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    shelf.clear();
    let Some(entry) = dir.find(&mut store, &table_name(), &mut sector)? else {
        return Ok(false);
    };
    if entry.length == 0 {
        return Err(Error::Table);
    }
    let length = entry.length as usize;
    let mut at = 0;
    while at < length {
        let read = dir.read(&mut store, &entry, at as u32, &mut bytes, &mut sector)?;
        if read == 0 {
            break;
        }
        let read = read.min(length - at);
        let chunk = &bytes[..read];
        let slots = if at == 0 {
            if !writable_table(chunk, length) {
                return Err(Error::Table);
            }
            &chunk[SLOT..]
        } else {
            chunk
        };
        for slot in slots.as_chunks::<SLOT>().0 {
            shelf.put(slot);
        }
        at += read;
    }
    Ok(true)
}

/// The table being written, a sector at a time.
struct Out {
    bytes: files::Sector,
    used: usize,
    crc: Crc32,
    writer: files::Writer,
    failed: Option<files::Error>,
}

impl Out {
    fn emit(&mut self, media: &mut impl ReadWrite, slot: &[u8]) {
        self.bytes[self.used..self.used + SLOT].copy_from_slice(slot);
        self.used += SLOT;
        if self.used == SECTOR_SIZE {
            self.flush(media);
        }
    }

    fn flush(&mut self, media: &mut impl ReadWrite) {
        let bytes = &self.bytes[..self.used];
        self.crc = self.crc.update(bytes);
        if let Err(error) = self.writer.write(media, bytes) {
            self.failed.get_or_insert(error);
        }
        self.used = 0;
    }

    fn finish(
        mut self,
        media: &mut impl ReadWrite,
        sector: &mut files::Sector,
    ) -> Result<(), Error> {
        if self.used > 0 {
            self.flush(media);
        }
        if let Some(error) = self.failed {
            return Err(error.into());
        }
        self.writer.commit(media, self.crc.finish(), sector)?;
        Ok(())
    }
}

/// Visit the slots of the table `entry`, the header first, one sector read
/// at a time into `bytes`.
fn each<M: ReadWrite>(
    store: &mut volume::Volume<'_, M>,
    dir: &files::Dir,
    entry: &files::Entry,
    sector: &mut files::Sector,
    bytes: &mut files::Sector,
    mut visit: impl FnMut(&mut volume::Volume<'_, M>, usize, &[u8]),
) -> Result<(), Error> {
    let length = entry.length as usize;
    let mut index = 0;
    for at in (0..length).step_by(SECTOR_SIZE) {
        let read = dir.read(store, entry, at as u32, bytes, sector)?;
        for slot in bytes[..read.min(length - at)].as_chunks::<SLOT>().0 {
            visit(store, index, slot);
            index += 1;
        }
    }
    Ok(())
}

/// Write the table again: its slots byte for byte and in their order,
/// without those of the rides `drops` names, then `added` last, so a table
/// that only gains rides only grows at its end. The shelf then matches.
/// Without a table yet, a file is made only to add a ride.
fn rewrite(
    media: &mut impl ReadWrite,
    shelf: &mut Shelf,
    added: Option<&[u8; SLOT]>,
    drops: impl Fn(&Ride) -> bool,
) -> Result<(), Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let name = table_name();
    let found = dir.find(&mut store, &name, &mut sector)?;
    let dropped = |slot: &[u8]| decode(slot).is_some_and(|record| drops(&record.ride));
    let mut kept = 0;
    if let Some(entry) = &found {
        let read = dir.read(&mut store, entry, 0, &mut bytes, &mut sector)?;
        let length = entry.length as usize;
        if !writable_table(&bytes[..read], length) {
            return Err(Error::Table);
        }
        each(
            &mut store,
            &dir,
            entry,
            &mut sector,
            &mut bytes,
            |_, index, slot| {
                kept += usize::from(index > 0 && !dropped(slot));
            },
        )?;
    } else if added.is_none() {
        return Ok(());
    }
    let length = SLOT * (1 + kept + usize::from(added.is_some()));
    if length > MAX_BYTES {
        return Err(Error::Full);
    }
    let mut out = Out {
        bytes: [0; SECTOR_SIZE],
        used: 0,
        crc: Crc32::new(),
        writer: dir.begin(&mut store, &name, length as u32, &mut sector)?,
        failed: None,
    };
    match &found {
        Some(entry) => each(
            &mut store,
            &dir,
            entry,
            &mut sector,
            &mut bytes,
            |store, index, slot| {
                if index == 0 || !dropped(slot) {
                    out.emit(store, slot);
                }
            },
        )?,
        None => out.emit(&mut store, &header()),
    }
    if let Some(slot) = added {
        out.emit(&mut store, slot);
    }
    out.finish(&mut store, &mut sector)?;
    shelf.retain(|ride| !drops(ride));
    if let Some(slot) = added {
        shelf.put(slot);
    }
    Ok(())
}

/// Hide `record`'s ride. Hiding a hidden ride writes nothing.
pub fn save(media: &mut impl ReadWrite, shelf: &mut Shelf, record: &Record) -> Result<(), Error> {
    if shelf.hides(&record.ride) {
        return Ok(());
    }
    if shelf.full() {
        return Err(Error::Full);
    }
    rewrite(media, shelf, Some(&encode(record)), |_| false)
}

/// Drop every record of `journal`, as its ride ids start over.
pub fn forget(media: &mut impl ReadWrite, shelf: &mut Shelf, journal: u8) -> Result<(), Error> {
    if !shelf.records().any(|record| record.ride.journal == journal) {
        return Ok(());
    }
    rewrite(media, shelf, None, |ride| ride.journal == journal)
}

#[cfg(test)]
mod tests;
