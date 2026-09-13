//! A two-sector, power-loss-tolerant journal for small records.

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
    UnsupportedGeometry,
}

pub struct Journal<F> {
    pub(crate) flash: F,
    scratch: Sector,
    region: Region,
}

impl<F: OwnedFlash> Journal<F> {
    pub fn new(flash: F, region: Region) -> Self {
        Self {
            flash,
            scratch: Sector::default(),
            region,
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
            .read(self.region, sector * SECTOR_SIZE, &mut self.scratch.0)
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

        self.flash
            .erase(self.region, target * SECTOR_SIZE, SECTOR_SIZE)
            .map_err(Error::Flash)?;
        self.flash
            .program(self.region, target * SECTOR_SIZE, &self.scratch.0)
            .map_err(Error::Flash)?;
        self.flash
            .program(
                self.region,
                target * SECTOR_SIZE + COMMIT_OFFSET,
                &COMMITTED.to_le_bytes(),
            )
            .map_err(Error::Flash)?;

        let mut verify = Sector::default();
        self.flash
            .read(self.region, target * SECTOR_SIZE, &mut verify.0)
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
        let geometry = self.flash.geometry(self.region);
        if geometry.capacity < SECTOR_COUNT * SECTOR_SIZE
            || geometry.program_size == 0
            || !4usize.is_multiple_of(geometry.program_size)
            || geometry.erase_size == 0
            || !SECTOR_SIZE.is_multiple_of(geometry.erase_size)
        {
            return Err(Error::UnsupportedGeometry);
        }
        let mut newest = None;
        let mut occupied = false;
        for sector in 0..SECTOR_COUNT {
            self.flash
                .read(self.region, sector * SECTOR_SIZE, &mut self.scratch.0)
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
pub(crate) mod tests {
    use super::*;

    #[derive(Clone)]
    pub(crate) struct Memory {
        pub(crate) bytes: [[u8; SECTOR_SIZE]; SECTOR_COUNT],
        fail_after: Option<usize>,
        tear_bytes: usize,
        pub(crate) operations: usize,
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

    impl OwnedFlash for Memory {
        type Error = ();
        fn geometry(&self, region: Region) -> Geometry {
            Geometry {
                capacity: if region == Region::Configuration {
                    SECTOR_SIZE * SECTOR_COUNT
                } else {
                    0
                },
                program_size: 4,
                erase_size: SECTOR_SIZE,
            }
        }
        fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
            assert_eq!(region, Region::Configuration);
            checked_range::<()>(SECTOR_SIZE * SECTOR_COUNT, offset, output.len(), 1)
                .map_err(|_| ())?;
            if self.fails() {
                return Err(());
            }
            for (i, byte) in output.iter_mut().enumerate() {
                *byte = self.bytes[(offset + i) / SECTOR_SIZE][(offset + i) % SECTOR_SIZE];
            }
            Ok(())
        }
        fn program(&mut self, region: Region, offset: usize, data: &[u8]) -> Result<(), ()> {
            assert_eq!(region, Region::Configuration);
            checked_range::<()>(SECTOR_SIZE * SECTOR_COUNT, offset, data.len(), 4)
                .map_err(|_| ())?;
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(data.len())
            } else {
                data.len()
            };
            for (i, byte) in data[..length].iter().enumerate() {
                self.bytes[(offset + i) / SECTOR_SIZE][(offset + i) % SECTOR_SIZE] &= *byte;
            }
            if fails { Err(()) } else { Ok(()) }
        }
        fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
            assert_eq!(region, Region::Configuration);
            checked_range::<()>(SECTOR_SIZE * SECTOR_COUNT, offset, length, SECTOR_SIZE)
                .map_err(|_| ())?;
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(length)
            } else {
                length
            };
            for i in offset..offset + length {
                self.bytes[i / SECTOR_SIZE][i % SECTOR_SIZE] = 0xff;
            }
            if fails { Err(()) } else { Ok(()) }
        }
    }

    #[test]
    fn journal_owns_commit_layout_and_preserves_operation_order() {
        use std::vec::Vec;
        struct Trace {
            memory: Memory,
            calls: Vec<(&'static str, Region, usize, usize)>,
        }
        impl OwnedFlash for Trace {
            type Error = ();
            fn geometry(&self, region: Region) -> Geometry {
                self.memory.geometry(region)
            }
            fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
                self.calls.push(("read", region, offset, output.len()));
                self.memory.read(region, offset, output)
            }
            fn program(&mut self, region: Region, offset: usize, data: &[u8]) -> Result<(), ()> {
                self.calls.push(("program", region, offset, data.len()));
                self.memory.program(region, offset, data)
            }
            fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
                self.calls.push(("erase", region, offset, length));
                self.memory.erase(region, offset, length)
            }
        }
        let mut journal = Journal::new(
            Trace {
                memory: Memory::default(),
                calls: Vec::new(),
            },
            Region::Configuration,
        );
        journal.save(b"existing format").unwrap();
        let region = Region::Configuration;
        assert_eq!(
            journal.flash.calls,
            [
                ("read", region, 0, 4096),
                ("read", region, 4096, 4096),
                ("erase", region, 0, 4096),
                ("program", region, 0, 4096),
                ("program", region, 20, 4),
                ("read", region, 0, 4096),
            ]
        );
        assert_eq!(&journal.flash.memory.bytes[0][..8], b"C606\x01\x00\x18\x00");
        assert_eq!(
            &journal.flash.memory.bytes[0][20..24],
            &COMMITTED.to_le_bytes()
        );
        journal.flash.calls.clear();
        journal.save(b"second").unwrap();
        assert_eq!(
            journal.flash.calls[2..],
            [
                ("erase", region, 4096, 4096),
                ("program", region, 4096, 4096),
                ("program", region, 4116, 4),
                ("read", region, 4096, 4096),
            ]
        );
    }

    #[test]
    fn insufficient_reservation_refuses_journal_access_before_io() {
        let mut journal = Journal::new(Memory::default(), Region::Data);
        assert_eq!(journal.load(&mut [0; 8]), Err(Error::UnsupportedGeometry));
        assert_eq!(journal.save(b"no"), Err(Error::UnsupportedGeometry));
        assert_eq!(journal.flash.operations, 0);
    }

    #[test]
    fn saves_reads_and_uses_full_capacity() {
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
        let payload = [0xa5; CAPACITY];
        assert_eq!(journal.save(&payload).unwrap().sequence, 1);
        let mut output = [0; CAPACITY];
        assert_eq!(journal.load(&mut output).unwrap().unwrap().length, CAPACITY);
        assert_eq!(output, payload);
        assert_eq!(journal.save(&[1, 2, 3]).unwrap().sequence, 2);
    }

    #[test]
    fn rejects_oversize_and_short_output() {
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
        assert_eq!(
            journal.save(&[0; CAPACITY + 1]),
            Err(Error::PayloadTooLarge)
        );
        journal.save(&[1, 2, 3]).unwrap();
        assert_eq!(journal.load(&mut [0; 2]), Err(Error::OutputTooSmall));
    }

    #[test]
    fn checksum_corruption_falls_back_to_previous_record() {
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
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
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
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
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
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
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
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
                let mut journal = Journal::new(Memory::default(), Region::Configuration);
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

/// Device-owned reservations. These names select disjoint bounds, not schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    Configuration,
    Data,
}

/// Sole physical owner of independently bounded reservations. Every operation
/// uses relative byte offsets; no caller can select a physical flash address.
/// Return means completed hardware access, including on failure. A failed
/// mutation may have changed bytes: inspect/rescan before deciding to retry.
pub trait OwnedFlash {
    type Error;
    fn availability(&self, _region: Region) -> crate::capabilities::Availability {
        crate::capabilities::Availability::Ready
    }
    fn geometry(&self, region: Region) -> Geometry;
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8])
    -> Result<(), Self::Error>;
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Self::Error>;
}

/// An exclusive borrow of one reservation. Journals and domain adapters can
/// use different reservations while the backend serializes physical access.
pub struct RegionAccess<'a, B> {
    backend: &'a mut B,
    region: Region,
}
impl<'a, B: OwnedFlash> RegionAccess<'a, B> {
    pub fn new(backend: &'a mut B, region: Region) -> Self {
        Self { backend, region }
    }
    pub fn geometry(&self) -> Geometry {
        self.backend.geometry(self.region)
    }
    pub fn read(&mut self, offset: usize, output: &mut [u8]) -> Result<(), B::Error> {
        self.backend.read(self.region, offset, output)
    }
    pub fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), B::Error> {
        self.backend.program(self.region, offset, bytes)
    }
    pub fn erase(&mut self, offset: usize, length: usize) -> Result<(), B::Error> {
        self.backend.erase(self.region, offset, length)
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
