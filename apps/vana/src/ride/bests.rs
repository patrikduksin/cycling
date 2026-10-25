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
//! Persisted version 1, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VBST` |
//! | 4 | version, 1 |
//! | 5 | body length `n` before the CRC: 38 in version 1 |
//! | 6..38 | the bests (`Bests::ENCODED`) |
//! | n..n+4 | CRC32 of bytes 0..n |
//!
//! Later versions only append fields before the CRC and raise the version; a
//! reader takes the fields it knows from any version at least 1. This reader
//! takes at most `READ_MAX` (84) bytes of a file, so a later version must keep
//! the whole file, CRC included, within 84 bytes or raise the reader first;
//! a longer one reads as damaged. A file that does not decode is never merged
//! into; only CLEAR BESTS replaces it.
//!
//! Only rides recorded live count, and the caller applies that rule: a demo
//! ride never reaches `merge`. The ride bests also need a ride of
//! `MIN_RIDE_S`, so a test press of a few seconds does not become "the
//! longest ride". A power above `PLAUSIBLE_WATTS` for its window is a
//! sensor glitch and never a best.
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
pub const VERSION: u8 = 1;
const BODY_V1: usize = 6 + Bests::ENCODED;
const SIZE: usize = BODY_V1 + 4;
/// The most bytes of a file this reader takes.
pub const READ_MAX: usize = 2 * SIZE;

/// Why bests could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No card, no FILES volume or no formatted file store: nothing to keep
    /// bests in.
    Unavailable,
    /// The medium failed this time; trying again may work.
    Media,
    /// A file is there but does not decode.
    Damaged,
    /// The store has no room for the file.
    Full,
}

impl From<files::Error> for Error {
    fn from(error: files::Error) -> Self {
        use device_api::bulk::Error as Media;
        match error {
            files::Error::Media(Media::Failed) | files::Error::Stale => Self::Media,
            files::Error::Media(_) => Self::Unavailable,
            files::Error::Full | files::Error::TooLarge => Self::Full,
            _ => Self::Unavailable,
        }
    }
}

fn open<'a, M: ReadWrite>(
    media: &'a mut M,
    sector: &mut files::Sector,
) -> Result<volume::Volume<'a, M>, Error> {
    volume::open(media, &volume::FILES, sector).map_err(|error| match error {
        volume::Error::Media(device_api::bulk::Error::Failed) => Error::Media,
        _ => Error::Unavailable,
    })
}

pub fn encode_file(bests: &Bests) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = VERSION;
    bytes[5] = BODY_V1 as u8;
    bytes[6..BODY_V1].copy_from_slice(&bests.encode());
    let crc = crc32(&bytes[..BODY_V1]);
    bytes[BODY_V1..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// The bests of a file of any version from 1 on, or `None` if damaged or
/// foreign.
pub fn decode_file(bytes: &[u8]) -> Option<Bests> {
    let body = usize::from(*bytes.get(5)?);
    let crc = bytes.get(body..body + 4)?;
    if bytes.get(..4)? != MAGIC
        || bytes[4] < 1
        || body < BODY_V1
        || crc32(&bytes[..body]).to_le_bytes() != crc
    {
        return None;
    }
    Bests::decode(&bytes[6..BODY_V1])
}

/// The stored bests; none when the store has no file yet.
pub fn load(media: &mut impl ReadWrite) -> Result<Bests, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let name = files::Name::new(FILE)?;
    let Some(entry) = dir.find(&mut store, &name, &mut sector)? else {
        return Ok(Bests::default());
    };
    let mut bytes = [0; READ_MAX];
    let length = (entry.length as usize).min(READ_MAX);
    dir.read(&mut store, &entry, 0, &mut bytes[..length], &mut sector)?;
    decode_file(&bytes[..length]).ok_or(Error::Damaged)
}

/// Store `bests`, replacing whatever file is there in one atomic commit. A
/// store the host has not formatted is not formatted here.
pub fn save(media: &mut impl ReadWrite, bests: &Bests) -> Result<(), Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    files::mount(&mut store, &mut sector)?;
    let name = files::Name::new(FILE)?;
    files::write(&mut store, &name, &encode_file(bests), &mut sector)?;
    Ok(())
}

/// The FTP a new 20 min best suggests: 95% of it, when that is at least 3%
/// above the FTP in force.
pub fn suggest_ftp(best_20_min: u16, ftp_watts: u16) -> Option<u16> {
    let suggested = (u32::from(best_20_min) * FTP_SHARE_PCT + 50) / 100;
    let enough = suggested * 100 >= u32::from(ftp_watts) * (100 + FTP_GAIN_PCT);
    (enough && suggested > 0).then_some(suggested.min(u32::from(u16::MAX)) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_file_round_trips_and_a_foreign_one_is_refused() {
        let mut bests = Bests::default();
        bests.merge(&ride([Some(300); 4], 50, 120, 800), 100);
        let bytes = encode_file(&bests);
        assert_eq!(&bytes[..4], b"VBST");
        assert_eq!(bytes.len(), 42);
        assert_eq!(decode_file(&bytes), Some(bests));
        // A later version appends fields before the CRC; this reader skips them.
        let mut longer = [0u8; SIZE + 2];
        longer[..BODY_V1].copy_from_slice(&bytes[..BODY_V1]);
        longer[4] = 2;
        longer[5] = (BODY_V1 + 2) as u8;
        let crc = crc32(&longer[..BODY_V1 + 2]);
        longer[BODY_V1 + 2..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_file(&longer), Some(bests));
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
}
