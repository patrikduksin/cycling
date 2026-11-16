use super::*;
use crate::ride::Contents;
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

fn media(seed: u64) -> Media {
    let mut media = Media::new(8_000_000);
    let mut sector = [0; SECTOR_SIZE];
    volume::create(&mut media, &mut sector).unwrap();
    let mut store = volume::open(&mut media, &volume::FILES, &mut sector).unwrap();
    files::format(&mut store, seed, &mut sector).unwrap();
    media
}

#[test]
fn any_slot_round_trips_and_every_tear_or_flipped_bit_is_refused() {
    for seed in 0..2_000 {
        let mut rng = Rng::new(seed);
        let record = any(&mut rng);
        let mut slot = [0; SLOT];
        encode_slot(&record, &mut slot);
        assert_eq!(decode_slot(&slot), Some(record), "seed {seed}");
        for torn in 0..SLOT {
            assert_eq!(
                decode_slot(&slot[..torn]),
                None,
                "seed {seed} torn at {torn}"
            );
        }
        let bit = rng.below(8 * SLOT as u64) as usize;
        slot[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(decode_slot(&slot), None, "seed {seed} slot bit {bit}");
    }
}

#[test]
fn a_slot_of_another_record_version_reads_as_damaged() {
    let mut slot = [0; SLOT];
    encode_slot(&record(1), &mut slot);
    slot[0] = 3;
    seal(&mut slot);
    assert_eq!(decode_slot(&slot), None);
}

#[test]
fn hostile_bytes_never_panic_and_a_profile_out_of_range_reads_as_none() {
    let mut decoded = 0;
    for seed in 0..5_000 {
        let mut rng = Rng::new(seed);
        let mut slot = [0; SLOT];
        encode_slot(&any(&mut rng), &mut slot);
        let mut bytes = rng.mutate(slot.to_vec());
        if bytes.len() == SLOT && !rng.one_in(4) {
            seal(&mut bytes);
        }
        if let Some(record) = decode_slot(&bytes) {
            decoded += 1;
            encode_slot(&record, &mut slot);
            assert_eq!(decode_slot(&slot), Some(record), "seed {seed}");
        }
        let _ = decode_table(&bytes).count();
    }
    assert!(decoded > 100, "{decoded} decoded");
    let mut bytes = [0; SLOT];
    encode_slot(&record(1), &mut bytes);
    put_u16(&mut bytes, FIELDS + 14, 0);
    seal(&mut bytes);
    let read = decode_slot(&bytes).expect("the other fields survive");
    assert_eq!(read.facts.profile, None, "FTP 0 is no profile");
    assert_eq!(read.facts.climbing, record(1).facts.climbing);
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
fn the_digest_tells_apart_tables_of_one_length_that_the_file_crc_cannot() {
    let mut region = formatted();
    assert_eq!(digest(&mut region).unwrap(), None, "no table");
    for ride in 1..=3 {
        save(&mut region, &record(ride)).unwrap();
    }
    let before = raw(&mut region, TABLE).unwrap();
    assert_eq!(digest(&mut region).unwrap(), table_digest(&before));
    let mut again = record(1);
    again.facts.recovery_bpm = Some(31);
    save(&mut region, &again).unwrap();
    let after = raw(&mut region, TABLE).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(
        crc32(&after),
        crc32(&before),
        "each slot ends in its own CRC32"
    );
    assert_eq!(digest(&mut region).unwrap(), table_digest(&after));
    assert_ne!(table_digest(&after), table_digest(&before));
    let mut moved = before.clone();
    moved[SLOT..].rotate_left(SLOT);
    assert_ne!(table_digest(&moved), table_digest(&before), "order counts");
}

#[test]
fn a_clear_forgets_its_journal_and_nothing_else() {
    let mut region = formatted();
    put(&mut region, "rides/b1.workout", b"a workout record");
    for ride in 1..=20 {
        save(&mut region, &record(ride)).unwrap();
        save(&mut region, &internal(ride)).unwrap();
    }
    forget(&mut region, b'b').unwrap();
    let expected: Vec<Record> = (1..=20).map(internal).collect();
    assert_eq!(table(&mut region), expected);
    assert_eq!(names(&mut region), ["rides/b1.workout", TABLE]);
    let before = raw(&mut region, TABLE);
    forget(&mut region, b'b').unwrap();
    assert_eq!(raw(&mut region, TABLE), before, "nothing left to forget");
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
fn the_table_gives_way_on_a_full_directory() {
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
        "the last entry stays free"
    );
    assert_eq!(names(&mut full).len(), files::CAPACITY - 1);
}

#[test]
fn a_damaged_header_keeps_the_rides_and_a_foreign_table_is_left_alone() {
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
        if at < 5 || (6..8).contains(&at) {
            assert_eq!(
                read(&mut region, 0, |_| {}),
                Err(Error::Table),
                "seed {seed}"
            );
            assert_eq!(save(&mut region, &record(4)), Err(Error::Table));
            assert_eq!(forget(&mut region, b'b'), Err(Error::Table));
            assert_eq!(raw(&mut region, TABLE), Some(damaged), "seed {seed}");
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
        assert!(table_sound(&written), "seed {seed}: a fresh header");
        let expected: Vec<Record> = (1..=4).map(record).collect();
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

#[test]
fn a_table_holding_a_slot_of_another_record_version_is_left_alone() {
    let mut region = formatted();
    for ride in 1..=3 {
        save(&mut region, &record(ride)).unwrap();
    }
    let mut foreign = raw(&mut region, TABLE).unwrap();
    assert_eq!(contents(&mut region), Ok(Contents::Current));
    foreign[SLOT * 2] = VERSION + 1;
    seal(&mut foreign[SLOT * 2..SLOT * 3]);
    put(&mut region, TABLE, &foreign);
    assert_eq!(contents(&mut region), Ok(Contents::Foreign));
    assert_eq!(save(&mut region, &record(4)), Err(Error::Table));
    assert_eq!(forget(&mut region, b'b'), Err(Error::Table));
    assert_eq!(raw(&mut region, TABLE), Some(foreign), "left as it was");
}

#[test]
fn a_table_of_another_version_or_length_is_left_alone() {
    let mut region = formatted();
    for ride in 1..=3 {
        save(&mut region, &record(ride)).unwrap();
    }
    let current = raw(&mut region, TABLE).unwrap();
    let mut later = current.clone();
    later[4] = 3;
    seal(&mut later[..SLOT]);
    let mut longer = current.clone();
    longer.extend_from_slice(&[0; SLOT / 2]);
    let mut wider = std::vec![0u8; 64 * 4];
    for (slot, wide) in current
        .as_chunks::<SLOT>()
        .0
        .iter()
        .zip(wider.as_chunks_mut::<64>().0)
    {
        wide[..SLOT].copy_from_slice(slot);
    }
    put_u16(&mut wider, 6, 64);
    seal(&mut wider[..64]);
    for (foreign, held) in [
        (later, Contents::Foreign),
        (longer, Contents::Damaged),
        (wider, Contents::Damaged),
    ] {
        put(&mut region, TABLE, &foreign);
        assert_eq!(contents(&mut region), Ok(held));
        assert_eq!(read(&mut region, 0, |_| {}), Err(Error::Table));
        assert_eq!(decode_table(&foreign).count(), 0);
        assert_eq!(save(&mut region, &record(4)), Err(Error::Table));
        assert_eq!(forget(&mut region, b'b'), Err(Error::Table));
        assert_eq!(raw(&mut region, TABLE), Some(foreign), "left as it was");
    }
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
