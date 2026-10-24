//! Small, versioned panic record format for reset-persistent device storage.
//!
//! A panic writes one record: what crashed (kind, firmware version and build
//! commit), when (uptime), where (the panic's file and line), why (the start
//! of its message) and how it got there (return addresses, newest first).
//! The record outlives the reset it causes. Boot marks it reported, so the
//! next boot's reset is not blamed on it, but keeps it for `CRASH` until a
//! later panic replaces it or the host clears it.
//!
//! Layout, in 32-bit words:
//!
//! | Words | Holds |
//! | --- | --- |
//! | 0 | `MAGIC` |
//! | 1 | schema, kind << 8, version length << 16, commit length << 24 |
//! | 2..4 | firmware version, 8 bytes |
//! | 4..7 | build commit, 12 bytes |
//! | 7 | uptime in ms, wrapping |
//! | 8 | panic line |
//! | 9 | file length, message length << 8, frame count << 16 |
//! | 10..20 | the file path's last 40 bytes, from a `/` when cut |
//! | 20..36 | the message's first 64 bytes |
//! | 36..60 | up to `FRAMES` return addresses |
//! | 60 | zero |
//! | 61 | checksum of words 0..61 |
//! | 62 | `REPORTED` once a boot has seen the record, else 0 |
//! | 63 | `COMMITTED`, written last |

pub const WORDS: usize = 64;
/// Return addresses a record keeps.
pub const FRAMES: usize = 24;
pub const FILE_BYTES: usize = 40;
pub const MESSAGE_BYTES: usize = 64;
const VERSION_BYTES: usize = 8;
const COMMIT_BYTES: usize = 12;

const MAGIC: u32 = 0x4352_5348;
const SCHEMA: u32 = 2;
pub const COMMITTED: u32 = 0xc06c_a55a;
pub const REPORTED: u32 = 0x5345_454e;

const HEADER: usize = 1;
const VERSION: usize = 2;
const COMMIT: usize = 4;
const UPTIME: usize = 7;
const LINE: usize = 8;
const LENGTHS: usize = 9;
const FILE: usize = 10;
const MESSAGE: usize = 20;
const PCS: usize = 36;
const RESERVED: usize = PCS + FRAMES;
const CHECKSUM: usize = 61;
/// The word boot sets once it has reported the record.
pub const SEEN: usize = 62;
/// The validity word, written last.
pub const VALID: usize = 63;

const _: () = assert!(FILE + FILE_BYTES / 4 == MESSAGE);
const _: () = assert!(MESSAGE + MESSAGE_BYTES / 4 == PCS);
const _: () = assert!(RESERVED <= CHECKSUM);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Reset {
    #[default]
    Unknown,
    Power,
    Software,
    Watchdog,
    Brownout,
    Usb,
    Other,
}

impl Reset {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Power => "power",
            Self::Software => "software",
            Self::Watchdog => "watchdog",
            Self::Brownout => "brownout",
            Self::Usb => "usb",
            Self::Other => "other",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::Unknown => b"UNK",
            Self::Power => b"PWR",
            Self::Software => b"SW",
            Self::Watchdog => b"WDT",
            Self::Brownout => b"BROWN",
            Self::Usb => b"USB",
            Self::Other => b"OTHER",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum Kind {
    #[default]
    Panic = 1,
    Controlled = 2,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Controlled => "controlled",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::Panic => b"PANIC",
            Self::Controlled => b"TEST",
        }
    }
}

/// What this boot's reset says about the previous run: small enough to keep
/// in status for the whole boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub kind: Kind,
    version: [u8; VERSION_BYTES],
    version_len: u8,
}

impl Report {
    pub fn version(&self) -> &str {
        text(&self.version, self.version_len)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Marker {
    #[default]
    None,
    Valid(Report),
    Invalid,
}

impl Marker {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Valid(report) => report.kind.name(),
            Self::Invalid => "invalid",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::None => b"NONE",
            Self::Valid(report) => report.kind.short(),
            Self::Invalid => b"BAD",
        }
    }

    pub fn firmware(&self) -> &str {
        match self {
            Self::Valid(report) => report.version(),
            _ => "",
        }
    }
}

/// A panic as it is recorded, before encoding.
#[derive(Clone, Copy, Debug, Default)]
pub struct Panic<'a> {
    pub kind: Kind,
    pub version: &'a str,
    pub commit: &'a str,
    pub uptime_ms: u32,
    pub file: &'a str,
    pub line: u32,
    pub message: &'a [u8],
    pub pcs: &'a [u32],
}

/// A decoded record, read back for `CRASH`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub kind: Kind,
    /// A boot has already reported this record: it is not this boot's cause.
    pub reported: bool,
    pub uptime_ms: u32,
    pub line: u32,
    version: [u8; VERSION_BYTES],
    version_len: u8,
    commit: [u8; COMMIT_BYTES],
    commit_len: u8,
    file: [u8; FILE_BYTES],
    file_len: u8,
    message: [u8; MESSAGE_BYTES],
    message_len: u8,
    pcs: [u32; FRAMES],
    frames: u8,
}

impl Record {
    pub fn version(&self) -> &str {
        text(&self.version, self.version_len)
    }
    pub fn commit(&self) -> &str {
        text(&self.commit, self.commit_len)
    }
    /// The panic's source file, from a path component within its last
    /// `FILE_BYTES` bytes when longer.
    pub fn file(&self) -> &str {
        text(&self.file, self.file_len)
    }
    /// The start of the panic message.
    pub fn message(&self) -> &str {
        text(&self.message, self.message_len)
    }
    pub fn pcs(&self) -> &[u32] {
        &self.pcs[..usize::from(self.frames)]
    }
    fn report(&self) -> Report {
        Report {
            kind: self.kind,
            version: self.version,
            version_len: self.version_len,
        }
    }
}

/// The longest prefix of `bytes` that is valid UTF-8 (a cut can split a
/// character).
fn text(bytes: &[u8], length: u8) -> &str {
    let bytes = &bytes[..usize::from(length).min(bytes.len())];
    match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => core::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
    }
}

fn pack(words: &mut [u32], bytes: &[u8]) {
    for (index, word) in words.iter_mut().enumerate() {
        let mut chunk = [0u8; 4];
        for (offset, byte) in chunk.iter_mut().enumerate() {
            *byte = bytes.get(index * 4 + offset).copied().unwrap_or(0);
        }
        *word = u32::from_le_bytes(chunk);
    }
}

fn unpack<const N: usize>(words: &[u32]) -> [u8; N] {
    let mut bytes = [0u8; N];
    for (index, word) in words.iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

pub fn encode(panic: &Panic) -> [u32; WORDS] {
    let version = &panic.version.as_bytes()[..panic.version.len().min(VERSION_BYTES)];
    let commit = &panic.commit.as_bytes()[..panic.commit.len().min(COMMIT_BYTES)];
    // The end of a long path names the file; its start is the build machine's.
    let file = panic.file.as_bytes();
    let file = if file.len() > FILE_BYTES {
        let tail = &file[file.len() - FILE_BYTES..];
        tail.iter()
            .position(|byte| *byte == b'/')
            .map_or(tail, |slash| &tail[slash + 1..])
    } else {
        file
    };
    let message = &panic.message[..panic.message.len().min(MESSAGE_BYTES)];
    let pcs = &panic.pcs[..panic.pcs.len().min(FRAMES)];
    let mut words = [0u32; WORDS];
    words[0] = MAGIC;
    words[HEADER] = SCHEMA
        | ((panic.kind as u32) << 8)
        | ((version.len() as u32) << 16)
        | ((commit.len() as u32) << 24);
    pack(&mut words[VERSION..COMMIT], version);
    pack(&mut words[COMMIT..UPTIME], commit);
    words[UPTIME] = panic.uptime_ms;
    words[LINE] = panic.line;
    words[LENGTHS] = file.len() as u32 | ((message.len() as u32) << 8) | ((pcs.len() as u32) << 16);
    pack(&mut words[FILE..MESSAGE], file);
    pack(&mut words[MESSAGE..PCS], message);
    words[PCS..PCS + pcs.len()].copy_from_slice(pcs);
    words[CHECKSUM] = checksum(&words[..CHECKSUM]);
    words[VALID] = COMMITTED;
    words
}

/// The record in `words`, if one is there and intact.
pub fn record(words: &[u32; WORDS]) -> Option<Record> {
    if words[VALID] != COMMITTED
        || words[0] != MAGIC
        || words[HEADER] & 0xff != SCHEMA
        || words[RESERVED..CHECKSUM].iter().any(|word| *word != 0)
        || !matches!(words[SEEN], 0 | REPORTED)
        || checksum(&words[..CHECKSUM]) != words[CHECKSUM]
    {
        return None;
    }
    let kind = match (words[HEADER] >> 8) & 0xff {
        1 => Kind::Panic,
        2 => Kind::Controlled,
        _ => return None,
    };
    let version_len = (words[HEADER] >> 16) & 0xff;
    let commit_len = words[HEADER] >> 24;
    let file_len = words[LENGTHS] & 0xff;
    let message_len = (words[LENGTHS] >> 8) & 0xff;
    let frames = (words[LENGTHS] >> 16) & 0xff;
    if version_len == 0
        || version_len as usize > VERSION_BYTES
        || commit_len as usize > COMMIT_BYTES
        || file_len as usize > FILE_BYTES
        || message_len as usize > MESSAGE_BYTES
        || frames as usize > FRAMES
        || words[LENGTHS] >> 24 != 0
    {
        return None;
    }
    let mut pcs = [0u32; FRAMES];
    pcs.copy_from_slice(&words[PCS..PCS + FRAMES]);
    Some(Record {
        kind,
        reported: words[SEEN] == REPORTED,
        uptime_ms: words[UPTIME],
        line: words[LINE],
        version: unpack(&words[VERSION..COMMIT]),
        version_len: version_len as u8,
        commit: unpack(&words[COMMIT..UPTIME]),
        commit_len: commit_len as u8,
        file: unpack(&words[FILE..MESSAGE]),
        file_len: file_len as u8,
        message: unpack(&words[MESSAGE..PCS]),
        message_len: message_len as u8,
        pcs,
        frames: frames as u8,
    })
}

/// What `words` says about the reset this boot follows: a record a previous
/// boot already reported says nothing.
pub fn inspect(words: &[u32; WORDS]) -> Marker {
    if words.iter().all(|word| *word == 0) {
        return Marker::None;
    }
    match record(words) {
        Some(record) if record.reported => Marker::None,
        Some(record) => Marker::Valid(record.report()),
        None => Marker::Invalid,
    }
}

fn checksum(words: &[u32]) -> u32 {
    words.iter().fold(0x811c_9dc5, |hash, word| {
        word.to_le_bytes().iter().fold(hash, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panic() -> Panic<'static> {
        Panic {
            kind: Kind::Panic,
            version: "0.1.0",
            commit: "15178ed1119a",
            uptime_ms: 3_600_123,
            file: "apps/vana/src/screens/ride/overflowing/map.rs",
            line: 217,
            message: b"attempt to subtract with overflow",
            pcs: &[0x4200_1234, 0x4200_5678, 0x4037_9abc],
        }
    }

    #[test]
    fn a_panic_round_trips_with_where_and_how() {
        let words = encode(&panic());
        let record = record(&words).expect("valid record");
        assert_eq!(record.kind, Kind::Panic);
        assert!(!record.reported);
        assert_eq!(record.version(), "0.1.0");
        assert_eq!(record.commit(), "15178ed1119a");
        assert_eq!(record.uptime_ms, 3_600_123);
        assert_eq!(record.file(), "src/screens/ride/overflowing/map.rs");
        assert_eq!(record.line, 217);
        assert_eq!(record.message(), "attempt to subtract with overflow");
        assert_eq!(record.pcs(), &[0x4200_1234, 0x4200_5678, 0x4037_9abc]);
        let Marker::Valid(report) = inspect(&words) else {
            panic!("valid record rejected")
        };
        assert_eq!(report.kind, Kind::Panic);
        assert_eq!(report.version(), "0.1.0");
    }

    #[test]
    fn long_fields_are_cut_to_their_room() {
        let message = [b'x'; 100];
        let pcs = [0x4200_0000; 40];
        let words = encode(&Panic {
            kind: Kind::Controlled,
            version: "123456789",
            commit: "0123456789abcdef",
            message: &message,
            pcs: &pcs,
            ..panic()
        });
        let record = record(&words).unwrap();
        assert_eq!(record.version(), "12345678");
        assert_eq!(record.commit(), "0123456789ab");
        assert_eq!(record.message().len(), MESSAGE_BYTES);
        assert_eq!(record.pcs().len(), FRAMES);
        // A cut inside a character keeps only whole characters.
        let words = encode(&Panic {
            message: "é".repeat(40).as_bytes(),
            ..panic()
        });
        assert_eq!(record_message_len(&words), MESSAGE_BYTES);
        assert_eq!(super::record(&words).unwrap().message(), "é".repeat(32));
    }

    fn record_message_len(words: &[u32; WORDS]) -> usize {
        ((words[LENGTHS] >> 8) & 0xff) as usize
    }

    #[test]
    fn a_reported_record_stays_readable_but_blames_no_later_reset() {
        let mut words = encode(&panic());
        words[SEEN] = REPORTED;
        assert_eq!(inspect(&words), Marker::None);
        assert!(record(&words).unwrap().reported);
        words[SEEN] = 7;
        assert_eq!(inspect(&words), Marker::Invalid);
    }

    #[test]
    fn absent_torn_corrupt_and_outdated_records_are_safe() {
        assert_eq!(inspect(&[0; WORDS]), Marker::None);
        let valid = encode(&panic());
        for index in (0..WORDS).filter(|index| *index != SEEN) {
            let mut damaged = valid;
            damaged[index] ^= 1;
            assert_eq!(inspect(&damaged), Marker::Invalid, "word {index}");
        }
        let mut torn = valid;
        torn[VALID] = 0;
        assert_eq!(inspect(&torn), Marker::Invalid);
        let mut outdated = valid;
        outdated[HEADER] = (outdated[HEADER] & !0xff) | 1;
        assert_eq!(inspect(&outdated), Marker::Invalid);
    }
}
