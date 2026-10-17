use super::*;
use device_api::bulk::{Error as MediaError, OwnedInfo};
use std::collections::BTreeMap;
use std::vec::Vec;

const TAG: &[u8; 16] = b"TEST-LOG-SCHEMA1";

/// How a failed write leaves its sector, like power lost mid-program.
#[derive(Clone, Copy, Debug)]
enum Tear {
    /// The first `n` bytes are new; the rest keep their old contents.
    Prefix(usize),
}

/// Sparse owned media. Never-written sectors hold deterministic garbage,
/// including well-formed records of other stores, like a reused partition.
struct Media {
    sectors: u64,
    written: BTreeMap<u64, Sector>,
    reads: u32,
    writes: u32,
    /// Sectors in the order they were written, last write last.
    order: Vec<u64>,
    /// Fail the write after this many succeed, tearing it; then the power is off.
    fail_after: Option<(u32, Tear)>,
    off: bool,
    unavailable: bool,
}

impl Media {
    fn new(sectors: u64) -> Self {
        Self {
            sectors,
            written: BTreeMap::new(),
            reads: 0,
            writes: 0,
            order: Vec::new(),
            fail_after: None,
            off: false,
            unavailable: false,
        }
    }

    /// Power returns: faults clear, written bytes stay.
    fn reboot(&mut self) {
        self.fail_after = None;
        self.off = false;
        self.reads = 0;
        self.writes = 0;
    }

    fn garbage(index: u64) -> Sector {
        let mut sector = [0u8; SECTOR_SIZE];
        let mut state = index.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        for chunk in sector.chunks_mut(8) {
            state = mix(state);
            chunk.copy_from_slice(&state.to_le_bytes());
        }
        // Every seventh stale sector is a valid record or checkpoint of another epoch.
        if index.is_multiple_of(7) {
            let kind = if index.is_multiple_of(2) {
                RECORD
            } else {
                CHECKPOINT
            };
            let store = mix(index) | 1;
            let offset = index.saturating_sub(DATA_START);
            seal(&mut sector, kind, store, offset, 0, b"stale").unwrap();
        }
        sector
    }

    fn get(&self, index: u64) -> Sector {
        self.written
            .get(&index)
            .copied()
            .unwrap_or_else(|| Self::garbage(index))
    }

    /// Damage a sector so it can never validate.
    fn corrupt(&mut self, index: u64) {
        self.written.insert(index, [0xa5; SECTOR_SIZE]);
    }
}

impl ReadWrite for Media {
    fn owned_info(&mut self) -> Result<OwnedInfo, MediaError> {
        if self.unavailable || self.off {
            return Err(MediaError::Unavailable);
        }
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: self.sectors,
            sector_size: 512,
        })
    }
    fn owned_read(&mut self, index: u64, output: &mut Sector) -> Result<(), MediaError> {
        if self.unavailable || self.off {
            return Err(MediaError::Unavailable);
        }
        if index >= self.sectors {
            return Err(MediaError::Range);
        }
        self.reads += 1;
        *output = self.get(index);
        Ok(())
    }
    fn owned_write(&mut self, index: u64, input: &Sector) -> Result<(), MediaError> {
        if self.unavailable || self.off {
            return Err(MediaError::Unavailable);
        }
        if index >= self.sectors {
            return Err(MediaError::Range);
        }
        if let Some((after, tear)) = self.fail_after {
            if after == 0 {
                let Tear::Prefix(n) = tear;
                let mut sector = self.get(index);
                sector[..n].copy_from_slice(&input[..n]);
                self.written.insert(index, sector);
                self.off = true;
                self.fail_after = None;
                return Err(MediaError::Failed);
            }
            self.fail_after = Some((after - 1, tear));
        }
        self.writes += 1;
        self.order.push(index);
        self.written.insert(index, *input);
        Ok(())
    }
}

fn body(index: u64) -> [u8; 64] {
    let mut body = [0u8; 64];
    body[..8].copy_from_slice(&index.to_le_bytes());
    body[8..16].copy_from_slice(&mix(index).to_le_bytes());
    body
}

#[derive(Debug, Default)]
struct Recovered {
    checkpoint: Vec<u8>,
    records: Vec<(u64, Vec<u8>)>,
    /// Where the log ends could not be proven; the log is read only.
    uncertain: bool,
    log: Option<Log>,
    reads: u32,
}

/// The most records any workload here appends between checkpoints.
const REPLAY: u64 = 32;

fn recover(media: &mut Media) -> Result<Recovered, Error> {
    recover_window(media, REPLAY)
}

fn recover_window(media: &mut Media, replay: u64) -> Result<Recovered, Error> {
    let mut recovery = Recovery::new(TAG, replay);
    let mut sector = [0u8; SECTOR_SIZE];
    let mut out = Recovered::default();
    let writes = media.writes;
    loop {
        match recovery.step(media, &mut sector)? {
            Step::More => {}
            Step::Checkpoint(payload) => out.checkpoint = payload.to_vec(),
            Step::Record { index, payload } => out.records.push((index, payload.to_vec())),
            Step::Done(log) | Step::Uncertain(log) => {
                out.uncertain = !log.writable();
                out.log = Some(log);
                out.reads = recovery.reads();
                assert_eq!(media.writes, writes, "recovery never writes");
                return Ok(out);
            }
        }
    }
}

/// Format, then append `records` with a checkpoint every `interval`.
/// Returns acknowledged record indices before any failure.
fn workload(media: &mut Media, records: u64, interval: u64) -> (Vec<u64>, Result<(), Error>) {
    let mut sector = [0u8; SECTOR_SIZE];
    let mut acknowledged = Vec::new();
    let mut log = match format(media, TAG, 42, &0u64.to_le_bytes(), &mut sector) {
        Ok(log) => log,
        Err(error) => return (acknowledged, Err(error)),
    };
    for index in 0..records {
        match log.append(media, &body(index), &mut sector) {
            Ok(written) => {
                assert_eq!(written, index);
                acknowledged.push(index);
            }
            Err(error) => return (acknowledged, Err(error)),
        }
        if (index + 1) % interval == 0
            && let Err(error) = log.checkpoint(media, &(index + 1).to_le_bytes(), &mut sector)
        {
            return (acknowledged, Err(error));
        }
    }
    (acknowledged, Ok(()))
}

#[test]
fn unformatted_or_absent_media_is_reported_without_writing() {
    let mut media = Media::new(4096);
    assert_eq!(recover(&mut media).unwrap_err(), Error::Uninitialized);
    assert!(media.written.is_empty());
    media.unavailable = true;
    assert_eq!(
        recover(&mut media).unwrap_err(),
        Error::Media(MediaError::Unavailable)
    );
}

#[test]
fn records_survive_reboot_and_boot_reads_stay_bounded_after_many_records() {
    let mut media = Media::new(DATA_START + 200_000);
    let (acknowledged, result) = workload(&mut media, 50_000, 32);
    result.unwrap();
    assert_eq!(acknowledged.len(), 50_000);
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    let log = recovered.log.unwrap();
    assert_eq!(log.head(), 50_000);
    // The newest checkpoint covers 49_984 records; only the rest are replayed.
    assert_eq!(recovered.checkpoint, 49_984u64.to_le_bytes());
    let replayed: Vec<u64> = recovered.records.iter().map(|(i, _)| *i).collect();
    assert_eq!(replayed, (49_984..50_000).collect::<Vec<_>>());
    for (index, payload) in &recovered.records {
        assert_eq!(payload[..], body(*index)[..]);
    }
    // Superblocks, ring, checkpoint and two replay windows past it.
    let bound = 2 + RING as u32 + 1 + 2 * REPLAY as u32;
    assert!(!recovered.uncertain);
    assert!(recovered.reads <= bound, "{} reads", recovered.reads);
    let mut sector = [0u8; SECTOR_SIZE];
    assert_eq!(
        log.read(&mut media, 12_345, &mut sector).unwrap(),
        Some(&body(12_345)[..])
    );
    assert_eq!(log.read(&mut media, 50_000, &mut sector).unwrap(), None);
}

#[test]
fn power_loss_at_every_write_keeps_every_acknowledged_record() {
    const RECORDS: u64 = 40;
    const INTERVAL: u64 = 8;
    // The format writes, then each record and checkpoint write.
    let total = 2 + RING + RECORDS + RECORDS / INTERVAL;
    for fail in 0..total as u32 {
        for tear in [0, 1, 31, 256, 508, SECTOR_SIZE] {
            let mut media = Media::new(DATA_START + 1_000);
            media.fail_after = Some((fail, Tear::Prefix(tear)));
            let (acknowledged, result) = workload(&mut media, RECORDS, INTERVAL);
            assert!(result.is_err(), "write {fail} must fail");
            media.reboot();
            let recovered = match recover(&mut media) {
                // Lost during format before a superblock landed: nothing to
                // keep. After one, before the ring is full: an empty epoch
                // that only a new format repairs.
                Err(Error::Uninitialized | Error::Corrupt) => {
                    assert!(
                        acknowledged.is_empty() && fail < 2 + RING as u32,
                        "write {fail}"
                    );
                    continue;
                }
                other => other.unwrap(),
            };
            let log = recovered.log.unwrap();
            let acked = acknowledged.len() as u64;
            // A torn write never validates; a complete one may have landed.
            assert!(
                (acked..=acked + 1).contains(&log.head()),
                "write {fail} tear {tear}: head {} acked {acked}",
                log.head()
            );
            assert!(!recovered.uncertain, "write {fail} tear {tear}");
            let mut sector = [0u8; SECTOR_SIZE];
            for index in 0..log.head() {
                assert_eq!(
                    log.read(&mut media, index, &mut sector).unwrap(),
                    Some(&body(index)[..]),
                    "write {fail} tear {tear} record {index}"
                );
            }
            // Appending resumes at the head, and a further reboot sees it.
            let mut log = log;
            let head = log.head();
            log.append(&mut media, &body(head), &mut sector).unwrap();
            media.reboot();
            assert_eq!(recover(&mut media).unwrap().log.unwrap().head(), head + 1);
        }
    }
}

#[test]
fn a_torn_newest_checkpoint_falls_back_to_the_previous_one() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 64, 32).1.unwrap();
    // The checkpoint at 64 records is sequence 11 (format wrote 9).
    media.corrupt(RING_START + 3);
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert_eq!(recovered.checkpoint, 32u64.to_le_bytes());
    assert_eq!(recovered.records.len(), 32);
    assert_eq!(recovered.log.unwrap().head(), 64);
}

#[test]
fn full_media_refuses_appends_without_wrapping() {
    let mut media = Media::new(DATA_START + 4);
    let (acknowledged, result) = workload(&mut media, 5, 100);
    assert_eq!(acknowledged.len(), 4);
    assert_eq!(result, Err(Error::Full));
    media.reboot();
    let log = recover(&mut media).unwrap().log.unwrap();
    assert_eq!((log.head(), log.capacity()), (4, 4));
}

#[test]
fn damaged_records_after_the_checkpoint_leave_the_log_uncertain_and_read_only() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 20, 100).1.unwrap();
    media.corrupt(DATA_START + 5);
    media.corrupt(DATA_START + 6);
    media.reboot();
    let before = media.written.clone();
    let recovered = recover(&mut media).unwrap();
    assert!(recovered.uncertain);
    assert_eq!(media.written, before);
    // Replay stops at the damage; the extent covers every valid record seen.
    assert_eq!(recovered.records.len(), 5);
    let log = recovered.log.unwrap();
    assert_eq!(log.head(), 20);
    let mut sector = [0u8; SECTOR_SIZE];
    for index in (0..5).chain(7..20) {
        assert_eq!(
            log.read(&mut media, index, &mut sector).unwrap(),
            Some(&body(index)[..])
        );
    }
    assert_eq!(log.read(&mut media, 5, &mut sector).unwrap(), None);
    // Nothing may be appended to, or checkpointed over, an unproven log.
    let mut log = log;
    assert_eq!(
        log.append(&mut media, &body(20), &mut sector),
        Err(Error::Corrupt)
    );
    assert_eq!(
        log.checkpoint(&mut media, b"x", &mut sector),
        Err(Error::Corrupt)
    );
    assert_eq!(media.written, before);
}

#[test]
fn a_damaged_newest_checkpoint_with_damaged_records_after_it_is_uncertain() {
    // 48 records, a checkpoint at 32 that is later damaged, as are records
    // 32 to 40: the valid 41 to 47 must not be cut off as the end.
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 48, 32).1.unwrap();
    // Format wrote sequence 9; the checkpoint at 32 records is sequence 10.
    media.corrupt(RING_START + 2);
    for index in 32..=40 {
        media.corrupt(DATA_START + index);
    }
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert!(recovered.uncertain);
    assert_eq!(recovered.log.unwrap().head(), 48);

    // Only the checkpoint damaged: records after it name a newer sequence.
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 48, 32).1.unwrap();
    media.corrupt(RING_START + 2);
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert!(recovered.uncertain);
    assert_eq!(recovered.records.len(), 32);
    assert_eq!(recovered.log.unwrap().head(), 48);
}

#[test]
fn an_older_checkpoint_the_ring_should_hold_is_proof_of_damage() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 200, 16).1.unwrap();
    // Newest is sequence 21 (format wrote 9); 15 to 21 must all be there.
    media.corrupt(RING_START + 9 % RING);
    media.reboot();
    assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);

    // The slot after the newest is exempt: a torn next checkpoint.
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 200, 16).1.unwrap();
    media.corrupt(RING_START + 14 % RING);
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert!(!recovered.uncertain);
    assert_eq!(recovered.log.unwrap().head(), 200);
}

#[test]
fn damaged_checkpoints_young_in_an_epoch_are_caught_by_the_filled_ring() {
    // Sequences 10 and 11 overwrite format's fillers 2 and 3, which recovery
    // checks: damage to both cannot pass for a torn next checkpoint and hide
    // the records further on.
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 80, 32).1.unwrap();
    media.corrupt(RING_START + 2);
    media.corrupt(RING_START + 3);
    for index in 0..80 {
        if index != 70 {
            media.corrupt(DATA_START + index);
        }
    }
    media.reboot();
    assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);
}

#[test]
fn a_damaged_tail_reads_as_the_power_cut_it_cannot_be_told_from() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 20, 100).1.unwrap();
    media.corrupt(DATA_START + 18);
    media.corrupt(DATA_START + 19);
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert!(!recovered.uncertain);
    assert_eq!(recovered.log.unwrap().head(), 18);
}

#[test]
fn a_failed_checkpoint_blocks_appends_until_one_is_written() {
    let mut media = Media::new(DATA_START + 1_000);
    let mut sector = [0u8; SECTOR_SIZE];
    let mut log = format(&mut media, TAG, 42, b"0", &mut sector).unwrap();
    log.append(&mut media, &body(0), &mut sector).unwrap();
    // The write fails after landing: recovery would see the new checkpoint.
    media.fail_after = Some((0, Tear::Prefix(SECTOR_SIZE)));
    assert!(log.checkpoint(&mut media, b"1", &mut sector).is_err());
    media.reboot();
    assert_eq!(
        log.append(&mut media, &body(1), &mut sector),
        Err(Error::Unsettled)
    );
    log.checkpoint(&mut media, b"1", &mut sector).unwrap();
    log.append(&mut media, &body(1), &mut sector).unwrap();
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert!(!recovered.uncertain);
    assert_eq!(recovered.checkpoint, b"1");
    assert_eq!(recovered.log.unwrap().head(), 2);
}

#[test]
fn lost_checkpoints_or_foreign_superblocks_are_kept_for_diagnosis() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 10, 4).1.unwrap();
    for slot in 0..RING {
        media.corrupt(RING_START + slot);
    }
    media.reboot();
    let before = media.written.clone();
    assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);
    assert_eq!(media.written, before);

    let mut media = Media::new(DATA_START + 1_000);
    let mut sector = [0u8; SECTOR_SIZE];
    format(&mut media, b"ANOTHER-OWNER-01", 1, &[], &mut sector).unwrap();
    media.reboot();
    assert_eq!(recover(&mut media).unwrap_err(), Error::Foreign);
    assert_eq!(
        format(&mut media, TAG, 2, &[], &mut sector).unwrap_err(),
        Error::Foreign
    );
    assert_eq!(media.writes, 0);

    // A region that grew at its end keeps the epoch and its capacity until
    // the next format; one that shrank may have lost records.
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 3, 100).1.unwrap();
    media.sectors += 5_000;
    media.reboot();
    let log = recover(&mut media).unwrap().log.unwrap();
    assert_eq!((log.head(), log.capacity()), (3, 1_000));
    let mut sector = [0u8; SECTOR_SIZE];
    assert_eq!(
        format(&mut media, TAG, 3, &[], &mut sector)
            .unwrap()
            .capacity(),
        6_000
    );
    media.sectors -= 1;
    media.reboot();
    assert_eq!(recover(&mut media).unwrap_err(), Error::Foreign);
}

#[test]
fn reformatting_starts_a_new_epoch_that_ignores_earlier_records() {
    let mut media = Media::new(DATA_START + 1_000);
    workload(&mut media, 30, 8).1.unwrap();
    media.reboot();
    let old = recover(&mut media).unwrap().log.unwrap();
    let mut sector = [0u8; SECTOR_SIZE];
    let mut new = format(&mut media, TAG, 42, b"fresh", &mut sector).unwrap();
    assert_ne!(new.store_id(), old.store_id());
    assert_eq!(new.generation(), old.generation() + 1);
    assert_eq!(new.read(&mut media, 0, &mut sector).unwrap(), None);
    new.append(&mut media, &body(900), &mut sector).unwrap();
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert_eq!(recovered.checkpoint, b"fresh");
    assert_eq!(recovered.log.unwrap().head(), 1);
    assert_eq!(recovered.records, [(0, body(900).to_vec())]);

    // Interrupted before any superblock copy: the current epoch stays.
    let current = recover(&mut media).unwrap().log.unwrap();
    media.fail_after = Some((1, Tear::Prefix(0)));
    assert!(format(&mut media, TAG, 7, b"x", &mut sector).is_err());
    media.reboot();
    assert_eq!(
        recover(&mut media).unwrap().log.unwrap().store_id(),
        current.store_id()
    );
    // Interrupted after one copy, or while filling the ring: the newer
    // generation wins, but is corrupt until a format completes.
    for fail in [2, 2 + RING as u32 - 1] {
        media.fail_after = Some((fail, Tear::Prefix(0)));
        assert!(format(&mut media, TAG, 7, b"x", &mut sector).is_err());
        media.reboot();
        assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);
    }
    format(&mut media, TAG, 7, b"x", &mut sector).unwrap();
    media.reboot();
    let after = recover(&mut media).unwrap();
    let log = after.log.unwrap();
    assert!(log.generation() > current.generation());
    assert_ne!(log.store_id(), current.store_id());
    assert_eq!((after.checkpoint.as_slice(), log.head()), (&b"x"[..], 0));
}

#[test]
fn lost_superblocks_with_records_are_corrupt_and_a_reformat_never_reuses_their_store() {
    let mut media = Media::new(DATA_START + 1_000);
    // Format with seed 42, 30 records and checkpoints, as the workload does.
    workload(&mut media, 30, 8).1.unwrap();
    media.reboot();
    let old = recover(&mut media).unwrap().log.unwrap();
    media.corrupt(0);
    media.corrupt(1);
    media.reboot();
    // Not unformatted: the records stay for diagnosis, and nothing is written.
    assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);
    assert_eq!(media.writes, 0);
    // Records alone, without a checkpoint past head 0, are enough to tell.
    for slot in 0..RING {
        media.corrupt(RING_START + slot);
    }
    assert_eq!(recover(&mut media).unwrap_err(), Error::Corrupt);

    // An explicit reformat with the very same seed and no superblock to
    // count generations from must not recreate the old store: its records
    // would validate again.
    let mut sector = [0u8; SECTOR_SIZE];
    let new = format(&mut media, TAG, 42, b"fresh", &mut sector).unwrap();
    assert_ne!(new.store_id(), old.store_id());
    media.reboot();
    let recovered = recover(&mut media).unwrap();
    assert_eq!(recovered.log.unwrap().head(), 0, "no old record resurrects");
    assert!(recovered.records.is_empty());
}

/// Random record counts, checkpoint spacing within the replay promise and
/// damage sets: recovery proves the exact head, or reports the log uncertain
/// or corrupt. It may report a shorter head only when every sector written
/// after that head is damaged, which no rule can tell from a power cut.
#[test]
fn recovery_is_exact_or_uncertain_under_random_damage() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % bound
    };
    let (mut exact, mut uncertain, mut corrupt, mut cut) = (0, 0, 0, 0);
    // Seeds clear of the stale stores the media's garbage holds.
    for case in 0..3_000 {
        let mut media = Media::new(DATA_START + 600);
        let mut sector = [0u8; SECTOR_SIZE];
        let mut log = format(
            &mut media,
            TAG,
            (case << 32) | 0x5eed,
            &0u64.to_le_bytes(),
            &mut sector,
        )
        .unwrap();
        let records = next(300);
        // Often the full replay window, which reaches furthest past damage.
        let full = next(2) == 0;
        let interval = move |random: u64| if full { REPLAY } else { 1 + random };
        let mut due = interval(next(REPLAY));
        for index in 0..records {
            log.append(&mut media, &body(index), &mut sector).unwrap();
            due -= 1;
            if due == 0 {
                log.checkpoint(&mut media, &(index + 1).to_le_bytes(), &mut sector)
                    .unwrap();
                due = interval(next(REPLAY));
            }
        }
        let order = media.order.clone();
        let last_write = |location: u64| order.iter().rposition(|&at| at == location);
        let mut damaged = Vec::new();
        // A run of consecutive writes, often the newest, then scattered
        // sectors anywhere.
        if next(2) == 0 {
            let run = 1 + next(48) as usize;
            let end = if next(2) == 0 {
                order.len()
            } else {
                1 + next(order.len() as u64) as usize
            };
            damaged.extend(order[end.saturating_sub(run)..end].iter().copied());
        }
        for _ in 0..next(4) {
            damaged.push(order[next(order.len() as u64) as usize]);
        }
        for &location in &damaged {
            media.corrupt(location);
        }
        media.reboot();
        let recovered = match recover(&mut media) {
            Err(_) => {
                corrupt += 1;
                continue;
            }
            Ok(recovered) => recovered,
        };
        let log = recovered.log.unwrap();
        assert!(log.head() <= records, "case {case}: head past the end");
        if recovered.uncertain {
            uncertain += 1;
            continue;
        }
        let head = log.head();
        // Damage below the head is the reader's to report, record by record.
        for index in (0..head).filter(|index| !damaged.contains(&(DATA_START + index))) {
            assert_eq!(
                log.read(&mut media, index, &mut sector).unwrap(),
                Some(&body(index)[..]),
                "case {case}: record {index} below a proven head"
            );
        }
        // A proven head must read as a power cut just after record head - 1
        // (or the checkpoint written at that head): everything written later
        // is damaged, and the checkpoints the ring holds by then are intact.
        let mut after = if head == 0 {
            RING as usize + 1 // The format's last write.
        } else {
            last_write(DATA_START + head - 1).unwrap()
        };
        if order
            .get(after + 1)
            .is_some_and(|&at| at < DATA_START && !damaged.contains(&at))
        {
            after += 1;
        }
        assert!(
            order[after + 1..]
                .iter()
                .all(|location| damaged.contains(location)),
            "case {case}: {records} records recovered as {head} with damage {damaged:?}"
        );
        // Every slot but the one after the newest checkpoint, the first
        // write or a later one (format's fillers come between).
        let newest = (0..=after)
            .rev()
            .find(|&at| (at == 0 || at > RING as usize + 1) && order[at] < RING_START + RING)
            .unwrap();
        let exempt = RING_START + (order[newest] - RING_START + 1) % RING;
        for checkpoint in (RING_START..RING_START + RING).filter(|&slot| slot != exempt) {
            assert!(
                !damaged.contains(&checkpoint),
                "case {case}: damaged checkpoint {checkpoint} behind proven head {head}"
            );
        }
        if head == records {
            exact += 1;
        } else {
            cut += 1;
        }
    }
    // Every outcome is exercised.
    assert!(
        exact > 300 && uncertain > 300 && corrupt > 100 && cut > 100,
        "{exact} {uncertain} {corrupt} {cut}"
    );
}
