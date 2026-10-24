//! A two-sector, power-loss-tolerant journal for small records.
use crate::checksum::Crc32;
use crate::le::{get_u16, get_u32, put_u16, put_u32};
use device_api::storage::*;

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
    /// No committed supported record exists, and the occupied sectors are not
    /// one torn first save beside a blank sector.
    /// Preserve the bytes for inspection instead of treating them as unused flash.
    UnrecognizedJournal,
    UnsupportedGeometry,
}

/// Where the next save goes: after the newest committed record, or into a
/// free sector with no committed record to follow.
enum Scan {
    Newest(usize, Record),
    Free { sector: usize, sequence: u32 },
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
        let Scan::Newest(sector, record) = self.scan()? else {
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
        let (target, sequence) = match self.scan()? {
            Scan::Newest(sector, record) => (1 - sector, record.sequence.wrapping_add(1)),
            Scan::Free { sector, sequence } => (sector, sequence),
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

    fn scan(&mut self) -> Result<Scan, Error<F::Error>> {
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
        let mut blank = [false; SECTOR_COUNT];
        let mut torn = [false; SECTOR_COUNT];
        for sector in 0..SECTOR_COUNT {
            self.flash
                .read(self.region, sector * SECTOR_SIZE, &mut self.scratch.0)
                .map_err(Error::Flash)?;
            blank[sector] = self.scratch.0.iter().all(|byte| *byte == 0xff);
            torn[sector] = torn_first_save(&self.scratch);
            if let Some(record) = decode(&self.scratch)
                && newest
                    .map(|(_, current): (usize, Record)| newer(record.sequence, current.sequence))
                    .unwrap_or(true)
            {
                newest = Some((sector, record));
            }
        }
        if let Some((sector, record)) = newest {
            return Ok(Scan::Newest(sector, record));
        }
        // A first save cut before its commit word stays in place for
        // inspection, and the next save goes into the other, blank sector.
        // A commit word cut mid-program can read whole on a later boot, so
        // that save outranks the torn sequence 1.
        let free = |sector| {
            Ok(Scan::Free {
                sector,
                sequence: 2,
            })
        };
        match (blank, torn) {
            ([true, true], _) => Ok(Scan::Free {
                sector: 0,
                sequence: 1,
            }),
            ([false, true], [true, false]) => free(1),
            ([true, false], [false, true]) => free(0),
            _ => Err(Error::UnrecognizedJournal),
        }
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

/// Whether a sector holds a partial first save. Programming runs in order,
/// so the header matches a sequence 1 header up to the byte where it stopped,
/// that byte holds only bits of its target, and nothing after it is
/// programmed. Once the header is whole, only the commit word may be partial.
fn torn_first_save(sector: &Sector) -> bool {
    let mut header = [0; 12];
    header[0..4].copy_from_slice(&MAGIC);
    put_u16(&mut header, 4, FORMAT_VERSION);
    put_u16(&mut header, 6, HEADER_SIZE as u16);
    put_u32(&mut header, 8, 1);
    let bytes = &sector.0;
    let commit = get_u32(bytes, COMMIT_OFFSET);
    match bytes
        .iter()
        .zip(header)
        .position(|(byte, target)| *byte != target)
    {
        None => commit != COMMITTED && commit & COMMITTED == COMMITTED,
        Some(stop) => {
            bytes[0] != 0xff
                && bytes[stop] & header[stop] == header[stop]
                && bytes[stop + 1..].iter().all(|byte| *byte == 0xff)
        }
    }
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

fn checksum(sequence: u32, payload: &[u8]) -> u32 {
    Crc32::new()
        .update(&sequence.to_le_bytes())
        .update(&(payload.len() as u32).to_le_bytes())
        .update(payload)
        .finish()
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
pub(crate) mod tests {
    use super::*;

    const PAGE_SIZE: usize = 256;

    #[derive(Clone)]
    pub(crate) struct Memory {
        pub(crate) bytes: [[u8; SECTOR_SIZE]; SECTOR_COUNT],
        fail_after: Option<usize>,
        tear_bytes: usize,
        /// A cut program leaves its page partly programmed: each byte keeps
        /// some of the bits it was clearing, drawn from this seed.
        page_parallel: Option<u64>,
        pub(crate) operations: usize,
    }

    impl Default for Memory {
        fn default() -> Self {
            Self {
                bytes: [[0xff; SECTOR_SIZE]; SECTOR_COUNT],
                fail_after: None,
                tear_bytes: 0,
                page_parallel: None,
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
            let landed = if fails {
                self.tear_bytes.min(data.len())
            } else {
                data.len()
            };
            let cut_page = (offset + landed) / PAGE_SIZE;
            let mut rng = self.page_parallel.map(crate::testing::Rng::new);
            let late = rng.as_mut().is_some_and(|rng| rng.one_in(2));
            for (i, byte) in data.iter().enumerate() {
                let address = offset + i;
                let target = match &mut rng {
                    _ if !fails => *byte,
                    None if i < landed => *byte,
                    None => break,
                    Some(rng) => match (address / PAGE_SIZE).cmp(&cut_page) {
                        core::cmp::Ordering::Less => *byte,
                        core::cmp::Ordering::Equal if late => *byte | (rng.byte() & rng.byte()),
                        core::cmp::Ordering::Equal => *byte | rng.byte() | rng.byte(),
                        core::cmp::Ordering::Greater => break,
                    },
                };
                self.bytes[address / SECTOR_SIZE][address % SECTOR_SIZE] &= target;
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
        for scenario in 0..10 {
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            match scenario {
                0 => journal.flash.bytes[0][0] = 0, // One unknown occupied sector.
                1 => journal
                    .flash
                    .bytes
                    .iter_mut()
                    .for_each(|sector| sector.fill(0)),
                // Two torn first saves: neither sector is free to write.
                4 => {
                    journal.save(b"existing").unwrap();
                    journal.flash.bytes[0][COMMIT_OFFSET..HEADER_SIZE].fill(0xff);
                    journal.flash.bytes[1] = journal.flash.bytes[0];
                }
                // An uncommitted later save beside a blank sector is not a first save.
                5 => {
                    journal.save(b"older").unwrap();
                    journal.save(b"existing").unwrap();
                    journal.flash.bytes[1][COMMIT_OFFSET..HEADER_SIZE].fill(0xff);
                    journal.flash.bytes[0].fill(0xff);
                }
                // A future format whose header is 28 bytes, not yet committed.
                6 => {
                    let header = &mut journal.flash.bytes[0];
                    header[0..4].copy_from_slice(&MAGIC);
                    put_u16(header, 4, 3);
                    put_u16(header, 6, 28);
                    put_u32(header, 8, 1);
                    put_u32(header, 12, 5);
                    put_u32(header, 16, 0);
                    header[28..33].copy_from_slice(b"three");
                }
                // A torn sequence 3 whose older records were erased.
                7 => {
                    for payload in [b"one", b"two", b"new"] {
                        journal.save(payload).unwrap();
                    }
                    journal.flash.bytes[0][COMMIT_OFFSET..HEADER_SIZE].fill(0xff);
                    journal.flash.bytes[1].fill(0xff);
                }
                // A partial header with bytes programmed past the first page.
                8 => {
                    journal.save(&[0; 300]).unwrap();
                    journal.flash.bytes[0][COMMIT_OFFSET..HEADER_SIZE].fill(0xff);
                    journal.flash.bytes[0][4] = 0xfd;
                }
                // Programmed bytes behind an untouched header.
                9 => journal.flash.bytes[0][100] = 0,
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
    fn a_torn_first_save_is_preserved_and_never_blocks_the_next_save() {
        for page_parallel in [None].into_iter().chain((0..200).map(Some)) {
            a_torn_first_save_is_recovered(page_parallel);
        }
    }

    fn a_torn_first_save_is_recovered(page_parallel: Option<u64>) {
        for failed_operation in 2..6 {
            for tear_bytes in [
                0,
                1,
                2,
                3,
                4,
                HEADER_SIZE,
                HEADER_SIZE + 1,
                PAGE_SIZE - 1,
                PAGE_SIZE,
                2048,
                SECTOR_SIZE,
            ] {
                let case = std::format!(
                    "failure at operation {failed_operation}, byte {tear_bytes}, \
                     page-parallel seed {page_parallel:?}"
                );
                let mut journal = Journal::new(Memory::default(), Region::Configuration);
                journal.flash.fail_after = Some(failed_operation);
                journal.flash.tear_bytes = tear_bytes;
                journal.flash.page_parallel = page_parallel;
                assert!(journal.save(b"first").is_err(), "{case}");
                journal.flash.fail_after = None;
                let mut output = [0; 8];
                let landed = journal.load(&mut output).expect(&case);
                if let Some(record) = landed {
                    assert_eq!(&output[..record.length], b"first", "{case}");
                }
                let torn = journal.flash.bytes[0];
                journal.save(b"retry").expect(&case);
                if landed.is_none() && torn.iter().any(|byte| *byte != 0xff) {
                    assert_eq!(journal.flash.bytes[0], torn, "{case}");
                }

                let record = reboot(&journal).load(&mut output).expect(&case).unwrap();
                assert_eq!(&output[..record.length], b"retry", "{case}");
            }
        }
    }

    #[test]
    fn a_torn_commit_word_that_later_reads_whole_loses_to_the_next_save() {
        let mut journal = Journal::new(Memory::default(), Region::Configuration);
        journal.flash.fail_after = Some(4);
        journal.flash.tear_bytes = 2;
        assert!(journal.save(b"first").is_err());
        journal.flash.fail_after = None;
        journal.save(b"retry").unwrap();
        journal.flash.bytes[0][COMMIT_OFFSET..HEADER_SIZE]
            .copy_from_slice(&COMMITTED.to_le_bytes());

        let mut journal = reboot(&journal);
        let mut output = [0; 8];
        let record = journal.load(&mut output).unwrap().unwrap();
        assert_eq!(&output[..record.length], b"retry");
        journal.save(b"next").unwrap();
        let record = reboot(&journal).load(&mut output).unwrap().unwrap();
        assert_eq!(&output[..record.length], b"next");
        journal.save(b"last").unwrap();
        let record = reboot(&journal).load(&mut output).unwrap().unwrap();
        assert_eq!(&output[..record.length], b"last");
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

    #[test]
    fn random_saves_cuts_and_corruption_load_only_saved_records() {
        use crate::testing::Rng;
        for seed in 0..500 {
            let mut rng = Rng::new(seed);
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            let mut saved: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
            let mut newest: Option<std::vec::Vec<u8>> = None;
            for _ in 0..rng.below(12) + 1 {
                let length = rng.below(200) as usize;
                let payload: std::vec::Vec<u8> = (0..length).map(|_| rng.next() as u8).collect();
                saved.push(payload.clone());
                let cut = rng.below(3) == 0;
                journal.flash.operations = 0;
                journal.flash.fail_after = cut.then(|| rng.below(6) as usize);
                journal.flash.tear_bytes = rng.below(SECTOR_SIZE as u64 + 1) as usize;
                journal.flash.page_parallel = rng.one_in(2).then(|| rng.next());
                let result = journal.save(&payload);
                journal.flash.fail_after = None;
                let mut output = [0; CAPACITY];
                match (result, journal.load(&mut output)) {
                    (Ok(_), loaded) => {
                        let record = loaded.unwrap().unwrap();
                        assert_eq!(&output[..record.length], payload, "seed {seed}");
                        newest = Some(payload);
                    }
                    (Err(_), Ok(Some(record))) => {
                        // The old record, or the new one if it landed whole.
                        let recovered = &output[..record.length];
                        assert!(
                            recovered == payload || Some(recovered) == newest.as_deref(),
                            "seed {seed}"
                        );
                        newest = Some(recovered.to_vec());
                    }
                    // Only a first save may leave nothing readable, and then
                    // the journal is occupied, never empty.
                    (Err(_), Ok(None)) => assert!(newest.is_none(), "seed {seed}"),
                    // Two torn first saves in a row fill both sectors.
                    (Err(_), Err(Error::UnrecognizedJournal)) => {
                        assert!(newest.is_none(), "seed {seed}");
                        assert!(no_free_sector(&journal), "seed {seed}");
                        break;
                    }
                    (Err(_), Err(error)) => panic!("seed {seed}: {error:?}"),
                }
            }
            // Bit rot anywhere: whatever loads was saved, and when nothing
            // loads, the next save keeps every occupied sector intact.
            for _ in 0..rng.below(4) {
                let sector = rng.below(SECTOR_COUNT as u64) as usize;
                let at = rng.below(SECTOR_SIZE as u64) as usize;
                journal.flash.bytes[sector][at] ^= 1 << rng.below(8);
            }
            let mut output = [0; CAPACITY];
            match journal.load(&mut output) {
                Ok(Some(record)) => assert!(
                    saved
                        .iter()
                        .any(|payload| payload[..] == output[..record.length]),
                    "seed {seed}"
                ),
                Ok(None) => {
                    let before = journal.flash.bytes;
                    journal.save(b"after").unwrap();
                    let record = journal.load(&mut output).unwrap().unwrap();
                    assert_eq!(&output[..record.length], b"after", "seed {seed}");
                    assert!(
                        (0..SECTOR_COUNT).all(|sector| before[sector] == [0xff; SECTOR_SIZE]
                            || journal.flash.bytes[sector] == before[sector]),
                        "seed {seed}"
                    );
                }
                Err(error) => assert_eq!(error, Error::UnrecognizedJournal, "seed {seed}"),
            }
        }
    }
    fn no_free_sector(journal: &Journal<Memory>) -> bool {
        journal
            .flash
            .bytes
            .iter()
            .all(|sector| sector.iter().any(|byte| *byte != 0xff))
    }

    /// Power returns: the same flash bytes under a journal that remembers
    /// nothing of the cut save.
    fn reboot(journal: &Journal<Memory>) -> Journal<Memory> {
        let mut flash = journal.flash.clone();
        flash.fail_after = None;
        flash.operations = 0;
        Journal::new(flash, Region::Configuration)
    }

    #[test]
    fn a_reboot_after_a_cut_save_keeps_the_newest_record_and_saves_again() {
        use crate::testing::Rng;
        use std::vec::Vec;
        for seed in 0..500 {
            let mut rng = Rng::new(seed);
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            let mut acknowledged = false;
            // Every record the next load may return: the last acknowledged
            // one and each cut save since, whether it landed or not.
            let mut possible: Vec<Vec<u8>> = Vec::new();
            for _ in 0..rng.range(2, 14) {
                if rng.one_in(2) {
                    journal = reboot(&journal);
                }
                let payload = rng.bytes(200);
                let bytes = journal.flash.bytes;
                let cut = rng.one_in(2);
                journal.flash.operations = 0;
                journal.flash.fail_after = cut.then(|| rng.below(6) as usize);
                journal.flash.tear_bytes = rng.below(SECTOR_SIZE as u64 + 1) as usize;
                journal.flash.page_parallel = rng.one_in(2).then(|| rng.next());
                let result = journal.save(&payload);
                journal.flash.fail_after = None;
                match result {
                    Ok(_) => {
                        acknowledged = true;
                        possible = std::vec![payload];
                    }
                    Err(Error::Flash(())) => possible.push(payload),
                    // Only two first saves cut in a row leave no sector free,
                    // and then the journal is kept as it is.
                    Err(Error::UnrecognizedJournal) => {
                        assert!(!acknowledged, "seed {seed}");
                        assert!(no_free_sector(&journal), "seed {seed}");
                        assert_eq!(journal.flash.bytes, bytes, "seed {seed}");
                        break;
                    }
                    Err(error) => panic!("seed {seed}: {error:?}"),
                }
                // Sometimes the next save starts from the torn sector as it
                // is, with no load in between.
                if rng.one_in(2) && result.is_err() {
                    continue;
                }
                let mut output = [0; CAPACITY];
                match journal.load(&mut output) {
                    Ok(Some(record)) => {
                        let loaded = output[..record.length].to_vec();
                        assert!(possible.contains(&loaded), "seed {seed}");
                        possible = std::vec![loaded];
                    }
                    Ok(None) => {
                        assert!(!acknowledged, "seed {seed}");
                        possible.clear();
                    }
                    Err(Error::UnrecognizedJournal) => {
                        assert!(!acknowledged, "seed {seed}");
                        assert!(no_free_sector(&journal), "seed {seed}");
                    }
                    Err(error) => panic!("seed {seed}: {error:?}"),
                }
            }
            journal = reboot(&journal);
            let mut output = [0; CAPACITY];
            match journal.load(&mut output) {
                Ok(Some(record)) => {
                    assert!(
                        possible.contains(&output[..record.length].to_vec()),
                        "seed {seed}"
                    );
                }
                Ok(None) => assert!(!acknowledged, "seed {seed}"),
                Err(Error::UnrecognizedJournal) => {
                    assert!(!acknowledged, "seed {seed}");
                    assert!(no_free_sector(&journal), "seed {seed}");
                }
                Err(error) => panic!("seed {seed}: {error:?}"),
            }
        }
    }
}
