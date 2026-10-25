use super::*;
use crate::testing::Rng;
use crate::workout::library::tests::{formatted, put};
use firmware_services::testing::{Media, Tear};
use std::string::{String, ToString};
use std::vec::Vec;

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

fn internal(ride_id: u32) -> Record {
    Record {
        journal: b'i',
        ..record(ride_id)
    }
}

fn any(rng: &mut Rng) -> Record {
    Record {
        journal: rng.pick(b"ib"),
        ride_id: rng.next() as u32,
        started_utc_ms: rng.next(),
        facts: Facts {
            profile: rng.option(|rng| {
                TrainingProfile::new(rng.range(50, 600) as u16, rng.range(80, 220) as u16).unwrap()
            }),
            recovery_bpm: rng.option(|rng| rng.next() as i16),
            climbing: rng.option(|rng| Climbing {
                count: rng.next() as u16,
                gain_m: rng.next() as u16,
            }),
        },
    }
}

/// The table's records as `read` gives them, piece by piece, in table order.
fn table(region: &mut impl ReadWrite) -> Vec<Record> {
    let mut found = Vec::new();
    let mut next = Some(0);
    while let Some(piece) = next {
        next = read(region, piece, |record| found.push(record))
            .unwrap()
            .and_then(|piece| piece.next);
    }
    found
}

/// The older files' records, by journal and ride id.
fn older(region: &mut impl ReadWrite) -> Vec<Record> {
    let mut found = Vec::new();
    let mut next = Some(0);
    while let Some(index) = next {
        next = read_files(region, index, |record| found.push(record)).unwrap();
    }
    found.sort_by_key(|record| (record.journal, record.ride_id));
    found
}

fn names(region: &mut impl ReadWrite) -> Vec<String> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    let mut names = Vec::new();
    dir.list(&mut store, &mut sector, |entry| {
        names.push(entry.name.as_str().to_string())
    })
    .unwrap();
    names.sort();
    names
}

fn raw(region: &mut impl ReadWrite, name: &str) -> Option<Vec<u8>> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    let entry = dir
        .find(&mut store, &files::Name::new(name).unwrap(), &mut sector)
        .unwrap()?;
    let mut bytes = std::vec![0; entry.length as usize];
    dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
        .unwrap();
    Some(bytes)
}

fn put_on(region: &mut impl ReadWrite, name: &str, bytes: &[u8]) {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
    let name = files::Name::new(name).unwrap();
    files::write(&mut store, &name, bytes, &mut sector).unwrap();
}

fn older_file(region: &mut impl ReadWrite, record: &Record) {
    let name = file(record.journal, record.ride_id);
    put_on(region, name.as_str(), &encode(record));
}

fn media(seed: u64) -> Media {
    let mut media = Media::new(8_000_000);
    let mut sector = [0; SECTOR_SIZE];
    volume::create(&mut media, &mut sector).unwrap();
    let mut store = volume::open(&mut media, &volume::FILES, &mut sector).unwrap();
    files::format(&mut store, seed, &mut sector).unwrap();
    media
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

        let mut slot = [0; SLOT];
        encode_slot(&record, &mut slot);
        assert_eq!(decode_slot(&slot), Some(record), "seed {seed}");
        let bit = rng.below(8 * SLOT as u64) as usize;
        slot[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(decode_slot(&slot), None, "seed {seed} slot bit {bit}");
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
        let _ = decode_table(&bytes).count();
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
fn a_later_file_version_with_more_fields_still_reads() {
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
fn the_table_keeps_the_newest_saved_rides_in_one_entry_and_a_save_replaces_its_ride() {
    let mut region = formatted();
    put(&mut region, "rides/b1.workout", b"a workout record");
    let extra = 5;
    for ride in 1..=(KEEP + extra) as u32 {
        save(&mut region, &record(ride)).unwrap();
    }
    let kept = table(&mut region);
    assert_eq!(kept.len(), KEEP);
    assert_eq!(kept[0], record(extra as u32 + 1), "the oldest saved went");
    assert_eq!(kept.last(), Some(&record((KEEP + extra) as u32)));
    assert_eq!(names(&mut region), ["rides/b1.workout", TABLE]);
    let bytes = raw(&mut region, TABLE).unwrap();
    assert_eq!(bytes.len(), SLOT * (KEEP + 1));
    assert_eq!(decode_table(&bytes).collect::<Vec<_>>(), kept);

    let mut again = record(10);
    again.facts.recovery_bpm = Some(31);
    save(&mut region, &again).unwrap();
    let kept = table(&mut region);
    assert_eq!(kept.len(), KEEP, "a ride saved again takes no more room");
    assert_eq!(kept.last(), Some(&again), "and moves to the end");
    assert_eq!(kept.iter().filter(|r| r.ride_id == 10).count(), 1);
}

#[test]
fn a_clear_forgets_its_journal_and_its_older_files_and_nothing_else() {
    let mut region = formatted();
    put(&mut region, "rides/b1.workout", b"a workout record");
    for ride in 1..=20 {
        save(&mut region, &record(ride)).unwrap();
        save(&mut region, &internal(ride)).unwrap();
    }
    for ride in 30..50 {
        older_file(&mut region, &record(ride));
    }
    older_file(&mut region, &internal(60));
    forget(&mut region, b'b').unwrap();
    let mut expected: Vec<Record> = (1..=20).map(internal).collect();
    expected.push(internal(60));
    assert_eq!(table(&mut region), expected);
    assert_eq!(names(&mut region), ["rides/b1.workout", TABLE]);
    let before = raw(&mut region, TABLE);
    forget(&mut region, b'b').unwrap();
    assert_eq!(raw(&mut region, TABLE), before, "nothing left to forget");
}

#[test]
fn older_files_fold_into_the_table_and_go_once_it_has_committed() {
    let mut region = formatted();
    put(&mut region, "notes.txt", b"host");
    put(&mut region, "rides/b3.workout", b"a workout record");
    // The table holds ride 2 from a later start, and ride 3 with the same
    // start and its recovery measured.
    let mut later = record(2);
    later.started_utc_ms += 86_400_000;
    let mut measured = record(3);
    measured.facts.recovery_bpm = Some(30);
    save(&mut region, &later).unwrap();
    save(&mut region, &measured).unwrap();
    for ride in 1..=20 {
        older_file(&mut region, &record(ride));
    }
    put(&mut region, "rides/b40.facts", b"not a record");
    put(&mut region, "rides/b41.facts", &encode(&record(42)));
    save(&mut region, &record(21)).unwrap();
    assert_eq!(
        older(&mut region).len(),
        20 - BATCH,
        "a save folds a batch of older files in"
    );
    while fold(&mut region).unwrap() {}
    assert!(!fold(&mut region).unwrap());
    let mut kept = table(&mut region);
    assert_eq!(kept.len(), 21);
    kept.sort_by_key(|record| record.ride_id);
    assert_eq!(kept[1], later, "a later start outranks an older file");
    assert_eq!(kept[2], measured, "the table wins a tie");
    for ride in [1, 4, 20] {
        assert_eq!(kept[ride as usize - 1], record(ride));
    }
    assert_eq!(
        names(&mut region),
        ["notes.txt", "rides/b3.workout", TABLE],
        "older files go, damaged or misnamed ones too; the rest stay"
    );
}

#[test]
fn an_older_file_with_a_later_start_outranks_the_table() {
    let mut region = formatted();
    save(&mut region, &record(5)).unwrap();
    let mut after_clear = record(5);
    after_clear.started_utc_ms += 7 * 86_400_000;
    older_file(&mut region, &after_clear);
    fold(&mut region).unwrap();
    assert_eq!(table(&mut region), [after_clear]);
}

#[test]
fn a_power_cut_while_saving_keeps_every_earlier_record() {
    let mut cut = 0;
    for seed in 0..300 {
        let mut rng = Rng::new(seed);
        let mut media = media(seed);
        let count = rng.range(1, 60) as u32;
        for ride in 1..=count {
            save(&mut media, &record(ride)).unwrap();
        }
        let old = table(&mut media);
        let mut new = record(rng.range(1, u64::from(count) + 1) as u32);
        new.facts.recovery_bpm = Some(25);
        media.cut = Some((rng.below(12), Tear::pick(&mut rng)));
        let saved = save(&mut media, &new).is_ok();
        media.reboot();
        let found = table(&mut media);
        let mut appended: Vec<Record> = old
            .iter()
            .copied()
            .filter(|r| r.key() != new.key())
            .collect();
        appended.push(new);
        if saved {
            assert_eq!(found, appended, "seed {seed}");
        } else {
            cut += 1;
            assert!(found == old || found == appended, "seed {seed}: {found:?}");
        }
    }
    assert!(cut > 100, "{cut} saves cut");
}

#[test]
fn a_power_cut_while_folding_loses_no_older_file() {
    let mut cut = 0;
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let mut media = media(seed);
        for ride in 1..=4 {
            save(&mut media, &internal(ride)).unwrap();
        }
        let files = rng.range(1, 12) as u32;
        for ride in 1..=files {
            older_file(&mut media, &record(ride));
        }
        media.cut = Some((rng.below(16), Tear::pick(&mut rng)));
        cut += usize::from(fold(&mut media).is_err());
        media.reboot();
        let mut union = table(&mut media);
        union.extend(older(&mut media));
        for ride in 1..=files {
            assert!(union.contains(&record(ride)), "seed {seed}: ride {ride}");
        }
        while fold(&mut media).unwrap() {}
        let mut kept = table(&mut media);
        kept.sort_by_key(|record| (record.journal, record.ride_id));
        let mut expected: Vec<Record> = (1..=files).map(record).collect();
        expected.extend((1..=4).map(internal));
        assert_eq!(kept, expected, "seed {seed}");
        assert_eq!(older(&mut media), [], "seed {seed}");
    }
    assert!(cut > 100, "{cut} folds cut");
}

#[test]
fn the_table_gives_way_on_a_full_directory_and_frees_the_entries_it_folds() {
    use crate::workout::history;
    let mut region = formatted();
    for host in 0..70 {
        put(&mut region, &std::format!("host/{host}"), b"x");
    }
    for ride in 1..=60 {
        let workout = history::Record {
            journal: b'b',
            ride_id: ride,
            started_utc_ms: None,
            name: crate::workout::model::Name::new("ENDURANCE"),
            test: None,
            summary: crate::workout::player::Summary::default(),
        };
        assert_eq!(history::save(&mut region, &workout), Ok(()), "ride {ride}");
        assert_eq!(save(&mut region, &record(ride)), Ok(()), "ride {ride}");
    }
    assert_eq!(table(&mut region).len(), 60, "every ride's facts");
    assert_eq!(names(&mut region).len(), 70 + history::KEEP + 1);

    let mut full = formatted();
    for host in 0..files::CAPACITY - 1 {
        put(&mut full, &std::format!("host/{host}"), b"x");
    }
    assert_eq!(
        save(&mut full, &record(1)),
        Err(Error::Files(files::Error::Full)),
        "without older files to free, the last entry stays free"
    );
    assert_eq!(names(&mut full).len(), files::CAPACITY - 1);

    // Older firmware's facts files and the host's fill the directory: the
    // table takes the one free entry, as the files it folds go right after.
    let mut crowded = formatted();
    for host in 0..files::CAPACITY - 65 {
        put(&mut crowded, &std::format!("host/{host}"), b"x");
    }
    for ride in 1..=64 {
        older_file(&mut crowded, &record(ride));
    }
    save(&mut crowded, &record(65)).unwrap();
    while fold(&mut crowded).unwrap() {}
    let kept = table(&mut crowded);
    assert_eq!(kept.len(), 65);
    assert_eq!(kept[BATCH], record(65), "after the first batch");
    assert_eq!(names(&mut crowded).len(), files::CAPACITY - 65 + 1);

    // With no entry free at all, the oldest file goes for the table that
    // holds its ride: every ride stays.
    let mut filled = formatted();
    for host in 0..files::CAPACITY - 64 {
        put(&mut filled, &std::format!("host/{host}"), b"x");
    }
    for ride in 1..=64 {
        older_file(&mut filled, &record(ride));
    }
    assert_eq!(names(&mut filled).len(), files::CAPACITY);
    save(&mut filled, &record(65)).unwrap();
    while fold(&mut filled).unwrap() {}
    let mut kept = table(&mut filled);
    kept.sort_by_key(|record| record.ride_id);
    assert_eq!(kept, (1..=65).map(record).collect::<Vec<_>>());
    assert_eq!(names(&mut filled).len(), files::CAPACITY - 64 + 1);
}

#[test]
fn a_clear_on_a_full_directory_deletes_its_journals_older_files() {
    let mut region = formatted();
    for host in 0..files::CAPACITY - 64 {
        put(&mut region, &std::format!("host/{host}"), b"x");
    }
    for ride in 1..=60 {
        older_file(&mut region, &record(ride));
    }
    for ride in 1..=4 {
        older_file(&mut region, &internal(ride));
    }
    forget(&mut region, b'b').unwrap();
    assert_eq!(older(&mut region), []);
    assert_eq!(
        table(&mut region),
        (1..=4).map(internal).collect::<Vec<_>>()
    );
    assert_eq!(names(&mut region).len(), files::CAPACITY - 64 + 1);
}

#[test]
fn a_full_directory_makes_room_with_a_file_that_holds_nothing_and_a_cut_loses_no_ride() {
    let mut cut = 0;
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let mut media = media(seed);
        for host in 0..files::CAPACITY - 64 {
            put_on(&mut media, &std::format!("host/{host}"), b"x");
        }
        put_on(&mut media, "rides/b99.facts", b"not a record");
        for ride in 1..=63 {
            older_file(&mut media, &record(ride));
        }
        media.cut = Some((rng.below(16), Tear::pick(&mut rng)));
        cut += usize::from(fold(&mut media).is_err());
        media.reboot();
        let mut union = table(&mut media);
        union.extend(older(&mut media));
        for ride in 1..=63 {
            assert!(union.contains(&record(ride)), "seed {seed}: ride {ride}");
        }
    }
    assert!(cut > 100, "{cut} folds cut");
}

/// Fails every read of a sector that holds an older file.
struct Unreadable<M>(M);

impl<M: ReadWrite> ReadWrite for Unreadable<M> {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        self.0.owned_info()
    }

    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        self.0.owned_read(sector, output)?;
        if output.starts_with(MAGIC) {
            return Err(device_api::bulk::Error::Failed);
        }
        Ok(())
    }

    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        self.0.owned_write(sector, input)
    }
}

#[test]
fn an_older_file_that_will_not_read_stays_and_the_folding_stops() {
    let mut region = formatted();
    save(&mut region, &internal(1)).unwrap();
    for ride in 1..=(2 * BATCH) as u32 {
        older_file(&mut region, &record(ride));
    }
    let mut failing = Unreadable(region);
    assert_eq!(fold(&mut failing), Ok(false), "no progress, no more steps");
    save(&mut failing, &internal(2)).unwrap();
    let mut region = failing.0;
    assert_eq!(
        older(&mut region).len(),
        2 * BATCH,
        "every older file stays"
    );
    assert_eq!(table(&mut region), [internal(1), internal(2)]);
    while fold(&mut region).unwrap() {}
    assert_eq!(table(&mut region).len(), 2 + 2 * BATCH);
}

#[test]
fn a_damaged_header_keeps_the_rides_and_a_file_without_the_magic_is_left_alone() {
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let mut region = formatted();
        for ride in 1..=3 {
            save(&mut region, &record(ride)).unwrap();
        }
        let bytes = raw(&mut region, TABLE).unwrap();
        let mut damaged = bytes.clone();
        let at = rng.below(SLOT as u64) as usize;
        damaged[at] ^= 1 << rng.below(8);
        put(&mut region, TABLE, &damaged);
        older_file(&mut region, &record(9));
        if at < 4 {
            assert_eq!(
                read(&mut region, 0, |_| {}),
                Err(Error::Table),
                "seed {seed}"
            );
            assert_eq!(save(&mut region, &record(4)), Err(Error::Table));
            assert_eq!(fold(&mut region), Err(Error::Table));
            assert_eq!(raw(&mut region, TABLE), Some(damaged), "seed {seed}");
            assert_eq!(older(&mut region), [record(9)]);
            continue;
        }
        let rides: Vec<Record> = (1..=3).map(record).collect();
        assert_eq!(
            table(&mut region),
            rides,
            "seed {seed}: read past the header"
        );
        save(&mut region, &record(4)).unwrap();
        let written = raw(&mut region, TABLE).unwrap();
        assert!(stride(&written).is_some(), "seed {seed}: a fresh header");
        let expected: Vec<Record> = [1, 2, 3, 9, 4].into_iter().map(record).collect();
        assert_eq!(table(&mut region), expected, "seed {seed}");
    }
}

#[test]
fn a_damaged_slot_costs_only_its_ride() {
    let mut region = formatted();
    for ride in 1..=40 {
        save(&mut region, &record(ride)).unwrap();
    }
    let mut bytes = raw(&mut region, TABLE).unwrap();
    bytes[SLOT * 17 + 9] ^= 4;
    put(&mut region, TABLE, &bytes);
    let kept = table(&mut region);
    assert_eq!(kept.len(), 39);
    assert!(!kept.iter().any(|r| r.ride_id == 17));
    save(&mut region, &record(41)).unwrap();
    assert_eq!(
        table(&mut region).len(),
        40,
        "the save drops the damaged slot"
    );
}

/// A table as a later firmware might write it: slots of `stride` bytes at
/// record version 2, with bytes this firmware does not know.
fn later_table(stride: usize, rides: &[u32]) -> Vec<u8> {
    let mut bytes = std::vec![0u8; stride * (rides.len() + 1)];
    bytes[..4].copy_from_slice(TABLE_MAGIC);
    bytes[4] = 2;
    put_u16(&mut bytes, 6, stride as u16);
    seal(&mut bytes[..stride]);
    for (index, ride) in rides.iter().enumerate() {
        let slot = &mut bytes[stride * (index + 1)..][..stride];
        encode_fields(&record(*ride), slot, SLOT_FIELDS);
        slot[0] = 2;
        if stride > 48 {
            slot[40..48].copy_from_slice(b"futurefd");
        }
        seal(slot);
    }
    bytes
}

#[test]
fn a_later_table_with_longer_slots_reads_and_keeps_their_bytes() {
    const STRIDE: usize = 64;
    let bytes = later_table(STRIDE, &[1, 2]);
    let mut region = formatted();
    put(&mut region, TABLE, &bytes);
    assert_eq!(table(&mut region), [record(1), record(2)]);
    save(&mut region, &record(3)).unwrap();
    let written = raw(&mut region, TABLE).unwrap();
    assert_eq!(written.len(), STRIDE * 4);
    assert_eq!(
        written[..STRIDE * 3],
        bytes[..],
        "header and slots as they were"
    );
    assert_eq!(table(&mut region), [record(1), record(2), record(3)]);
}

#[test]
fn a_damaged_header_finds_the_stride_its_slots_pass_at_or_leaves_the_table_alone() {
    for stride in [32, 64, 128, 256, 512] {
        for damage in [6, stride - 1] {
            let rides = [1, 2, 3];
            let mut bytes = later_table(stride, &rides);
            bytes[damage] ^= 0x10;
            let mut region = formatted();
            put(&mut region, TABLE, &bytes);
            let expected: Vec<Record> = rides.into_iter().map(record).collect();
            assert_eq!(
                table(&mut region),
                expected,
                "stride {stride} byte {damage}"
            );
            assert_eq!(decode_table(&bytes).collect::<Vec<_>>(), expected);
            save(&mut region, &record(4)).unwrap();
            let written = raw(&mut region, TABLE).unwrap();
            assert_eq!(stride_of(&written), Some(stride), "a fresh header");
            assert_eq!(
                written[stride..stride * 4],
                bytes[stride..],
                "stride {stride}: the slots as they were"
            );
            let mut kept = expected.clone();
            kept.push(record(4));
            assert_eq!(table(&mut region), kept, "stride {stride} byte {damage}");
        }

        let mut bytes = later_table(stride, &[1, 2, 3]);
        bytes[6] ^= 0x10;
        for ride in 1..=3 {
            bytes[stride * ride + 9] ^= 1;
        }
        let mut region = formatted();
        put(&mut region, TABLE, &bytes);
        assert_eq!(
            read(&mut region, 0, |_| {}),
            Err(Error::Table),
            "stride {stride}"
        );
        assert_eq!(save(&mut region, &record(4)), Err(Error::Table));
        assert_eq!(raw(&mut region, TABLE), Some(bytes), "left as it was");
    }
}

fn stride_of(bytes: &[u8]) -> Option<usize> {
    stride(bytes)
}

#[test]
fn a_read_visits_the_table_a_few_sectors_at_a_time() {
    let mut region = formatted();
    for ride in 1..=300 {
        save(&mut region, &record(ride)).unwrap();
    }
    let mut pieces = 0;
    let mut next = Some(0);
    let mut seen = 0;
    while let Some(index) = next {
        let mut visited = 0;
        let piece = read(&mut region, index, |_| visited += 1).unwrap().unwrap();
        assert!(visited <= PIECE * SECTOR_SIZE / SLOT);
        seen += visited;
        pieces += 1;
        next = piece.next;
    }
    assert_eq!(seen, 300);
    assert_eq!(pieces, (SLOT * 301).div_ceil(PIECE * SECTOR_SIZE));
    assert_eq!(read(&mut formatted(), 0, |_| {}), Ok(None), "no table yet");
}

fn shelf(records: usize) -> Shelf {
    Shelf::new(std::vec![0u8; records * SLOT].leak())
}

#[test]
fn the_shelf_finds_by_journal_ride_and_start_and_a_save_outranks_a_read() {
    let mut shelf = shelf(8);
    for ride in [5, 1, 3] {
        assert!(shelf.put(&record(ride), false));
    }
    assert!(shelf.put(&internal(2), false));
    let start = |ride| Some(record(ride).started_utc_ms);
    assert_eq!(shelf.find(b'b', 3, start(3)), Some(record(3).facts));
    assert_eq!(shelf.find(b'i', 2, start(2)), Some(internal(2).facts));
    assert_eq!(shelf.find(b'b', 2, start(2)), None);
    assert_eq!(shelf.find(b'b', 3, None), None, "undated");

    let mut measured = record(3);
    measured.facts.recovery_bpm = Some(28);
    assert!(shelf.put(&measured, true));
    assert!(!shelf.put(&record(3), false), "a read of the same start");
    assert_eq!(shelf.find(b'b', 3, start(3)), Some(measured.facts));
    let mut after_clear = record(3);
    after_clear.started_utc_ms += 86_400_000;
    assert!(shelf.put(&after_clear, false), "a read of a later start");
    assert_eq!(shelf.find(b'b', 3, start(3)), None);

    shelf.forget(b'b');
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf.find(b'i', 2, start(2)), Some(internal(2).facts));
    shelf.clear();
    assert!(shelf.is_empty());
}

#[test]
fn a_full_shelf_lets_the_oldest_start_go() {
    let mut shelf = shelf(4);
    for ride in [4, 2, 3, 5] {
        shelf.put(&record(ride), false);
    }
    assert!(!shelf.put(&record(1), false), "older than every kept ride");
    assert!(shelf.put(&record(6), false));
    let start = |ride| Some(record(ride).started_utc_ms);
    assert_eq!(shelf.find(b'b', 2, start(2)), None);
    assert!(shelf.put(&record(1), true), "a save is always kept");
    assert_eq!(shelf.find(b'b', 1, start(1)), Some(record(1).facts));
    assert_eq!(shelf.len(), 4);
    for ride in [1, 4, 5, 6] {
        assert!(shelf.find(b'b', ride, start(ride)).is_some(), "ride {ride}");
    }
}

#[test]
fn any_shelf_answers_as_a_plain_list_does() {
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let capacity = rng.range(1, 24) as usize;
        let mut shelf = shelf(capacity);
        let mut list: Vec<Record> = Vec::new();
        for _ in 0..rng.range(1, 80) {
            let mut drawn = record(rng.range(1, 30) as u32);
            drawn.journal = rng.pick(b"ib");
            drawn.started_utc_ms += rng.below(3) * 86_400_000;
            let saved = rng.one_in(3);
            shelf.put(&drawn, saved);
            match list.iter().position(|kept| kept.key() == drawn.key()) {
                Some(at) if saved || list[at].started_utc_ms < drawn.started_utc_ms => {
                    list[at] = drawn
                }
                Some(_) => {}
                None if list.len() < capacity => list.push(drawn),
                None => {
                    let oldest = (0..list.len())
                        .min_by_key(|&at| (list[at].started_utc_ms, list[at].key()))
                        .unwrap();
                    if saved || list[oldest].started_utc_ms < drawn.started_utc_ms {
                        list[oldest] = drawn;
                    }
                }
            }
            if rng.one_in(20) {
                let journal = rng.pick(b"ib");
                shelf.forget(journal);
                list.retain(|kept| kept.journal != journal);
            }
        }
        assert_eq!(shelf.len(), list.len(), "seed {seed}");
        for kept in &list {
            let found = shelf.find(kept.journal, kept.ride_id, Some(kept.started_utc_ms));
            assert_eq!(found, Some(kept.facts), "seed {seed}: {kept:?}");
        }
    }
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
