use super::*;
use crate::testing::{Media, Rng, Tear};
use std::collections::{BTreeMap, BTreeSet};
use std::string::{String, ToString};
use std::vec::Vec;

fn sector() -> Sector {
    [0; SECTOR_SIZE]
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

fn dir(text: &str) -> Dir {
    Dir::new(text).unwrap()
}

fn formatted(chunks: u64, chunk: u32) -> Media {
    let mut media = Media::new(DATA + chunks * u64::from(chunk));
    format_chunks(&mut media, 7, chunk, &mut sector()).unwrap();
    media
}

fn bytes(length: usize, rng: &mut Rng) -> Vec<u8> {
    (0..length).map(|_| rng.byte()).collect()
}

fn snapshot(media: &mut impl ReadWrite) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    let mut boot = Boot::mount(media)?;
    let mut files = BTreeMap::new();
    for n in boot.names()? {
        let bytes = boot.read(&n)?;
        files.insert(n, bytes);
    }
    Ok(files)
}

struct Boot<'m, M> {
    media: &'m mut M,
    store: Store,
    open: Option<(String, Appender)>,
    sector: Sector,
}

impl<'m, M: ReadWrite> Boot<'m, M> {
    fn mount(media: &'m mut M) -> Result<Self, Error> {
        let mut sector = sector();
        let store = mount(media, &mut sector)?;
        Ok(Self {
            media,
            store,
            open: None,
            sector,
        })
    }

    fn create(&mut self, text: &str) -> Result<(), Error> {
        self.close()?;
        let appender =
            self.store
                .create(self.media, &name(text), Some(dir(TRACES)), &mut self.sector)?;
        self.open = Some((text.to_string(), appender));
        Ok(())
    }

    fn reopen(&mut self, text: &str) -> Result<(), Error> {
        self.close()?;
        let appender =
            self.store
                .open_append(self.media, &name(text), Some(dir(TRACES)), &mut self.sector)?;
        self.open = Some((text.to_string(), appender));
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let (_, file) = self.open.as_mut().expect("an open file");
        file.append(&mut self.store, self.media, bytes, &mut self.sector)
    }

    fn sync(&mut self) -> Result<(), Error> {
        let (_, file) = self.open.as_mut().expect("an open file");
        file.sync(&mut self.store, self.media, &mut self.sector)
    }

    fn close(&mut self) -> Result<(), Error> {
        if self.open.is_some() {
            self.sync()?;
        }
        self.open = None;
        Ok(())
    }

    fn delete(&mut self, text: &str) -> Result<(), Error> {
        self.store.delete(self.media, &name(text), &mut self.sector)
    }

    fn names(&mut self) -> Result<Vec<String>, Error> {
        let mut names = Vec::new();
        self.store
            .list(self.media, &Dir::ROOT, &mut self.sector, |e| {
                names.push(e.name.as_str().to_string())
            })?;
        names.sort();
        Ok(names)
    }

    fn read(&mut self, text: &str) -> Result<Vec<u8>, Error> {
        let mut reader = self
            .store
            .reader(self.media, &name(text), &mut self.sector)?;
        let mut out = [0u8; PAYLOAD];
        let mut bytes = Vec::new();
        loop {
            match reader.next(&self.store, self.media, &mut out, &mut self.sector)? {
                Next::Record(n) => bytes.extend_from_slice(&out[..n]),
                Next::Hole { .. } => return Err(Error::Corrupt),
                Next::End => return Ok(bytes),
            }
        }
    }
}

fn events(media: &mut Media, text: &str) -> Vec<String> {
    let mut s = sector();
    let store = mount(media, &mut s).unwrap();
    let mut reader = store.reader(media, &name(text), &mut s).unwrap();
    let mut out = [0u8; PAYLOAD];
    let mut found = Vec::new();
    loop {
        match reader.next(&store, media, &mut out, &mut s).unwrap() {
            Next::Record(n) => found.push(std::format!("{:02x}x{n}", out[0])),
            Next::Hole { from, to } => found.push(std::format!("hole {from}..{to}")),
            Next::End => return found,
        }
    }
}

#[test]
fn names_and_directories_are_plain_paths() {
    for good in ["a", "traces/0001", "A.B_C-9/x/y", &"x".repeat(40)] {
        assert_eq!(Name::new(good).unwrap().as_str(), good);
    }
    for bad in [
        "",
        "a b",
        "ü",
        "a\0",
        &"x".repeat(41),
        "/a",
        "a/",
        "a//b",
        "*",
    ] {
        assert_eq!(Name::new(bad), Err(Error::Name), "{bad:?}");
    }
    for good in ["", "traces/", "a/b/"] {
        assert_eq!(Dir::new(good).unwrap().as_str(), good);
    }
    for bad in ["/", "traces", "a//", "/a/", " /"] {
        assert_eq!(Dir::new(bad), Err(Error::Name), "{bad:?}");
    }
    assert!(name("traces/0001").is_in(&dir("traces/")));
    assert!(name("traces/old/1").is_in(&dir("traces/")));
    assert!(!name("traces2/0001").is_in(&dir("traces/")));
    assert!(name("x").is_in(&Dir::ROOT));
}

#[test]
fn files_grow_across_chunks_and_read_back_after_a_reboot() {
    let mut media = formatted(16, 4);
    let mut s = sector();
    let mut rng = Rng::new(1);
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store
        .create(&mut media, &name("traces/a"), None, &mut s)
        .unwrap();
    let mut expected = Vec::new();
    for round in 0..12 {
        let more = bytes(rng.range(1, 900) as usize, &mut rng);
        file.append(&mut store, &mut media, &more, &mut s).unwrap();
        expected.extend_from_slice(&more);
        if round % 4 == 3 {
            file.sync(&mut store, &mut media, &mut s).unwrap();
        }
    }
    file.sync(&mut store, &mut media, &mut s).unwrap();
    let mut entries = Vec::new();
    store
        .list(&mut media, &Dir::ROOT, &mut s, |e| entries.push(*e))
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].chunks >= 3, "{entries:?}");
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store
        .open_append(&mut media, &name("traces/a"), None, &mut s)
        .unwrap();
    file.append(&mut store, &mut media, b"after reboot", &mut s)
        .unwrap();
    file.sync(&mut store, &mut media, &mut s).unwrap();
    expected.extend_from_slice(b"after reboot");
    let found = snapshot(&mut media).unwrap();
    assert_eq!(found["traces/a"], expected);
    let store = mount(&mut media, &mut s).unwrap();
    let mut reader = store.reader(&mut media, &name("traces/a"), &mut s).unwrap();
    let mut out = [0u8; PAYLOAD];
    let mut records = Vec::new();
    loop {
        let number = reader.record(&store);
        match reader.next(&store, &mut media, &mut out, &mut s).unwrap() {
            Next::Record(n) => records.push((number, out[..n].to_vec())),
            next => {
                assert_eq!(next, Next::End);
                break;
            }
        }
    }
    let (end, last) = (reader.record(&store), records[records.len() - 1].0);
    assert!(end > last, "{end} {last}");
    for (number, bytes) in [&records[5], &records[1], &records[records.len() - 1]] {
        reader.seek(&store, &mut media, *number, &mut s).unwrap();
        assert_eq!(
            reader.next(&store, &mut media, &mut out, &mut s),
            Ok(Next::Record(bytes.len()))
        );
        assert_eq!(out[..bytes.len()], bytes[..]);
    }
    for past in [end, 1000] {
        reader.seek(&store, &mut media, past, &mut s).unwrap();
        assert_eq!(
            reader.next(&store, &mut media, &mut out, &mut s),
            Ok(Next::End)
        );
    }
}

#[test]
fn a_full_last_chunk_ends_the_file_and_the_next_record_links_a_chunk() {
    let mut media = formatted(4, 2);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store.create(&mut media, &name("a"), None, &mut s).unwrap();
    file.append(&mut store, &mut media, &[1; 2 * PAYLOAD], &mut s)
        .unwrap();
    assert_eq!(snapshot(&mut media).unwrap()["a"], [1; 2 * PAYLOAD]);
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store
        .open_append(&mut media, &name("a"), None, &mut s)
        .unwrap();
    file.append(&mut store, &mut media, &[2; 3], &mut s)
        .unwrap();
    file.sync(&mut store, &mut media, &mut s).unwrap();
    let mut expected = std::vec![1; 2 * PAYLOAD];
    expected.extend_from_slice(&[2; 3]);
    assert_eq!(snapshot(&mut media).unwrap()["a"], expected);
}

#[test]
fn rollover_deletes_the_oldest_closed_file_in_its_directory_only() {
    let mut media = formatted(6, 2);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    let traces = Some(dir("traces/"));
    let record = [9u8; PAYLOAD];
    let mut other = store
        .create(&mut media, &name("other/settings"), None, &mut s)
        .unwrap();
    for _ in 0..3 {
        other
            .append(&mut store, &mut media, &record, &mut s)
            .unwrap();
    }
    for t in ["traces/1", "traces/2"] {
        let mut file = store.create(&mut media, &name(t), traces, &mut s).unwrap();
        file.append(&mut store, &mut media, &record, &mut s)
            .unwrap();
    }
    let mut open = store
        .create(&mut media, &name("traces/3"), traces, &mut s)
        .unwrap();
    for _ in 0..8 {
        open.append(&mut store, &mut media, &record, &mut s)
            .unwrap();
    }
    let found = snapshot(&mut media).unwrap();
    assert_eq!(
        found.keys().collect::<Vec<_>>(),
        ["other/settings", "traces/3"]
    );
    assert_eq!(found["traces/3"].len(), 8 * PAYLOAD);
    assert_eq!(
        open.append(&mut store, &mut media, &record, &mut s),
        Err(Error::Full)
    );
    assert_eq!(
        store.delete_oldest(&mut media, &dir("traces/"), &mut s),
        Ok(None)
    );
    assert_eq!(
        store
            .create(&mut media, &name("traces/4"), None, &mut s)
            .err(),
        Some(Error::Full)
    );
    let mut next = store
        .create(&mut media, &name("traces/4"), traces, &mut s)
        .unwrap();
    assert_eq!(
        open.append(&mut store, &mut media, &record, &mut s),
        Err(Error::Stale)
    );
    next.append(&mut store, &mut media, b"new", &mut s).unwrap();
    next.sync(&mut store, &mut media, &mut s).unwrap();
    let found = snapshot(&mut media).unwrap();
    assert_eq!(
        found.keys().collect::<Vec<_>>(),
        ["other/settings", "traces/4"]
    );
    assert_eq!(found["traces/4"], b"new");
}

#[test]
fn every_entry_used_rolls_over_too() {
    let mut media = formatted(CAPACITY as u64 + 4, 1);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    for i in 0..CAPACITY {
        store
            .create(
                &mut media,
                &name(&std::format!("traces/{i:03}")),
                None,
                &mut s,
            )
            .unwrap();
    }
    assert_eq!(
        store
            .create(&mut media, &name("traces/new"), None, &mut s)
            .err(),
        Some(Error::Full)
    );
    store
        .create(
            &mut media,
            &name("traces/new"),
            Some(dir("traces/")),
            &mut s,
        )
        .unwrap();
    let names = snapshot(&mut media).unwrap();
    assert_eq!(names.len(), CAPACITY);
    assert!(!names.contains_key("traces/000") && names.contains_key("traces/new"));
}

#[test]
fn names_are_unique_and_missing_files_are_reported() {
    let mut media = formatted(8, 2);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    store.create(&mut media, &name("a"), None, &mut s).unwrap();
    let writes = media.writes;
    assert_eq!(
        store.create(&mut media, &name("a"), None, &mut s).err(),
        Some(Error::Exists)
    );
    assert_eq!(media.writes, writes);
    let missing = name("b");
    assert_eq!(
        store.delete(&mut media, &missing, &mut s),
        Err(Error::NotFound)
    );
    assert_eq!(
        store.reader(&mut media, &missing, &mut s).err(),
        Some(Error::NotFound)
    );
    assert_eq!(
        store.open_append(&mut media, &missing, None, &mut s).err(),
        Some(Error::NotFound)
    );
}

#[test]
fn a_store_changed_by_another_writer_is_stale_until_mounted_again() {
    let mut media = formatted(8, 2);
    let mut s = sector();
    let mut app = mount(&mut media, &mut s).unwrap();
    for n in ["traces/1", "traces/2"] {
        app.create(&mut media, &name(n), None, &mut s).unwrap();
    }
    let mut trace = app
        .open_append(&mut media, &name("traces/2"), None, &mut s)
        .unwrap();
    trace
        .append(&mut app, &mut media, b"before", &mut s)
        .unwrap();
    trace.sync(&mut app, &mut media, &mut s).unwrap();

    let mut host = mount(&mut media, &mut s).unwrap();
    host.delete(&mut media, &name("traces/1"), &mut s).unwrap();
    let writes = media.writes;
    assert_eq!(
        app.create(&mut media, &name("traces/3"), None, &mut s)
            .err(),
        Some(Error::Stale)
    );
    assert_eq!(media.writes, writes, "a stale store writes nothing");

    let mut fresh = mount(&mut media, &mut s).unwrap();
    assert_ne!(fresh.generation(), app.generation());
    assert_eq!(
        trace.append(&mut fresh, &mut media, b"x", &mut s),
        Err(Error::Stale)
    );
    let mut trace = fresh
        .open_append(&mut media, &name("traces/2"), None, &mut s)
        .unwrap();
    trace
        .append(&mut fresh, &mut media, b" after", &mut s)
        .unwrap();
    trace.sync(&mut fresh, &mut media, &mut s).unwrap();
    assert_eq!(snapshot(&mut media).unwrap()["traces/2"], b"before after");

    let mut host = mount(&mut media, &mut s).unwrap();
    host.delete(&mut media, &name("traces/2"), &mut s).unwrap();
    let mut fresh = mount(&mut media, &mut s).unwrap();
    assert_eq!(
        fresh
            .open_append(&mut media, &name("traces/2"), None, &mut s)
            .err(),
        Some(Error::NotFound)
    );
    fresh.create(&mut media, &name("b"), None, &mut s).unwrap();
    let mut reader = fresh.reader(&mut media, &name("b"), &mut s).unwrap();
    fresh.delete(&mut media, &name("b"), &mut s).unwrap();
    assert_eq!(
        reader.next(&fresh, &mut media, &mut [0; PAYLOAD], &mut s),
        Err(Error::Stale)
    );
}

#[test]
fn a_commit_another_writer_left_unfinished_does_not_vouch_for_the_next() {
    let mut failures = 0;
    for fail in 0..(BODY + 2) as u64 {
        for tear in [Tear::Old, Tear::Prefix(100), Tear::New] {
            let label = std::format!("host delete failed at write {fail} {tear:?}");
            let mut media = formatted(16, 2);
            let mut s = sector();
            let mut app = mount(&mut media, &mut s).unwrap();
            for i in 0..=PER_SECTOR {
                app.create(&mut media, &name(&std::format!("f{i}")), None, &mut s)
                    .unwrap();
            }
            let mut host = mount(&mut media, &mut s).unwrap();
            media.cut = Some((fail, tear));
            let deleted = host.delete(&mut media, &name("f0"), &mut s);
            media.reboot();
            if deleted.is_ok() {
                continue;
            }
            failures += 1;
            let created = app.create(&mut media, &name("new"), None, &mut s);
            let files = snapshot(&mut media).unwrap_or_else(|e| panic!("{label}: {e:?}"));
            if created.is_ok() {
                assert!(
                    files.contains_key("f0") && files.contains_key("new"),
                    "{label}: {:?}",
                    files.keys()
                );
            } else {
                assert_eq!(created.err(), Some(Error::Stale), "{label}");
            }
        }
    }
    assert!(failures >= 3 * 3, "{failures}");
}

#[test]
fn deleting_the_open_file_makes_its_appender_stale() {
    let mut media = formatted(8, 2);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store.create(&mut media, &name("a"), None, &mut s).unwrap();
    store.delete(&mut media, &name("a"), &mut s).unwrap();
    assert_eq!(
        file.append(&mut store, &mut media, b"x", &mut s),
        Err(Error::Stale)
    );
    assert!(snapshot(&mut media).unwrap().is_empty());
}

#[test]
fn foreign_and_damaged_metadata_are_reported_and_never_formatted_over() {
    let mut s = sector();
    let mut media = Media::new(DATA + 64);
    assert_eq!(mount(&mut media, &mut s), Err(Error::Uninitialized));
    assert_eq!(
        format(&mut Media::new(DATA + 100), 1, &mut s).err(),
        Some(Error::Range),
        "smaller than one chunk"
    );
    let store = format_chunks(&mut media, 1, 8, &mut s).unwrap();
    let good = media.get(base(store.copy));
    let reseal = |offset: usize, value: u8| {
        let mut changed = good;
        changed[offset] = value;
        let checksum = crc32(&changed[..CRC]);
        changed[CRC..].copy_from_slice(&checksum.to_le_bytes());
        changed
    };
    for (offset, value) in [
        (16, 2),
        (18, 9),
        (20, 3),
        (22, 1),
        (48, 200),
        (53, 4),
        (60, 1),
        (200, 1),
    ] {
        media.set(base(store.copy), reseal(offset, value));
        let writes = media.writes;
        assert_eq!(mount(&mut media, &mut s), Err(Error::Foreign), "{offset}");
        assert_eq!(format(&mut media, 2, &mut s).err(), Some(Error::Foreign));
        assert_eq!(media.writes, writes);
    }
    media.set(base(store.copy), good);
    let mut store = mount(&mut media, &mut s).unwrap();
    store.create(&mut media, &name("a"), None, &mut s).unwrap();
    let store = mount(&mut media, &mut s).unwrap();
    media.set(base(store.copy) + 1, [0x5a; SECTOR_SIZE]);
    assert_eq!(snapshot(&mut media).err(), Some(Error::Corrupt));
    format_chunks(&mut media, 3, 8, &mut s).unwrap();
    assert!(snapshot(&mut media).unwrap().is_empty());
}

#[test]
fn only_a_volume_without_the_magic_is_uninitialized() {
    let mut s = sector();
    for fill in [None, Some(0x00), Some(0xff)] {
        let mut media = Media::new(DATA + 64);
        if let Some(byte) = fill {
            media.set(0, [byte; SECTOR_SIZE]);
            media.set(COPY_B, [byte; SECTOR_SIZE]);
        }
        assert_eq!(mount(&mut media, &mut s).err(), Some(Error::Uninitialized));
    }
    let mut media = formatted(8, 8);
    let store = mount(&mut media, &mut s).unwrap();
    let mut damaged = media.get(base(store.copy));
    damaged[100] ^= 1;
    media.set(base(store.copy), damaged);
    assert_eq!(mount(&mut media, &mut s).err(), Some(Error::Corrupt));
}

#[test]
fn a_missing_record_at_the_end_of_a_full_chunk_is_a_hole_not_its_end() {
    let mut media = formatted(4, 2);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store.create(&mut media, &name("a"), None, &mut s).unwrap();
    for k in 1..4 {
        file.append(&mut store, &mut media, &[k; PAYLOAD], &mut s)
            .unwrap();
    }
    media.set(DATA + 1, [0; SECTOR_SIZE]);
    assert_eq!(
        events(&mut media, "a"),
        [
            std::format!("01x{PAYLOAD}"),
            "hole 1..2".into(),
            std::format!("03x{PAYLOAD}")
        ]
    );
}

#[test]
fn a_damaged_record_before_the_end_neither_loses_nor_reorders_synced_records() {
    let mut media = formatted(8, 64);
    let mut s = sector();
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store.create(&mut media, &name("a"), None, &mut s).unwrap();
    for k in 0..40u8 {
        file.append(&mut store, &mut media, &[k; PAYLOAD], &mut s)
            .unwrap();
    }
    media.set(DATA + 32, [0; SECTOR_SIZE]);
    assert_eq!(events(&mut media, "a")[32..34], ["hole 32..33", "21x486"]);
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store
        .open_append(&mut media, &name("a"), None, &mut s)
        .unwrap();
    for _ in 0..3 {
        file.append(&mut store, &mut media, &[0xee; PAYLOAD], &mut s)
            .unwrap();
    }
    media.cut = Some((0, Tear::Prefix(300)));
    let _ = file.append(&mut store, &mut media, &[0xcc; PAYLOAD], &mut s);
    media.reboot();
    let mut store = mount(&mut media, &mut s).unwrap();
    let mut file = store
        .open_append(&mut media, &name("a"), None, &mut s)
        .unwrap();
    file.append(&mut store, &mut media, &[0xaa; 5], &mut s)
        .unwrap();
    file.sync(&mut store, &mut media, &mut s).unwrap();

    let mut expected: Vec<String> = (0..40u8)
        .map(|k| match k {
            32 => "hole 32..33".into(),
            k => std::format!("{k:02x}x{PAYLOAD}"),
        })
        .collect();
    expected.extend(std::iter::repeat_n(std::format!("eex{PAYLOAD}"), 3));
    expected.push("aax5".into());
    assert_eq!(events(&mut media, "a"), expected);
}

const TRACES: &str = "traces/";
const CHUNKS: u64 = 32;
const SMALL_CHUNK: u32 = 8;

#[derive(Clone, Debug)]
enum Op {
    Create(String),
    Append(Vec<u8>),
    Sync,
    Put(String, Vec<u8>),
    Delete(String),
}

#[derive(Clone, Debug, Default)]
struct Model {
    streams: BTreeMap<String, Vec<u8>>,
    synced: BTreeMap<String, usize>,
    open: Option<String>,
}

impl Model {
    fn step(&mut self, op: &Op) {
        match op {
            Op::Create(n) => {
                self.streams.insert(n.clone(), Vec::new());
                self.synced.insert(n.clone(), 0);
                self.open = Some(n.clone());
            }
            Op::Append(bytes) => {
                let open = self.open.clone().unwrap();
                self.streams
                    .get_mut(&open)
                    .unwrap()
                    .extend_from_slice(bytes);
            }
            Op::Sync => {
                let open = self.open.clone().unwrap();
                self.synced.insert(open.clone(), self.streams[&open].len());
            }
            Op::Put(n, bytes) => {
                self.streams.insert(n.clone(), bytes.clone());
                self.synced.insert(n.clone(), bytes.len());
            }
            Op::Delete(n) => {
                self.synced.remove(n);
            }
        }
    }
}

fn apply<M: ReadWrite>(
    boot: &mut Boot<'_, M>,
    op: &Op,
    open: &mut Option<String>,
) -> Result<(), Error> {
    match op {
        Op::Create(n) => {
            boot.create(n)?;
            *open = Some(n.clone());
            Ok(())
        }
        Op::Append(bytes) => boot.append(bytes),
        Op::Sync => boot.sync(),
        Op::Put(n, bytes) => {
            boot.create(n)?;
            boot.append(bytes)?;
            boot.close()?;
            match open {
                Some(trace) => boot.reopen(trace),
                None => Ok(()),
            }
        }
        Op::Delete(n) => boot.delete(n),
    }
}

fn scenario(seed: u64) -> Vec<Op> {
    let mut rng = Rng::new(seed);
    let mut ops = std::vec![Op::Put("other/settings".into(), rng.bytes(900))];
    for t in 0..16 {
        ops.push(Op::Create(std::format!("traces/{t:05}")));
        for _ in 0..14 {
            let length = rng.range(50, 1500) as usize;
            ops.push(Op::Append(bytes(length, &mut rng)));
            if rng.one_in(3) {
                ops.push(Op::Sync);
            }
        }
        ops.push(Op::Sync);
        if t == 3 {
            ops.push(Op::Delete("other/settings".into()));
            ops.push(Op::Put("other/x".into(), rng.bytes(3000)));
        }
    }
    ops
}

struct Reference {
    names: Vec<BTreeSet<String>>,
    models: Vec<Model>,
    writes: Vec<u64>,
}

fn reference(ops: &[Op]) -> Reference {
    let mut media = formatted(CHUNKS, SMALL_CHUNK);
    let start = media.writes;
    let mut names = std::vec![BTreeSet::new()];
    let mut models = std::vec![Model::default()];
    let mut writes = Vec::new();
    let mut boot = Boot::mount(&mut media).unwrap();
    let mut open = None;
    for op in ops {
        apply(&mut boot, op, &mut open).unwrap_or_else(|e| panic!("reference {op:?}: {e:?}"));
        writes.push(boot.media.writes - start);
        let mut model = models.last().unwrap().clone();
        model.step(op);
        models.push(model);
        names.push(boot.names().unwrap().into_iter().collect());
    }
    let created = ops.iter().filter(|o| matches!(o, Op::Create(_))).count();
    let traces = names
        .last()
        .unwrap()
        .iter()
        .filter(|n| n.starts_with(TRACES))
        .count();
    assert!(traces < created, "the scenario never rolled over");
    Reference {
        names,
        models,
        writes,
    }
}

fn is_header(at: u64) -> bool {
    at == 0 || at == COPY_B
}

fn is_body(at: u64) -> bool {
    at < DATA && !is_header(at)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disk {
    Honest,
    WriteBack,
    HeaderFirst,
    NoInvalidate,
}

struct Faulty<'a> {
    media: &'a mut Media,
    disk: Disk,
    held: Vec<(u64, Sector)>,
}

impl ReadWrite for Faulty<'_> {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, MediaError> {
        self.media.owned_info()
    }

    fn owned_read(&mut self, at: u64, output: &mut Sector) -> Result<(), MediaError> {
        match self.held.iter().rev().find(|(held, _)| *held == at) {
            Some((_, bytes)) => {
                *output = *bytes;
                Ok(())
            }
            None => self.media.owned_read(at, output),
        }
    }

    fn owned_write(&mut self, at: u64, input: &Sector) -> Result<(), MediaError> {
        let blank = input.iter().all(|b| *b == 0);
        match self.disk {
            Disk::Honest => self.media.owned_write(at, input),
            Disk::WriteBack => {
                for (held, bytes) in self.held.drain(..) {
                    self.media.owned_write(held, &bytes)?;
                }
                self.held.push((at, *input));
                Ok(())
            }
            Disk::HeaderFirst if is_body(at) => {
                self.held.push((at, *input));
                Ok(())
            }
            Disk::HeaderFirst if is_header(at) && !blank => {
                self.media.owned_write(at, input)?;
                for (held, bytes) in self.held.drain(..) {
                    self.media.owned_write(held, &bytes)?;
                }
                Ok(())
            }
            Disk::NoInvalidate if is_header(at) && blank => Ok(()),
            Disk::HeaderFirst | Disk::NoInvalidate => self.media.owned_write(at, input),
        }
    }

    fn entropy(&mut self) -> u64 {
        self.media.entropy()
    }
}

#[derive(Default, Debug)]
struct Outcome {
    cuts: u64,
    lost: usize,
    unsynced: usize,
}

fn run_ops<M: ReadWrite>(media: &mut M, ops: &[Op]) -> Option<usize> {
    let mut boot = Boot::mount(media).ok()?;
    let mut open = None;
    ops.iter()
        .position(|op| apply(&mut boot, op, &mut open).is_err())
}

fn cut(ops: &[Op], r: &Reference, at: u64, tear: Tear, outcome: &mut Outcome, disk: Disk) {
    let label = std::format!("{disk:?} cut at write {at} {tear:?}");
    let mut media = formatted(CHUNKS, SMALL_CHUNK);
    media.cut = Some((at, tear));
    let stopped = run_ops(
        &mut Faulty {
            media: &mut media,
            disk,
            held: Vec::new(),
        },
        ops,
    );
    let i = stopped.unwrap_or_else(|| panic!("{label}: no op failed"));
    if disk == Disk::Honest {
        assert_eq!(
            r.writes.partition_point(|w| *w <= at),
            i,
            "{label}: failed in another op"
        );
    }
    media.reboot();
    let found = snapshot(&mut media).unwrap_or_else(|e| panic!("{label}: not consistent: {e:?}"));
    let (before, after) = (&r.models[i], &r.models[i + 1]);
    for (n, bytes) in &found {
        assert!(
            r.names[i].contains(n) || r.names[i + 1].contains(n),
            "{label}: unexpected file {n} during {:?}",
            ops[i]
        );
        assert!(
            after.streams[n].starts_with(bytes),
            "{label}: {n} is not a prefix of what was appended"
        );
        if let Some(synced) = before.synced.get(n) {
            assert!(
                bytes.len() >= *synced,
                "{label}: {n} lost synced bytes ({} < {synced})",
                bytes.len()
            );
        }
    }
    for n in r.names[i].intersection(&r.names[i + 1]) {
        assert!(
            found.contains_key(n),
            "{label}: lost {n} during {:?}",
            ops[i]
        );
    }
    if let (Some(open), Op::Append(_) | Op::Sync) = (&before.open, &ops[i])
        && let Some(bytes) = found.get(open)
    {
        let handed = before.streams[open].len();
        outcome.lost = outcome.lost.max(handed.saturating_sub(bytes.len()));
        outcome.unsynced = outcome.unsynced.max(handed - before.synced[open]);
    }
    let target = before
        .open
        .clone()
        .filter(|o| found.contains_key(o))
        .unwrap_or_else(|| "traces/99999".into());
    let extra: Vec<u8> = (0..1000u32).map(|i| (i * 7 + at as u32) as u8).collect();
    let resumed = Boot::mount(&mut media).and_then(|mut boot| {
        if found.contains_key(&target) {
            boot.reopen(&target)?;
        } else {
            boot.create(&target)?;
        }
        boot.append(&extra)?;
        boot.close()
    });
    resumed.unwrap_or_else(|e| panic!("{label}: unusable after the cut: {e:?}"));
    let again = snapshot(&mut media).unwrap_or_else(|e| panic!("{label}: after resume: {e:?}"));
    let mut expected = found.get(&target).cloned().unwrap_or_default();
    expected.extend_from_slice(&extra);
    assert_eq!(
        again.get(&target),
        Some(&expected),
        "{label}: resumed {target}"
    );
    for (n, bytes) in &found {
        if *n != target && again.contains_key(n) {
            assert_eq!(&again[n], bytes, "{label}: {n} changed on resume");
        }
    }
    outcome.cuts += 1;
}

fn every_cut(seed: u64, disk: Disk) -> Outcome {
    let ops = scenario(seed);
    let r = reference(&ops);
    let total = *r.writes.last().unwrap();
    let mut rng = Rng::new(seed ^ 0xc0ffee);
    let mut outcome = Outcome::default();
    for at in 0..total - u64::from(disk == Disk::WriteBack) {
        for tear in [Tear::Old, Tear::Prefix(rng.below(512) as usize), Tear::New] {
            cut(&ops, &r, at, tear, &mut outcome, disk);
        }
    }
    std::println!(
        "{total} writes in the scenario, {} cuts passed; most appended bytes lost {} (unsynced at the time {})",
        outcome.cuts,
        outcome.lost,
        outcome.unsynced
    );
    outcome
}

#[test]
fn every_power_cut_keeps_whole_files_and_their_synced_bytes() {
    let outcome = every_cut(1, Disk::Honest);
    assert!(outcome.lost < PAYLOAD, "{outcome:?}");
    for seed in 2..40 {
        let ops = scenario(seed);
        let r = reference(&ops);
        let total = *r.writes.last().unwrap();
        let mut rng = Rng::new(seed);
        let mut outcome = Outcome::default();
        for _ in 0..40 {
            let at = rng.below(total);
            cut(
                &ops,
                &r,
                at,
                Tear::pick(&mut rng),
                &mut outcome,
                Disk::Honest,
            );
        }
    }
}

#[test]
#[should_panic(expected = "lost synced bytes")]
fn control_a_write_acknowledged_before_it_lands_fails_the_property() {
    every_cut(1, Disk::WriteBack);
}

#[test]
#[should_panic(expected = "not consistent: Corrupt")]
fn control_a_header_written_before_its_body_fails_the_property() {
    every_cut(1, Disk::HeaderFirst);
}

#[test]
#[should_panic(expected = "Corrupt")]
fn control_a_commit_that_keeps_the_old_header_valid_fails_the_property() {
    every_cut(1, Disk::NoInvalidate);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Mount,
    Format,
    List,
    Reader,
    OpenAppend,
    Delete,
    Create,
    CreateRollingOver,
    Append,
    Next,
    Seek,
}

fn bound(kind: Kind, table: u64, chunk: u32) -> (u64, u64) {
    let (d, t) = (DIR_SECTORS as u64, table);
    match kind {
        Kind::Mount => (2, 0),
        Kind::Format => (2, d + t + 2),
        Kind::List | Kind::Reader => (d + t, 0),
        Kind::OpenAppend => (d + t, 0),
        Kind::Delete => (2 * d + t + 2, d + t + 2),
        Kind::Create => (2 * d + 2 * t + 2, d + t + 2),
        Kind::CreateRollingOver => (5 * d + 4 * t + 4, 2 * d + 2 * t + 4),
        Kind::Append => (3 * d + 4 * t + 4, 2 * d + 2 * t + 5),
        Kind::Next => (t + u64::from(chunk), 0),
        Kind::Seek => (t, 0),
    }
}

#[derive(Default)]
struct Costs(BTreeMap<Kind, (u64, u64, u64)>);

impl Costs {
    fn measure<R>(&mut self, media: &mut Media, kind: Kind, f: impl FnOnce(&mut Media) -> R) -> R {
        let (reads, writes) = (media.reads, media.writes);
        let result = f(media);
        self.add(kind, media.reads - reads, media.writes - writes);
        result
    }

    fn add(&mut self, kind: Kind, reads: u64, writes: u64) {
        let cost = self.0.entry(kind).or_default();
        cost.0 += 1;
        cost.1 = cost.1.max(reads);
        cost.2 = cost.2.max(writes);
    }

    fn check(&self, table: u64, chunk: u32) {
        for (kind, (count, reads, writes)) in &self.0 {
            let (max_reads, max_writes) = bound(*kind, table, chunk);
            std::println!(
                "{kind:?}: {count} ops, worst {reads} reads / {writes} writes (bound {max_reads} / {max_writes})"
            );
            assert!(
                *reads <= max_reads && *writes <= max_writes,
                "{kind:?}: {reads}/{writes} over {max_reads}/{max_writes}"
            );
        }
    }
}

#[test]
fn operations_have_the_documented_worst_case_costs() {
    let (chunks, chunk) = (1039, 4);
    let mut media = Media::new(DATA + chunks * u64::from(chunk));
    let mut costs = Costs::default();
    let mut s = sector();
    let store = costs.measure(&mut media, Kind::Format, |m| {
        format_chunks(m, 1, chunk, &mut s)
    });
    let table = store.unwrap().head.table() as u64;
    assert_eq!(table, 9);
    let mut rng = Rng::new(5);
    let traces = Some(dir(TRACES));
    let (mut rolled_on_create, mut rolled_on_append) = (0, 0);
    let count = |media: &mut Media, s: &mut Sector| {
        let mut n = 0;
        mount(media, s)
            .unwrap()
            .list(media, &dir(TRACES), s, |_| n += 1)
            .unwrap();
        n
    };
    {
        let mut store = mount(&mut media, &mut s).unwrap();
        let mut other = store
            .create(&mut media, &name("other/settings"), None, &mut s)
            .unwrap();
        other
            .append(&mut store, &mut media, &[1; 3000], &mut s)
            .unwrap();
        other.sync(&mut store, &mut media, &mut s).unwrap();
    }
    for session in 0..200 {
        let mut store = costs.measure(&mut media, Kind::Mount, |m| mount(m, &mut s).unwrap());
        let before = count(&mut media, &mut s);
        let trace = name(&std::format!("traces/{session:05}"));
        let (reads, writes) = (media.reads, media.writes);
        let mut file = store
            .create(&mut media, &trace, traces, &mut s)
            .unwrap_or_else(|e| panic!("session {session}: {e:?}"));
        let (reads, writes) = (media.reads - reads, media.writes - writes);
        if count(&mut media, &mut s) <= before {
            rolled_on_create += 1;
            costs.add(Kind::CreateRollingOver, reads, writes);
        } else {
            costs.add(Kind::Create, reads, writes);
        }
        let records = match session {
            0..120 => rng.range(1, 3 * chunks * u64::from(chunk) / 40),
            _ => rng.range(1, 2),
        };
        for record in 0..records {
            let before = store.generation();
            let more = bytes(rng.range(1, PAYLOAD as u64) as usize, &mut rng);
            costs.measure(&mut media, Kind::Append, |m| {
                file.append(&mut store, m, &more, &mut s).unwrap()
            });
            if store.generation() > before + 1 {
                rolled_on_append += 1;
            }
            if rng.one_in(50) {
                costs.measure(&mut media, Kind::Append, |m| {
                    file.sync(&mut store, m, &mut s).unwrap()
                });
            }
            if record == records / 2 && session % 3 == 0 {
                media.cut = Some((1, Tear::Prefix(100)));
                let _ = store.create(&mut media, &name("traces/cut"), traces, &mut s);
                media.reboot();
                store = costs.measure(&mut media, Kind::Mount, |m| mount(m, &mut s).unwrap());
                file = costs
                    .measure(&mut media, Kind::OpenAppend, |m| {
                        store.open_append(m, &trace, traces, &mut s)
                    })
                    .unwrap();
                let _ = store.delete(&mut media, &name("traces/cut"), &mut s);
            }
        }
        while session % 4 == 1
            && (file.index.is_some_and(|index| index < chunk)
                || store.free_chunk(&mut media, &mut s).unwrap().is_some())
        {
            costs.measure(&mut media, Kind::Append, |m| {
                file.append(&mut store, m, &[0; PAYLOAD], &mut s).unwrap()
            });
        }
        file.sync(&mut store, &mut media, &mut s).unwrap();
    }
    assert!(
        rolled_on_create > 20 && rolled_on_append > 20,
        "{rolled_on_create} {rolled_on_append}"
    );
    let store = mount(&mut media, &mut s).unwrap();
    let mut names = Vec::new();
    costs.measure(&mut media, Kind::List, |m| {
        store
            .list(m, &Dir::ROOT, &mut s, |e| names.push(*e))
            .unwrap()
    });
    names.sort_by_key(|e| e.id);
    let longest = names.iter().max_by_key(|e| e.chunks).unwrap().name;
    let mut reader = costs
        .measure(&mut media, Kind::Reader, |m| {
            store.reader(m, &longest, &mut s)
        })
        .unwrap();
    let mut out = [0; PAYLOAD];
    let mut pulled = 0;
    while let Next::Record(n) = costs.measure(&mut media, Kind::Next, |m| {
        reader.next(&store, m, &mut out, &mut s).unwrap()
    }) {
        pulled += n;
    }
    assert!(pulled > 0);
    for _ in 0..50 {
        let at = rng.below(u64::from(reader.record(&store)) + 1) as u32;
        costs.measure(&mut media, Kind::Seek, |m| {
            reader.seek(&store, m, at, &mut s).unwrap()
        });
    }
    let mut store = store;
    for e in &names[..10] {
        costs.measure(&mut media, Kind::Delete, |m| {
            store.delete(m, &e.name, &mut s).unwrap()
        });
    }
    costs.measure(&mut media, Kind::Delete, |m| {
        store.delete_oldest(m, &dir(TRACES), &mut s).unwrap()
    });
    costs.check(table, chunk);
}

#[test]
fn the_c606_store_volume_has_1039_chunks_and_bounded_costs_at_full_chunk_size() {
    let mut media = Media::new(532_479);
    let mut costs = Costs::default();
    let mut s = sector();
    let store = costs.measure(&mut media, Kind::Format, |m| format(m, 1, &mut s).unwrap());
    assert_eq!((store.chunks(), store.head.table()), (1039, 9));
    let mut store = costs.measure(&mut media, Kind::Mount, |m| mount(m, &mut s).unwrap());
    let mut file = costs
        .measure(&mut media, Kind::Create, |m| {
            store.create(m, &name("traces/00001"), Some(dir(TRACES)), &mut s)
        })
        .unwrap();
    let record = [3u8; PAYLOAD];
    for _ in 0..2 * CHUNK + 100 {
        costs.measure(&mut media, Kind::Append, |m| {
            file.append(&mut store, m, &record, &mut s).unwrap()
        });
    }
    let mut store = mount(&mut media, &mut s).unwrap();
    costs
        .measure(&mut media, Kind::OpenAppend, |m| {
            store.open_append(m, &name("traces/00001"), None, &mut s)
        })
        .unwrap();
    let mut reader = costs
        .measure(&mut media, Kind::Reader, |m| {
            store.reader(m, &name("traces/00001"), &mut s)
        })
        .unwrap();
    let mut out = [0; PAYLOAD];
    let mut records = 0;
    while let Next::Record(_) = costs.measure(&mut media, Kind::Next, |m| {
        reader.next(&store, m, &mut out, &mut s).unwrap()
    }) {
        records += 1;
    }
    assert_eq!(records, 2 * CHUNK + 100);
    costs.measure(&mut media, Kind::Seek, |m| {
        reader.seek(&store, m, CHUNK + 1, &mut s).unwrap()
    });
    costs.measure(&mut media, Kind::List, |m| {
        store.list(m, &Dir::ROOT, &mut s, |_| {}).unwrap()
    });
    costs.measure(&mut media, Kind::Delete, |m| {
        store.delete(m, &name("traces/00001"), &mut s).unwrap()
    });
    costs.check(9, CHUNK);
}
