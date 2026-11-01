use super::*;
use crate::commands::volumes;
use device_api::bulk::{Error, OwnedInfo, SECTOR_SIZE};
use firmware_services::checksum::Crc32;
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

const C606: u64 = 5_777_407;

#[derive(Default)]
struct Media {
    written: BTreeMap<u64, [u8; SECTOR_SIZE]>,
    reads: u64,
}

impl ReadWrite for Media {
    fn owned_info(&mut self) -> Result<OwnedInfo, Error> {
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: C606,
            sector_size: 512,
        })
    }
    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        if sector >= C606 {
            return Err(Error::Range);
        }
        self.reads += 1;
        *output = self
            .written
            .get(&sector)
            .copied()
            .unwrap_or([0x33; SECTOR_SIZE]);
        Ok(())
    }
    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        if sector >= C606 {
            return Err(Error::Range);
        }
        self.written.insert(sector, *input);
        Ok(())
    }
}

struct Host {
    session: Session,
    media: Media,
    id: u32,
}

impl Host {
    fn new() -> Self {
        let mut media = Media::default();
        let mut out = String::new();
        assert_eq!(
            volumes::execute(volumes::Command::Init, &mut media, &mut out),
            "OK",
            "{out}"
        );
        Self {
            session: Session::default(),
            media,
            id: 0,
        }
    }

    /// Through the wire encoding both ways.
    fn call(&mut self, op: u16, arg: u64, count: u32, payload: &[u8]) -> (u16, u64, u32, Vec<u8>) {
        self.id += 1;
        let header = encode_request(&Request {
            op,
            id: self.id,
            arg,
            count,
            length: payload.len() as u32,
            payload_crc: crc32(payload),
        });
        let request = super::request(&header).unwrap();
        let mut out = [0u8; MAX_PAYLOAD];
        let reply = self
            .session
            .handle(&request, payload, &mut self.media, &mut out);
        let wire = encode_reply(&request, &reply, &out[..reply.length]);
        assert_eq!(&wire[..4], REPLY);
        assert_eq!(get_u32(&wire, 32), crc32(&wire[..32]));
        assert_eq!(get_u16(&wire, 6), op);
        assert_eq!(get_u32(&wire, 8), self.id);
        (
            reply.status,
            reply.arg,
            reply.count,
            out[..reply.length].to_vec(),
        )
    }
}

fn spec(data: &[u8]) -> Vec<u8> {
    let mut spec = Vec::new();
    spec.extend_from_slice(&(data.len() as u64).to_le_bytes());
    spec.extend_from_slice(&crc32(data).to_le_bytes());
    spec.extend_from_slice(b"CHILE-TERRAIN-V1");
    spec.extend_from_slice(&[9; 32]);
    spec.extend_from_slice(&1_760_000_000u64.to_le_bytes());
    spec
}

fn padded(data: &[u8], from: usize, sectors: usize) -> Vec<u8> {
    let mut bytes = std::vec![0u8; sectors * 512];
    let end = data.len().min(from + sectors * 512);
    if from < end {
        bytes[..end - from].copy_from_slice(&data[from..end]);
    }
    bytes
}

#[test]
fn headers_are_checked_and_ids_must_increase() {
    let mut host = Host::new();
    let mut header = encode_request(&Request {
        op: op::HELLO,
        id: 1,
        arg: 0,
        count: 0,
        length: 0,
        payload_crc: 0,
    });
    assert!(request(&header).is_some());
    header[12] ^= 1;
    assert!(request(&header).is_none());
    let (status, _, _, hello) = host.call(op::HELLO, 0, 0, &[]);
    assert_eq!(status, status::OK);
    assert_eq!(get_u64(&hello, 0), C606);
    assert_eq!(get_u32(&hello, 8), MAX_PAYLOAD as u32);
    host.id = 0;
    assert_eq!(host.call(op::HELLO, 0, 0, &[]).0, status::INVALID);
    assert_eq!(host.call(99, 0, 0, &[]).0, status::INVALID);
    assert!(!host.session.ended());
    assert_eq!(host.call(op::END, 0, 0, &[]).0, status::OK);
    assert!(host.session.ended());
}

#[test]
fn the_volume_table_is_reported() {
    let mut host = Host::new();
    let (status, _, count, table) = host.call(op::VOLUMES, 0, 0, &[]);
    assert_eq!((status, count, table[0]), (status::OK, 4, 2));
    assert_eq!(get_u64(&table, 8), C606);
    assert_eq!(get_u64(&table, 32), 0);
    let names: Vec<_> = table[40..].chunks(25).map(|e| e[..8].to_vec()).collect();
    assert_eq!(
        names,
        [
            b"RIDES\0\0\0".to_vec(),
            b"FILES\0\0\0".to_vec(),
            b"TERRAIN\0".to_vec(),
            b"STORE\0\0\0".to_vec()
        ]
    );
    let mut blank = Host::new();
    blank.media.written.clear();
    assert_eq!(blank.call(op::VOLUMES, 0, 0, &[]).3[0], 0);
    assert_eq!(
        blank.call(op::TERRAIN_STATUS, 0, 0, &[]).0,
        status::UNINITIALIZED
    );
}

#[test]
fn a_terrain_pack_is_written_resumed_verified_and_committed() {
    let mut host = Host::new();
    let data: Vec<u8> = (0..40_000u32).map(|i| (i * 7 + i / 300) as u8).collect();
    let sectors = data.len().div_ceil(512);
    assert_eq!(host.call(op::TERRAIN_STATUS, 0, 0, &[]).3[0], 0);
    assert_eq!(
        host.call(op::TERRAIN_WRITE, 0, 0, &padded(&data, 0, 8)).0,
        status::STATE
    );
    let (status, _, _, info) = host.call(op::TERRAIN_BEGIN, 0, 0, &spec(&data));
    assert_eq!((status, info[0]), (status::OK, 1));
    // Out of order is refused; in order advances the cursor.
    assert_eq!(
        host.call(op::TERRAIN_WRITE, 8, 0, &padded(&data, 4096, 8))
            .0,
        status::RANGE
    );
    assert_eq!(
        host.call(op::TERRAIN_WRITE, 0, 0, &padded(&data, 0, 8)).1,
        8
    );
    assert_eq!(
        host.call(op::TERRAIN_WRITE, 8, 0, &padded(&data, 4096, 8))
            .1,
        16
    );
    // A new session (after a reboot) resumes: verify what landed, then write.
    host.session = Session::default();
    host.id = 0;
    let (status, cursor, _, info) = host.call(op::TERRAIN_RESUME, 0, 0, &[]);
    assert_eq!((status, cursor, info[0]), (status::OK, 0, 1));
    let expected = Crc32::new().update(&padded(&data, 0, 16)).finish();
    let (status, cursor, advanced, reply) =
        host.call(op::TERRAIN_VERIFY, 0, 16, &expected.to_le_bytes());
    assert_eq!((status, cursor, advanced), (status::OK, 16, 1));
    assert_eq!(get_u32(&reply, 0), expected);
    let (_, cursor, advanced, _) = host.call(op::TERRAIN_VERIFY, 16, 8, &0u32.to_le_bytes());
    assert_eq!((cursor, advanced), (16, 0));
    assert_eq!(host.call(op::TERRAIN_COMMIT, 0, 0, &[]).0, status::RANGE);
    let mut at = 16;
    while at < sectors {
        let n = (sectors - at).min(8);
        let (status, cursor, _, _) =
            host.call(op::TERRAIN_WRITE, at as u64, 0, &padded(&data, at * 512, n));
        assert_eq!(status, status::OK);
        at += n;
        assert_eq!(cursor, at as u64);
    }
    let (status, _, _, info) = host.call(op::TERRAIN_COMMIT, 0, 0, &[]);
    assert_eq!((status, info[0]), (status::OK, 2));
    let (_, _, _, info) = host.call(op::TERRAIN_STATUS, 0, 0, &[]);
    assert_eq!(info[0], 2);
    assert_eq!(get_u64(&info, 9), data.len() as u64);
    assert_eq!(&info[21..37], b"CHILE-TERRAIN-V1");
    // Host readback check of any range.
    let (status, _, _, crc) = host.call(op::TERRAIN_CRC, 8, 8, &[]);
    assert_eq!(status, status::OK);
    assert_eq!(get_u32(&crc, 0), crc32(&padded(&data, 4096, 8)));
    // Only the table, TERRAIN and nothing of RIDES or FILES was written.
    let terrain = firmware_services::volume::plan(C606)
        .unwrap()
        .find(&firmware_services::volume::TERRAIN)
        .unwrap();
    assert!(
        host.media
            .written
            .keys()
            .all(|s| *s < 2 || (terrain.start..terrain.start + terrain.sectors).contains(s))
    );
}

#[test]
fn files_are_put_listed_read_and_deleted() {
    let mut host = Host::new();
    assert_eq!(
        host.call(op::FILES_STATUS, 0, 0, &[]).0,
        status::UNINITIALIZED
    );
    assert_eq!(host.call(op::FILES_FORMAT, 1, 0, &[]).0, status::OK);
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(
        host.call(op::FILES_BEGIN, 0, 5000, b"bad name").0,
        status::INVALID
    );
    assert_eq!(
        host.call(op::FILES_BEGIN, 0, 5000, b"workouts/ftp.json").0,
        status::OK
    );
    assert_eq!(host.call(op::FILES_WRITE, 0, 0, &data[..4096]).1, 4096);
    assert_eq!(
        host.call(op::FILES_WRITE, 0, 0, &data[4096..]).0,
        status::RANGE
    );
    assert_eq!(
        host.call(op::FILES_BEGIN, 0, 5000, b"workouts/ftp.json").0,
        status::OK
    );
    host.call(op::FILES_WRITE, 0, 0, &data[..4096]);
    host.call(op::FILES_WRITE, 4096, 0, &data[4096..]);
    let (status, _, _, entry) = host.call(op::FILES_COMMIT, 0, crc32(&data), &[]);
    assert_eq!(status, status::OK);
    assert_eq!(&entry[2..19], b"workouts/ftp.json");
    assert_eq!(get_u32(&entry, 42), 5000);
    let (status, next, count, list) = host.call(op::FILES_LIST, 0, 128, &[]);
    assert_eq!((status, next, count, list.len()), (status::OK, 128, 1, 54));
    // A huge count is bounded by the directory, never overflowing on 32-bit targets.
    let (status, next, count, _) = host.call(op::FILES_LIST, 1, u32::MAX, &[]);
    assert_eq!((status, next, count), (status::OK, 128, 0));
    let (status, offset, crc, read) = host.call(op::FILES_READ, 4000, 4096, b"workouts/ftp.json");
    assert_eq!((status, offset, crc), (status::OK, 4000, crc32(&data)));
    assert_eq!(read, data[4000..]);
    assert_eq!(
        host.call(op::FILES_STAT, 0, 0, b"workouts/ftp.json").0,
        status::OK
    );
    assert_eq!(
        host.call(op::FILES_DELETE, 0, 0, b"workouts/ftp.json").0,
        status::OK
    );
    assert_eq!(
        host.call(op::FILES_STAT, 0, 0, b"workouts/ftp.json").0,
        status::NOT_FOUND
    );
    let (_, _, count, _) = host.call(op::FILES_STATUS, 0, 0, &[]);
    assert_eq!(count, 0);
}

#[test]
fn line_commands_report_volumes_terrain_and_files() {
    let mut media = Media::default();
    let mut out = String::new();
    assert_eq!(
        volumes::execute(volumes::Command::Status, &mut media, &mut out),
        "OK"
    );
    assert_eq!(out, "layout=blank sectors=5777407");
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::Terrain, &mut media, &mut out),
        "UNINITIALIZED"
    );
    volumes::execute(volumes::Command::Init, &mut media, &mut out);
    out.clear();
    volumes::execute(volumes::Command::Status, &mut media, &mut out);
    assert_eq!(
        out,
        "layout=volumes sectors=5777407 generation=1 unallocated=0 rides=2048+2097152 files=2099200+524288 terrain=2623488+2621440 store=5244928+532479"
    );
    out.clear();
    volumes::execute(volumes::Command::Terrain, &mut media, &mut out);
    assert_eq!(out, "state=empty capacity=1342173184");
    out.clear();
    volumes::execute(volumes::Command::Files, &mut media, &mut out);
    assert_eq!(out, "state=uninitialized");
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::FilesFormat, &mut media, &mut out),
        "OK"
    );
    assert!(
        out.starts_with("state=ready files=0 capacity=128 max_file=1048576"),
        "{out}"
    );
    let mut sector = [0u8; 512];
    let mut vol =
        firmware_services::volume::open(&mut media, &firmware_services::volume::FILES, &mut sector)
            .unwrap();
    for i in 0..10 {
        let name = firmware_services::files::Name::new(&std::format!("f{i}")).unwrap();
        firmware_services::files::write(&mut vol, &name, b"abc", &mut sector).unwrap();
    }
    out.clear();
    volumes::execute(volumes::Command::FilesList(0), &mut media, &mut out);
    assert!(out.starts_with("f0:3:352441c2:"), "{out}");
    assert!(out.ends_with(" next=8"), "{out}");
    let name = firmware_services::files::Name::new("f3").unwrap();
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::FilesDelete(name), &mut media, &mut out),
        "OK"
    );
    assert_eq!(
        volumes::execute(volumes::Command::FilesStat(name), &mut media, &mut out),
        "NOT_FOUND"
    );
    let mut words = "LIST 200".split_ascii_whitespace();
    assert_eq!(volumes::parse("FILES", &mut words), None);
    let mut words = "FORMAT".split_ascii_whitespace();
    assert_eq!(volumes::parse("FILES", &mut words), None);
}

#[test]
fn line_commands_add_and_report_the_store() {
    let mut media = Media::default();
    let mut out = String::new();
    // No table yet: nothing to add STORE to.
    assert_eq!(
        volumes::execute(volumes::Command::AddStore, &mut media, &mut out),
        "UNINITIALIZED"
    );
    assert!(media.written.is_empty());
    volumes::execute(volumes::Command::Init, &mut media, &mut out);
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::Store, &mut media, &mut out),
        "OK"
    );
    assert_eq!(out, "state=uninitialized");
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::AddStore, &mut media, &mut out),
        "OK"
    );
    assert!(out.ends_with(" store=5244928+532479"), "{out}");
    out.clear();
    volumes::execute(volumes::Command::Store, &mut media, &mut out);
    assert!(
        out.starts_with(
            "state=ready files=0 capacity=64 chunks=1039 free=1039 generation=1 store="
        ),
        "{out}"
    );
    // A store with files is kept as it is.
    let mut sector = [0u8; 512];
    let mut vol =
        firmware_services::volume::open(&mut media, &firmware_services::volume::STORE, &mut sector)
            .unwrap();
    let mut store = firmware_services::store::mount(&mut vol, &mut sector).unwrap();
    for i in 0..10 {
        let name = firmware_services::store::Name::new(&std::format!("traces/{i}")).unwrap();
        store.create(&mut vol, &name, None, &mut sector).unwrap();
    }
    let writes = media.written.clone();
    out.clear();
    assert_eq!(
        volumes::execute(volumes::Command::AddStore, &mut media, &mut out),
        "OK"
    );
    assert!(media.written == writes, "nothing written");
    out.clear();
    volumes::execute(volumes::Command::Store, &mut media, &mut out);
    assert!(
        out.starts_with("state=ready files=10 capacity=64 chunks=1039 free=1029 "),
        "{out}"
    );
    out.clear();
    volumes::execute(volumes::Command::StoreList(0), &mut media, &mut out);
    assert!(out.starts_with("traces/0:1:1 traces/1:2:1 "), "{out}");
    assert!(out.ends_with(" next=8"), "{out}");
    out.clear();
    volumes::execute(volumes::Command::StoreList(8), &mut media, &mut out);
    assert_eq!(out, "traces/8:9:1 traces/9:10:1");
    for (line, parsed) in [
        ("ADD STORE", Some(volumes::Command::AddStore)),
        ("ADD", None),
        ("ADD FILES", None),
    ] {
        let mut words = line.split_ascii_whitespace();
        assert_eq!(volumes::parse("VOLUME", &mut words), parsed, "{line}");
    }
    for (line, parsed) in [
        ("", Some(volumes::Command::Store)),
        ("LIST", Some(volumes::Command::StoreList(0))),
        ("LIST 63", Some(volumes::Command::StoreList(63))),
        ("LIST 64", None),
        ("DELETE traces/1", None),
    ] {
        let mut words = line.split_ascii_whitespace();
        assert_eq!(volumes::parse("STORE", &mut words), parsed, "{line}");
    }
}

/// What the app leaves in STORE: a trace of `records` records, each as long
/// as its number allows, and a small settings file.
fn record_traces(media: &mut Media, records: u32) -> Vec<u8> {
    use firmware_services::store::{self, Name};
    let mut sector = [0u8; 512];
    let mut vol = firmware_services::volume::open(media, &volume::STORE, &mut sector).unwrap();
    let mut mounted = store::mount(&mut vol, &mut sector).unwrap();
    let mut other = mounted
        .create(
            &mut vol,
            &Name::new("other/settings").unwrap(),
            None,
            &mut sector,
        )
        .unwrap();
    other
        .append(&mut mounted, &mut vol, b"settings", &mut sector)
        .unwrap();
    other.sync(&mut mounted, &mut vol, &mut sector).unwrap();
    let mut trace = mounted
        .create(
            &mut vol,
            &Name::new("traces/0001").unwrap(),
            None,
            &mut sector,
        )
        .unwrap();
    let mut bytes = Vec::new();
    for i in 0..records {
        let record: Vec<u8> = (0..store::PAYLOAD - (i as usize % 7) * 50)
            .map(|b| (b as u32 * 13 + i) as u8)
            .collect();
        trace
            .append(&mut mounted, &mut vol, &record, &mut sector)
            .unwrap();
        trace.sync(&mut mounted, &mut vol, &mut sector).unwrap();
        bytes.extend_from_slice(&record);
    }
    bytes
}

#[test]
fn store_files_are_listed_pulled_by_record_range_and_deleted() {
    let mut host = Host::new();
    assert_eq!(
        host.call(op::STORE_STATUS, 0, 0, &[]).0,
        status::UNINITIALIZED
    );
    host.session = Session::default();
    host.id = 0;
    let mut out = String::new();
    volumes::execute(volumes::Command::AddStore, &mut host.media, &mut out);
    let records = 2 * firmware_services::store::CHUNK + 300;
    let trace = record_traces(&mut host.media, records);

    let (status, generation, files, info) = host.call(op::STORE_STATUS, 0, 0, &[]);
    assert_eq!((status, files), (status::OK, 2));
    assert_eq!(get_u64(&info, 0), generation);
    assert_eq!(get_u32(&info, 24) - get_u32(&info, 28), 4, "chunks in use");
    let (status, _, count, list) = host.call(op::STORE_LIST, 0, 0, b"traces/");
    assert_eq!((status, count, list.len()), (status::OK, 1, 48));
    assert_eq!(&list[..11], b"traces/0001");
    assert_eq!(get_u32(&list, 44), 3, "chunks");
    assert_eq!(host.call(op::STORE_LIST, 0, 0, b"").2, 2);
    assert_eq!(
        host.call(op::STORE_LIST, 0, 0, b"traces").0,
        status::INVALID
    );

    // A pull keeps its reader open: one read per record (2.1 per KB of full
    // records), plus opening the file, a table pass at each chunk, and at
    // the end the rest of the last chunk, which the last two requests both
    // read.
    let reads = host.media.reads;
    let (mut pulled, mut at) = (Vec::new(), 0);
    loop {
        let (status, next, count, data) = host.call(op::STORE_READ, at, 100, b"traces/0001");
        assert_eq!(status, status::OK);
        if count == 0 {
            break;
        }
        assert_eq!(next, at + u64::from(count));
        pulled.extend_from_slice(&data);
        at = next;
    }
    assert_eq!(at, u64::from(records));
    assert!(pulled == trace, "the pulled trace differs");
    let (table, dir) = (9, 1);
    let rest =
        u64::from(firmware_services::store::CHUNK - records % firmware_services::store::CHUNK);
    let overhead = host.media.reads - reads - u64::from(records);
    assert!(
        overhead <= dir + 3 * table + 2 * rest,
        "{overhead} reads besides records"
    );

    // Any record range, in another chunk or back at the start.
    let first = firmware_services::store::CHUNK + 5;
    let (_, next, count, data) = host.call(op::STORE_READ, u64::from(first), 2, b"traces/0001");
    assert_eq!((next, count), (u64::from(first) + 2, 2));
    let offset: usize = (0..first)
        .map(|i| firmware_services::store::PAYLOAD - (i as usize % 7) * 50)
        .sum();
    assert!(data == trace[offset..offset + data.len()]);
    let (_, _, _, data) = host.call(op::STORE_READ, 0, 1, b"traces/0001");
    assert!(data == trace[..firmware_services::store::PAYLOAD]);
    assert_eq!(
        host.call(op::STORE_READ, 0, 1, b"traces/none").0,
        status::NOT_FOUND
    );
    assert_eq!(host.call(op::STORE_READ, 0, 1, b"/bad").0, status::INVALID);

    // A delete is a new generation the app compares after the session.
    let (status, after, _, _) = host.call(op::STORE_DELETE, 0, 0, b"traces/0001");
    assert_eq!(status, status::OK);
    assert!(after > generation);
    assert_eq!(
        host.call(op::STORE_READ, 0, 1, b"traces/0001").0,
        status::NOT_FOUND
    );
    let (_, _, files, info) = host.call(op::STORE_STATUS, 0, 0, &[]);
    assert_eq!((files, get_u64(&info, 0)), (1, after));
    // A new session sees what this one committed.
    host.session = Session::default();
    host.id = 0;
    let (_, _, count, list) = host.call(op::STORE_LIST, 0, 0, b"");
    assert_eq!((count, &list[..14]), (1, &b"other/settings"[..]));
}

#[test]
fn a_pull_stops_at_records_the_medium_lost_and_says_which() {
    use firmware_services::store::{CHUNK, DATA, PAYLOAD};
    let mut host = Host::new();
    volumes::execute(
        volumes::Command::AddStore,
        &mut host.media,
        &mut String::new(),
    );
    let trace = record_traces(&mut host.media, CHUNK + 100);
    let offset =
        |record: u32| -> usize { (0..record).map(|i| PAYLOAD - (i as usize % 7) * 50).sum() };
    let mut sector = [0u8; 512];
    let start = volume::open(&mut host.media, &volume::STORE, &mut sector)
        .unwrap()
        .extent()
        .start;
    // The trace holds chunks 1 and 2, after the settings file's chunk 0.
    let lost = CHUNK + 20;
    host.media
        .written
        .insert(start + DATA + 2 * u64::from(CHUNK) + 20, [0; 512]);

    let (status, next, count, data) =
        host.call(op::STORE_READ, u64::from(lost - 5), 100, b"traces/0001");
    assert_eq!((status, next, count), (status::OK, u64::from(lost), 5));
    assert!(data == trace[offset(lost - 5)..offset(lost)]);
    let (status, first, missing, data) =
        host.call(op::STORE_READ, u64::from(lost), 100, b"traces/0001");
    assert_eq!(
        (status, first, missing, data.len()),
        (status::HOLE, u64::from(lost), 1, 0)
    );
    let (status, _, count, data) =
        host.call(op::STORE_READ, u64::from(lost + 1), 1, b"traces/0001");
    assert_eq!((status, count), (status::OK, 1));
    assert!(data == trace[offset(lost + 1)..offset(lost + 2)]);
}
