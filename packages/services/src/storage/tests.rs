use super::*;

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
    fn availability(&self, _region: Region) -> device_api::observation::Availability {
        device_api::observation::Availability::Ready
    }
    fn geometry(&self, region: Region) -> Geometry {
        Geometry {
            capacity: if region == Region::Configuration {
                SECTOR_SIZE * SECTOR_COUNT
            } else {
                0
            },
            program_size: 4,
            erase_size: SECTOR_SIZE,
            durable: true,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
        assert_eq!(region, Region::Configuration);
        checked_range::<()>(SECTOR_SIZE * SECTOR_COUNT, offset, output.len(), 1).map_err(|_| ())?;
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
        checked_range::<()>(SECTOR_SIZE * SECTOR_COUNT, offset, data.len(), 4).map_err(|_| ())?;
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
        fn availability(&self, _region: Region) -> device_api::observation::Availability {
            device_api::observation::Availability::Ready
        }
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
    assert_eq!(&journal.flash.memory.bytes[0][..8], b"C606\x02\x00\x18\x00");
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
fn an_erase_cut_anywhere_never_brings_an_older_record_back() {
    for saves in 1..=3 {
        for cut in 0..=4 {
            let mut journal = Journal::new(Memory::default(), Region::Configuration);
            for save in 0..saves {
                journal.save(&[save]).unwrap();
            }
            journal.flash.fail_after = Some(journal.flash.operations + cut);
            let erased = journal.erase();
            journal.flash.fail_after = None;
            let mut output = [0; 16];
            match journal.load(&mut output) {
                Ok(None) => assert!(erased.is_ok(), "{saves} {cut}"),
                Ok(Some(_)) => {
                    assert!(erased.is_err());
                    assert_eq!(output[0], saves - 1, "{saves} {cut}");
                }
                Err(error) => assert_eq!(error, Error::UnrecognizedJournal),
            }
        }
    }
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
            6 => {
                let header = &mut journal.flash.bytes[0];
                header[0..4].copy_from_slice(&MAGIC);
                put_u16(header, VERSION_AT, 4);
                put_u16(header, HEADER_SIZE_AT, 28);
                put_u32(header, SEQUENCE_AT, 1);
                put_u32(header, LENGTH_AT, 5);
                put_u32(header, CHECKSUM_AT, 0);
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
                journal.flash.bytes[0][0..4].copy_from_slice(&MAGIC);
                journal.flash.bytes[0][PAGE_SIZE] = 0;
            }
            // Programmed bytes behind an untouched header.
            9 => journal.flash.bytes[0][100] = 0,
            _ => {
                journal.save(b"existing").unwrap();
                if scenario == 2 {
                    journal.flash.bytes[0][4] = 1;
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

fn cut_in_its_commit(tear_bytes: usize, page_parallel: Option<u64>) -> Journal<Memory> {
    let mut journal = Journal::new(Memory::default(), Region::Configuration);
    journal.flash.fail_after = Some(4);
    journal.flash.tear_bytes = tear_bytes;
    journal.flash.page_parallel = page_parallel;
    assert!(journal.save(b"wipe").is_err());
    reboot(&journal)
}

#[test]
fn a_recommit_completes_a_cut_commit_word_and_programs_only_it() {
    for page_parallel in [None].into_iter().chain((0..64).map(Some)) {
        for tear_bytes in 0..4 {
            let case = std::format!("{tear_bytes} bytes, seed {page_parallel:?}");
            let mut journal = cut_in_its_commit(tear_bytes, page_parallel);
            let before = journal.flash.bytes;
            let record = journal.recommit(0).expect(&case);
            assert_eq!(record.sequence, 1, "{case}");
            for (at, (now, was)) in journal.flash.bytes[0].iter().zip(before[0]).enumerate() {
                if !(COMMIT_OFFSET..HEADER_SIZE).contains(&at) {
                    assert_eq!(*now, was, "{case}: byte {at}");
                }
            }
            assert_eq!(journal.flash.bytes[1], before[1], "{case}");
            let mut output = [0; 8];
            let loaded = reboot(&journal).load(&mut output).expect(&case).unwrap();
            assert_eq!(&output[..loaded.length], b"wipe", "{case}");
            let committed = journal.flash.bytes;
            assert_eq!(journal.recommit(0), Ok(record), "{case}");
            assert_eq!(journal.flash.bytes, committed, "{case}");
        }
    }
}

#[test]
fn a_recommit_refuses_without_a_write_unless_a_record_was_begun() {
    type Damage = fn(&mut [u8; SECTOR_SIZE]);
    let damages: [Damage; 5] = [
        |sector| sector.fill(0xff),
        |sector| sector[HEADER_SIZE] ^= 1,
        |sector| sector[VERSION_AT] = 1,
        |sector| sector[COMMIT_OFFSET + 1] = !COMMITTED.to_le_bytes()[1],
        |sector| sector.fill(0),
    ];
    for (case, damage) in damages.iter().enumerate() {
        let mut journal = cut_in_its_commit(2, None);
        damage(&mut journal.flash.bytes[0]);
        let before = journal.flash.bytes;
        assert_eq!(journal.recommit(0), Err(Error::NoRecord), "case {case}");
        assert_eq!(journal.flash.bytes, before, "case {case}");
        assert_eq!(journal.flash.operations, 1, "case {case}: one read");
    }
}

/// Flash whose cut commit words are marginal: each read returns such a
/// byte as programmed or as the cut left it, until a program completes
/// over it.
struct Marginal {
    memory: Memory,
    bytes: std::vec::Vec<(usize, u8)>,
    reads: crate::testing::Rng,
}

impl OwnedFlash for Marginal {
    type Error = ();
    fn availability(&self, _region: Region) -> device_api::observation::Availability {
        device_api::observation::Availability::Ready
    }
    fn geometry(&self, region: Region) -> Geometry {
        self.memory.geometry(region)
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
        self.memory.read(region, offset, output)?;
        for &(at, whole) in &self.bytes {
            if let Some(byte) = at.checked_sub(offset).and_then(|i| output.get_mut(i))
                && self.reads.one_in(2)
            {
                *byte = whole;
            }
        }
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, data: &[u8]) -> Result<(), ()> {
        self.memory.program(region, offset, data)?;
        self.bytes
            .retain(|&(at, _)| !(offset..offset + data.len()).contains(&at));
        Ok(())
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
        self.memory.erase(region, offset, length)
    }
}

#[test]
fn a_recommitted_marginal_word_reads_committed_on_every_read() {
    for seed in 0..64 {
        let journal = cut_in_its_commit(2, None);
        let mut flash = Marginal {
            memory: journal.flash.clone(),
            bytes: std::vec![
                (COMMIT_OFFSET + 2, COMMITTED.to_le_bytes()[2]),
                (COMMIT_OFFSET + 3, COMMITTED.to_le_bytes()[3]),
            ],
            reads: crate::testing::Rng::new(seed),
        };
        let mut payload = [0; 8];
        let mut read =
            |flash: &mut Marginal| Journal::new(flash, Region::Configuration).held(0, &mut payload);
        let held: std::vec::Vec<_> = (0..64).map(|_| read(&mut flash)).collect();
        assert!(held.contains(&Ok(Held::Other)), "seed {seed}");
        assert!(
            held.iter()
                .any(|held| matches!(held, Ok(Held::Committed(_)))),
            "seed {seed}"
        );
        let mut journal = Journal::new(&mut flash, Region::Configuration);
        assert!(journal.recommit(0).is_ok(), "seed {seed}");
        for _ in 0..16 {
            assert!(
                matches!(read(&mut flash), Ok(Held::Committed(_))),
                "seed {seed}"
            );
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
    journal.flash.bytes[0][COMMIT_OFFSET..HEADER_SIZE].copy_from_slice(&COMMITTED.to_le_bytes());

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
