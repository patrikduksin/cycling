//! What VANA knew about a saved ride that its journal does not hold, kept on
//! the FILES volume beside the ride: one small file per ride,
//! `rides/<journal><ride id>.facts`, like the workout records.
//!
//! Persisted version 1, little-endian:
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
//! Later versions only append fields before the CRC and raise the version; a
//! reader takes the fields it knows from any version at least 1, and reads
//! up to [`READ_MAX`] bytes, as many as a one-byte body length allows. Ride
//! ids restart when a journal is cleared or initialized, and `forget` then
//! deletes that journal's files; the start time guards the rest, as firmware
//! from before these files clears a journal without deleting them. So a ride
//! whose start time is unknown has no facts. A profile out of range reads as
//! none and leaves the other fields. Only the newest [`KEEP`] files stay.
use super::zones::TrainingProfile;
use core::fmt::Write;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::crc32;
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::{files, volume};

pub const FOLDER: &str = "rides/";
const SUFFIX: &str = ".facts";
pub const VERSION: u8 = 1;
const MAGIC: &[u8; 4] = b"VFCT";
/// Body bytes of version 1, before the CRC.
const BODY_V1: usize = 30;
pub const SIZE: usize = BODY_V1 + 4;
/// The longest file any version can be: a one-byte body length and the CRC.
pub const READ_MAX: usize = u8::MAX as usize + 4;
/// Files kept; saving another deletes the oldest. The FILES directory holds
/// 128 files: these, the 32 workout records, `bests.dat` and the 8 uploads
/// the picker lists leave 23 for the host, and 64 rides cover the two weeks
/// the rides list totals at up to four rides a day.
pub const KEEP: usize = 64;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Volume(volume::Error),
    Files(files::Error),
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

fn is_facts(name: &files::Name) -> bool {
    name.as_str().starts_with(FOLDER) && name.as_str().ends_with(SUFFIX)
}

pub fn encode(record: &Record) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    let facts = &record.facts;
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = VERSION;
    bytes[5] = BODY_V1 as u8;
    bytes[6] = record.journal;
    bytes[7] = flag(facts.profile.is_some(), PROFILE)
        | flag(facts.recovery_bpm.is_some(), RECOVERY)
        | flag(facts.climbing.is_some(), CLIMBS);
    put_u32(&mut bytes, 8, record.ride_id);
    put_u64(&mut bytes, 12, record.started_utc_ms);
    let profile = facts.profile.unwrap_or(TrainingProfile {
        ftp_watts: 0,
        lthr_bpm: 0,
    });
    put_u16(&mut bytes, 20, profile.ftp_watts);
    put_u16(&mut bytes, 22, profile.lthr_bpm);
    put_u16(&mut bytes, 24, facts.recovery_bpm.unwrap_or(0) as u16);
    let climbing = facts.climbing.unwrap_or_default();
    put_u16(&mut bytes, 26, climbing.count);
    put_u16(&mut bytes, 28, climbing.gain_m);
    let crc = crc32(&bytes[..BODY_V1]);
    put_u32(&mut bytes, BODY_V1, crc);
    bytes
}

/// A record of any version from 1 on, or `None` if torn, damaged or foreign.
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
    let has = |bit: u8| flags & bit != 0;
    Some(Record {
        journal: bytes[6],
        ride_id: get_u32(bytes, 8),
        started_utc_ms: get_u64(bytes, 12),
        facts: Facts {
            profile: has(PROFILE)
                .then(|| TrainingProfile::new(get_u16(bytes, 20), get_u16(bytes, 22)))
                .flatten(),
            recovery_bpm: has(RECOVERY).then(|| get_u16(bytes, 24) as i16),
            climbing: has(CLIMBS).then(|| Climbing {
                count: get_u16(bytes, 26),
                gain_m: get_u16(bytes, 28),
            }),
        },
    })
}

/// Save `record`, replacing the file of the same ride and deleting the
/// oldest once [`KEEP`] are stored. Returns the file it deleted.
pub fn save(media: &mut impl ReadWrite, record: &Record) -> Result<Option<files::Name>, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let name = file(record.journal, record.ride_id);
    let dir = files::mount(&mut store, &mut sector)?;
    let mut kept = 0;
    let mut replaces = false;
    let mut oldest: Option<files::Entry> = None;
    dir.list(&mut store, &mut sector, |entry| {
        if !is_facts(&entry.name) {
            return;
        }
        kept += 1;
        replaces |= entry.name == name;
        if oldest.is_none_or(|old| entry.revision < old.revision) {
            oldest = Some(*entry);
        }
    })?;
    let mut deleted = None;
    if !replaces
        && kept >= KEEP
        && let Some(oldest) = oldest
    {
        dir.delete(&mut store, &oldest.name, &mut sector)?;
        deleted = Some(oldest.name);
    }
    files::write(&mut store, &name, &encode(record), &mut sector)?;
    Ok(deleted)
}

/// Delete every file of `journal`, as its ride ids start over.
pub fn forget(media: &mut impl ReadWrite, journal: u8) -> Result<(), Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)?;
    let mut prefix = heapless::String::<8>::new();
    let _ = write!(
        prefix,
        "{FOLDER}{}",
        char::from(journal.to_ascii_lowercase())
    );
    loop {
        let dir = files::mount(&mut store, &mut sector)?;
        let mut names: heapless::Vec<files::Name, 16> = heapless::Vec::new();
        dir.list(&mut store, &mut sector, |entry| {
            if is_facts(&entry.name) && entry.name.as_str().starts_with(prefix.as_str()) {
                let _ = names.push(entry.name);
            }
        })?;
        if names.is_empty() {
            return Ok(());
        }
        for name in &names {
            files::delete(&mut store, name, &mut sector)?;
        }
    }
}

/// Visit the records listed in directory sector `index`, one sector's
/// entries per call so a scan never holds up a tick for long. Files that do
/// not decode or name another ride than their own are skipped. Returns the
/// next sector, or `None` after the last.
pub fn read(
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
        let length = dir.read(&mut store, entry, 0, &mut bytes[..length], &mut sector)?;
        if let Some(record) = decode(&bytes[..length])
            && file(record.journal, record.ride_id) == entry.name
        {
            visit(record);
        }
    }
    let next = index + 1;
    Ok((next < files::DIR_SECTORS).then_some(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Rng;
    use crate::workout::library::tests::{formatted, put};

    fn record(ride_id: u32) -> Record {
        Record {
            journal: b'b',
            ride_id,
            started_utc_ms: 1_760_000_000_000 + u64::from(ride_id) * 3_600_000,
            facts: Facts {
                profile: TrainingProfile::new(250, 172),
                recovery_bpm: Some(-4),
                climbing: Some(Climbing {
                    count: 3,
                    gain_m: 410,
                }),
            },
        }
    }

    fn any(rng: &mut Rng) -> Record {
        Record {
            journal: rng.pick(b"ib"),
            ride_id: rng.next() as u32,
            started_utc_ms: rng.next(),
            facts: Facts {
                profile: rng.option(|rng| {
                    TrainingProfile::new(rng.range(50, 600) as u16, rng.range(80, 220) as u16)
                        .unwrap()
                }),
                recovery_bpm: rng.option(|rng| rng.next() as i16),
                climbing: rng.option(|rng| Climbing {
                    count: rng.next() as u16,
                    gain_m: rng.next() as u16,
                }),
            },
        }
    }

    fn all(region: &mut impl ReadWrite) -> std::vec::Vec<Record> {
        let mut found = std::vec::Vec::new();
        let mut next = Some(0);
        while let Some(slot) = next {
            next = read(region, slot, |record| found.push(record)).unwrap();
        }
        found.sort_by_key(|record| (record.journal, record.ride_id));
        found
    }

    #[test]
    fn any_record_round_trips_and_every_tear_or_flipped_bit_is_refused() {
        for seed in 0..2_000 {
            let mut rng = Rng::new(seed);
            let record = any(&mut rng);
            let bytes = encode(&record);
            assert_eq!(decode(&bytes), Some(record), "seed {seed}");
            for torn in 0..SIZE {
                assert_eq!(decode(&bytes[..torn]), None, "seed {seed} torn at {torn}");
            }
            let mut flipped = bytes;
            let bit = rng.below(8 * SIZE as u64) as usize;
            flipped[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(decode(&flipped), None, "seed {seed} bit {bit}");
        }
    }

    #[test]
    fn hostile_bytes_never_panic_and_a_profile_out_of_range_reads_as_none() {
        let mut decoded = 0;
        for seed in 0..5_000 {
            let mut rng = Rng::new(seed);
            let drawn = encode(&any(&mut rng)).to_vec();
            let mut bytes = rng.mutate(drawn);
            if bytes.len() > 9 && !rng.one_in(4) {
                let body = usize::from(bytes[5]).min(bytes.len() - 4);
                bytes[5] = body as u8;
                let crc = crc32(&bytes[..body]);
                put_u32(&mut bytes, body, crc);
            }
            if let Some(record) = decode(&bytes) {
                decoded += 1;
                assert_eq!(decode(&encode(&record)), Some(record), "seed {seed}");
            }
        }
        assert!(decoded > 100, "{decoded} decoded");
        let mut bytes = encode(&record(1));
        put_u16(&mut bytes, 20, 0);
        let crc = crc32(&bytes[..BODY_V1]);
        put_u32(&mut bytes, BODY_V1, crc);
        let read = decode(&bytes).expect("the other fields survive");
        assert_eq!(read.facts.profile, None, "FTP 0 is no profile");
        assert_eq!(read.facts.climbing, record(1).facts.climbing);
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
    fn a_record_names_its_ride_by_journal_id_and_start_second() {
        let saved = record(7);
        let start = Some(saved.started_utc_ms);
        assert!(saved.of(b'b', 7, start));
        assert!(
            saved.of(b'b', 7, start.map(|ms| ms + 999)),
            "the index keeps seconds"
        );
        assert!(!saved.of(b'i', 7, start));
        assert!(!saved.of(b'b', 8, start));
        assert!(
            !saved.of(b'b', 7, start.map(|ms| ms + 1_000)),
            "a ride after a clear"
        );
        assert!(!saved.of(b'b', 7, None));
    }

    #[test]
    fn records_save_per_ride_the_oldest_go_and_a_clear_forgets_its_journal() {
        let mut region = formatted();
        put(&mut region, "rides/b1.workout", b"a workout record");
        put(&mut region, "rides/b2.facts", b"not a record");
        put(&mut region, "rides/b3.facts", &encode(&record(4)));
        assert_eq!(file(b'b', 12).as_str(), "rides/b12.facts");
        assert_eq!(
            all(&mut region),
            [],
            "damaged or misnamed files are skipped"
        );
        let mut deleted = std::vec::Vec::new();
        for ride in 4..=KEEP as u32 + 4 {
            deleted.extend(save(&mut region, &record(ride)).unwrap());
        }
        let names: std::vec::Vec<_> = deleted.iter().map(files::Name::as_str).collect();
        assert_eq!(
            names,
            ["rides/b2.facts", "rides/b3.facts", "rides/b4.facts"]
        );
        let kept = all(&mut region);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept[0], record(5));
        // Saving a ride again replaces its file and deletes nothing.
        let mut again = record(5);
        again.facts.recovery_bpm = Some(31);
        assert_eq!(save(&mut region, &again), Ok(None));
        assert_eq!(all(&mut region)[0], again);
        // Clearing the bulk journal forgets its files, not the others'.
        let internal = Record {
            journal: b'i',
            ..record(4)
        };
        save(&mut region, &internal).unwrap();
        forget(&mut region, b'b').unwrap();
        assert_eq!(all(&mut region), [internal]);
        let mut sector = [0; SECTOR_SIZE];
        let mut store = volume::open(&mut region, &volume::FILES, &mut sector).unwrap();
        let dir = files::mount(&mut store, &mut sector).unwrap();
        let workout = files::Name::new("rides/b1.workout").unwrap();
        assert!(
            dir.find(&mut store, &workout, &mut sector)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_power_cut_while_saving_leaves_the_old_record_or_the_new_one() {
        use firmware_services::testing::{Media, Tear};
        let mut cut = 0;
        for seed in 0..300 {
            let mut rng = Rng::new(seed);
            let mut media = Media::new(8_000_000);
            let mut sector = [0; SECTOR_SIZE];
            volume::create(&mut media, &mut sector).unwrap();
            let mut store = volume::open(&mut media, &volume::FILES, &mut sector).unwrap();
            files::format(&mut store, seed, &mut sector).unwrap();
            let old = record(9);
            save(&mut media, &old).unwrap();
            let mut new = old;
            new.facts.recovery_bpm = Some(25);
            media.cut = Some((rng.below(8), Tear::pick(&mut rng)));
            let saved = save(&mut media, &new).is_ok();
            media.reboot();
            let found = all(&mut media);
            if saved {
                assert_eq!(found, [new], "seed {seed}");
            } else {
                cut += 1;
                assert!(found == [old] || found == [new], "seed {seed}: {found:?}");
            }
        }
        assert!(cut > 100, "{cut} saves cut");
    }

    #[test]
    fn a_climb_adds_its_rounded_gain_and_saturates() {
        let mut climbing = Climbing::default();
        climbing.add(1_234);
        climbing.add(5);
        assert_eq!(
            climbing,
            Climbing {
                count: 2,
                gain_m: 124
            }
        );
        climbing.add(u32::MAX);
        assert_eq!(climbing.gain_m, u16::MAX);
    }
}
