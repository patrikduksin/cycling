//! A two-sector, power-loss-tolerant journal for small settings records.

pub const SECTOR_SIZE: usize = 4096;
pub const SECTOR_COUNT: usize = 2;
pub const HEADER_SIZE: usize = 24;
pub const CAPACITY: usize = SECTOR_SIZE - HEADER_SIZE;

const MAGIC: [u8; 4] = *b"C606";
const FORMAT_VERSION: u16 = 1;
const COMMITTED: u32 = 0x434f_4d54;
const COMMIT_OFFSET: usize = 20;

#[repr(C, align(4))]
pub struct Sector(pub [u8; SECTOR_SIZE]);

impl Default for Sector {
    fn default() -> Self {
        Self([0; SECTOR_SIZE])
    }
}

pub trait Flash {
    type Error;

    fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error>;
    fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error>;
    fn write_sector(&mut self, sector: usize, data: &Sector) -> Result<(), Self::Error>;
    fn commit_sector(&mut self, sector: usize, commit: &[u8; 4]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub sequence: u32,
    pub length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Flash(E),
    PayloadTooLarge,
    OutputTooSmall,
    ReadbackFailed,
    /// No committed supported record exists, but at least one sector is occupied.
    /// Preserve the bytes for inspection instead of treating them as unused flash.
    UnrecognizedJournal,
    /// Outer journal is valid but its settings payload cannot be safely replaced.
    UnrecognizedSettings,
}

pub struct Journal<F> {
    flash: F,
    scratch: Sector,
}

impl<F: Flash> Journal<F> {
    pub fn new(flash: F) -> Self {
        Self {
            flash,
            scratch: Sector::default(),
        }
    }

    /// Lend the sole backend for a disjoint, bounds-checked storage operation.
    pub fn flash_mut(&mut self) -> &mut F {
        &mut self.flash
    }

    pub fn load(&mut self, output: &mut [u8]) -> Result<Option<Record>, Error<F::Error>> {
        let newest = self.scan()?;
        let Some((sector, record)) = newest else {
            return Ok(None);
        };
        if output.len() < record.length {
            return Err(Error::OutputTooSmall);
        }
        self.flash
            .read_sector(sector, &mut self.scratch)
            .map_err(Error::Flash)?;
        let verified = decode(&self.scratch).ok_or(Error::ReadbackFailed)?;
        if verified != record {
            return Err(Error::ReadbackFailed);
        }
        output[..verified.length]
            .copy_from_slice(&self.scratch.0[HEADER_SIZE..HEADER_SIZE + verified.length]);
        Ok(Some(verified))
    }

    pub fn save(&mut self, payload: &[u8]) -> Result<Record, Error<F::Error>> {
        if payload.len() > CAPACITY {
            return Err(Error::PayloadTooLarge);
        }
        let newest = self.scan()?;
        let (target, sequence) = match newest {
            Some((sector, record)) => (1 - sector, record.sequence.wrapping_add(1)),
            None => (0, 1),
        };

        self.scratch.0.fill(0xff);
        self.scratch.0[0..4].copy_from_slice(&MAGIC);
        put_u16(&mut self.scratch.0, 4, FORMAT_VERSION);
        put_u16(&mut self.scratch.0, 6, HEADER_SIZE as u16);
        put_u32(&mut self.scratch.0, 8, sequence);
        put_u32(&mut self.scratch.0, 12, payload.len() as u32);
        self.scratch.0[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
        let checksum = checksum(sequence, payload);
        put_u32(&mut self.scratch.0, 16, checksum);

        self.flash.erase_sector(target).map_err(Error::Flash)?;
        self.flash
            .write_sector(target, &self.scratch)
            .map_err(Error::Flash)?;
        self.flash
            .commit_sector(target, &COMMITTED.to_le_bytes())
            .map_err(Error::Flash)?;

        let mut verify = Sector::default();
        self.flash
            .read_sector(target, &mut verify)
            .map_err(Error::Flash)?;
        let record = decode(&verify).ok_or(Error::ReadbackFailed)?;
        if record.sequence != sequence
            || record.length != payload.len()
            || verify.0[HEADER_SIZE..HEADER_SIZE + payload.len()] != *payload
        {
            return Err(Error::ReadbackFailed);
        }
        Ok(record)
    }

    fn scan(&mut self) -> Result<Option<(usize, Record)>, Error<F::Error>> {
        let mut newest = None;
        let mut occupied = false;
        for sector in 0..SECTOR_COUNT {
            self.flash
                .read_sector(sector, &mut self.scratch)
                .map_err(Error::Flash)?;
            occupied |= self.scratch.0.iter().any(|byte| *byte != 0xff);
            if let Some(record) = decode(&self.scratch)
                && newest
                    .map(|(_, current): (usize, Record)| newer(record.sequence, current.sequence))
                    .unwrap_or(true)
            {
                newest = Some((sector, record));
            }
        }
        if newest.is_none() && occupied {
            return Err(Error::UnrecognizedJournal);
        }
        Ok(newest)
    }
}

fn decode(sector: &Sector) -> Option<Record> {
    let bytes = &sector.0;
    let length = get_u32(bytes, 12) as usize;
    if bytes[0..4] != MAGIC
        || get_u16(bytes, 4) != FORMAT_VERSION
        || get_u16(bytes, 6) as usize != HEADER_SIZE
        || length > CAPACITY
        || get_u32(bytes, COMMIT_OFFSET) != COMMITTED
    {
        return None;
    }
    let sequence = get_u32(bytes, 8);
    let payload = &bytes[HEADER_SIZE..HEADER_SIZE + length];
    (get_u32(bytes, 16) == checksum(sequence, payload)).then_some(Record { sequence, length })
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

fn checksum(sequence: u32, payload: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for byte in sequence
        .to_le_bytes()
        .into_iter()
        .chain((payload.len() as u32).to_le_bytes())
        .chain(payload.iter().copied())
    {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Memory {
        bytes: [[u8; SECTOR_SIZE]; SECTOR_COUNT],
        fail_after: Option<usize>,
        tear_bytes: usize,
        operations: usize,
    }

    impl Default for Memory {
        fn default() -> Self {
            Self {
                bytes: [[0xff; SECTOR_SIZE]; SECTOR_COUNT],
                fail_after: None,
                tear_bytes: 0,
                operations: 0,
            }
        }
    }

    impl Memory {
        fn fails(&mut self) -> bool {
            if self.fail_after == Some(self.operations) {
                return true;
            }
            self.operations += 1;
            false
        }
    }

    impl Flash for Memory {
        type Error = ();

        fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
            if self.fails() {
                return Err(());
            }
            output.0.copy_from_slice(&self.bytes[sector]);
            Ok(())
        }

        fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(SECTOR_SIZE)
            } else {
                SECTOR_SIZE
            };
            self.bytes[sector][..length].fill(0xff);
            if fails { Err(()) } else { Ok(()) }
        }

        fn write_sector(&mut self, sector: usize, data: &Sector) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(SECTOR_SIZE)
            } else {
                SECTOR_SIZE
            };
            for (stored, new) in self.bytes[sector][..length].iter_mut().zip(data.0) {
                *stored &= new;
            }
            if fails { Err(()) } else { Ok(()) }
        }

        fn commit_sector(&mut self, sector: usize, commit: &[u8; 4]) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails { self.tear_bytes.min(4) } else { 4 };
            for (stored, new) in self.bytes[sector][COMMIT_OFFSET..COMMIT_OFFSET + 4]
                .iter_mut()
                .zip(commit)
                .take(length)
            {
                *stored &= *new;
            }
            if fails { Err(()) } else { Ok(()) }
        }
    }

    impl OwnedFlash for Memory {
        fn geometry(&self) -> Geometry {
            Geometry {
                capacity: 0,
                program_size: 4,
                erase_size: SECTOR_SIZE,
            }
        }
        fn read_data(&mut self, _: usize, _: &mut [u8]) -> Result<(), ()> {
            panic!("settings test accessed data reservation")
        }
        fn program_data(&mut self, _: usize, _: &[u8]) -> Result<(), ()> {
            panic!("settings test programmed data reservation")
        }
        fn erase_data(&mut self, _: usize, _: usize) -> Result<(), ()> {
            panic!("settings test erased data reservation")
        }
    }

    #[test]
    fn unsupported_and_malformed_settings_are_readable_but_cannot_be_overwritten() {
        use crate::preferences::{Settings, Source};
        for (payload, source) in [
            (b"cycling\x05".as_slice(), Source::Unsupported),
            (b"unrecognized".as_slice(), Source::Malformed),
            (b"cycling\x04".as_slice(), Source::Malformed),
        ] {
            let mut journal = Journal::new(Memory::default());
            journal.save(&Settings::default().encode()).unwrap();
            journal.save(payload).unwrap();
            let mut store = Store { journal };
            assert_eq!(store.load().unwrap().source, source);
            let before = store.journal.flash.bytes;
            let operations = store.journal.flash.operations;
            assert_eq!(
                store.save(Settings::default()),
                Err(Error::UnrecognizedSettings)
            );
            assert_eq!(store.journal.flash.bytes, before);
            // Two scan reads plus a verified load; no mutation operation.
            assert_eq!(store.journal.flash.operations - operations, 3);
        }
    }

    #[test]
    fn supported_settings_versions_and_missing_journal_remain_writable() {
        use crate::preferences::{Settings, Source};
        for payload in [
            None,
            Some(b"cycling\x01".as_slice()),
            Some(b"cycling\x02\x32\x00".as_slice()),
            Some(Settings::default().encode().as_slice()),
        ] {
            let mut journal = Journal::new(Memory::default());
            if let Some(payload) = payload {
                journal.save(payload).unwrap();
            }
            let mut store = Store { journal };
            let settings = Settings::new(75).unwrap();
            store.save(settings).unwrap();
            let loaded = store.load().unwrap();
            assert_eq!(loaded.source, Source::Current);
            assert_eq!(loaded.settings, settings);
        }
    }

    #[test]
    fn saves_reads_and_uses_full_capacity() {
        let mut journal = Journal::new(Memory::default());
        let payload = [0xa5; CAPACITY];
        assert_eq!(journal.save(&payload).unwrap().sequence, 1);
        let mut output = [0; CAPACITY];
        assert_eq!(journal.load(&mut output).unwrap().unwrap().length, CAPACITY);
        assert_eq!(output, payload);
        assert_eq!(journal.save(&[1, 2, 3]).unwrap().sequence, 2);
    }

    #[test]
    fn rejects_oversize_and_short_output() {
        let mut journal = Journal::new(Memory::default());
        assert_eq!(
            journal.save(&[0; CAPACITY + 1]),
            Err(Error::PayloadTooLarge)
        );
        journal.save(&[1, 2, 3]).unwrap();
        assert_eq!(journal.load(&mut [0; 2]), Err(Error::OutputTooSmall));
    }

    #[test]
    fn checksum_corruption_falls_back_to_previous_record() {
        let mut journal = Journal::new(Memory::default());
        journal.save(b"old").unwrap();
        journal.save(b"new").unwrap();
        journal.flash.bytes[1][HEADER_SIZE] ^= 1;
        let mut output = [0; 8];
        let record = journal.load(&mut output).unwrap().unwrap();
        assert_eq!(record.sequence, 1);
        assert_eq!(&output[..record.length], b"old");
    }

    #[test]
    fn erased_journal_is_missing_and_can_be_saved_explicitly() {
        let mut journal = Journal::new(Memory::default());
        assert_eq!(journal.load(&mut [0; 8]), Ok(None));
        assert!(
            journal
                .flash
                .bytes
                .iter()
                .flatten()
                .all(|byte| *byte == 0xff)
        );
        assert_eq!(journal.save(b"first").unwrap().sequence, 1);
    }

    #[test]
    fn occupied_unrecognized_journals_refuse_load_and_save_without_mutation() {
        for scenario in 0..4 {
            let mut journal = Journal::new(Memory::default());
            match scenario {
                0 => journal.flash.bytes[0][0] = 0, // One unknown occupied sector.
                1 => journal
                    .flash
                    .bytes
                    .iter_mut()
                    .for_each(|sector| sector.fill(0)),
                _ => {
                    journal.save(b"existing").unwrap();
                    if scenario == 2 {
                        journal.flash.bytes[0][4..6].copy_from_slice(&2u16.to_le_bytes());
                    } else {
                        journal.flash.bytes[0][HEADER_SIZE] ^= 1;
                    }
                }
            }
            let before = journal.flash.bytes;
            let operations = journal.flash.operations;
            assert_eq!(journal.load(&mut [0; 16]), Err(Error::UnrecognizedJournal));
            assert_eq!(
                journal.save(b"replacement"),
                Err(Error::UnrecognizedJournal)
            );
            assert_eq!(journal.flash.bytes, before);
            assert_eq!(journal.flash.operations - operations, SECTOR_COUNT * 2);
        }
    }

    #[test]
    fn torn_first_write_is_preserved_and_cannot_be_retried_as_empty() {
        let mut journal = Journal::new(Memory::default());
        journal.flash.fail_after = Some(3); // First payload program, after scan + erase.
        journal.flash.tear_bytes = HEADER_SIZE + 1;
        assert_eq!(journal.save(b"first"), Err(Error::Flash(())));
        journal.flash.fail_after = None;
        let before = journal.flash.bytes;
        assert_eq!(journal.load(&mut [0; 8]), Err(Error::UnrecognizedJournal));
        assert_eq!(journal.save(b"retry"), Err(Error::UnrecognizedJournal));
        assert_eq!(journal.flash.bytes, before);
    }

    #[test]
    fn interruption_before_commit_keeps_previous_record() {
        for failed_operation in 2..5 {
            for tear_bytes in [0, 1, 2, 3, 16, 2048, 4095] {
                let mut journal = Journal::new(Memory::default());
                journal.save(b"older").unwrap();
                journal.save(b"old").unwrap();
                journal.flash.operations = 0;
                journal.flash.fail_after = Some(failed_operation);
                journal.flash.tear_bytes = tear_bytes;
                assert!(journal.save(b"new").is_err());
                journal.flash.fail_after = None;
                let mut output = [0; 8];
                let record = journal.load(&mut output).unwrap().unwrap();
                let recovered = &output[..record.length];
                assert!(
                    recovered == b"old" || recovered == b"new",
                    "failure at operation {failed_operation}, byte {tear_bytes}"
                );
            }
        }
    }
}

/// Physical geometry of an exclusively owned byte reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub capacity: usize,
    pub program_size: usize,
    pub erase_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessError<E> {
    OutOfBounds,
    Unaligned,
    Device(E),
}

/// Validate relative ranges before translating them to physical addresses.
pub fn checked_range<E>(
    capacity: usize,
    offset: usize,
    length: usize,
    alignment: usize,
) -> Result<(), AccessError<E>> {
    if offset.checked_add(length).is_none_or(|end| end > capacity) {
        return Err(AccessError::OutOfBounds);
    }
    if alignment == 0 || !offset.is_multiple_of(alignment) || !length.is_multiple_of(alignment) {
        return Err(AccessError::Unaligned);
    }
    Ok(())
}

/// Sole physical owner of the journal and a disjoint application reservation.
/// Implementations must enforce their fixed reservations on every operation.
pub trait OwnedFlash: Flash {
    fn geometry(&self) -> Geometry;
    fn read_data(&mut self, offset: usize, output: &mut [u8]) -> Result<(), Self::Error>;
    fn program_data(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;
    fn erase_data(&mut self, offset: usize, length: usize) -> Result<(), Self::Error>;
}

pub struct Loaded {
    pub settings: crate::preferences::Settings,
    pub source: crate::preferences::Source,
    pub sequence: Option<u32>,
    pub length: usize,
}

/// Settings journal and generic owned bytes. No application format or reclaim policy.
/// Calls are synchronous: the caller yields between bounded media operations.
/// ESP flash programming/erase can suspend interrupts and acquisition. Callers
/// must retain transport loss detection and reset parsing after detected gaps.
/// A failed mutation is ambiguous; reconcile by reading, never blindly retry it.
pub struct Store<B> {
    journal: Journal<B>,
}

impl<B: OwnedFlash> Store<B> {
    pub fn new(backend: B) -> Self {
        Self {
            journal: Journal::new(backend),
        }
    }

    pub fn load(&mut self) -> Result<Loaded, Error<B::Error>> {
        let mut payload = [0u8; CAPACITY];
        let record = self.journal.load(&mut payload)?;
        let bytes = record.map(|record| &payload[..record.length]);
        let (settings, source) = crate::preferences::decode(bytes);
        Ok(Loaded {
            settings,
            source,
            sequence: record.map(|r| r.sequence),
            length: record.map(|r| r.length).unwrap_or(0),
        })
    }

    pub fn save(
        &mut self,
        settings: crate::preferences::Settings,
    ) -> Result<Record, Error<B::Error>> {
        if matches!(
            self.load()?.source,
            crate::preferences::Source::Unsupported | crate::preferences::Source::Malformed
        ) {
            return Err(Error::UnrecognizedSettings);
        }
        match self.journal.save(&settings.encode()) {
            Ok(record) => Ok(record),
            Err(error) => match self.load() {
                Ok(loaded)
                    if loaded.source == crate::preferences::Source::Current
                        && loaded.settings == settings =>
                {
                    Ok(Record {
                        sequence: loaded.sequence.unwrap(),
                        length: loaded.length,
                    })
                }
                _ => Err(error),
            },
        }
    }

    pub fn geometry(&mut self) -> Geometry {
        self.journal.flash_mut().geometry()
    }
    pub fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), B::Error> {
        self.journal.flash_mut().read_data(offset, output)
    }
    pub fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), B::Error> {
        self.journal.flash_mut().program_data(offset, bytes)
    }
    pub fn erase(&mut self, offset: usize, length: usize) -> Result<(), B::Error> {
        self.journal.flash_mut().erase_data(offset, length)
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    #[test]
    fn reservations_reject_overflow_boundary_crossing_and_alignment() {
        assert_eq!(checked_range::<()>(8192, 8192, 0, 1), Ok(()));
        assert_eq!(checked_range::<()>(8192, 8191, 1, 1), Ok(()));
        assert_eq!(
            checked_range::<()>(8192, 8192, 1, 1),
            Err(AccessError::OutOfBounds)
        );
        assert_eq!(
            checked_range::<()>(8192, usize::MAX, 2, 1),
            Err(AccessError::OutOfBounds)
        );
        assert_eq!(
            checked_range::<()>(8192, 1, 4, 4),
            Err(AccessError::Unaligned)
        );
        assert_eq!(
            checked_range::<()>(8192, 0, 4095, 4096),
            Err(AccessError::Unaligned)
        );
        assert_eq!(checked_range::<()>(8192, 4096, 4096, 4096), Ok(()));
    }
}
