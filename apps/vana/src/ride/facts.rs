//! What VANA knew about a saved ride that its journal does not hold, kept on
//! the FILES volume in one table, `rides/facts.tbl`, of up to [`KEEP`] rides.
//!
//! The table, little-endian, is a header slot and then one slot per ride,
//! oldest saved first. Every slot is [`SLOT`] bytes and ends in the CRC32 of
//! the bytes before it, so a damaged slot costs only its own ride.
//!
//! | Header bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VFTB` |
//! | 4 | table version, 1 |
//! | 6..8 | stride, 32 |
//! | 28..32 | CRC32 of bytes 0..28 |
//!
//! | Slot bytes | Field |
//! | --- | --- |
//! | 0 | record version, 1 |
//! | 2 | journal: `i` internal, `b` bulk |
//! | 3 | flags: 1 profile, 2 HR recovery, 4 climbs |
//! | 4..8 | ride id in that journal |
//! | 8..16 | ride start, UTC ms |
//! | 16..18 | FTP at the ride, W |
//! | 18..20 | LTHR at the ride, BPM |
//! | 20..22 | HR recovery: the fall over the minute after, BPM (i16) |
//! | 22..24 | climbs finished |
//! | 24..26 | climbing of those climbs, m |
//! | 28..32 | CRC32 of bytes 0..28 |
//!
//! A slot of another record version reads as damaged. A file without the
//! magic, of another table version or stride, or not a whole number of slots, is
//! foreign: it is left alone, with nothing saved over it. A current table
//! whose header fails its CRC keeps its slots, and the next rewrite gives it
//! a fresh header.
//!
//! The store has no append in place: a save writes the whole table again,
//! copy-on-write, with the ride appended last. It drops the ride's earlier
//! slot, slots that fail their CRC and, past [`KEEP`], the oldest saved. The
//! commit is one directory write, so a cut leaves the old table or the new.
//!
//! Ride ids restart when a journal is cleared or initialized, and `forget`
//! then drops that journal's rides; the start time guards the rest. So a ride
//! whose start time is unknown has no facts. A profile out of range reads as
//! none and leaves the other fields.
use super::zones::TrainingProfile;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::{Crc32, crc32};
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::{files, volume};

pub const TABLE: &str = "rides/facts.tbl";
pub const VERSION: u8 = 2;
const TABLE_MAGIC: &[u8; 4] = b"VFTB";
const TABLE_VERSION: u8 = 2;
/// The file, its magic and its version byte at offset 4: what tells this
/// table in another version from other bytes.
pub const STAMP: (&str, &[u8; 4], u8) = (TABLE, TABLE_MAGIC, TABLE_VERSION);
pub const SLOT: usize = 32;
const FIELDS: usize = 2;
/// Rides the table keeps: FORM's 120 days at eight rides a day, in 32 KB.
pub const KEEP: usize = 1000;
/// Bytes the composition lends to hold the table in memory.
pub const SHELF_BYTES: usize = KEEP * SLOT;
/// Table sectors one [`read`] visits.
pub const PIECE: usize = 4;

const PROFILE: u8 = 1;
const RECOVERY: u8 = 2;
const CLIMBS: u8 = 4;

/// The climbs a ride finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Climbing {
    pub count: u16,
    /// Their gain added up, saturating.
    pub gain_m: u16,
}

impl Climbing {
    pub fn add(&mut self, gain_dm: u32) {
        let gain_m = u16::try_from(gain_dm.saturating_add(5) / 10).unwrap_or(u16::MAX);
        self.count = self.count.saturating_add(1);
        self.gain_m = self.gain_m.saturating_add(gain_m);
    }
}

/// What a saved ride keeps beside its journal; each part is unknown when the
/// ride did not measure it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The FTP and LTHR in force when the ride was saved.
    pub profile: Option<TrainingProfile>,
    /// Beats per minute the heart rate fell in the minute after the ride;
    /// negative when it rose.
    pub recovery_bpm: Option<i16>,
    pub climbing: Option<Climbing>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    /// `b'i'` for the internal journal, `b'b'` for the bulk log.
    pub journal: u8,
    pub ride_id: u32,
    pub started_utc_ms: u64,
    pub facts: Facts,
}

impl Record {
    /// Whether this is the record of a journal ride that started at
    /// `started_utc_ms`; never of an undated ride. The ride index keeps whole
    /// seconds.
    pub fn of(&self, journal: u8, ride_id: u32, started_utc_ms: Option<u64>) -> bool {
        self.journal == journal
            && self.ride_id == ride_id
            && started_utc_ms.is_some_and(|ms| ms / 1000 == self.started_utc_ms / 1000)
    }

    fn key(&self) -> (u8, u32) {
        (self.journal, self.ride_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Volume(volume::Error),
    Files(files::Error),
    /// The table file is foreign; it is left as it is.
    Table,
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

fn encode_fields(record: &Record, slot: &mut [u8]) {
    let at = FIELDS;
    let facts = &record.facts;
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    slot[at] = record.journal;
    slot[at + 1] = flag(facts.profile.is_some(), PROFILE)
        | flag(facts.recovery_bpm.is_some(), RECOVERY)
        | flag(facts.climbing.is_some(), CLIMBS);
    put_u32(slot, at + 2, record.ride_id);
    put_u64(slot, at + 6, record.started_utc_ms);
    let profile = facts.profile.unwrap_or(TrainingProfile {
        ftp_watts: 0,
        lthr_bpm: 0,
    });
    put_u16(slot, at + 14, profile.ftp_watts);
    put_u16(slot, at + 16, profile.lthr_bpm);
    put_u16(slot, at + 18, facts.recovery_bpm.unwrap_or(0) as u16);
    let climbing = facts.climbing.unwrap_or_default();
    put_u16(slot, at + 20, climbing.count);
    put_u16(slot, at + 22, climbing.gain_m);
}

fn decode_fields(slot: &[u8]) -> Record {
    let at = FIELDS;
    let flags = slot[at + 1];
    let has = |bit: u8| flags & bit != 0;
    Record {
        journal: slot[at],
        ride_id: get_u32(slot, at + 2),
        started_utc_ms: get_u64(slot, at + 6),
        facts: Facts {
            profile: has(PROFILE)
                .then(|| TrainingProfile::new(get_u16(slot, at + 14), get_u16(slot, at + 16)))
                .flatten(),
            recovery_bpm: has(RECOVERY).then(|| get_u16(slot, at + 18) as i16),
            climbing: has(CLIMBS).then(|| Climbing {
                count: get_u16(slot, at + 20),
                gain_m: get_u16(slot, at + 22),
            }),
        },
    }
}

fn seal(slot: &mut [u8]) {
    let end = slot.len() - 4;
    let crc = crc32(&slot[..end]);
    put_u32(slot, end, crc);
}

fn encode_slot(record: &Record, slot: &mut [u8]) {
    slot.fill(0);
    slot[0] = VERSION;
    encode_fields(record, slot);
    seal(slot);
}

/// A current slot, or `None` if damaged, empty or of another version; a
/// table holding a slot of another version is refused, never rewritten.
pub fn decode_slot(slot: &[u8]) -> Option<Record> {
    if slot.len() != SLOT
        || slot[0] != VERSION
        || crc32(&slot[..SLOT - 4]) != get_u32(slot, SLOT - 4)
    {
        return None;
    }
    Some(decode_fields(slot))
}

fn foreign_slot(slot: &[u8]) -> bool {
    slot.len() == SLOT && slot[0] != VERSION && crc32(&slot[..SLOT - 4]) == get_u32(slot, SLOT - 4)
}

/// The rides a whole table holds, oldest saved first, read as the firmware
/// reads them; none if it would refuse the table.
pub fn decode_table(bytes: &[u8]) -> impl Iterator<Item = Record> + '_ {
    let slots = if header_state(bytes, bytes.len()) != Header::Foreign {
        bytes
    } else {
        &[]
    };
    slots
        .as_chunks::<SLOT>()
        .0
        .iter()
        .skip(1)
        .filter_map(|slot| decode_slot(slot))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Header {
    Foreign,
    Sound,
    Damaged,
}

fn header_state(bytes: &[u8], length: usize) -> Header {
    let current = bytes.len() >= 8
        && bytes[..4] == TABLE_MAGIC[..]
        && bytes[4] == TABLE_VERSION
        && usize::from(get_u16(bytes, 6)) == SLOT
        && length.is_multiple_of(SLOT);
    if !current {
        Header::Foreign
    } else if bytes.len() >= SLOT && crc32(&bytes[..SLOT - 4]) == get_u32(bytes, SLOT - 4) {
        Header::Sound
    } else {
        Header::Damaged
    }
}

/// Whether a whole table held in `bytes` is current with a sound header: for
/// a host taking a table back to the rides it held earlier.
pub fn table_sound(bytes: &[u8]) -> bool {
    header_state(bytes, bytes.len()) == Header::Sound
}

fn header(slot: &mut [u8]) {
    slot.fill(0);
    slot[..4].copy_from_slice(TABLE_MAGIC);
    slot[4] = TABLE_VERSION;
    put_u16(slot, 6, SLOT as u16);
    seal(slot);
}

#[derive(Clone, Copy)]
struct Table {
    entry: files::Entry,
    sound: bool,
}

impl Table {
    fn open<M: ReadWrite>(
        store: &mut volume::Volume<'_, M>,
        dir: &files::Dir,
        entry: files::Entry,
        sector: &mut files::Sector,
        bytes: &mut files::Sector,
    ) -> Result<Self, Error> {
        let read = dir.read(store, &entry, 0, bytes, sector)?;
        let sound = match header_state(&bytes[..read], entry.length as usize) {
            Header::Foreign => return Err(Error::Table),
            Header::Sound => true,
            Header::Damaged => false,
        };
        Ok(Self { entry, sound })
    }

    fn sectors(&self) -> usize {
        (self.entry.length as usize).div_ceil(SECTOR_SIZE)
    }

    /// Visit the slots in table sectors `sectors`, the header first if they
    /// start at 0, one sector read at a time into `bytes`.
    fn each<M: ReadWrite>(
        &self,
        store: &mut volume::Volume<'_, M>,
        dir: &files::Dir,
        sectors: core::ops::Range<usize>,
        sector: &mut files::Sector,
        bytes: &mut files::Sector,
        mut visit: impl FnMut(&mut volume::Volume<'_, M>, Slot<'_>),
    ) -> Result<(), Error> {
        for index in sectors.start..sectors.end.min(self.sectors()) {
            let at = (index * SECTOR_SIZE) as u32;
            let read = dir.read(store, &self.entry, at, bytes, sector)?;
            for (n, slot) in bytes[..read].as_chunks::<SLOT>().0.iter().enumerate() {
                if index == 0 && n == 0 {
                    visit(store, Slot::Header(slot));
                } else {
                    visit(store, Slot::Ride(slot));
                }
            }
        }
        Ok(())
    }
}

enum Slot<'a> {
    Header(&'a [u8]),
    Ride(&'a [u8]),
}

#[derive(Clone, Copy)]
enum Change {
    Save(Record),
    Forget(u8),
}

impl Change {
    fn keeps(&self, slot: &[u8]) -> Option<Record> {
        decode_slot(slot).filter(|record| match self {
            Self::Save(saved) => saved.key() != record.key(),
            Self::Forget(journal) => record.journal != *journal,
        })
    }
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
        self.bytes[self.used..self.used + slot.len()].copy_from_slice(slot);
        self.used += slot.len();
        if self.used == SECTOR_SIZE {
            self.flush(media);
        }
    }

    fn header(&mut self, media: &mut impl ReadWrite) {
        header(&mut self.bytes[self.used..self.used + SLOT]);
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

fn can_add_file_keeping_one_entry_free(dir: &files::Dir) -> bool {
    files::CAPACITY.saturating_sub(dir.len()) > 1
}

#[inline(never)]
fn rewrite(media: &mut impl ReadWrite, change: Change) -> Result<(), Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let name = table_name();
    let table = match dir.find(&mut store, &name, &mut sector)? {
        Some(entry) => Some(Table::open(
            &mut store,
            &dir,
            entry,
            &mut sector,
            &mut bytes,
        )?),
        None => None,
    };

    let mut kept = 0;
    let mut foreign = false;
    let mut dropped = table.is_some_and(|table| !table.sound);
    if let Some(table) = table {
        table.each(
            &mut store,
            &dir,
            0..usize::MAX,
            &mut sector,
            &mut bytes,
            |_, slot| {
                if let Slot::Ride(slot) = slot {
                    foreign |= foreign_slot(slot);
                    match change.keeps(slot) {
                        Some(_) => kept += 1,
                        None => dropped = true,
                    }
                }
            },
        )?;
    }
    if foreign {
        return Err(Error::Table);
    }
    let saved = match change {
        Change::Save(record) => Some(record),
        Change::Forget(_) => None,
    };
    if !dropped && saved.is_none() {
        return Ok(());
    }
    if table.is_none() && !can_add_file_keeping_one_entry_free(&dir) {
        return Err(files::Error::Full.into());
    }
    let total = kept + usize::from(saved.is_some());
    let mut skip = total.saturating_sub(KEEP);
    let length = SLOT * (1 + total - skip);
    let mut out = Out {
        bytes: [0; SECTOR_SIZE],
        used: 0,
        crc: Crc32::new(),
        writer: dir.begin(&mut store, &name, length as u32, &mut sector)?,
        failed: None,
    };
    match table {
        Some(table) => table.each(
            &mut store,
            &dir,
            0..usize::MAX,
            &mut sector,
            &mut bytes,
            |store, slot| match slot {
                Slot::Header(_) if !table.sound => out.header(store),
                Slot::Header(header) => out.emit(store, header),
                Slot::Ride(ride) if change.keeps(ride).is_some() => {
                    if skip > 0 {
                        skip -= 1;
                    } else {
                        out.emit(store, ride);
                    }
                }
                Slot::Ride(_) => {}
            },
        )?,
        None => out.header(&mut store),
    }
    if let Some(record) = saved.filter(|_| skip == 0) {
        encode_slot(&record, &mut bytes[..SLOT]);
        out.emit(&mut store, &bytes[..SLOT]);
    }
    out.finish(&mut store, &mut sector)
}

/// Save `record`, in place of any earlier record of its ride. The table
/// never takes the last free directory entry.
pub fn save(media: &mut impl ReadWrite, record: &Record) -> Result<(), Error> {
    rewrite(media, Change::Save(*record))
}

/// Drop every record of `journal`, as its ride ids start over.
pub fn forget(media: &mut impl ReadWrite, journal: u8) -> Result<(), Error> {
    rewrite(media, Change::Forget(journal))
}

/// One piece of the table that [`read`] visited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    /// Changes whenever the table is written, so a reader that sees another
    /// knows the pieces it read before may have moved.
    pub revision: u32,
    /// The next piece, or `None` after the last.
    pub next: Option<usize>,
}

/// Visit the records in [`PIECE`] sectors of the table from piece `index`,
/// so a scan never holds up a tick for long; `None` without a table.
pub fn read(
    media: &mut impl ReadWrite,
    index: usize,
    visit: impl FnMut(Record),
) -> Result<Option<Piece>, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let Some(entry) = dir.find(&mut store, &table_name(), &mut sector)? else {
        return Ok(None);
    };
    let table = Table::open(&mut store, &dir, entry, &mut sector, &mut bytes)?;
    let from = index.saturating_mul(PIECE);
    let to = from.saturating_add(PIECE);
    let mut visit = visit;
    table.each(
        &mut store,
        &dir,
        from..to,
        &mut sector,
        &mut bytes,
        |_, slot| {
            if let Slot::Ride(slot) = slot
                && let Some(record) = decode_slot(slot)
            {
                visit(record);
            }
        },
    )?;
    Ok(Some(Piece {
        revision: entry.revision,
        next: (to < table.sectors()).then_some(index + 1),
    }))
}

/// What the table holds and in what order, for a host to check a copy
/// against: the CRC32 of every slot's bytes before its own CRC, the header
/// first, in file order. `None` without a table.
pub fn digest(media: &mut impl ReadWrite) -> Result<Option<u32>, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let Some(entry) = dir.find(&mut store, &table_name(), &mut sector)? else {
        return Ok(None);
    };
    let table = Table::open(&mut store, &dir, entry, &mut sector, &mut bytes)?;
    let mut crc = Crc32::new();
    table.each(
        &mut store,
        &dir,
        0..table.sectors(),
        &mut sector,
        &mut bytes,
        |_, (Slot::Header(slot) | Slot::Ride(slot))| crc = crc.update(&slot[..slot.len() - 4]),
    )?;
    Ok(Some(crc.finish()))
}

/// [`digest`] of a whole table held in `bytes`, whose header is sound.
pub fn table_digest(bytes: &[u8]) -> Option<u32> {
    table_sound(bytes).then(|| {
        bytes
            .as_chunks::<SLOT>()
            .0
            .iter()
            .fold(Crc32::new(), |crc, slot| crc.update(&slot[..SLOT - 4]))
            .finish()
    })
}

/// The table in memory, ordered by journal and ride id for lookup, in storage
/// the composition lends: [`SLOT`] bytes a record.
pub struct Shelf {
    slots: &'static mut [u8],
    len: usize,
}

impl Shelf {
    /// An empty shelf in `slots` (a multiple of [`SLOT`] bytes).
    pub fn new(slots: &'static mut [u8]) -> Self {
        Self { slots, len: 0 }
    }

    fn capacity(&self) -> usize {
        self.slots.len() / SLOT
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot(&self, index: usize) -> &[u8] {
        &self.slots[index * SLOT..][..SLOT]
    }

    fn get(&self, index: usize) -> Option<Record> {
        decode_slot(self.slot(index))
    }

    fn key(&self, index: usize) -> (u8, u32) {
        let slot = self.slot(index);
        (slot[FIELDS], get_u32(slot, FIELDS + 2))
    }

    fn search(&self, key: (u8, u32)) -> Result<usize, usize> {
        let (mut low, mut high) = (0, self.len);
        while low < high {
            let middle = low + (high - low) / 2;
            match self.key(middle).cmp(&key) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Ok(middle),
            }
        }
        Err(low)
    }

    /// The facts of the ride `ride_id` of `journal` that started at
    /// `started_utc_ms`.
    pub fn find(&self, journal: u8, ride_id: u32, started_utc_ms: Option<u64>) -> Option<Facts> {
        let index = self.search((journal, ride_id)).ok()?;
        self.get(index)
            .filter(|record| record.of(journal, ride_id, started_utc_ms))
            .map(|record| record.facts)
    }

    fn remove(&mut self, index: usize) {
        self.slots
            .copy_within((index + 1) * SLOT..self.len * SLOT, index * SLOT);
        self.len -= 1;
    }

    /// Keep `record`: a ride just saved always, one read from FILES only over
    /// an earlier start of its ride. Once full, the oldest start gives way,
    /// where the table drops the oldest saved: the same ride unless rides
    /// were saved out of start order, and each boot reads the table again.
    /// Returns whether anything changed.
    pub fn put(&mut self, record: &Record, saved: bool) -> bool {
        let mut index = match self.search(record.key()) {
            Ok(index) => {
                let earlier = self
                    .get(index)
                    .is_none_or(|kept| kept.started_utc_ms < record.started_utc_ms);
                if !saved && !earlier {
                    return false;
                }
                encode_slot(record, &mut self.slots[index * SLOT..][..SLOT]);
                return true;
            }
            Err(index) => index,
        };
        if self.capacity() == 0 {
            return false;
        }
        if self.len == self.capacity() {
            let start = |index| self.get(index).map_or(0, |kept| kept.started_utc_ms);
            let oldest = (0..self.len).min_by_key(|&index| start(index)).unwrap_or(0);
            if !saved && start(oldest) >= record.started_utc_ms {
                return false;
            }
            self.remove(oldest);
            if oldest < index {
                index -= 1;
            }
        }
        self.slots
            .copy_within(index * SLOT..self.len * SLOT, (index + 1) * SLOT);
        encode_slot(record, &mut self.slots[index * SLOT..][..SLOT]);
        self.len += 1;
        true
    }

    /// Drop every record of `journal`.
    pub fn forget(&mut self, journal: u8) {
        let mut kept = 0;
        for index in 0..self.len {
            if self.key(index).0 != journal {
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
}

#[cfg(test)]
mod tests;
