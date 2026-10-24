//! Two-slot flash journal for the last panic record, so it outlives the
//! power-off that clears RTC memory.
//!
//! Each slot is one erase sector. Its first bytes hold, little-endian:
//!
//! | Bytes | Holds |
//! | --- | --- |
//! | 0..4 | `MAGIC` |
//! | 4..8 | sequence; the higher one (wrapping) is newer |
//! | 8..328 | the record's `crash::WORDS` words |
//! | 328..332 | CRC32 of bytes 0..328 |
//! | 332..336 | `COMMITTED`, programmed last |
//!
//! A write erases and fills the slot that does not hold the newest record, so
//! a power cut at any byte leaves the previous record readable. Bytes that are
//! not a committed slot count as free: this reservation holds nothing else.

use crate::checksum::Crc32;
use crate::le::{get_u32, put_u32};
use crate::storage::RegionAccess;
use device_api::crash::WORDS;
use device_api::storage::OwnedFlash;

pub const SLOT_SIZE: usize = 4096;
pub const SLOTS: usize = 2;

const MAGIC: u32 = u32::from_le_bytes(*b"CRJ1");
const COMMITTED: u32 = 0x434f_4d54;
const SEQUENCE: usize = 4;
const RECORD: usize = 8;
const CRC: usize = RECORD + WORDS * 4;
const COMMIT: usize = CRC + 4;
const LENGTH: usize = COMMIT + 4;

const _: () = assert!(LENGTH <= SLOT_SIZE && COMMIT % 4 == 0);

#[repr(C, align(4))]
struct Bytes([u8; LENGTH]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub slot: usize,
    pub sequence: u32,
    pub words: [u32; WORDS],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Flash(E),
    ReadbackFailed,
}

/// The newest committed record, if any.
pub fn newest<B: OwnedFlash>(flash: &mut RegionAccess<'_, B>) -> Result<Option<Entry>, B::Error> {
    let mut newest: Option<Entry> = None;
    for slot in 0..SLOTS {
        if let Some(entry) = read(flash, slot)?
            && newest.is_none_or(|current| newer(entry.sequence, current.sequence))
        {
            newest = Some(entry);
        }
    }
    Ok(newest)
}

/// Make `words` the newest record. Writes only when it is not already, so
/// each record costs one sector erase however often boot offers it.
/// Returns whether it wrote.
pub fn keep<B: OwnedFlash>(
    flash: &mut RegionAccess<'_, B>,
    words: &[u32; WORDS],
) -> Result<bool, Error<B::Error>> {
    let newest = newest(flash).map_err(Error::Flash)?;
    if newest.is_some_and(|entry| entry.words == *words) {
        return Ok(false);
    }
    let (slot, sequence) = newest.map_or((0, 1), |entry| {
        (SLOTS - 1 - entry.slot, entry.sequence.wrapping_add(1))
    });
    let mut bytes = Bytes([0xff; LENGTH]);
    put_u32(&mut bytes.0, 0, MAGIC);
    put_u32(&mut bytes.0, SEQUENCE, sequence);
    for (index, word) in words.iter().enumerate() {
        put_u32(&mut bytes.0, RECORD + index * 4, *word);
    }
    let crc = Crc32::new().update(&bytes.0[..CRC]).finish();
    put_u32(&mut bytes.0, CRC, crc);
    let base = slot * SLOT_SIZE;
    flash.erase(base, SLOT_SIZE).map_err(Error::Flash)?;
    flash
        .program(base, &bytes.0[..COMMIT])
        .map_err(Error::Flash)?;
    flash
        .program(base + COMMIT, &COMMITTED.to_le_bytes())
        .map_err(Error::Flash)?;
    let expected = Entry {
        slot,
        sequence,
        words: *words,
    };
    if read(flash, slot).map_err(Error::Flash)? != Some(expected) {
        return Err(Error::ReadbackFailed);
    }
    Ok(true)
}

/// Erase both slots, the older first, so a cut between the two erases
/// leaves the newest record rather than an older one.
pub fn clear<B: OwnedFlash>(flash: &mut RegionAccess<'_, B>) -> Result<(), B::Error> {
    let older = newest(flash)?.map_or(0, |entry| SLOTS - 1 - entry.slot);
    flash.erase(older * SLOT_SIZE, SLOT_SIZE)?;
    flash.erase((SLOTS - 1 - older) * SLOT_SIZE, SLOT_SIZE)
}

fn read<B: OwnedFlash>(
    flash: &mut RegionAccess<'_, B>,
    slot: usize,
) -> Result<Option<Entry>, B::Error> {
    let mut bytes = Bytes([0; LENGTH]);
    flash.read(slot * SLOT_SIZE, &mut bytes.0)?;
    let bytes = &bytes.0;
    if get_u32(bytes, 0) != MAGIC
        || get_u32(bytes, COMMIT) != COMMITTED
        || get_u32(bytes, CRC) != Crc32::new().update(&bytes[..CRC]).finish()
    {
        return Ok(None);
    }
    let mut words = [0; WORDS];
    for (index, word) in words.iter_mut().enumerate() {
        *word = get_u32(bytes, RECORD + index * 4);
    }
    Ok(Some(Entry {
        slot,
        sequence: get_u32(bytes, SEQUENCE),
        words,
    }))
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::storage::{Geometry, Region, checked_range};

    /// Crash reservation media whose power fails after `budget` byte changes.
    #[derive(Clone)]
    struct Media {
        bytes: [u8; SLOTS * SLOT_SIZE],
        budget: Option<usize>,
        writes: usize,
    }

    impl Media {
        fn new() -> Self {
            Self {
                bytes: [0xff; SLOTS * SLOT_SIZE],
                budget: None,
                writes: 0,
            }
        }
        fn change(&mut self, index: usize, value: u8) -> Result<(), ()> {
            if let Some(budget) = self.budget.as_mut() {
                if *budget == 0 {
                    return Err(());
                }
                *budget -= 1;
            }
            self.bytes[index] = value;
            Ok(())
        }
    }

    impl OwnedFlash for Media {
        type Error = ();
        fn geometry(&self, _: Region) -> Geometry {
            Geometry {
                capacity: SLOTS * SLOT_SIZE,
                program_size: 4,
                erase_size: SLOT_SIZE,
            }
        }
        fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
            assert_eq!(region, Region::Crash);
            checked_range::<()>(self.bytes.len(), offset, output.len(), 1).map_err(|_| ())?;
            output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
            Ok(())
        }
        fn program(&mut self, region: Region, offset: usize, data: &[u8]) -> Result<(), ()> {
            assert_eq!(region, Region::Crash);
            checked_range::<()>(self.bytes.len(), offset, data.len(), 4).map_err(|_| ())?;
            self.writes += 1;
            for (index, byte) in data.iter().enumerate() {
                let value = self.bytes[offset + index] & byte;
                self.change(offset + index, value)?;
            }
            Ok(())
        }
        fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
            assert_eq!(region, Region::Crash);
            checked_range::<()>(self.bytes.len(), offset, length, SLOT_SIZE).map_err(|_| ())?;
            self.writes += 1;
            for index in offset..offset + length {
                self.change(index, 0xff)?;
            }
            Ok(())
        }
    }

    fn record(seed: u32) -> [u32; WORDS] {
        core::array::from_fn(|index| seed.wrapping_mul(0x9e37_79b9) ^ index as u32)
    }

    fn newest_words(media: &mut Media) -> Option<[u32; WORDS]> {
        let mut flash = RegionAccess::new(media, Region::Crash);
        newest(&mut flash).unwrap().map(|entry| entry.words)
    }

    fn kept(media: &mut Media, words: &[u32; WORDS]) -> Result<bool, Error<()>> {
        keep(&mut RegionAccess::new(media, Region::Crash), words)
    }

    /// Media holding `count` committed records, record(1) oldest.
    fn holding(count: u32) -> Media {
        let mut media = Media::new();
        for seed in 1..=count {
            assert_eq!(kept(&mut media, &record(seed)), Ok(true));
        }
        media
    }

    #[test]
    fn records_alternate_slots_and_the_newest_wins() {
        let mut media = Media::new();
        assert_eq!(newest_words(&mut media), None);
        for seed in 1..=5 {
            assert_eq!(kept(&mut media, &record(seed)), Ok(true));
            let entry = newest(&mut RegionAccess::new(&mut media, Region::Crash))
                .unwrap()
                .unwrap();
            assert_eq!(entry.words, record(seed));
            assert_eq!(entry.sequence, seed);
            assert_eq!(entry.slot, (seed as usize - 1) % SLOTS);
        }
    }

    #[test]
    fn the_same_record_is_written_once() {
        let mut media = holding(1);
        let writes = media.writes;
        for _ in 0..10 {
            assert_eq!(kept(&mut media, &record(1)), Ok(false));
        }
        assert_eq!(media.writes, writes);
    }

    #[test]
    fn a_power_cut_at_any_byte_keeps_the_previous_record() {
        for count in 0..=2 {
            let before = holding(count);
            let previous = (count > 0).then(|| record(count));
            let mut whole = before.clone();
            whole.budget = Some(usize::MAX);
            kept(&mut whole, &record(9)).unwrap();
            let total = usize::MAX - whole.budget.unwrap();
            assert!(total > SLOT_SIZE);
            for budget in 0..total {
                let mut media = before.clone();
                media.budget = Some(budget);
                assert!(kept(&mut media, &record(9)).is_err(), "{count} {budget}");
                media.budget = None;
                let found = newest_words(&mut media);
                assert!(
                    found == previous || found == Some(record(9)),
                    "{count} records, cut after {budget} bytes"
                );
                assert_eq!(kept(&mut media, &record(9)), Ok(true));
                assert_eq!(newest_words(&mut media), Some(record(9)));
            }
        }
    }

    #[test]
    fn a_cut_clear_never_brings_back_an_older_record() {
        let before = holding(2);
        for budget in 0..2 * SLOT_SIZE {
            let mut media = before.clone();
            media.budget = Some(budget);
            assert!(clear(&mut RegionAccess::new(&mut media, Region::Crash)).is_err());
            media.budget = None;
            let found = newest_words(&mut media);
            assert!(found.is_none() || found == Some(record(2)), "{budget}");
        }
        let mut media = before;
        clear(&mut RegionAccess::new(&mut media, Region::Crash)).unwrap();
        assert_eq!(newest_words(&mut media), None);
        assert!(media.bytes.iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn a_damaged_byte_drops_only_its_slot() {
        for count in 1..=2 {
            let before = holding(count);
            let damaged_slot = (count as usize - 1) % SLOTS;
            for index in 0..LENGTH {
                let mut media = before.clone();
                media.bytes[damaged_slot * SLOT_SIZE + index] ^= 0x10;
                let expected = (count == 2).then(|| record(1));
                assert_eq!(newest_words(&mut media), expected, "byte {index}");
            }
        }
    }

    #[test]
    fn foreign_bytes_are_free_space() {
        let mut media = Media::new();
        for (index, byte) in media.bytes.iter_mut().enumerate() {
            *byte = (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[1];
        }
        assert_eq!(newest_words(&mut media), None);
        assert_eq!(kept(&mut media, &record(3)), Ok(true));
        assert_eq!(newest_words(&mut media), Some(record(3)));
    }

    #[test]
    fn sequences_compare_across_wrap() {
        assert!(newer(0, u32::MAX));
        assert!(!newer(u32::MAX, 0));
        assert!(newer(1, 0));
        assert!(!newer(5, 5));
    }

    #[test]
    fn a_write_that_does_not_read_back_fails() {
        struct Stuck(Media);
        impl OwnedFlash for Stuck {
            type Error = ();
            fn geometry(&self, region: Region) -> Geometry {
                self.0.geometry(region)
            }
            fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
                self.0.read(region, offset, output)
            }
            fn program(&mut self, _: Region, _: usize, _: &[u8]) -> Result<(), ()> {
                Ok(())
            }
            fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
                self.0.erase(region, offset, length)
            }
        }
        let mut stuck = Stuck(Media::new());
        assert_eq!(
            keep(
                &mut RegionAccess::new(&mut stuck, Region::Crash),
                &record(1)
            ),
            Err(Error::ReadbackFailed)
        );
    }
}
