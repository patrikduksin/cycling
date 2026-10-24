use super::*;
use crate::testing::{Media, Rng, Tear};
use std::collections::BTreeMap;
use std::string::{String, ToString};
use std::vec::Vec;

type Model = BTreeMap<String, Vec<u8>>;

fn sector() -> Sector {
    [0; SECTOR_SIZE]
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

fn content(length: usize, rng: &mut Rng) -> Vec<u8> {
    (0..length).map(|_| rng.next() as u8).collect()
}

/// Everything the store holds, every file verified against its CRC.
fn snapshot(media: &mut Media) -> Result<Model, Error> {
    let mut s = sector();
    let dir = mount(media, &mut s)?;
    let mut entries = Vec::new();
    dir.list(media, &mut s, |e| entries.push(*e))?;
    assert_eq!(entries.len(), dir.len());
    let mut model = Model::new();
    for entry in entries {
        dir.verify(media, &entry, &mut s)?;
        let mut bytes = std::vec![0u8; entry.length as usize];
        assert_eq!(dir.read(media, &entry, 0, &mut bytes, &mut s)?, bytes.len());
        assert_eq!(crc32(&bytes), entry.crc);
        assert!(
            model
                .insert(entry.name.as_str().to_string(), bytes)
                .is_none()
        );
    }
    Ok(model)
}

#[test]
fn names_are_short_plain_paths() {
    for good in ["a", "workouts/ftp-test.json", "A.B_C-9", &"x".repeat(40)] {
        assert_eq!(Name::new(good).unwrap().as_str(), good);
    }
    for bad in ["", "a b", "ü", "a\0", &"x".repeat(41), "*"] {
        assert_eq!(Name::new(bad), Err(Error::Name), "{bad:?}");
    }
}

#[test]
fn files_are_written_listed_read_replaced_and_deleted() {
    let mut media = Media::new(DATA + 4096);
    let mut s = sector();
    assert_eq!(mount(&mut media, &mut s), Err(Error::Uninitialized));
    format(&mut media, 1, &mut s).unwrap();
    let mut rng = Rng(2);
    let first = content(1300, &mut rng);
    let entry = write(&mut media, &name("w/one.json"), &first, &mut s).unwrap();
    assert_eq!((entry.length, entry.crc), (1300, crc32(&first)));
    write(&mut media, &name("empty"), &[], &mut s).unwrap();
    let second = content(512, &mut rng);
    write(&mut media, &name("two"), &second, &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    assert_eq!(dir.len(), 3);
    let found = dir
        .find(&mut media, &name("w/one.json"), &mut s)
        .unwrap()
        .unwrap();
    let mut part = [0u8; 100];
    assert_eq!(
        dir.read(&mut media, &found, 1250, &mut part, &mut s),
        Ok(50)
    );
    assert_eq!(part[..50], first[1250..]);
    assert_eq!(
        dir.read(&mut media, &found, 1301, &mut part, &mut s),
        Err(Error::Range)
    );
    // Replace keeps one entry of that name with the new content.
    let replaced = content(9000, &mut rng);
    write(&mut media, &name("w/one.json"), &replaced, &mut s).unwrap();
    delete(&mut media, &name("two"), &mut s).unwrap();
    assert_eq!(
        delete(&mut media, &name("two"), &mut s),
        Err(Error::NotFound)
    );
    let model = snapshot(&mut media).unwrap();
    assert_eq!(model.len(), 2);
    assert_eq!(model["w/one.json"], replaced);
    assert_eq!(model["empty"], Vec::<u8>::new());
}

#[test]
fn limits_are_enforced_before_anything_is_written() {
    let mut media = Media::new(DATA + 64);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    assert_eq!(
        dir.begin(&mut media, &name("big"), MAX_FILE + 1, &mut s)
            .err(),
        Some(Error::TooLarge)
    );
    let writes = media.writes;
    assert_eq!(
        dir.begin(&mut media, &name("big"), 65 * 512, &mut s).err(),
        Some(Error::Full)
    );
    let mut writer = dir.begin(&mut media, &name("a"), 1000, &mut s).unwrap();
    assert_eq!(media.writes, writes);
    // Partial sectors only end a file; nothing past its length.
    assert_eq!(writer.write(&mut media, &[1; 100]), Err(Error::Range));
    assert_eq!(writer.write(&mut media, &[1; 1024]), Err(Error::Range));
    writer.write(&mut media, &[1; 512]).unwrap();
    assert_eq!(
        writer.commit(&mut media, 0, &mut s).err(),
        Some(Error::Range)
    );
    writer.write(&mut media, &[1; 488]).unwrap();
    assert_eq!(
        writer.commit(&mut media, 0, &mut s).err(),
        Some(Error::Mismatch)
    );
    writer
        .commit(&mut media, crc32(&[1; 1000]), &mut s)
        .unwrap();
    // Every entry used: a new name is refused, replacing still works.
    let mut media = Media::new(DATA + 4096);
    format(&mut media, 1, &mut s).unwrap();
    for i in 0..CAPACITY {
        write(&mut media, &name(&std::format!("f{i}")), &[i as u8], &mut s).unwrap();
    }
    assert_eq!(
        write(&mut media, &name("more"), &[1], &mut s).err(),
        Some(Error::Full)
    );
    write(&mut media, &name("f7"), &[1, 2, 3], &mut s).unwrap();
    assert_eq!(snapshot(&mut media).unwrap().len(), CAPACITY);
}

#[test]
fn a_writer_whose_directory_changed_commits_nothing() {
    let mut media = Media::new(DATA + 256);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    let mut writer = dir.begin(&mut media, &name("a"), 4, &mut s).unwrap();
    writer.write(&mut media, b"aaaa").unwrap();
    // The host writes another file meanwhile, possibly into the same sectors.
    write(&mut media, &name("b"), b"bbbb", &mut s).unwrap();
    assert_eq!(
        writer.commit(&mut media, crc32(b"aaaa"), &mut s).err(),
        Some(Error::Stale)
    );
    let model = snapshot(&mut media).unwrap();
    assert_eq!(model.keys().collect::<Vec<_>>(), ["b"]);
    assert_eq!(model["b"], b"bbbb");
}

#[test]
fn a_delete_through_a_stale_directory_never_drops_a_newer_file() {
    let mut media = Media::new(DATA + 256);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    write(&mut media, &name("a"), b"aaaa", &mut s).unwrap();
    let cached = mount(&mut media, &mut s).unwrap();
    write(&mut media, &name("b"), b"bbbb", &mut s).unwrap();
    let writes = media.writes;
    assert_eq!(
        cached.delete(&mut media, &name("a"), &mut s).err(),
        Some(Error::Stale)
    );
    assert_eq!(media.writes, writes);
    let model = snapshot(&mut media).unwrap();
    assert_eq!(
        (model["a"].as_slice(), model["b"].as_slice()),
        (&b"aaaa"[..], &b"bbbb"[..])
    );
}

#[test]
fn a_writer_whose_staged_bytes_were_overwritten_never_publishes_them() {
    let mut media = Media::new(DATA + 256);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    // Both writers get the same free sectors; B stages over A before A commits.
    let mut a = dir.begin(&mut media, &name("a"), 4, &mut s).unwrap();
    let mut b = dir.begin(&mut media, &name("b"), 4, &mut s).unwrap();
    a.write(&mut media, b"aaaa").unwrap();
    b.write(&mut media, b"bbbb").unwrap();
    assert_eq!(
        a.commit(&mut media, crc32(b"aaaa"), &mut s).err(),
        Some(Error::Mismatch)
    );
    b.commit(&mut media, crc32(b"bbbb"), &mut s).unwrap();
    let model = snapshot(&mut media).unwrap();
    assert!(!model.contains_key("a"));
    assert_eq!(model["b"], b"bbbb");
}

#[test]
fn a_stale_writer_never_writes_over_a_file_committed_meanwhile() {
    let mut media = Media::new(DATA + 256);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    // Both writers get the same free sectors.
    let mut writer = dir.begin(&mut media, &name("a"), 600, &mut s).unwrap();
    writer.write(&mut media, &[b'a'; 512]).unwrap();
    write(&mut media, &name("b"), &[b'b'; 1000], &mut s).unwrap();
    let writes = media.writes;
    assert_eq!(
        writer.write(&mut media, &[b'a'; 88]).err(),
        Some(Error::Stale)
    );
    assert_eq!(media.writes, writes);
    assert_eq!(snapshot(&mut media).unwrap()["b"], [b'b'; 1000]);
}

#[test]
fn allocation_reuses_freed_space_and_never_overlaps_live_files() {
    let mut media = Media::new(DATA + 40);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    let mut rng = Rng(3);
    let mut model = Model::new();
    for round in 0..300 {
        let key = std::format!("f{}", rng.below(6));
        if rng.below(4) == 0 {
            let _ = delete(&mut media, &name(&key), &mut s);
            model.remove(&key);
        } else {
            let bytes = content(rng.below(12 * 512) as usize, &mut rng);
            match write(&mut media, &name(&key), &bytes, &mut s) {
                Ok(_) => {
                    model.insert(key, bytes);
                }
                Err(Error::Full) => {}
                Err(e) => panic!("round {round}: {e:?}"),
            }
        }
        assert_eq!(snapshot(&mut media).unwrap(), model, "round {round}");
    }
}

#[test]
fn damaged_entries_are_corrupt_not_silently_lost() {
    let mut media = Media::new(DATA + 64);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    write(&mut media, &name("a"), b"x", &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    media.set(base(dir.copy) + 1, [0x5a; SECTOR_SIZE]);
    assert_eq!(snapshot(&mut media), Err(Error::Corrupt));
    // An explicit format starts over.
    format(&mut media, 2, &mut s).unwrap();
    assert!(snapshot(&mut media).unwrap().is_empty());
}

#[test]
fn foreign_directories_are_reported() {
    let mut media = Media::new(DATA + 64);
    let mut s = sector();
    let dir = format(&mut media, 1, &mut s).unwrap();
    let mut header = media.get(base(dir.copy));
    header[16] = 2;
    let checksum = crc32(&header[..CRC]);
    header[CRC..].copy_from_slice(&checksum.to_le_bytes());
    media.set(base(dir.copy), header);
    assert_eq!(mount(&mut media, &mut s), Err(Error::Foreign));
    let before = media.writes;
    assert_eq!(format(&mut media, 2, &mut s), Err(Error::Foreign));
    assert_eq!(media.writes, before);
    assert_eq!(mount(&mut media, &mut s), Err(Error::Foreign));
}

enum Op {
    Write(String, Vec<u8>),
    Delete(String),
    Format,
}

fn apply(media: &mut Media, op: &Op, seed: u64) -> Result<(), Error> {
    let mut s = sector();
    match op {
        Op::Write(key, bytes) => write(media, &name(key), bytes, &mut s).map(|_| ()),
        Op::Delete(key) => delete(media, &name(key), &mut s),
        Op::Format => format(media, seed, &mut s).map(|_| ()),
    }
}

fn expect(model: &Model, op: &Op) -> Model {
    let mut after = model.clone();
    match op {
        Op::Write(key, bytes) => {
            after.insert(key.clone(), bytes.clone());
        }
        Op::Delete(key) => {
            after.remove(key);
        }
        Op::Format => after.clear(),
    }
    after
}

fn random_op(rng: &mut Rng) -> Op {
    let key = std::format!("w/{}", rng.below(5));
    match rng.below(10) {
        0 => Op::Format,
        1..=3 => Op::Delete(key),
        _ => {
            let length = match rng.below(4) {
                0 => 0,
                1 => rng.below(512) as usize,
                _ => rng.below(20 * 512) as usize,
            };
            Op::Write(key, content(length, rng))
        }
    }
}

#[test]
fn every_power_cut_leaves_the_store_before_or_after_the_interrupted_change() {
    for case in 0..400u64 {
        let mut rng = Rng((case << 32) | 0x5eed);
        let mut media = Media::new(DATA + 120);
        let mut s = sector();
        format(&mut media, case, &mut s).unwrap();
        let mut model = Model::new();
        // Some history, without cuts.
        for _ in 0..rng.below(8) {
            let op = random_op(&mut rng);
            if apply(&mut media, &op, rng.next()).is_ok() {
                model = expect(&model, &op);
            }
        }
        assert_eq!(snapshot(&mut media).unwrap(), model);
        // Measure the change, then repeat it with power cut at one of its writes.
        let op = random_op(&mut rng);
        let seed = rng.next();
        let before = media.written.clone();
        let writes = media.writes;
        let outcome = apply(&mut media, &op, seed);
        let total = media.writes - writes;
        media.written = before;
        if outcome.is_err() || total == 0 {
            continue;
        }
        let at = rng.below(total);
        let tear = Tear::pick(&mut rng);
        media.cut = Some((at, tear));
        assert!(apply(&mut media, &op, seed).is_err());
        media.reboot();
        let after = expect(&model, &op);
        let found = snapshot(&mut media).unwrap_or_else(|e| panic!("case {case}: {e:?}"));
        assert!(
            found == model || found == after,
            "case {case} cut at {at}/{total} {tear:?}"
        );
        // The store stays usable after the cut.
        let mut model = found;
        for _ in 0..3 {
            let op = random_op(&mut rng);
            if apply(&mut media, &op, rng.next()).is_ok() {
                model = expect(&model, &op);
            }
            assert_eq!(snapshot(&mut media).unwrap(), model, "case {case} after");
        }
    }
}

#[test]
fn operations_read_and_write_a_bounded_number_of_sectors() {
    let mut media = Media::new(DATA + 4096);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    assert_eq!(media.writes, 1 + DIR_SECTORS as u64);
    for i in 0..20 {
        write(&mut media, &name(&std::format!("f{i}")), &[1; 600], &mut s).unwrap();
    }
    let (reads, writes) = (media.reads, media.writes);
    write(&mut media, &name("f3"), &[2; 600], &mut s).unwrap();
    // Two data sectors, at most two changed entry sectors and the header.
    assert!(media.writes - writes <= 5, "{}", media.writes - writes);
    assert!(
        // ... plus a freshness remount and the staged data read back.
        media.reads - reads <= 2 + 16 + 2 + 2 + 3 * 16 + 1 + 2 + 2,
        "{}",
        media.reads - reads
    );
}

#[test]
fn a_delete_repairs_a_header_count_below_its_entries() {
    let mut media = Media::new(DATA + 64);
    let mut s = sector();
    format(&mut media, 1, &mut s).unwrap();
    write(&mut media, &name("a"), b"x", &mut s).unwrap();
    let dir = mount(&mut media, &mut s).unwrap();
    let mut header = media.get(base(dir.copy));
    header[18..20].copy_from_slice(&0u16.to_le_bytes());
    let checksum = crc32(&header[..CRC]);
    header[CRC..].copy_from_slice(&checksum.to_le_bytes());
    media.set(base(dir.copy), header);
    assert_eq!(delete(&mut media, &name("a"), &mut s), Ok(()));
    assert!(snapshot(&mut media).unwrap().is_empty());
}
