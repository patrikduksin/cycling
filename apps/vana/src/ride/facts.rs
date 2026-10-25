//! What VANA knew about a saved ride that its journal does not hold, kept on
//! the FILES volume in one table, `rides/facts.tbl`, of up to [`KEEP`] rides.
//!
//! The table, little-endian, is a header slot and then one slot per ride,
//! oldest saved first. Every slot is `stride` bytes and ends in the CRC32 of
//! the bytes before it, so a damaged slot costs only its own ride.
//!
//! | Header bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VFTB` |
//! | 4 | table version, 1 |
//! | 6..8 | stride: 32 in version 1, a power of two up to 512 |
//!
//! | Slot bytes | Field |
//! | --- | --- |
//! | 0 | record version, 1 |
//! | 2.. | the fields of a version 1 file below, from its byte 6 |
//! | 26..stride-4 | zero in version 1 |
//!
//! Later versions only add fields before the CRC, or lengthen the stride for
//! them; a reader takes the fields it knows from any record version at least
//! 1, and a save copies the slots it keeps byte for byte, so it never drops
//! fields it does not know. A table whose header is damaged reads its slots at
//! version 1's stride, and the next rewrite gives it a fresh header; a file
//! without the magic is not ours and is left alone, with nothing saved over
//! it.
//!
//! The store has no append in place: a save writes the whole table again,
//! copy-on-write, with the ride appended last. It drops the ride's earlier
//! slot, slots that fail their CRC and, past [`KEEP`], the oldest saved. The
//! commit is one directory write, so a cut leaves the old table or the new.
//!
//! Firmware from before the table kept a file per ride,
//! `rides/<journal><ride id>.facts`, version 1:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VFCT` |
//! | 4 | version, 1 |
//! | 5 | body length `n` before the CRC: 30 in version 1 |
//! | 6 | journal: `i` internal, `b` bulk |
//! | 7 | flags: 1 profile, 2 HR recovery, 4 climbs |
//! | 8..12 | ride id in that journal |
//! | 12..20 | ride start, UTC ms |
//! | 20..22 | FTP at the ride, W |
//! | 22..24 | LTHR at the ride, BPM |
//! | 24..26 | HR recovery: the fall over the minute after, BPM (i16) |
//! | 26..28 | climbs finished |
//! | 28..30 | climbing of those climbs, m |
//! | n..n+4 | CRC32 of bytes 0..n |
//!
//! Each rewrite folds up to [`BATCH`] of those files into the table and
//! deletes them once it has committed; a ride in both keeps the later start,
//! and the table's on a tie. Ride ids restart when a journal is cleared or
//! initialized, and `forget` then drops that journal's rides; the start time
//! guards the rest, as older firmware clears a journal without touching the
//! table. So a ride whose start time is unknown has no facts. A profile out of
//! range reads as none and leaves the other fields.
use super::zones::TrainingProfile;
use core::fmt::Write;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::{Crc32, crc32};
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::{files, volume};

pub const FOLDER: &str = "rides/";
/// Not `.facts` or `.workout`: older firmware lists, rotates and deletes
/// files under `rides/` by those suffixes.
pub const TABLE: &str = "rides/facts.tbl";
const SUFFIX: &str = ".facts";
pub const VERSION: u8 = 1;
const MAGIC: &[u8; 4] = b"VFCT";
/// Body bytes of version 1, before the CRC.
const BODY_V1: usize = 30;
pub const SIZE: usize = BODY_V1 + 4;
/// The longest file any version can be: a one-byte body length and the CRC.
pub const READ_MAX: usize = u8::MAX as usize + 4;
const TABLE_MAGIC: &[u8; 4] = b"VFTB";
const TABLE_VERSION: u8 = 1;
/// Bytes a slot takes in version 1.
pub const SLOT: usize = 32;
/// Where the fields start in a slot, and in a version 1 file.
const SLOT_FIELDS: usize = 2;
const FILE_FIELDS: usize = 6;
/// Rides the table keeps: FORM's 120 days at eight rides a day, in 32 KB.
pub const KEEP: usize = 1000;
/// Bytes the composition lends to hold the table in memory.
pub const SHELF_BYTES: usize = KEEP * SLOT;
/// Older files folded into the table by one rewrite.
pub const BATCH: usize = 8;
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
    /// The table file lacks the magic; it is left as it is.
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

pub fn file(journal: u8, ride_id: u32) -> files::Name {
    let mut text = heapless::String::<{ files::NAME }>::new();
    let _ = write!(
        text,
        "{FOLDER}{}{ride_id}{SUFFIX}",
        char::from(journal.to_ascii_lowercase())
    );
    files::Name::new(&text).unwrap_or(files::Name::new("rides/x.facts").unwrap())
}

fn table_name() -> files::Name {
    files::Name::new(TABLE).unwrap()
}

fn is_facts(name: &files::Name) -> bool {
    name.as_str().starts_with(FOLDER) && name.as_str().ends_with(SUFFIX)
}

fn encode_fields(record: &Record, bytes: &mut [u8], at: usize) {
    let facts = &record.facts;
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    bytes[at] = record.journal;
    bytes[at + 1] = flag(facts.profile.is_some(), PROFILE)
        | flag(facts.recovery_bpm.is_some(), RECOVERY)
        | flag(facts.climbing.is_some(), CLIMBS);
    put_u32(bytes, at + 2, record.ride_id);
    put_u64(bytes, at + 6, record.started_utc_ms);
    let profile = facts.profile.unwrap_or(TrainingProfile {
        ftp_watts: 0,
        lthr_bpm: 0,
    });
    put_u16(bytes, at + 14, profile.ftp_watts);
    put_u16(bytes, at + 16, profile.lthr_bpm);
    put_u16(bytes, at + 18, facts.recovery_bpm.unwrap_or(0) as u16);
    let climbing = facts.climbing.unwrap_or_default();
    put_u16(bytes, at + 20, climbing.count);
    put_u16(bytes, at + 22, climbing.gain_m);
}

fn decode_fields(bytes: &[u8], at: usize) -> Record {
    let flags = bytes[at + 1];
    let has = |bit: u8| flags & bit != 0;
    Record {
        journal: bytes[at],
        ride_id: get_u32(bytes, at + 2),
        started_utc_ms: get_u64(bytes, at + 6),
        facts: Facts {
            profile: has(PROFILE)
                .then(|| TrainingProfile::new(get_u16(bytes, at + 14), get_u16(bytes, at + 16)))
                .flatten(),
            recovery_bpm: has(RECOVERY).then(|| get_u16(bytes, at + 18) as i16),
            climbing: has(CLIMBS).then(|| Climbing {
                count: get_u16(bytes, at + 20),
                gain_m: get_u16(bytes, at + 22),
            }),
        },
    }
}

/// A version 1 file.
pub fn encode(record: &Record) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = VERSION;
    bytes[5] = BODY_V1 as u8;
    encode_fields(record, &mut bytes, FILE_FIELDS);
    let crc = crc32(&bytes[..BODY_V1]);
    put_u32(&mut bytes, BODY_V1, crc);
    bytes
}

/// A file of any version from 1 on, or `None` if torn, damaged or foreign.
pub fn decode(bytes: &[u8]) -> Option<Record> {
    let body = usize::from(*bytes.get(5)?);
    if bytes.get(..4)? != MAGIC
        || bytes[4] < 1
        || body < BODY_V1
        || bytes.len() < body + 4
        || crc32(&bytes[..body]) != get_u32(bytes, body)
    {
        return None;
    }
    Some(decode_fields(bytes, FILE_FIELDS))
}

fn seal(slot: &mut [u8]) {
    let end = slot.len() - 4;
    let crc = crc32(&slot[..end]);
    put_u32(slot, end, crc);
}

/// `record` as a version 1 slot of `slot.len()` bytes.
fn encode_slot(record: &Record, slot: &mut [u8]) {
    slot.fill(0);
    slot[0] = VERSION;
    encode_fields(record, slot, SLOT_FIELDS);
    seal(slot);
}

/// A slot of any record version from 1 on, or `None` if damaged or empty.
fn decode_slot(slot: &[u8]) -> Option<Record> {
    let end = slot.len().checked_sub(4)?;
    if end < SLOT_FIELDS + 24 || slot[0] < 1 || crc32(&slot[..end]) != get_u32(slot, end) {
        return None;
    }
    Some(decode_fields(slot, SLOT_FIELDS))
}

/// The rides a whole table holds, oldest saved first, read as the firmware
/// reads them; none without the magic.
pub fn decode_table(bytes: &[u8]) -> impl Iterator<Item = Record> + '_ {
    let ours = bytes.starts_with(TABLE_MAGIC);
    let stride = stride(bytes).unwrap_or(if ours { SLOT } else { bytes.len().max(1) });
    bytes.chunks_exact(stride).skip(1).filter_map(decode_slot)
}

fn header(slot: &mut [u8]) {
    slot.fill(0);
    slot[..4].copy_from_slice(TABLE_MAGIC);
    slot[4] = TABLE_VERSION;
    put_u16(slot, 6, slot.len() as u16);
    seal(slot);
}

/// The stride the header at the start of `bytes` names, if it is sound.
fn stride(bytes: &[u8]) -> Option<usize> {
    let stride = usize::from(get_u16(bytes.get(..8)?, 6));
    let sound = bytes[..4] == TABLE_MAGIC[..]
        && bytes[4] >= 1
        && stride.is_power_of_two()
        && (SLOT..=SECTOR_SIZE).contains(&stride)
        && bytes.len() >= stride
        && crc32(&bytes[..stride - 4]) == get_u32(bytes, stride - 4);
    sound.then_some(stride)
}

/// The table file and the length of its slots.
#[derive(Clone, Copy)]
struct Table {
    entry: files::Entry,
    stride: usize,
    /// The header parsed; else the slots are read at version 1's stride
    /// and a rewrite writes a fresh header.
    sound: bool,
}

impl Table {
    /// The table, or `Error::Table` for a file without the magic, which is
    /// not ours to read or write over.
    fn open<M: ReadWrite>(
        store: &mut volume::Volume<'_, M>,
        dir: &files::Dir,
        entry: files::Entry,
        sector: &mut files::Sector,
        bytes: &mut files::Sector,
    ) -> Result<Self, Error> {
        let read = dir.read(store, &entry, 0, bytes, sector)?;
        if bytes[..read.min(4)] != TABLE_MAGIC[..] {
            return Err(Error::Table);
        }
        Ok(match stride(&bytes[..read]) {
            Some(stride) if (entry.length as usize).is_multiple_of(stride) => Self {
                entry,
                stride,
                sound: true,
            },
            _ => Self {
                entry,
                stride: SLOT,
                sound: false,
            },
        })
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
            for (n, slot) in bytes[..read].chunks_exact(self.stride).enumerate() {
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

/// What one rewrite of the table does beside folding older files in.
#[derive(Clone, Copy)]
enum Change {
    Save(Record),
    Forget(u8),
    Fold,
}

impl Change {
    /// Whether the rewrite leaves `record` out, before older files and the
    /// bound are weighed.
    fn drops(&self, record: &Record) -> bool {
        match self {
            Self::Save(saved) => saved.key() == record.key(),
            Self::Forget(journal) => record.journal == *journal,
            Self::Fold => false,
        }
    }
}

/// Older files of one rewrite: their entries, oldest first, whether each
/// read, and what each holds if it decodes and the change keeps it. Only a
/// file that read is ever deleted.
struct Batch {
    entries: heapless::Vec<files::Entry, BATCH>,
    read: [bool; BATCH],
    records: [Option<Record>; BATCH],
}

impl Batch {
    /// The file that goes before the table can take a directory entry: one
    /// that read but holds nothing to fold, else the oldest that read, whose
    /// record this rewrite holds. A cut before the commit loses only that
    /// ride, as older firmware's rotation would.
    fn room(&self) -> Option<&files::Entry> {
        let mut read = (0..self.entries.len()).filter(|&at| self.read[at]);
        let at = read
            .clone()
            .find(|&at| self.records[at].is_none())
            .or_else(|| read.next())?;
        Some(&self.entries[at])
    }

    /// The ride in `slot` if the rewrite keeps it: it decodes, the change
    /// keeps it and no file holds a later start of its ride.
    fn keeps(&self, change: &Change, slot: &[u8]) -> Option<Record> {
        decode_slot(slot).filter(|record| {
            !change.drops(record)
                && !self.records.iter().flatten().any(|file| {
                    file.key() == record.key() && file.started_utc_ms > record.started_utc_ms
                })
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

/// Write the table again with `change` and up to [`BATCH`] older files
/// folded in, then delete those files. Returns whether more remain.
#[inline(never)]
fn rewrite(media: &mut impl ReadWrite, change: Change) -> Result<bool, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let name = table_name();
    let mut found = None;
    let mut stored = 0;
    let mut more = false;
    let mut batch = Batch {
        entries: heapless::Vec::new(),
        read: [false; BATCH],
        records: [None; BATCH],
    };
    dir.list(&mut store, &mut sector, |entry| {
        stored += 1;
        if entry.name == name {
            found = Some(*entry);
        } else if is_facts(&entry.name) && batch.entries.push(*entry).is_err() {
            more = true;
        }
    })?;
    // By hand: the library sort opens a 3 KB frame for its scratch.
    for at in 1..batch.entries.len() {
        let mut at = at;
        while at > 0 && batch.entries[at - 1].revision > batch.entries[at].revision {
            batch.entries.swap(at - 1, at);
            at -= 1;
        }
    }
    for (at, entry) in batch.entries.iter().enumerate() {
        let length = (entry.length as usize).min(READ_MAX);
        let Ok(read) = dir.read(&mut store, entry, 0, &mut bytes[..length], &mut sector) else {
            continue;
        };
        batch.read[at] = true;
        batch.records[at] = decode(&bytes[..read])
            .filter(|record| file(record.journal, record.ride_id) == entry.name)
            .filter(|record| !change.drops(record));
    }
    let table = match found {
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
    let mut dropped = table.is_some_and(|table| !table.sound);
    let mut folds = batch.records.map(|record| record.is_some());
    if let Some(table) = table {
        let rides = 0..usize::MAX;
        table.each(
            &mut store,
            &dir,
            rides,
            &mut sector,
            &mut bytes,
            |_, slot| {
                let Slot::Ride(slot) = slot else {
                    return;
                };
                let Some(record) = batch.keeps(&change, slot) else {
                    dropped = true;
                    return;
                };
                kept += 1;
                for (file, folds) in batch.records.iter().zip(folds.iter_mut()) {
                    *folds &= !file.is_some_and(|file| {
                        file.key() == record.key() && file.started_utc_ms <= record.started_utc_ms
                    });
                }
            },
        )?;
    }
    let saved = match change {
        Change::Save(record) => Some(record),
        Change::Forget(_) | Change::Fold => None,
    };
    let folded = folds.iter().filter(|folds| **folds).count();
    let total = kept + folded + usize::from(saved.is_some());
    let mut skip = total.saturating_sub(KEEP);

    if dropped || folded > 0 || saved.is_some() {
        if table.is_none() {
            let free = files::CAPACITY.saturating_sub(stored);
            // Never the last free entry, which the next workout record or the
            // host may need, unless older files go right after; and never a
            // ride's facts to make room.
            let frees = batch.read.iter().any(|read| *read);
            match batch.room() {
                Some(room) if free == 0 => {
                    dir.delete(&mut store, &room.name, &mut sector)?;
                }
                None if free == 0 => return Err(files::Error::Full.into()),
                _ if free == 1 && !frees => return Err(files::Error::Full.into()),
                _ => {}
            }
        }
        let dir = files::mount(&mut store, &mut sector)?;
        let stride = table.map_or(SLOT, |table| table.stride);
        let length = stride * (1 + total - skip);
        let mut out = Out {
            bytes: [0; SECTOR_SIZE],
            used: 0,
            crc: Crc32::new(),
            writer: dir.begin(&mut store, &name, length as u32, &mut sector)?,
            failed: None,
        };
        match table {
            Some(table) => {
                let all = 0..usize::MAX;
                let mut fresh = [0u8; SLOT];
                header(&mut fresh);
                table.each(
                    &mut store,
                    &dir,
                    all,
                    &mut sector,
                    &mut bytes,
                    |store, slot| match slot {
                        Slot::Header(_) if !table.sound => out.emit(store, &fresh),
                        Slot::Header(header) => out.emit(store, header),
                        Slot::Ride(ride) if batch.keeps(&change, ride).is_some() => {
                            if skip > 0 {
                                skip -= 1;
                            } else {
                                out.emit(store, ride);
                            }
                        }
                        Slot::Ride(_) => {}
                    },
                )?
            }
            None => {
                header(&mut bytes[..stride]);
                out.emit(&mut store, &bytes[..stride]);
            }
        }
        let files = batch.records.iter().zip(folds);
        for record in files
            .filter_map(|(record, folds)| record.filter(|_| folds))
            .chain(saved)
        {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            encode_slot(&record, &mut bytes[..stride]);
            out.emit(&mut store, &bytes[..stride]);
        }
        out.finish(&mut store, &mut sector)?;
    }
    let mut deleted = false;
    for (entry, _) in batch
        .entries
        .iter()
        .zip(batch.read)
        .filter(|(_, read)| *read)
    {
        match files::delete(&mut store, &entry.name, &mut sector) {
            Ok(()) | Err(files::Error::NotFound) => deleted = true,
            Err(error) => return Err(error.into()),
        }
    }
    // Files that would not read stay, and stop the folding until next time.
    Ok(more && deleted)
}

/// Save `record`, in place of any earlier record of its ride, and fold
/// older files in. The table never takes the last free directory entry
/// unless older files it folds free one right after.
pub fn save(media: &mut impl ReadWrite, record: &Record) -> Result<(), Error> {
    rewrite(media, Change::Save(*record)).map(|_| ())
}

/// Drop every record of `journal`, as its ride ids start over, and delete its
/// older files.
pub fn forget(media: &mut impl ReadWrite, journal: u8) -> Result<(), Error> {
    while rewrite(media, Change::Forget(journal))? {}
    Ok(())
}

/// Fold up to [`BATCH`] older files into the table and delete them. Returns
/// whether more remain.
pub fn fold(media: &mut impl ReadWrite) -> Result<bool, Error> {
    rewrite(media, Change::Fold)
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

/// Visit the older files listed in directory sector `index`, one sector's
/// entries per call. Files that do not decode or name another ride than their
/// own are skipped. Returns the next sector, or `None` after the last.
pub fn read_files(
    media: &mut impl ReadWrite,
    index: usize,
    mut visit: impl FnMut(Record),
) -> Result<Option<usize>, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let entries = dir.entries(&mut store, index, &mut sector)?;
    let mut bytes = [0; READ_MAX];
    for entry in entries.iter().flatten() {
        if !is_facts(&entry.name) {
            continue;
        }
        let length = (entry.length as usize).min(READ_MAX);
        let Ok(length) = dir.read(&mut store, entry, 0, &mut bytes[..length], &mut sector) else {
            continue;
        };
        if let Some(record) = decode(&bytes[..length])
            && file(record.journal, record.ride_id) == entry.name
        {
            visit(record);
        }
    }
    let next = index + 1;
    Ok((next < files::DIR_SECTORS).then_some(next))
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
        (slot[SLOT_FIELDS], get_u32(slot, SLOT_FIELDS + 2))
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
