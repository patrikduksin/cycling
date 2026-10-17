//! How a saved ride's workout went, kept on the FILES volume beside the
//! ride: one small file per ride, `rides/<journal><ride id>.workout`.
//!
//! Persisted version 1, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VWKO` |
//! | 4 | version, 1 |
//! | 5 | body length `n` before the CRC: 66 in version 1 |
//! | 6 | journal: `i` internal, `b` bulk |
//! | 7 | flags: 1 finished, 2 start time known, 4 FTP measured |
//! | 8..12 | ride id in that journal |
//! | 12..20 | ride start, UTC ms (0 when unknown) |
//! | 20 | name length, then 20 name bytes |
//! | 41 | test: 0 none, 1 twenty minutes, 2 ramp |
//! | 42..48 | steps done, steps, skipped (u16 each) |
//! | 48..64 | targeted, in, under and over target ms (u32 each) |
//! | 64..66 | FTP the test measured, W |
//! | n..n+4 | CRC32 of bytes 0..n |
//!
//! Later versions only append fields before the CRC and raise the version;
//! a reader takes the fields it knows from any version at least 1. Ride ids
//! restart when a journal is cleared or initialized, and `forget` then
//! deletes that journal's records. Only the newest [`KEEP`] records stay.
use super::library::LoadError;
use super::model::{NAME_BYTES, Name, Test};
use super::player::Summary;
use core::fmt::Write;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::crc32;
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::{files, volume};

pub const FOLDER: &str = "rides/";
const SUFFIX: &str = ".workout";
pub const VERSION: u8 = 1;
const MAGIC: &[u8; 4] = b"VWKO";
/// Body bytes of version 1, before the CRC.
const BODY_V1: usize = 66;
pub const SIZE: usize = BODY_V1 + 4;
/// Records kept; saving another deletes the oldest.
pub const KEEP: usize = 32;

const FINISHED: u8 = 1;
const DATED: u8 = 2;
const MEASURED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    /// `b'i'` for the internal journal, `b'b'` for the bulk log.
    pub journal: u8,
    pub ride_id: u32,
    pub started_utc_ms: Option<u64>,
    pub name: Name,
    pub test: Option<Test>,
    pub summary: Summary,
}

/// The file of ride `ride_id` in `journal`.
pub fn file(journal: u8, ride_id: u32) -> files::Name {
    let mut text = heapless::String::<{ files::NAME }>::new();
    let _ = write!(
        text,
        "{FOLDER}{}{ride_id}{SUFFIX}",
        char::from(journal.to_ascii_lowercase())
    );
    files::Name::new(&text).unwrap_or(files::Name::new("rides/x.workout").unwrap())
}

fn millis(ms: u64) -> u32 {
    u32::try_from(ms).unwrap_or(u32::MAX)
}

pub fn encode(record: &Record) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    let summary = &record.summary;
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = VERSION;
    bytes[5] = BODY_V1 as u8;
    bytes[6] = record.journal;
    bytes[7] = if summary.finished { FINISHED } else { 0 }
        | if record.started_utc_ms.is_some() {
            DATED
        } else {
            0
        }
        | if summary.ftp.is_some() { MEASURED } else { 0 };
    put_u32(&mut bytes, 8, record.ride_id);
    put_u64(&mut bytes, 12, record.started_utc_ms.unwrap_or(0));
    let name = record.name.as_bytes();
    bytes[20] = name.len() as u8;
    bytes[21..21 + name.len()].copy_from_slice(name);
    bytes[41] = match record.test {
        None => 0,
        Some(Test::Twenty) => 1,
        Some(Test::Ramp) => 2,
    };
    put_u16(&mut bytes, 42, summary.steps_done);
    put_u16(&mut bytes, 44, summary.steps);
    put_u16(&mut bytes, 46, summary.skipped);
    put_u32(&mut bytes, 48, millis(summary.targeted_ms));
    put_u32(&mut bytes, 52, millis(summary.in_ms));
    put_u32(&mut bytes, 56, millis(summary.under_ms));
    put_u32(&mut bytes, 60, millis(summary.over_ms));
    put_u16(&mut bytes, 64, summary.ftp.unwrap_or(0));
    let crc = crc32(&bytes[..BODY_V1]);
    put_u32(&mut bytes, BODY_V1, crc);
    bytes
}

/// A record of any version from 1 on, or `None` if damaged or foreign.
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
    let flags = bytes[7];
    let name_len = usize::from(bytes[20]);
    if name_len > NAME_BYTES {
        return None;
    }
    let test = match bytes[41] {
        0 => None,
        1 => Some(Test::Twenty),
        2 => Some(Test::Ramp),
        _ => return None,
    };
    Some(Record {
        journal: bytes[6],
        ride_id: get_u32(bytes, 8),
        started_utc_ms: (flags & DATED != 0).then(|| get_u64(bytes, 12)),
        name: Name::from_bytes(&bytes[21..21 + name_len]),
        test,
        summary: Summary {
            steps_done: get_u16(bytes, 42),
            steps: get_u16(bytes, 44),
            skipped: get_u16(bytes, 46),
            targeted_ms: u64::from(get_u32(bytes, 48)),
            in_ms: u64::from(get_u32(bytes, 52)),
            under_ms: u64::from(get_u32(bytes, 56)),
            over_ms: u64::from(get_u32(bytes, 60)),
            finished: flags & FINISHED != 0,
            ftp: (flags & MEASURED != 0).then(|| get_u16(bytes, 64)),
        },
    })
}

/// Save `record`, replacing one for the same ride and deleting the oldest
/// record once [`KEEP`] are stored.
pub fn save(media: &mut impl ReadWrite, record: &Record) -> Result<(), LoadError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
    let name = file(record.journal, record.ride_id);
    let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
    let mut kept = 0;
    let mut replaces = false;
    let mut oldest: Option<files::Entry> = None;
    dir.list(&mut store, &mut sector, |entry| {
        if !entry.name.as_str().starts_with(FOLDER) || !entry.name.as_str().ends_with(SUFFIX) {
            return;
        }
        kept += 1;
        replaces |= entry.name == name;
        if oldest.is_none_or(|old| entry.revision < old.revision) {
            oldest = Some(*entry);
        }
    })
    .map_err(LoadError::Files)?;
    if !replaces
        && kept >= KEEP
        && let Some(oldest) = oldest
    {
        dir.delete(&mut store, &oldest.name, &mut sector)
            .map_err(LoadError::Files)?;
    }
    files::write(&mut store, &name, &encode(record), &mut sector).map_err(LoadError::Files)?;
    Ok(())
}

/// Delete every record of `journal`, as its ride ids start over.
pub fn forget(media: &mut impl ReadWrite, journal: u8) -> Result<(), LoadError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
    let mut prefix = heapless::String::<8>::new();
    let _ = write!(
        prefix,
        "{FOLDER}{}",
        char::from(journal.to_ascii_lowercase())
    );
    loop {
        let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
        let mut names: heapless::Vec<files::Name, 16> = heapless::Vec::new();
        dir.list(&mut store, &mut sector, |entry| {
            let name = entry.name.as_str();
            if name.starts_with(prefix.as_str()) && name.ends_with(SUFFIX) {
                let _ = names.push(entry.name);
            }
        })
        .map_err(LoadError::Files)?;
        if names.is_empty() {
            return Ok(());
        }
        for name in &names {
            files::delete(&mut store, name, &mut sector).map_err(LoadError::Files)?;
        }
    }
}

/// The record of ride `ride_id` in `journal`, if one is stored.
pub fn load(
    media: &mut impl ReadWrite,
    journal: u8,
    ride_id: u32,
) -> Result<Option<Record>, LoadError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
    let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
    let Some(entry) = dir
        .find(&mut store, &file(journal, ride_id), &mut sector)
        .map_err(LoadError::Files)?
    else {
        return Ok(None);
    };
    let mut bytes = [0; 2 * SIZE];
    let length = (entry.length as usize).min(bytes.len());
    dir.read(&mut store, &entry, 0, &mut bytes[..length], &mut sector)
        .map_err(LoadError::Files)?;
    decode(&bytes[..length])
        .filter(|record| record.journal == journal && record.ride_id == ride_id)
        .map(Some)
        .ok_or(LoadError::Decode(super::codec::DecodeError::Checksum))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::tests::{formatted, put};

    fn record(ride_id: u32) -> Record {
        Record {
            journal: b'b',
            ride_id,
            started_utc_ms: Some(1_760_000_000_000),
            name: Name::new("FTP 20 MIN TEST"),
            test: Some(Test::Twenty),
            summary: Summary {
                steps_done: 11,
                steps: 12,
                skipped: 10,
                targeted_ms: 1_200_000,
                in_ms: 1_100_000,
                under_ms: 60_000,
                over_ms: 40_000,
                finished: true,
                ftp: Some(247),
            },
        }
    }

    #[test]
    fn a_record_round_trips_and_damage_is_refused() {
        let bytes = encode(&record(3));
        assert_eq!(decode(&bytes), Some(record(3)));
        let mut damaged = bytes;
        damaged[30] ^= 1;
        assert_eq!(decode(&damaged), None);
        assert_eq!(decode(&bytes[..SIZE - 1]), None);
        let free = Record {
            started_utc_ms: None,
            test: None,
            summary: Summary::default(),
            ..record(4)
        };
        assert_eq!(decode(&encode(&free)), Some(free));
    }

    #[test]
    fn a_later_version_with_more_fields_still_reads() {
        let mut bytes = [0u8; SIZE + 6];
        bytes[..BODY_V1].copy_from_slice(&encode(&record(5))[..BODY_V1]);
        bytes[4] = 2;
        bytes[5] = (BODY_V1 + 6) as u8;
        bytes[BODY_V1..BODY_V1 + 6].copy_from_slice(&[9; 6]);
        let crc = crc32(&bytes[..BODY_V1 + 6]);
        put_u32(&mut bytes, BODY_V1 + 6, crc);
        assert_eq!(decode(&bytes), Some(record(5)));
    }

    #[test]
    fn records_save_per_ride_and_the_oldest_go() {
        let mut region = formatted();
        put(&mut region, "workouts/keep.vw", b"not a record");
        assert_eq!(file(b'b', 12).as_str(), "rides/b12.workout");
        for ride in 1..=KEEP as u32 + 2 {
            save(&mut region, &record(ride)).unwrap();
        }
        assert_eq!(load(&mut region, b'b', 1), Ok(None));
        assert_eq!(load(&mut region, b'b', 2), Ok(None));
        assert_eq!(load(&mut region, b'b', 3), Ok(Some(record(3))));
        assert_eq!(load(&mut region, b'i', 3), Ok(None));
        // Saving a ride again replaces its record and deletes nothing.
        let mut again = record(3);
        again.summary.skipped = 0;
        save(&mut region, &again).unwrap();
        assert_eq!(load(&mut region, b'b', 3), Ok(Some(again)));
        assert_eq!(load(&mut region, b'b', 4), Ok(Some(record(4))));
        // Clearing the bulk journal forgets its records, not the others'.
        let internal = Record {
            journal: b'i',
            ..record(4)
        };
        save(&mut region, &internal).unwrap();
        forget(&mut region, b'b').unwrap();
        assert_eq!(load(&mut region, b'b', 4), Ok(None));
        assert_eq!(load(&mut region, b'i', 4), Ok(Some(internal)));
    }
}
