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
    assert_eq!((status, count, table[0]), (status::OK, 3, 2));
    assert_eq!(get_u64(&table, 8), C606);
    assert_eq!(get_u64(&table, 32), 532_479);
    let names: Vec<_> = table[40..].chunks(25).map(|e| e[..8].to_vec()).collect();
    assert_eq!(
        names,
        [
            b"RIDES\0\0\0".to_vec(),
            b"FILES\0\0\0".to_vec(),
            b"TERRAIN\0".to_vec()
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
        "layout=volumes sectors=5777407 generation=1 unallocated=532479 rides=2048+2097152:log files=2099200+524288:files terrain=2623488+2621440:pack"
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
