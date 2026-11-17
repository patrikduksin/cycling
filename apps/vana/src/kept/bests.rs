//! The rider's all-time bests: power over 5 s, 1, 5 and 20 min (the
//! `EFFORT_SECONDS`), the longest ride by distance and by time, and the most
//! ascent, each with the day it was set.
//!
//! Bests are ride history, not preferences, so they live in one small file on
//! the FILES volume (`bests.dat`), beside the workout outcomes
//! (`workout::history`), and the preference record is untouched. A ride's
//! outcome merges in once, when the ride is saved; it counts only when
//! strictly higher than the stored best, so a tie keeps the older day. A value
//! of 0 is "no best yet". Where there is no formatted FILES store (no memory
//! card, no FILES volume, or a store the host has not formatted), bests are
//! not kept at all: formatting is the host's decision, never a ride's, and a
//! host `files format` erases them.
//!
//! Persisted, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VBST` |
//! | 4 | version, 2 |
//! | 5 | body length before the CRC, 38 |
//! | 6..38 | the bests (`Bests::ENCODED`) |
//! | 38..42 | CRC32 of bytes 0..38 |
//!
//! A file of another version is foreign: never merged into nor replaced,
//! CLEAR BESTS included; only FORMAT erases it. A file of this version that
//! does not decode is damaged, and CLEAR BESTS replaces it.
//!
//! The ride bests need a ride of `MIN_RIDE_S`, so a test press of a few
//! seconds does not become "the longest ride". A power above
//! `PLAUSIBLE_WATTS` for its window is a sensor glitch and never a best.
use super::{RETRY_MS, StoreError};
use crate::ride::{Contents, another_version};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::crc32;
use firmware_services::{files, volume};

/// The shortest ride that sets a distance, time or ascent best.
pub const MIN_RIDE_S: u32 = 10 * 60;
/// The most average watts a person holds for 5 s, 1, 5 and 20 min; anything
/// above is a sensor glitch and is ignored.
pub const PLAUSIBLE_WATTS: [u32; 4] = [2_500, 1_500, 800, 600];
/// Share of a 20 min best that is the FTP it suggests, in percent.
const FTP_SHARE_PCT: u32 = 95;
/// How much higher than the stored FTP a suggestion must be, in percent.
const FTP_GAIN_PCT: u32 = 3;

/// A best and the day it was set, as days since 1970-01-01 in the clock's
/// time zone; day 0 when the clock was unknown. Value 0 is no best.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    pub value: u32,
    pub day: u16,
}

impl Mark {
    pub const NONE: Self = Self { value: 0, day: 0 };

    /// Raise to `value` when it beats the best; returns whether it did.
    fn raise(&mut self, value: u32, day: u16) -> bool {
        if value <= self.value {
            return false;
        }
        *self = Self { value, day };
        true
    }
}

/// What a ride adds up to, in the units the bests keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Best average watts over each of `EFFORT_SECONDS`.
    pub power: [Option<u16>; 4],
    pub distance_m: u32,
    pub time_s: u32,
    pub ascent_m: u32,
}

/// Which bests a ride set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub power: [bool; 4],
    pub distance: bool,
    pub time: bool,
    pub ascent: bool,
}

impl Found {
    pub const EMPTY: Self = Self {
        power: [false; 4],
        distance: false,
        time: false,
        ascent: false,
    };

    pub fn count(&self) -> usize {
        self.power.iter().filter(|set| **set).count()
            + usize::from(self.distance)
            + usize::from(self.time)
            + usize::from(self.ascent)
    }

    pub fn any(&self) -> bool {
        self.count() != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bests {
    /// Watts over each of `EFFORT_SECONDS`.
    pub power: [Mark; 4],
    pub distance_m: Mark,
    pub time_s: Mark,
    pub ascent_m: Mark,
}

impl Bests {
    pub const EMPTY: Self = Self {
        power: [Mark::NONE; 4],
        distance_m: Mark::NONE,
        time_s: Mark::NONE,
        ascent_m: Mark::NONE,
    };

    /// Bytes of the stored field, little-endian: four power marks of watts
    /// and day (u16 each), distance in metres (u32) and day (u16), time in
    /// seconds (u32) and day (u16), ascent in metres (u16) and day (u16).
    pub const ENCODED: usize = 4 * 4 + 6 + 6 + 4;

    /// Take a ride's outcome in; returns which bests it set.
    pub fn merge(&mut self, ride: &Outcome, day: u16) -> Found {
        let mut found = Found::default();
        let windows = self.power.iter_mut().zip(ride.power).zip(PLAUSIBLE_WATTS);
        for (((best, watts), limit), set) in windows.zip(&mut found.power) {
            *set = watts
                .map(u32::from)
                .is_some_and(|watts| watts <= limit && best.raise(watts, day));
        }
        if ride.time_s >= MIN_RIDE_S {
            found.distance = self.distance_m.raise(ride.distance_m, day);
            found.time = self.time_s.raise(ride.time_s, day);
            found.ascent = self.ascent_m.raise(ride.ascent_m, day);
        }
        found
    }

    /// How many bests are set.
    pub fn count(&self) -> usize {
        self.power
            .iter()
            .chain([&self.distance_m, &self.time_s, &self.ascent_m])
            .filter(|mark| mark.value != 0)
            .count()
    }

    pub fn encode(&self) -> [u8; Self::ENCODED] {
        let mut bytes = heapless::Vec::<u8, { Self::ENCODED }>::new();
        let mut put = |source: &[u8]| {
            let _ = bytes.extend_from_slice(source);
        };
        let short = |mark: &Mark| (mark.value.min(u32::from(u16::MAX)) as u16).to_le_bytes();
        for mark in &self.power {
            put(&short(mark));
            put(&mark.day.to_le_bytes());
        }
        put(&self.distance_m.value.to_le_bytes());
        put(&self.distance_m.day.to_le_bytes());
        put(&self.time_s.value.to_le_bytes());
        put(&self.time_s.day.to_le_bytes());
        put(&short(&self.ascent_m));
        put(&self.ascent_m.day.to_le_bytes());
        let mut encoded = [0; Self::ENCODED];
        encoded.copy_from_slice(&bytes);
        encoded
    }

    /// The bests of a stored field; none for a field of the wrong length.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; Self::ENCODED] = bytes.try_into().ok()?;
        let short = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let long = |at: usize| u32::from(short(at)) | u32::from(short(at + 2)) << 16;
        let mut bests = Self::default();
        for (index, mark) in bests.power.iter_mut().enumerate() {
            *mark = Mark {
                value: u32::from(short(index * 4)),
                day: short(index * 4 + 2),
            };
        }
        bests.distance_m = Mark {
            value: long(16),
            day: short(20),
        };
        bests.time_s = Mark {
            value: long(22),
            day: short(26),
        };
        bests.ascent_m = Mark {
            value: u32::from(short(28)),
            day: short(30),
        };
        Some(bests)
    }
}

const FILE: &str = "bests.dat";
const MAGIC: &[u8; 4] = b"VBST";
pub const VERSION: u8 = 2;
const VERSION_AT: usize = 4;
const BODY_LENGTH_AT: usize = 5;
const FIELDS_AT: usize = 6;
const BODY: usize = FIELDS_AT + Bests::ENCODED;
const SIZE: usize = BODY + 4;

fn error(error: files::Error) -> StoreError {
    use device_api::bulk::Error as Media;
    match error {
        files::Error::Media(Media::Failed) | files::Error::Stale => StoreError::Media,
        files::Error::Media(_) => StoreError::Unavailable,
        files::Error::Full | files::Error::TooLarge => StoreError::Full,
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

pub fn encode_file(bests: &Bests) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[VERSION_AT] = VERSION;
    bytes[BODY_LENGTH_AT] = BODY as u8;
    bytes[FIELDS_AT..BODY].copy_from_slice(&bests.encode());
    let crc = crc32(&bytes[..BODY]);
    bytes[BODY..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// The bests of a current file, or `None` if damaged or foreign.
pub fn decode_file(bytes: &[u8]) -> Option<Bests> {
    let bytes: &[u8; SIZE] = bytes.try_into().ok()?;
    if bytes[..4] != *MAGIC
        || bytes[VERSION_AT] != VERSION
        || usize::from(bytes[BODY_LENGTH_AT]) != BODY
        || crc32(&bytes[..BODY]).to_le_bytes() != bytes[BODY..]
    {
        return None;
    }
    Bests::decode(&bytes[FIELDS_AT..BODY])
}

enum File {
    Missing,
    Current(Bests),
    Damaged,
    Foreign,
}

fn read(media: &mut impl ReadWrite) -> Result<File, StoreError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector).map_err(error)?;
    let name = files::Name::new(FILE).map_err(error)?;
    let Some(entry) = dir.find(&mut store, &name, &mut sector).map_err(error)? else {
        return Ok(File::Missing);
    };
    let mut bytes = [0; SIZE];
    let length = (entry.length as usize).min(SIZE);
    dir.read(&mut store, &entry, 0, &mut bytes[..length], &mut sector)
        .map_err(error)?;
    let bytes = &bytes[..length];
    Ok(if another_version(bytes, MAGIC, VERSION) {
        File::Foreign
    } else if entry.length as usize != SIZE {
        File::Damaged
    } else {
        decode_file(bytes).map_or(File::Damaged, File::Current)
    })
}

pub fn contents(media: &mut impl ReadWrite) -> Result<Contents, StoreError> {
    Ok(match read(media)? {
        File::Missing => Contents::Missing,
        File::Current(_) => Contents::Current,
        File::Damaged => Contents::Damaged,
        File::Foreign => Contents::Foreign,
    })
}

/// The stored bests; none when the store has no file yet.
pub fn load(media: &mut impl ReadWrite) -> Result<Bests, StoreError> {
    match read(media)? {
        File::Missing => Ok(Bests::default()),
        File::Current(bests) => Ok(bests),
        File::Damaged => Err(StoreError::Damaged),
        File::Foreign => Err(StoreError::Foreign),
    }
}

/// Store `bests`, replacing a current or damaged file in one atomic commit.
/// A store the host has not formatted is not formatted here.
pub fn save(media: &mut impl ReadWrite, bests: &Bests) -> Result<(), StoreError> {
    if contents(media)? == Contents::Foreign {
        return Err(StoreError::Foreign);
    }
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    files::mount(&mut store, &mut sector).map_err(error)?;
    let name = files::Name::new(FILE).map_err(error)?;
    files::write(&mut store, &name, &encode_file(bests), &mut sector).map_err(error)?;
    Ok(())
}

/// The FTP a new 20 min best suggests: 95% of it, when that is at least 3%
/// above the FTP in force.
pub fn suggest_ftp(best_20_min: u16, ftp_watts: u16) -> Option<u16> {
    let suggested = (u32::from(best_20_min) * FTP_SHARE_PCT + 50) / 100;
    let enough = suggested * 100 >= u32::from(ftp_watts) * (100 + FTP_GAIN_PCT);
    (enough && suggested > 0).then_some(suggested.min(u32::from(u16::MAX)) as u16)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Store {
    /// Not read yet, or to be read again.
    #[default]
    Unread,
    Kept,
    /// No card, no FILES volume or no formatted file store.
    Unavailable,
    /// A file is there that does not decode; it is left alone.
    Damaged,
    Foreign,
    /// The store would not take a write: the bests are in memory only.
    NotSaved,
}

/// The bests in force and what the BESTS view is doing with them.
#[derive(Default)]
pub(crate) struct Kept {
    pub(crate) bests: Bests,
    pub(crate) store: Store,
    /// BESTS is on screen, so it reads the file again once on opening.
    open: bool,
    /// ACT asked to clear them; the rider has yet to confirm.
    pub(crate) confirming: bool,
    /// The rider confirmed: the next service clears the file.
    pub(crate) clear: bool,
    /// A finished ride's outcome whose read failed this time.
    pending: Option<(Outcome, u16)>,
    /// `bests` hold a merge the store has not taken yet.
    unsaved: bool,
    retry_at: u64,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
}

/// What the bests' service sees of the rest of the device.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    /// BESTS is on screen.
    pub(crate) open: bool,
    /// The bulk medium is usable.
    pub(crate) ready: bool,
    /// A ride is open or being written: the file waits.
    pub(crate) busy: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Out {
    /// What BESTS and the BESTS row show may have changed.
    pub(crate) changed: bool,
    /// The confirmed clear did not reach the store.
    pub(crate) clear_failed: bool,
}

impl Kept {
    /// Take a ride's outcome into the stored bests; returns what it set,
    /// which is nothing when the store could not be used.
    pub(crate) fn merge(
        &mut self,
        media: &mut impl ReadWrite,
        outcome: Outcome,
        day: u16,
        now: u64,
    ) -> Found {
        let base = if self.unsaved {
            Ok(self.bests)
        } else {
            load(media)
        };
        let mut bests = match base {
            Ok(bests) => bests,
            Err(StoreError::Media) => {
                log::warn!(target: "sdk.bests", "load_failed error=Media");
                self.pending = Some((outcome, day));
                self.retry_at = now.saturating_add(RETRY_MS);
                return Found::default();
            }
            Err(StoreError::Damaged) => {
                self.store = Store::Damaged;
                return Found::default();
            }
            Err(StoreError::Foreign) => {
                self.store = Store::Foreign;
                return Found::default();
            }
            Err(_) => {
                self.store = Store::Unavailable;
                return Found::default();
            }
        };
        self.pending = None;
        let found = bests.merge(&outcome, day);
        self.bests = bests;
        if !found.any() && !self.unsaved {
            self.store = Store::Kept;
            return Found::default();
        }
        self.write(media, now);
        if self.unsaved {
            Found::default()
        } else {
            found
        }
    }

    /// Write the bests; if the store refuses, keep them in memory.
    fn write(&mut self, media: &mut impl ReadWrite, now: u64) {
        match save(media, &self.bests) {
            Ok(()) => {
                self.store = Store::Kept;
                self.unsaved = false;
            }
            Err(StoreError::Unavailable) => self.store = Store::Unavailable,
            Err(StoreError::Foreign) => self.store = Store::Foreign,
            Err(error) => {
                log::warn!(target: "sdk.bests", "save_failed error={error:?}");
                self.store = Store::NotSaved;
                self.unsaved = true;
                self.retry_at = now.saturating_add(RETRY_MS);
            }
        }
    }

    pub(crate) fn service(&mut self, seen: Seen, media: &mut impl ReadWrite, now: u64) -> Out {
        let Seen { open, ready, busy } = seen;
        let mut out = Out::default();
        if open && !self.open {
            if self.unsaved {
                self.retry_at = 0;
            } else {
                self.store = Store::Unread;
            }
        }
        if ready && !self.card_ready && self.store == Store::Unavailable {
            self.store = Store::Unread;
        }
        self.card_ready = ready;
        self.open = open;
        if !open {
            self.confirming = false;
        }
        if busy {
            return out;
        }
        if self.clear {
            self.clear = false;
            match save(media, &Bests::default()) {
                Ok(()) => {
                    *self = Kept {
                        store: Store::Kept,
                        open,
                        card_ready: ready,
                        ..Kept::default()
                    };
                }
                Err(error) => {
                    log::warn!(target: "sdk.bests", "clear_failed error={error:?}");
                    out.clear_failed = true;
                }
            }
            out.changed = true;
        }
        if now < self.retry_at {
            return out;
        }
        if self.unsaved && self.pending.is_none() && open {
            self.write(media, now);
            out.changed = true;
        } else if let Some((outcome, day)) = self.pending {
            self.merge(media, outcome, day, now);
            out.changed = true;
        } else if self.store == Store::Unread {
            match load(media) {
                Ok(bests) => {
                    self.bests = bests;
                    self.store = Store::Kept;
                }
                Err(StoreError::Media) => self.retry_at = now.saturating_add(RETRY_MS),
                Err(StoreError::Damaged) => self.store = Store::Damaged,
                Err(StoreError::Foreign) => self.store = Store::Foreign,
                Err(_) => self.store = Store::Unavailable,
            }
            out.changed = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::tests::{Region, formatted, put};

    fn stored(region: &mut Region) -> Option<std::vec::Vec<u8>> {
        let mut sector = [0; SECTOR_SIZE];
        let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
        let dir = files::mount(&mut store, &mut sector).unwrap();
        let name = files::Name::new(FILE).unwrap();
        let entry = dir.find(&mut store, &name, &mut sector).unwrap()?;
        let mut bytes = std::vec![0; entry.length as usize];
        dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
            .unwrap();
        Some(bytes)
    }

    #[test]
    fn a_damaged_file_is_replaced_and_one_of_another_version_never_is() {
        let mut region = formatted();
        assert_eq!(contents(&mut region), Ok(Contents::Missing));
        let bests = Bests {
            time_s: mark(3600, 8),
            ..Bests::default()
        };
        save(&mut region, &bests).unwrap();
        assert_eq!(contents(&mut region), Ok(Contents::Current));
        assert_eq!(load(&mut region), Ok(bests));

        let mut damaged = encode_file(&bests);
        damaged[FIELDS_AT] ^= 1;
        put(&mut region, FILE, &damaged);
        assert_eq!(load(&mut region), Err(StoreError::Damaged));
        put(&mut region, FILE, b"VBST");
        assert_eq!(load(&mut region), Err(StoreError::Damaged));
        save(&mut region, &Bests::default()).unwrap();
        assert_eq!(load(&mut region), Ok(Bests::default()));

        let mut foreign = encode_file(&bests).to_vec();
        foreign[VERSION_AT] = VERSION + 1;
        foreign.extend_from_slice(&[0; 8]);
        put(&mut region, FILE, &foreign);
        assert_eq!(contents(&mut region), Ok(Contents::Foreign));
        assert_eq!(load(&mut region), Err(StoreError::Foreign));
        assert_eq!(
            save(&mut region, &Bests::default()),
            Err(StoreError::Foreign)
        );
        assert_eq!(stored(&mut region), Some(foreign), "left as it was");
    }

    fn ride(power: [Option<u16>; 4], km: u32, minutes: u32, ascent: u32) -> Outcome {
        Outcome {
            power,
            distance_m: km * 1000,
            time_s: minutes * 60,
            ascent_m: ascent,
        }
    }

    fn mark(value: u32, day: u16) -> Mark {
        Mark { value, day }
    }

    #[test]
    fn the_first_ride_sets_every_best_it_has() {
        let mut bests = Bests::default();
        let found = bests.merge(
            &ride([Some(600), Some(400), None, None], 30, 60, 300),
            20_000,
        );
        assert_eq!(found.power, [true, true, false, false]);
        assert!(found.distance && found.time && found.ascent);
        assert_eq!(found.count(), 5);
        assert_eq!(bests.power[0], mark(600, 20_000));
        assert_eq!(bests.power[2], Mark::default());
        assert_eq!(bests.count(), 5);
    }

    #[test]
    fn only_a_higher_value_replaces_a_best_and_its_day() {
        let mut bests = Bests::default();
        bests.merge(&ride([Some(300); 4], 50, 120, 800), 100);
        // A tie keeps the older day; lower values change nothing.
        let tie = ride([Some(300), Some(299), Some(200), None], 50, 120, 800);
        assert!(!bests.merge(&tie, 101).any());
        assert_eq!(bests.power[0].day, 100);
        let better = ride([None, Some(310), None, Some(301)], 60, 90, 799);
        let found = bests.merge(&better, 102);
        assert_eq!(found.power, [false, true, false, true]);
        assert!(found.distance && !found.time && !found.ascent);
        assert_eq!(bests.power[1], mark(310, 102));
        assert_eq!(bests.power[0].day, 100);
        assert_eq!(bests.distance_m, mark(60_000, 102));
        assert_eq!(bests.time_s, mark(7200, 100));
    }

    #[test]
    fn a_short_ride_sets_power_bests_but_not_ride_bests() {
        let mut bests = Bests::default();
        let found = bests.merge(&ride([Some(500), None, None, None], 3, 9, 40), 5);
        assert_eq!(found.count(), 1);
        assert_eq!(bests.distance_m, Mark::default());
        let found = bests.merge(&ride([None; 4], 3, 10, 0), 5);
        assert!(found.distance && found.time && !found.ascent);
    }

    #[test]
    fn bests_round_trip_and_the_layout_is_pinned() {
        let bests = Bests {
            power: [
                mark(0x0102, 3),
                Mark::default(),
                mark(312, 4),
                mark(250, 0xffff),
            ],
            distance_m: mark(0x0001_0203, 7),
            time_s: mark(3600, 8),
            ascent_m: mark(1500, 9),
        };
        let bytes = bests.encode();
        assert_eq!(
            bytes,
            [
                2, 1, 3, 0, 0, 0, 0, 0, 0x38, 1, 4, 0, 250, 0, 0xff, 0xff, // power
                3, 2, 1, 0, 7, 0, // distance
                0x10, 0x0e, 0, 0, 8, 0, // time
                0xdc, 5, 9, 0, // ascent
            ]
        );
        assert_eq!(Bests::decode(&bytes), Some(bests));
        assert_eq!(Bests::decode(&[0; Bests::ENCODED]), Some(Bests::default()));
        assert_eq!(Bests::decode(&bytes[1..]), None);
        assert_eq!(Bests::default().encode(), [0; Bests::ENCODED]);
    }

    #[test]
    fn a_glitch_above_what_a_person_holds_is_never_a_best() {
        let mut bests = Bests::default();
        let glitch = ride([Some(2501), Some(1501), Some(801), Some(601)], 30, 60, 300);
        let found = bests.merge(&glitch, 9);
        assert_eq!(found.power, [false; 4]);
        assert!(found.distance);
        let limits = ride([Some(2500), Some(1500), Some(800), Some(600)], 30, 60, 300);
        assert_eq!(bests.merge(&limits, 9).power, [true; 4]);
    }

    fn sealed(version: u8, fields: &[u8]) -> std::vec::Vec<u8> {
        let length = (FIELDS_AT + fields.len()) as u8;
        let mut bytes = [&MAGIC[..], &[version, length], fields].concat();
        let crc = crc32(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }

    #[test]
    fn the_file_round_trips_and_a_foreign_one_is_refused() {
        let mut bests = Bests::default();
        bests.merge(&ride([Some(300); 4], 50, 120, 800), 100);
        let bytes = encode_file(&bests);
        assert_eq!(&bytes[..4], b"VBST");
        assert_eq!(bytes.len(), 42);
        assert_eq!(decode_file(&bytes), Some(bests));
        let fields = &bytes[FIELDS_AT..BODY];
        assert_eq!(decode_file(&sealed(VERSION, fields)), Some(bests));
        assert_eq!(decode_file(&sealed(VERSION + 1, fields)), None);
        let appended = [fields, &[0, 0]].concat();
        assert_eq!(decode_file(&sealed(VERSION + 1, &appended)), None);
        let mut torn = bytes;
        torn[10] ^= 1;
        assert_eq!(decode_file(&torn), None);
        assert_eq!(decode_file(&bytes[..SIZE - 1]), None);
        assert_eq!(decode_file(b"VWKO"), None);
    }

    #[test]
    fn an_ftp_is_suggested_only_for_a_clear_gain() {
        // 95% of 258 is 245.1, which is 14% over 215.
        assert_eq!(suggest_ftp(258, 215), Some(245));
        // 3% over 245 is 252.35: 253 suggests, 252 does not.
        assert_eq!(suggest_ftp(266, 245), Some(253));
        assert_eq!(suggest_ftp(265, 245), None);
        assert_eq!(suggest_ftp(200, 215), None);
        assert_eq!(suggest_ftp(0, 215), None);
    }

    #[derive(Default)]
    struct Card {
        region: Region,
        reads: usize,
        writes: usize,
        fail_reads: bool,
        fail_writes: bool,
    }

    fn fails(failing: bool) -> Result<(), device_api::bulk::Error> {
        if failing {
            Err(device_api::bulk::Error::Failed)
        } else {
            Ok(())
        }
    }

    impl ReadWrite for Card {
        fn entropy(&mut self) -> u64 {
            0
        }
        fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
            self.region.owned_info()
        }
        fn owned_read(
            &mut self,
            sector: u64,
            output: &mut [u8; SECTOR_SIZE],
        ) -> Result<(), device_api::bulk::Error> {
            self.reads += 1;
            fails(self.fail_reads)?;
            self.region.owned_read(sector, output)
        }
        fn owned_write(
            &mut self,
            sector: u64,
            input: &[u8; SECTOR_SIZE],
        ) -> Result<(), device_api::bulk::Error> {
            self.writes += 1;
            fails(self.fail_writes)?;
            self.region.owned_write(sector, input)
        }
    }

    const IDLE: Seen = Seen {
        open: false,
        ready: true,
        busy: false,
    };
    const OPEN: Seen = Seen { open: true, ..IDLE };
    const CHANGED: Out = Out {
        changed: true,
        clear_failed: false,
    };

    #[test]
    fn the_store_is_read_once_retried_after_a_failure_and_left_alone_during_a_ride() {
        let mut card = Card {
            region: formatted(),
            ..Card::default()
        };
        let set = Bests {
            time_s: mark(3600, 8),
            ..Bests::default()
        };
        save(&mut card, &set).unwrap();
        let mut kept = Kept::default();

        let busy = Seen { busy: true, ..IDLE };
        let reads = card.reads;
        assert_eq!(kept.service(busy, &mut card, 0), Out::default());
        assert_eq!(
            (card.reads, kept.store),
            (reads, Store::Unread),
            "not during a ride"
        );

        card.fail_reads = true;
        assert_eq!(kept.service(IDLE, &mut card, 0), CHANGED);
        assert_eq!(kept.store, Store::Unread, "a failed read is tried again");
        card.fail_reads = false;
        let reads = card.reads;
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS - 1), Out::default());
        assert_eq!(card.reads, reads, "not before the retry");
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS), CHANGED);
        assert_eq!((kept.store, kept.bests), (Store::Kept, set));

        let reads = card.reads;
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS), Out::default());
        assert_eq!(card.reads, reads, "read once");
        assert_eq!(kept.service(OPEN, &mut card, RETRY_MS), CHANGED);
        assert!(card.reads > reads, "opening BESTS reads the file again");

        put(&mut card.region, FILE, b"VBST");
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS), Out::default());
        assert_eq!(kept.service(OPEN, &mut card, RETRY_MS), CHANGED);
        assert_eq!(kept.store, Store::Damaged);
    }

    #[test]
    fn a_confirmed_clear_empties_the_file_or_says_it_failed() {
        let mut card = Card {
            region: formatted(),
            ..Card::default()
        };
        save(
            &mut card,
            &Bests {
                time_s: mark(3600, 8),
                ..Bests::default()
            },
        )
        .unwrap();
        let mut kept = Kept::default();
        kept.service(OPEN, &mut card, 0);

        kept.confirming = true;
        kept.clear = true;
        card.fail_writes = true;
        let out = kept.service(OPEN, &mut card, 0);
        assert_eq!(
            out,
            Out {
                changed: true,
                clear_failed: true,
            }
        );
        assert!(!kept.clear, "a failed clear is not tried again by itself");
        assert_eq!(load(&mut card.region).map(|bests| bests.count()), Ok(1));

        card.fail_writes = false;
        kept.clear = true;
        assert_eq!(kept.service(OPEN, &mut card, 0), CHANGED);
        assert_eq!(load(&mut card), Ok(Bests::default()));
        assert_eq!((kept.store, kept.bests.count()), (Store::Kept, 0));
        assert!(
            kept.open && kept.card_ready,
            "the view stays open on a ready card"
        );
    }

    #[test]
    fn a_merge_the_store_refused_waits_for_bests_to_open_and_one_it_could_not_read_retries() {
        let mut card = Card {
            region: formatted(),
            ..Card::default()
        };
        let mut kept = Kept::default();
        kept.service(IDLE, &mut card, 0);
        let outcome = ride([Some(400); 4], 40, 90, 500);

        card.fail_reads = true;
        kept.merge(&mut card, outcome, 9, 0);
        card.fail_reads = false;
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS - 1), Out::default());
        assert_eq!(kept.service(IDLE, &mut card, RETRY_MS), CHANGED);
        assert_eq!(
            load(&mut card).map(|bests| bests.count()),
            Ok(7),
            "the read retried"
        );

        let better = ride([Some(500); 4], 40, 90, 500);
        card.fail_writes = true;
        kept.merge(&mut card, better, 10, RETRY_MS);
        card.fail_writes = false;
        assert_eq!(kept.store, Store::NotSaved);
        let later = 3 * RETRY_MS;
        let writes = card.writes;
        assert_eq!(kept.service(IDLE, &mut card, later), Out::default());
        assert_eq!(card.writes, writes, "not until BESTS opens");
        assert_eq!(kept.service(OPEN, &mut card, later), CHANGED);
        assert_eq!(kept.store, Store::Kept);
        assert_eq!(load(&mut card).map(|bests| bests.power[0].value), Ok(500));
    }
}
