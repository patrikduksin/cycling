//! Binary volume transfer session, independent of USB.
//!
//! `VOLUME TRANSFER` on the ordinary terminal hands the transport to this
//! session until `END` or an idle timeout. Requests and replies are a 36-byte
//! little-endian header followed by `length` payload bytes:
//!
//! | offset | request (`C6VQ`) / reply (`C6VP`) |
//! | --- | --- |
//! | 0 | magic |
//! | 4 | request: op u16, 0 u16 / reply: status u16, op u16 |
//! | 8 | id u32, strictly increasing within a session |
//! | 12 | arg u64 |
//! | 20 | count u32 |
//! | 24 | payload length u32, at most `MAX_PAYLOAD` |
//! | 28 | payload CRC32 |
//! | 32 | CRC32 of bytes 0..32 |
//!
//! Each request gets exactly one reply. Raw writes exist only for the
//! terrain pack's data sectors in order (`pack::Writer`) and file data before
//! its commit (`files::Writer`); every other write is a header or directory
//! commit owned by those mechanisms. Nothing here touches RIDES.

use device_api::bulk::ReadWrite;
use firmware_services::checksum::crc32;
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::volume::{self, Layout};
use firmware_services::{files, pack};

pub const HEADER: usize = 36;
pub const MAX_PAYLOAD: usize = 4096;
pub const VERSION: u32 = 1;
pub const REQUEST: &[u8; 4] = b"C6VQ";
pub const REPLY: &[u8; 4] = b"C6VP";
/// The session ends after this long without a request.
pub const IDLE_MS: u64 = 30_000;

pub mod op {
    pub const HELLO: u16 = 1;
    pub const END: u16 = 2;
    pub const VOLUMES: u16 = 3;
    pub const TERRAIN_STATUS: u16 = 10;
    pub const TERRAIN_BEGIN: u16 = 11;
    pub const TERRAIN_RESUME: u16 = 12;
    pub const TERRAIN_WRITE: u16 = 13;
    pub const TERRAIN_VERIFY: u16 = 14;
    pub const TERRAIN_COMMIT: u16 = 15;
    pub const TERRAIN_CRC: u16 = 16;
    pub const FILES_STATUS: u16 = 20;
    pub const FILES_FORMAT: u16 = 21;
    pub const FILES_LIST: u16 = 22;
    pub const FILES_BEGIN: u16 = 23;
    pub const FILES_WRITE: u16 = 24;
    pub const FILES_COMMIT: u16 = 25;
    pub const FILES_READ: u16 = 26;
    pub const FILES_DELETE: u16 = 27;
    pub const FILES_STAT: u16 = 28;
}

/// Reply statuses; the host maps them to messages.
pub mod status {
    pub const OK: u16 = 0;
    pub const INVALID: u16 = 1;
    pub const MEDIA: u16 = 2;
    pub const STATE: u16 = 3;
    pub const RANGE: u16 = 4;
    pub const MISMATCH: u16 = 5;
    pub const STALE: u16 = 6;
    pub const FULL: u16 = 7;
    pub const NOT_FOUND: u16 = 8;
    pub const FOREIGN: u16 = 9;
    pub const UNINITIALIZED: u16 = 10;
    pub const TOO_LARGE: u16 = 11;
    pub const CORRUPT: u16 = 12;
    pub const EMPTY: u16 = 13;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub op: u16,
    pub id: u32,
    pub arg: u64,
    pub count: u32,
    pub length: u32,
    pub payload_crc: u32,
}

/// Decode a request header; `None` if it is not one.
pub fn request(header: &[u8; HEADER]) -> Option<Request> {
    if header[..4] != REQUEST[..]
        || get_u32(header, 32) != crc32(&header[..32])
        || get_u16(header, 6) != 0
    {
        return None;
    }
    let request = Request {
        op: get_u16(header, 4),
        id: get_u32(header, 8),
        arg: get_u64(header, 12),
        count: get_u32(header, 20),
        length: get_u32(header, 24),
        payload_crc: get_u32(header, 28),
    };
    (request.length as usize <= MAX_PAYLOAD).then_some(request)
}

/// Encode a request header, for hosts and tests.
pub fn encode_request(request: &Request) -> [u8; HEADER] {
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(REQUEST);
    put_u16(&mut header, 4, request.op);
    put_u32(&mut header, 8, request.id);
    put_u64(&mut header, 12, request.arg);
    put_u32(&mut header, 20, request.count);
    put_u32(&mut header, 24, request.length);
    put_u32(&mut header, 28, request.payload_crc);
    let checksum = crc32(&header[..32]);
    put_u32(&mut header, 32, checksum);
    header
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub arg: u64,
    pub count: u32,
    /// Payload bytes at the start of the output buffer.
    pub length: usize,
}

impl Reply {
    fn ok(arg: u64, count: u32, length: usize) -> Self {
        Self {
            status: status::OK,
            arg,
            count,
            length,
        }
    }
    fn error(status: u16) -> Self {
        Self {
            status,
            arg: 0,
            count: 0,
            length: 0,
        }
    }
}

pub fn encode_reply(request: &Request, reply: &Reply, payload: &[u8]) -> [u8; HEADER] {
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(REPLY);
    put_u16(&mut header, 4, reply.status);
    put_u16(&mut header, 6, request.op);
    put_u32(&mut header, 8, request.id);
    put_u64(&mut header, 12, reply.arg);
    put_u32(&mut header, 20, reply.count);
    put_u32(&mut header, 24, payload.len() as u32);
    put_u32(&mut header, 28, crc32(payload));
    let checksum = crc32(&header[..32]);
    put_u32(&mut header, 32, checksum);
    header
}

fn volume_error(error: volume::Error) -> u16 {
    match error {
        volume::Error::Media(_) => status::MEDIA,
        volume::Error::Foreign => status::FOREIGN,
        volume::Error::Missing | volume::Error::Small => status::UNINITIALIZED,
    }
}

fn pack_error(error: pack::Error) -> u16 {
    match error {
        pack::Error::Media(_) => status::MEDIA,
        pack::Error::Foreign => status::FOREIGN,
        pack::Error::Empty => status::EMPTY,
        pack::Error::State => status::STATE,
        pack::Error::Range => status::RANGE,
        pack::Error::Mismatch => status::MISMATCH,
        pack::Error::Stale => status::STALE,
    }
}

fn files_error(error: files::Error) -> u16 {
    match error {
        files::Error::Media(_) => status::MEDIA,
        files::Error::Uninitialized => status::UNINITIALIZED,
        files::Error::Foreign => status::FOREIGN,
        files::Error::Corrupt => status::CORRUPT,
        files::Error::Name => status::INVALID,
        files::Error::TooLarge => status::TOO_LARGE,
        files::Error::Full => status::FULL,
        files::Error::NotFound => status::NOT_FOUND,
        files::Error::Stale => status::STALE,
        files::Error::Range => status::RANGE,
        files::Error::Mismatch => status::MISMATCH,
    }
}

/// Fixed-size header encoding shared by `TERRAIN_*` replies: state (0 empty,
/// 1 writing, 2 ready), generation, length, CRC, tag, digest, stamp, the
/// volume's byte capacity and the session writer's cursor.
const TERRAIN_INFO: usize = 1 + 8 + 8 + 4 + 16 + 32 + 8 + 8 + 8;

fn terrain_info(header: Option<pack::Header>, capacity: u64, cursor: u64, out: &mut [u8]) -> usize {
    out[..TERRAIN_INFO].fill(0);
    if let Some(h) = header {
        out[0] = match h.state {
            pack::State::Writing => 1,
            pack::State::Ready => 2,
        };
        put_u64(out, 1, h.generation);
        put_u64(out, 9, h.spec.length);
        put_u32(out, 17, h.spec.crc);
        out[21..37].copy_from_slice(&h.spec.tag);
        out[37..69].copy_from_slice(&h.spec.digest);
        put_u64(out, 69, h.spec.stamp);
    }
    put_u64(out, 77, capacity);
    put_u64(out, 85, cursor);
    TERRAIN_INFO
}

/// Bytes before the entries of a `VOLUMES` reply.
const TABLE_HEAD: usize = 40;

/// File entry encoding: slot u16, name 40 (zero padded), length, CRC, revision.
const ENTRY: usize = 2 + files::NAME + 12;

fn file_entry(slot: usize, entry: &files::Entry, out: &mut [u8]) -> usize {
    out[..ENTRY].fill(0);
    put_u16(out, 0, slot as u16);
    let name = entry.name.as_str().as_bytes();
    out[2..2 + name.len()].copy_from_slice(name);
    put_u32(out, 2 + files::NAME, entry.length);
    put_u32(out, 6 + files::NAME, entry.crc);
    put_u32(out, 10 + files::NAME, entry.revision);
    ENTRY
}

fn name(payload: &[u8]) -> Option<files::Name> {
    files::Name::new(core::str::from_utf8(payload).ok()?).ok()
}

/// One session's state: the open terrain and file writers.
#[derive(Default)]
pub struct Session {
    last_id: u32,
    terrain: Option<pack::Writer>,
    file: Option<files::Writer>,
    ended: bool,
}

impl Session {
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Serve one request whose payload was received and CRC-checked. The
    /// reply payload is written to the start of `out`.
    pub fn handle(
        &mut self,
        request: &Request,
        payload: &[u8],
        media: &mut impl ReadWrite,
        out: &mut [u8; MAX_PAYLOAD],
    ) -> Reply {
        if request.id <= self.last_id || payload.len() != request.length as usize {
            return Reply::error(status::INVALID);
        }
        self.last_id = request.id;
        let mut sector = [0u8; 512];
        match request.op {
            op::HELLO => {
                let sectors = match media.owned_info() {
                    Ok(info) => info.sectors,
                    Err(_) => return Reply::error(status::MEDIA),
                };
                put_u64(out, 0, sectors);
                put_u32(out, 8, MAX_PAYLOAD as u32);
                put_u32(out, 12, VERSION);
                Reply::ok(0, 0, 16)
            }
            op::END => {
                self.ended = true;
                Reply::ok(0, 0, 0)
            }
            op::VOLUMES => {
                let region = match media.owned_info() {
                    Ok(info) => info.sectors,
                    Err(_) => return Reply::error(status::MEDIA),
                };
                // Layout code, then region, generation, planned and
                // unallocated sectors at 8..40, then 25-byte entries.
                out[..TABLE_HEAD].fill(0);
                put_u64(out, 8, region);
                let mut length = TABLE_HEAD;
                match volume::read(media, &mut sector) {
                    Ok(Layout::Blank) => out[0] = 0,
                    Ok(Layout::Legacy) => out[0] = 1,
                    Ok(Layout::Lost) => out[0] = 4,
                    Ok(Layout::Table(table)) => {
                        out[0] = 2;
                        put_u64(out, 16, table.generation);
                        put_u64(out, 24, table.planned);
                        put_u64(out, 32, table.unallocated(region));
                        for v in table.volumes() {
                            let e = &mut out[length..length + 25];
                            e[..8].copy_from_slice(&v.name);
                            e[8] = v.kind as u8;
                            put_u64(e, 9, v.start);
                            put_u64(e, 17, v.sectors);
                            length += 25;
                        }
                    }
                    Err(volume::Error::Foreign) => out[0] = 3,
                    Err(e) => return Reply::error(volume_error(e)),
                }
                Reply::ok(0, ((length - TABLE_HEAD) / 25) as u32, length)
            }
            op::TERRAIN_STATUS..=op::TERRAIN_CRC => self.terrain(request, payload, media, out),
            op::FILES_STATUS..=op::FILES_STAT => self.files(request, payload, media, out),
            _ => Reply::error(status::INVALID),
        }
    }

    fn terrain(
        &mut self,
        request: &Request,
        payload: &[u8],
        media: &mut impl ReadWrite,
        out: &mut [u8; MAX_PAYLOAD],
    ) -> Reply {
        let mut sector = [0u8; 512];
        let mut vol = match volume::open(media, &volume::TERRAIN, &mut sector) {
            Ok(v) => v,
            Err(e) => return Reply::error(volume_error(e)),
        };
        let capacity = pack::capacity(vol.extent().sectors);
        let cursor = self.terrain.map_or(0, |w| w.cursor());
        let info = |header, cursor, out: &mut [u8; MAX_PAYLOAD]| {
            let n = terrain_info(header, capacity, cursor, out);
            Reply::ok(cursor, 0, n)
        };
        match request.op {
            op::TERRAIN_STATUS => match pack::status(&mut vol, &mut sector) {
                Ok(header) => info(header, cursor, out),
                Err(e) => Reply::error(pack_error(e)),
            },
            op::TERRAIN_BEGIN => {
                if payload.len() != 68 {
                    return Reply::error(status::INVALID);
                }
                let mut tag = [0; 16];
                tag.copy_from_slice(&payload[12..28]);
                let mut digest = [0; 32];
                digest.copy_from_slice(&payload[28..60]);
                let spec = pack::Spec {
                    length: get_u64(payload, 0),
                    crc: get_u32(payload, 8),
                    tag,
                    digest,
                    stamp: get_u64(payload, 60),
                };
                self.terrain = None;
                match pack::begin(&mut vol, spec, &mut sector) {
                    Ok(writer) => {
                        self.terrain = Some(writer);
                        info(Some(writer.header()), 0, out)
                    }
                    Err(e) => Reply::error(pack_error(e)),
                }
            }
            op::TERRAIN_RESUME => match pack::Writer::resume(&mut vol, &mut sector) {
                Ok(writer) => {
                    self.terrain = Some(writer);
                    info(Some(writer.header()), 0, out)
                }
                Err(e) => Reply::error(pack_error(e)),
            },
            op::TERRAIN_WRITE => {
                let Some(writer) = self.terrain.as_mut() else {
                    return Reply::error(status::STATE);
                };
                if payload.is_empty()
                    || !payload.len().is_multiple_of(512)
                    || request.arg != writer.cursor()
                {
                    return Reply::error(status::RANGE);
                }
                let (sectors, _) = payload.as_chunks::<512>();
                for data in sectors {
                    let index = writer.cursor();
                    if let Err(e) = writer.write(&mut vol, index, data) {
                        // The cursor stays at the sector that failed.
                        return Reply {
                            arg: writer.cursor(),
                            ..Reply::error(pack_error(e))
                        };
                    }
                }
                Reply::ok(writer.cursor(), 0, 0)
            }
            op::TERRAIN_VERIFY => {
                let Some(writer) = self.terrain.as_mut() else {
                    return Reply::error(status::STATE);
                };
                if payload.len() != 4 || request.arg != writer.cursor() {
                    return Reply::error(status::RANGE);
                }
                let expected = get_u32(payload, 0);
                match writer.verify(&mut vol, u64::from(request.count), expected, &mut sector) {
                    Ok(verified) => {
                        put_u32(out, 0, verified.crc);
                        out[4] = u8::from(verified.advanced);
                        Reply::ok(writer.cursor(), u32::from(verified.advanced), 5)
                    }
                    Err(e) => Reply::error(pack_error(e)),
                }
            }
            op::TERRAIN_COMMIT => {
                let Some(writer) = self.terrain else {
                    return Reply::error(status::STATE);
                };
                match writer.commit(&mut vol, &mut sector) {
                    Ok(header) => {
                        self.terrain = None;
                        info(Some(header), 0, out)
                    }
                    Err(e) => Reply::error(pack_error(e)),
                }
            }
            op::TERRAIN_CRC => {
                match pack::crc(&mut vol, request.arg, u64::from(request.count), &mut sector) {
                    Ok(crc) => {
                        put_u32(out, 0, crc);
                        Reply::ok(request.arg, request.count, 4)
                    }
                    Err(e) => Reply::error(pack_error(e)),
                }
            }
            _ => Reply::error(status::INVALID),
        }
    }

    fn files(
        &mut self,
        request: &Request,
        payload: &[u8],
        media: &mut impl ReadWrite,
        out: &mut [u8; MAX_PAYLOAD],
    ) -> Reply {
        let mut sector = [0u8; 512];
        let mut vol = match volume::open(media, &volume::FILES, &mut sector) {
            Ok(v) => v,
            Err(e) => return Reply::error(volume_error(e)),
        };
        if request.op == op::FILES_FORMAT {
            self.file = None;
            return match files::format(&mut vol, request.arg, &mut sector) {
                Ok(dir) => Reply::ok(dir.generation(), 0, 0),
                Err(e) => Reply::error(files_error(e)),
            };
        }
        if request.op == op::FILES_WRITE {
            let Some(writer) = self.file.as_mut() else {
                return Reply::error(status::STATE);
            };
            if request.arg != u64::from(writer.written()) {
                return Reply::error(status::RANGE);
            }
            return match writer.write(&mut vol, payload) {
                Ok(()) => Reply::ok(u64::from(writer.written()), 0, 0),
                Err(e) => {
                    self.file = None;
                    Reply::error(files_error(e))
                }
            };
        }
        if request.op == op::FILES_COMMIT {
            let Some(writer) = self.file.take() else {
                return Reply::error(status::STATE);
            };
            return match writer.commit(&mut vol, request.count, &mut sector) {
                Ok(entry) => {
                    let n = file_entry(0, &entry, out);
                    Reply::ok(0, 0, n)
                }
                Err(e) => Reply::error(files_error(e)),
            };
        }
        let dir = match files::mount(&mut vol, &mut sector) {
            Ok(dir) => dir,
            Err(e) => return Reply::error(files_error(e)),
        };
        match request.op {
            op::FILES_STATUS => {
                put_u64(out, 0, dir.generation());
                put_u64(out, 8, dir.store_id());
                put_u32(out, 16, dir.len() as u32);
                put_u32(out, 20, files::CAPACITY as u32);
                put_u32(out, 24, files::MAX_FILE);
                put_u64(out, 28, dir.data_sectors());
                Reply::ok(0, dir.len() as u32, 36)
            }
            op::FILES_LIST => {
                // Slots [arg, arg + count), as many entries as fit.
                let start = request.arg.min(files::CAPACITY as u64) as usize;
                let end = start
                    .saturating_add(request.count as usize)
                    .min(files::CAPACITY);
                let mut length = 0;
                let mut slot = start;
                while slot < end && length + ENTRY <= MAX_PAYLOAD {
                    match dir.entry(&mut vol, slot, &mut sector) {
                        Ok(Some(e)) => length += file_entry(slot, &e, &mut out[length..]),
                        Ok(None) => {}
                        Err(e) => return Reply::error(files_error(e)),
                    }
                    slot += 1;
                }
                // `arg` is where to continue.
                Reply::ok(slot as u64, (length / ENTRY) as u32, length)
            }
            op::FILES_BEGIN => {
                let Some(name) = name(payload) else {
                    return Reply::error(status::INVALID);
                };
                self.file = None;
                match dir.begin(&mut vol, &name, request.count, &mut sector) {
                    Ok(writer) => {
                        self.file = Some(writer);
                        Reply::ok(0, request.count, 0)
                    }
                    Err(e) => Reply::error(files_error(e)),
                }
            }
            op::FILES_READ | op::FILES_STAT | op::FILES_DELETE => {
                let Some(name) = name(payload) else {
                    return Reply::error(status::INVALID);
                };
                if request.op == op::FILES_DELETE {
                    return match dir.delete(&mut vol, &name, &mut sector) {
                        Ok(_) => Reply::ok(0, 0, 0),
                        Err(e) => Reply::error(files_error(e)),
                    };
                }
                let entry = match dir.find(&mut vol, &name, &mut sector) {
                    Ok(Some(e)) => e,
                    Ok(None) => return Reply::error(status::NOT_FOUND),
                    Err(e) => return Reply::error(files_error(e)),
                };
                if request.op == op::FILES_STAT {
                    let n = file_entry(0, &entry, out);
                    return Reply::ok(0, 0, n);
                }
                let Ok(offset) = u32::try_from(request.arg) else {
                    return Reply::error(status::RANGE);
                };
                let wanted = (request.count as usize).min(MAX_PAYLOAD);
                match dir.read(&mut vol, &entry, offset, &mut out[..wanted], &mut sector) {
                    Ok(n) => Reply::ok(request.arg, entry.crc, n),
                    Err(e) => Reply::error(files_error(e)),
                }
            }
            _ => Reply::error(status::INVALID),
        }
    }
}

#[cfg(test)]
mod tests;
