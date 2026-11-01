//! What a trace's header holds beyond its own fields: what the device
//! started from. [`Config`] is the header's `config`, the settings VANA and
//! the shell started with and the working memory composition could give
//! VANA; [`Media`] is its `media`, where each medium stood at power-on.
//! Composition gathers both. A replay starts from them, so a device trace
//! replays with the media it names and nothing else beside it.
//!
//! A later version may append fields; this one reads its known prefix.
use super::{Reader, Wire, Writer};
use crate::volume::{self, Layout, Volume};
use device_api::bulk::{Error as MediaError, ReadWrite};

/// The header's `config`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config<'a> {
    /// The shell's settings record as it persists it, with VANA's
    /// preferences, the saved ANT sensors and the saved BLE sensor inside.
    pub settings: &'a [u8],
    pub memory: Memory,
    /// Secrets were replaced as they were recorded; see [`super::redact`].
    pub redacted: bool,
}

/// The working memory composition gave VANA at power-on. Without each
/// block VANA does without its feature: no terrain cache is NO MAP on the
/// climb page, no breadcrumb is NO TRACK and no course.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub terrain: bool,
    pub back_to_start: bool,
    pub phone: bool,
    pub ride_index: bool,
    pub ride_facts: bool,
}

impl Memory {
    pub const ALL: Self = Self {
        terrain: true,
        back_to_start: true,
        phone: true,
        ride_index: true,
        ride_facts: true,
    };
}

/// The header's `media`. VANA reports the ride journals; `None` is one it
/// did not report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Media<'a> {
    pub journal: Option<Journal>,
    pub rides: Option<RideLog>,
    pub files: Found<Files<'a>>,
    /// VANA's ride facts table, `rides/facts.tbl` in FILES, by the digest
    /// of its slots (`vana::ride::facts::digest`); `None` without a table
    /// or when it did not read.
    pub facts: Option<u32>,
    pub terrain: Found<Terrain>,
}

/// A volume as composition found it at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found<T> {
    /// Nothing there: no card, no volume table, no such volume, or one
    /// nothing was ever written to.
    Absent,
    Read(T),
    /// There, but it did not read.
    Unreadable(Failure),
}

/// Why a volume that is there did not read at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The card did not answer.
    Card = 0,
    /// The volume table was damaged or of another version.
    Table = 1,
    /// The volume's header or directory was damaged or of another version.
    Volume = 2,
}

/// The internal ride journal when VANA's boot scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Journal {
    Scanned {
        next: u32,
        /// The CRC32 of the slot before `next`; `None` when the journal is
        /// empty or that slot did not read.
        last: Option<u32>,
    },
    /// The scan could not read the journal.
    Unusable,
}

/// The card's ride log as VANA's boot scan found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RideLog {
    /// No ride log: RIDES, or the volume table, was never initialized.
    Uninitialized,
    /// A log whose end the scan proved: its epoch's store id and head.
    Log { store: u64, head: u64 },
    /// A log whose end the scan could not prove, so VANA kept it read only:
    /// `head` is one past the last valid record it saw.
    Uncertain { store: u64, head: u64 },
    /// No log VANA could use: the volume table or the log's metadata was
    /// corrupt, lost or foreign, or the card did not read.
    Unusable,
}

/// The FILES store, and every file in it, as [`FileEntry`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Files<'a> {
    pub store: u64,
    pub generation: u64,
    pub entries: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileEntry<'a> {
    pub name: &'a str,
    pub length: u32,
    pub crc: u32,
    pub revision: u32,
}

/// The TERRAIN pack's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terrain {
    pub ready: bool,
    pub generation: u64,
    pub length: u64,
    pub tag: [u8; 16],
    pub digest: [u8; 32],
}

impl<'a> Config<'a> {
    /// Its bytes in `out`; `None` when they do not fit.
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        encode(self, out)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        decode(bytes)
    }
}

impl<'a> Media<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        encode(self, out)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        decode(bytes)
    }
}

impl<'a> Files<'a> {
    pub fn entries(&self) -> impl Iterator<Item = FileEntry<'a>> + use<'a> {
        let mut r = Reader::new(self.entries, 0);
        core::iter::from_fn(move || {
            if r.at >= r.bytes.len() {
                return None;
            }
            let entry = FileEntry::take(&mut r);
            r.ok().then_some(entry)
        })
    }
}

impl FileEntry<'_> {
    /// Append it to the entries in `out[..*len]`; false when it does not fit.
    pub fn append(&self, out: &mut [u8], len: &mut usize) -> bool {
        let Some(rest) = out.get_mut(*len..) else {
            return false;
        };
        let mut w = Writer::new(rest, 0);
        self.put(&mut w);
        if w.overflow {
            return false;
        }
        *len += w.len;
        true
    }
}

/// FILES and TERRAIN as they stand on the owned region `bulk`: what
/// [`Media`] holds besides the ride journals, which VANA reports. The file
/// entries go into `entries`; files that do not fit are a damaged directory.
pub fn volumes<'e>(
    bulk: &mut impl ReadWrite,
    entries: &'e mut [u8],
    sector: &mut volume::Sector,
) -> (Found<Files<'e>>, Found<Terrain>) {
    use crate::{files, pack};
    let table = match volume::read(bulk, sector) {
        Ok(Layout::Table(table)) => table,
        Ok(Layout::Blank | Layout::Legacy) | Err(volume::Error::Media(MediaError::Unsupported)) => {
            return (Found::Absent, Found::Absent);
        }
        Ok(Layout::Lost)
        | Err(volume::Error::Foreign | volume::Error::Missing | volume::Error::Small) => {
            return (
                Found::Unreadable(Failure::Table),
                Found::Unreadable(Failure::Table),
            );
        }
        Err(volume::Error::Media(_)) => {
            return (
                Found::Unreadable(Failure::Card),
                Found::Unreadable(Failure::Card),
            );
        }
    };
    let store = |error: files::Error| match error {
        files::Error::Media(_) => Failure::Card,
        _ => Failure::Volume,
    };
    let files = table.find(&volume::FILES).map_or(Ok(None), |extent| {
        let mut files = Volume::new(&mut *bulk, extent);
        let dir = match files::mount(&mut files, sector) {
            Ok(dir) => dir,
            Err(files::Error::Uninitialized) => return Ok(None),
            Err(error) => return Err(store(error)),
        };
        let mut len = 0;
        for index in 0..files::DIR_SECTORS {
            for entry in dir
                .entries(&mut files, index, sector)
                .map_err(store)?
                .iter()
                .flatten()
            {
                let entry = FileEntry {
                    name: entry.name.as_str(),
                    length: entry.length,
                    crc: entry.crc,
                    revision: entry.revision,
                };
                if !entry.append(entries, &mut len) {
                    return Err(Failure::Volume);
                }
            }
        }
        Ok(Some((dir.store_id(), dir.generation(), len)))
    });
    let terrain = table.find(&volume::TERRAIN).map_or(Ok(None), |extent| {
        let header = pack::status(&mut Volume::new(&mut *bulk, extent), sector).map_err(
            |error| match error {
                pack::Error::Media(_) => Failure::Card,
                _ => Failure::Volume,
            },
        )?;
        Ok(header.map(|header| Terrain {
            ready: header.state == pack::State::Ready,
            generation: header.generation,
            length: header.spec.length,
            tag: header.spec.tag,
            digest: header.spec.digest,
        }))
    });
    let entries: &'e [u8] = entries;
    let files = Found::from(files).map(|(store, generation, len)| Files {
        store,
        generation,
        entries: &entries[..len],
    });
    (files, terrain.into())
}

impl<T> From<Result<Option<T>, Failure>> for Found<T> {
    fn from(read: Result<Option<T>, Failure>) -> Self {
        match read {
            Ok(Some(value)) => Self::Read(value),
            Ok(None) => Self::Absent,
            Err(failure) => Self::Unreadable(failure),
        }
    }
}

impl<T> Found<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Found<U> {
        match self {
            Self::Absent => Found::Absent,
            Self::Read(value) => Found::Read(f(value)),
            Self::Unreadable(failure) => Found::Unreadable(failure),
        }
    }
}

/// The header block of a trace this build records, from what the device
/// started from, in `out`; `config` and `media` are encoded through
/// `scratch` first. `None` when either does not fit.
pub fn header(
    commit: &str,
    boot: u32,
    profile: u8,
    config: &Config<'_>,
    media: &Media<'_>,
    scratch: &mut [u8],
    out: &mut [u8],
) -> Option<usize> {
    let configured = config.encode(scratch)?;
    let (config, rest) = scratch.split_at_mut(configured);
    let media = media.encode(rest)?;
    let header = super::Header {
        version: super::VERSION,
        oldest_reader: super::OLDEST_READER,
        build: build(commit),
        boot,
        profile,
        config,
        media: &rest[..media],
    };
    super::encode_header(&header, out)
}

/// A commit as a header's `build`: its hex digits as bytes, as many as fit
/// and given, zero after them. A short commit is a prefix of the full one.
/// A build with uncommitted changes (`<commit>-dirty`) is zero, unknown,
/// since no commit holds what it ran.
pub fn build(commit: &str) -> [u8; 20] {
    let mut id = [0u8; 20];
    if commit.ends_with("-dirty") {
        return id;
    }
    for (byte, pair) in id.iter_mut().zip(commit.as_bytes().chunks(2)) {
        *byte = core::str::from_utf8(pair)
            .ok()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .unwrap_or(0);
    }
    id
}

/// The FILES store's id and generation and the TERRAIN pack's generation,
/// which change with every write a host makes to them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Generations {
    pub files: Option<(u64, u64)>,
    pub terrain: Option<u64>,
}

/// [`Generations`] as they stand on the owned region `bulk`.
pub fn generations(bulk: &mut impl ReadWrite, sector: &mut volume::Sector) -> Generations {
    let Ok(Layout::Table(table)) = volume::read(bulk, sector) else {
        return Generations::default();
    };
    let files = table.find(&volume::FILES).and_then(|extent| {
        let dir = crate::files::mount(&mut Volume::new(&mut *bulk, extent), sector).ok()?;
        Some((dir.store_id(), dir.generation()))
    });
    let terrain = table.find(&volume::TERRAIN).and_then(|extent| {
        let header = crate::pack::status(&mut Volume::new(&mut *bulk, extent), sector).ok()??;
        Some(header.generation)
    });
    Generations { files, terrain }
}

/// Whether two stamps name the same commit: the shorter, up to its last
/// non-zero byte, starts the longer. A zero stamp names no commit.
pub fn same_build(a: &[u8; 20], b: &[u8; 20]) -> bool {
    let named = |build: &[u8; 20]| build.iter().rposition(|&byte| byte != 0).map(|i| i + 1);
    match (named(a), named(b)) {
        (Some(a_len), Some(b_len)) => {
            let len = a_len.min(b_len);
            a[..len] == b[..len]
        }
        _ => false,
    }
}

/// Room for every file FILES can hold in [`Files::entries`].
pub const FILE_ENTRIES: usize = crate::files::CAPACITY * (1 + crate::files::NAME + 3 * 5 + 4);

fn encode<'a, T: Wire<'a>>(value: &T, out: &mut [u8]) -> Option<usize> {
    let mut w = Writer::new(out, 0);
    value.put(&mut w);
    (!w.overflow).then_some(w.len)
}

fn decode<'a, T: Wire<'a>>(bytes: &'a [u8]) -> Option<T> {
    let mut r = Reader::new(bytes, 0);
    let value = T::take(&mut r);
    r.ok().then_some(value)
}

impl<'a> Wire<'a> for Config<'a> {
    fn put(&self, w: &mut Writer) {
        self.settings.put(w);
        self.memory.put(w);
        self.redacted.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        Self {
            settings: Wire::take(r),
            memory: Wire::take(r),
            redacted: Wire::take(r),
        }
    }
}

impl Wire<'_> for Memory {
    fn put(&self, w: &mut Writer) {
        let bits = [
            self.terrain,
            self.back_to_start,
            self.phone,
            self.ride_index,
            self.ride_facts,
        ];
        let byte = bits
            .iter()
            .enumerate()
            .fold(0u8, |byte, (bit, on)| byte | u8::from(*on) << bit);
        w.byte(byte);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let byte = r.byte();
        let bit = |n: u8| byte & 1 << n != 0;
        Self {
            terrain: bit(0),
            back_to_start: bit(1),
            phone: bit(2),
            ride_index: bit(3),
            ride_facts: bit(4),
        }
    }
}

impl<'a> Wire<'a> for Media<'a> {
    fn put(&self, w: &mut Writer) {
        self.journal.put(w);
        self.rides.put(w);
        self.files.put(w);
        self.facts.put(w);
        self.terrain.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        Self {
            journal: Wire::take(r),
            rides: Wire::take(r),
            files: Wire::take(r),
            facts: Wire::take(r),
            terrain: Wire::take(r),
        }
    }
}

impl Wire<'_> for Journal {
    fn put(&self, w: &mut Writer) {
        match *self {
            Self::Scanned { next, last } => {
                w.byte(0);
                next.put(w);
                last.map(u32::to_le_bytes).put(w);
            }
            Self::Unusable => w.byte(1),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Scanned {
                next: Wire::take(r),
                last: <Option<[u8; 4]> as Wire>::take(r).map(u32::from_le_bytes),
            },
            1 => Self::Unusable,
            _ => {
                r.fail();
                Self::Unusable
            }
        }
    }
}

impl<'a, T: Wire<'a>> Wire<'a> for Found<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Absent => w.byte(0),
            Self::Read(value) => {
                w.byte(1);
                value.put(w);
            }
            Self::Unreadable(failure) => {
                w.byte(2);
                w.byte(*failure as u8);
            }
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        match r.byte() {
            0 => Self::Absent,
            1 => Self::Read(T::take(r)),
            2 => match r.byte() {
                0 => Self::Unreadable(Failure::Card),
                1 => Self::Unreadable(Failure::Table),
                2 => Self::Unreadable(Failure::Volume),
                _ => {
                    r.fail();
                    Self::Absent
                }
            },
            _ => {
                r.fail();
                Self::Absent
            }
        }
    }
}

impl Wire<'_> for RideLog {
    fn put(&self, w: &mut Writer) {
        match *self {
            Self::Uninitialized => w.byte(0),
            Self::Log { store, head } => {
                w.byte(1);
                (store, head).put(w);
            }
            Self::Uncertain { store, head } => {
                w.byte(2);
                (store, head).put(w);
            }
            Self::Unusable => w.byte(3),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Uninitialized,
            1 => {
                let (store, head) = Wire::take(r);
                Self::Log { store, head }
            }
            2 => {
                let (store, head) = Wire::take(r);
                Self::Uncertain { store, head }
            }
            3 => Self::Unusable,
            _ => {
                r.fail();
                Self::Unusable
            }
        }
    }
}

impl<'a> Wire<'a> for Files<'a> {
    fn put(&self, w: &mut Writer) {
        self.store.put(w);
        self.generation.put(w);
        self.entries.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        Self {
            store: Wire::take(r),
            generation: Wire::take(r),
            entries: Wire::take(r),
        }
    }
}

impl<'a> Wire<'a> for FileEntry<'a> {
    fn put(&self, w: &mut Writer) {
        self.name.put(w);
        self.length.put(w);
        w.bytes(&self.crc.to_le_bytes());
        self.revision.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        Self {
            name: Wire::take(r),
            length: Wire::take(r),
            crc: u32::from_le_bytes(<[u8; 4]>::take(r)),
            revision: Wire::take(r),
        }
    }
}

impl Wire<'_> for Terrain {
    fn put(&self, w: &mut Writer) {
        self.ready.put(w);
        self.generation.put(w);
        self.length.put(w);
        w.bytes(&self.tag);
        w.bytes(&self.digest);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self {
            ready: Wire::take(r),
            generation: Wire::take(r),
            length: Wire::take(r),
            tag: Wire::take(r),
            digest: Wire::take(r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn config_and_media_decode_to_what_was_encoded() {
        let config = Config {
            settings: b"cycling\x06settings",
            memory: Memory {
                terrain: false,
                ..Memory::ALL
            },
            redacted: true,
        };
        let mut out = [0u8; 64];
        let len = config.encode(&mut out).unwrap();
        assert_eq!(Config::decode(&out[..len]), Some(config));
        assert!(config.encode(&mut out[..8]).is_none());

        let mut entries = vec![0u8; 256];
        let mut used = 0;
        let files = [
            FileEntry {
                name: "course",
                length: 1_200,
                crc: 0xdead_beef,
                revision: 3,
            },
            FileEntry {
                name: "rides/1-2.workout",
                length: 90,
                crc: 7,
                revision: 1,
            },
        ];
        for file in &files {
            assert!(file.append(&mut entries, &mut used));
        }
        let media = Media {
            journal: Some(Journal::Scanned {
                next: 41,
                last: Some(0xdead_beef),
            }),
            rides: Some(RideLog::Uncertain {
                store: 0x1234,
                head: 900,
            }),
            files: Found::Read(Files {
                store: 5,
                generation: 77,
                entries: &entries[..used],
            }),
            facts: Some(0x2144_df1c),
            terrain: Found::Read(Terrain {
                ready: true,
                generation: 2,
                length: 1 << 30,
                tag: *b"CHILE-V1\0\0\0\0\0\0\0\0",
                digest: [9; 32],
            }),
        };
        let mut out = [0u8; 512];
        let len = media.encode(&mut out).unwrap();
        let decoded = Media::decode(&out[..len]).unwrap();
        assert_eq!(decoded, media);
        let Found::Read(listed) = decoded.files else {
            panic!("{:?}", decoded.files);
        };
        assert_eq!(listed.entries().collect::<Vec<_>>(), files);
        let none = Media {
            journal: None,
            rides: None,
            files: Found::Absent,
            facts: None,
            terrain: Found::Absent,
        };
        let len = none.encode(&mut out).unwrap();
        assert_eq!(Media::decode(&out[..len]), Some(none));
        for rides in [
            RideLog::Uninitialized,
            RideLog::Log { store: 7, head: 3 },
            RideLog::Uncertain { store: 7, head: 3 },
            RideLog::Unusable,
        ] {
            let media = Media {
                rides: Some(rides),
                ..none
            };
            let len = media.encode(&mut out).unwrap();
            assert_eq!(Media::decode(&out[..len]), Some(media));
        }
        for failure in [Failure::Card, Failure::Table, Failure::Volume] {
            let media = Media {
                journal: Some(Journal::Unusable),
                files: Found::Unreadable(failure),
                terrain: Found::Unreadable(failure),
                ..none
            };
            let len = media.encode(&mut out).unwrap();
            assert_eq!(Media::decode(&out[..len]), Some(media));
        }
    }

    #[test]
    fn a_volume_that_does_not_read_is_unreadable_and_not_absent() {
        use crate::testing::Media as Card;
        use crate::{checksum::crc32, files, pack};
        let mut card = Card::new(5_777_407);
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let mut entries = vec![0u8; FILE_ENTRIES];
        let read = |card: &mut Card, entries: &mut [u8]| {
            let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
            let (files, terrain) = volumes(card, entries, &mut sector);
            (files.map(|files| files.store), terrain.map(|t| t.length))
        };
        assert_eq!(
            read(&mut card, &mut entries),
            (Found::Absent, Found::Absent),
            "no volume table"
        );
        let table = volume::create(&mut card, &mut sector).unwrap();
        assert_eq!(
            read(&mut card, &mut entries),
            (Found::Absent, Found::Absent),
            "nothing written"
        );

        let mut store = volume::open(&mut card, &volume::FILES, &mut sector).unwrap();
        let dir = files::format(&mut store, 7, &mut sector).unwrap();
        let name = files::Name::new("course.dat").unwrap();
        files::write(&mut store, &name, b"course", &mut sector).unwrap();
        let mut terrain = volume::open(&mut card, &volume::TERRAIN, &mut sector).unwrap();
        let spec = pack::Spec {
            length: 512,
            crc: 0,
            tag: [1; 16],
            digest: [2; 32],
            stamp: 0,
        };
        pack::begin(&mut terrain, spec, &mut sector).unwrap();
        assert_eq!(
            read(&mut card, &mut entries),
            (Found::Read(dir.store_id()), Found::Read(512))
        );

        let files_at = table.find(&volume::FILES).unwrap().start;
        card.set(files_at + 1, [0x5a; 512]);
        card.set(files_at + files::COPY_B + 1, [0x5a; 512]);
        let pack_at = table.find(&volume::TERRAIN).unwrap().start;
        for copy in [pack_at, pack_at + 1] {
            let mut header = card.get(copy);
            if header[..16] == pack::MAGIC[..] {
                header[16] = 9;
                let crc = crc32(&header[..508]).to_le_bytes();
                header[508..].copy_from_slice(&crc);
                card.set(copy, header);
            }
        }
        assert_eq!(
            read(&mut card, &mut entries),
            (
                Found::Unreadable(Failure::Volume),
                Found::Unreadable(Failure::Volume)
            ),
            "a directory sector that fails its CRC, a pack header of another version"
        );
        card.off = true;
        assert_eq!(
            read(&mut card, &mut entries),
            (
                Found::Unreadable(Failure::Card),
                Found::Unreadable(Failure::Card)
            )
        );
    }

    #[test]
    fn a_build_names_its_commit_and_a_dirty_build_names_none() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        let mut stamped = [0u8; 20];
        stamped[..6].copy_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
        assert_eq!(build(&full[..12]), stamped, "the C606 stamps 12 digits");
        assert_eq!(build(full)[..6], stamped[..6]);
        assert_eq!(build(full)[19], 0x67);
        assert_eq!(build(&std::format!("{full}-dirty")), [0; 20]);
        assert_eq!(build(""), [0; 20]);
    }
}
