use super::{RETRY_MS, StoreError};
use crate::ride::course::stored::{self, Stored};
use crate::ride::course::turns::Turn;
use crate::ride::course::{Course, DecodeError, decode, encode};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::Crc32;
use firmware_services::{files, volume};

const FILE: &str = "course.dat";
const TURNS_FILE: &str = "course-turns.dat";

fn error(error: files::Error) -> StoreError {
    use device_api::bulk::Error as Media;
    match error {
        files::Error::Media(Media::Failed) | files::Error::Stale => StoreError::Media,
        files::Error::Full | files::Error::TooLarge => StoreError::Full,
        files::Error::Mismatch => StoreError::Damaged,
        _ => StoreError::Unavailable,
    }
}

fn open<'a, M: ReadWrite>(
    media: &'a mut M,
    sector: &mut files::Sector,
) -> Result<volume::Volume<'a, M>, StoreError> {
    volume::open(media, &volume::FILES, sector).map_err(|error| match error {
        volume::Error::Media(device_api::bulk::Error::Failed) => StoreError::Media,
        _ => StoreError::Unavailable,
    })
}

fn name() -> files::Name {
    files::Name::new(FILE).expect("a valid file name")
}

/// The stored course into `course`; empty when the store has none.
pub fn load(media: &mut impl ReadWrite, course: &mut Course) -> Result<(), StoreError> {
    course.clear();
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector).map_err(error)?;
    let Some(entry) = dir.find(&mut store, &name(), &mut sector).map_err(error)? else {
        return Ok(());
    };
    let read = |offset: u32, out: &mut [u8]| -> Result<(), files::Error> {
        let mut sector = [0; SECTOR_SIZE];
        let n = dir.read(&mut store, &entry, offset, out, &mut sector)?;
        if n == out.len() {
            Ok(())
        } else {
            Err(files::Error::Range)
        }
    };
    match decode(entry.length, read, course) {
        Ok(()) => Ok(()),
        Err(DecodeError::Read(files::Error::Range)) | Err(DecodeError::Damaged) => {
            Err(StoreError::Damaged)
        }
        Err(DecodeError::Read(read)) => Err(error(read)),
    }
}

/// Store `course`, replacing the file there in one atomic commit. A store
/// the host has not formatted is not formatted here.
pub fn save(media: &mut impl ReadWrite, course: &Course) -> Result<(), StoreError> {
    write(media, &name(), |put| encode(course, put))
}

/// Write the file `encode` gives, twice: once for its length and CRC, then
/// to the store.
fn write(
    media: &mut impl ReadWrite,
    name: &files::Name,
    encode: impl Fn(&mut dyn FnMut(&[u8])),
) -> Result<(), StoreError> {
    let mut length = 0u32;
    let mut crc = Crc32::new();
    encode(&mut |bytes| {
        length += bytes.len() as u32;
        crc = crc.update(bytes);
    });
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector).map_err(error)?;
    let mut writer = dir
        .begin(&mut store, name, length, &mut sector)
        .map_err(error)?;
    // Whole sectors to the writer, and the short last one at the end.
    let mut held = [0u8; SECTOR_SIZE];
    let mut filled = 0;
    let mut failed = None;
    encode(&mut |mut bytes| {
        while !bytes.is_empty() && failed.is_none() {
            let n = bytes.len().min(SECTOR_SIZE - filled);
            held[filled..filled + n].copy_from_slice(&bytes[..n]);
            filled += n;
            bytes = &bytes[n..];
            if filled == SECTOR_SIZE {
                failed = writer.write(&mut store, &held).err();
                filled = 0;
            }
        }
    });
    if let Some(failed) = failed {
        return Err(error(failed));
    }
    writer.write(&mut store, &held[..filled]).map_err(error)?;
    writer
        .commit(&mut store, crc.finish(), &mut sector)
        .map_err(error)?;
    Ok(())
}

fn turns_name() -> files::Name {
    files::Name::new(TURNS_FILE).expect("a valid file name")
}

/// The turns stored for the course `key` into `turns`: how many, or `None`
/// when there are none of this course's that decode.
pub fn load_turns(
    media: &mut impl ReadWrite,
    key: u32,
    turns: &mut [Turn],
) -> Result<Option<Stored>, StoreError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector).map_err(error)?;
    let Some(entry) = dir
        .find(&mut store, &turns_name(), &mut sector)
        .map_err(error)?
    else {
        return Ok(None);
    };
    let read = |offset: u32, out: &mut [u8]| -> Result<(), files::Error> {
        let mut sector = [0; SECTOR_SIZE];
        let n = dir.read(&mut store, &entry, offset, out, &mut sector)?;
        if n == out.len() {
            Ok(())
        } else {
            Err(files::Error::Range)
        }
    };
    match stored::decode(entry.length, read, turns) {
        Ok(stored) => Ok(stored.filter(|stored| stored.key == key)),
        Err(files::Error::Range) => Ok(None),
        Err(failed) => Err(error(failed)),
    }
}

/// Store `turns` of the course `key`, replacing the file there in one
/// atomic commit.
pub fn save_turns(
    media: &mut impl ReadWrite,
    key: u32,
    turns: &[Turn],
    full: bool,
) -> Result<(), StoreError> {
    write(media, &turns_name(), |put| {
        stored::encode(key, turns, full, put)
    })
}

/// Remove the stored turns.
pub fn delete_turns(media: &mut impl ReadWrite) -> Result<(), StoreError> {
    remove(media, &turns_name())
}

/// Remove the course, and then the turns stored beside it. A cut between
/// leaves turns that name no course.
pub fn delete(media: &mut impl ReadWrite) -> Result<(), StoreError> {
    remove(media, &name())?;
    if let Err(error) = delete_turns(media) {
        log::warn!(target: "sdk.course", "turns_not_deleted error={error:?}");
    }
    Ok(())
}

fn remove(media: &mut impl ReadWrite, name: &files::Name) -> Result<(), StoreError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    match files::delete(&mut store, name, &mut sector) {
        Ok(()) | Err(files::Error::NotFound) => Ok(()),
        Err(failed) => Err(error(failed)),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Store {
    /// Not read yet, or the course in memory is not the stored one.
    #[default]
    Unread,
    /// The course in memory is the stored one, or there is none.
    Kept,
    /// No card, no FILES volume or no formatted file store.
    Unavailable,
    /// A file is there that does not decode; it stays until replaced.
    Damaged,
}

impl Store {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Unread => "reading",
            Self::Kept => "kept",
            Self::Unavailable => "unavailable",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Kept {
    pub(crate) store: Store,
    retry_at: u64,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    /// The bulk medium is usable.
    pub(crate) ready: bool,
    pub(crate) uploading: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Out {
    pub(crate) read: bool,
}

impl Kept {
    /// The result of reading or writing the stored course.
    fn took(&mut self, result: Result<(), StoreError>, now: u64) {
        self.store = match result {
            Ok(()) => Store::Kept,
            Err(StoreError::Media) => {
                self.retry_at = now.saturating_add(RETRY_MS);
                Store::Unread
            }
            Err(StoreError::Damaged) => Store::Damaged,
            // The course never reads `Foreign`.
            Err(StoreError::Unavailable | StoreError::Full | StoreError::Foreign) => {
                Store::Unavailable
            }
        };
    }

    pub(crate) fn service(
        &mut self,
        seen: Seen,
        media: &mut impl ReadWrite,
        course: &mut Course,
        now: u64,
    ) -> Out {
        let Seen { ready, uploading } = seen;
        if ready && !self.card_ready && self.store == Store::Unavailable {
            self.store = Store::Unread;
        }
        self.card_ready = ready;
        if self.store != Store::Unread || uploading || now < self.retry_at {
            return Out::default();
        }
        let result = load(media, course);
        if let Err(error) = result {
            log::warn!(target: "sdk.course", "load_failed error={error:?}");
        }
        self.took(result, now);
        Out { read: true }
    }
}
