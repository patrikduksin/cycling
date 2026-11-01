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
use device_api::bulk::ReadWrite;

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

/// The header's `media`. A medium that could not be read is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Media<'a> {
    pub journal: Option<Journal>,
    /// The card's ride log when VANA's boot scan ended: its epoch's store id
    /// and head. RIDES only grows, so a host rewinds a later copy to this.
    pub rides: Option<(u64, u64)>,
    pub files: Option<Files<'a>>,
    /// VANA's ride facts table, `rides/facts.tbl` in FILES, by the digest
    /// of its slots (`vana::ride::facts::digest`); `None` without a table.
    /// VANA rewrites it as rides are saved, and each of its slots ends in
    /// its own CRC32, so its entry's CRC tells only its length.
    pub facts: Option<u32>,
    pub terrain: Option<Terrain>,
}

/// The internal ride journal when VANA's boot scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Journal {
    /// Its next slot.
    pub next: u32,
    /// The CRC32 of the slot before `next`; `None` when the journal is
    /// empty or that slot did not read. The journal only grows until a clear
    /// starts it again from slot 0, so a host tells a later copy that still
    /// holds the slots of power-on from one cleared and filled again since.
    pub last: Option<u32>,
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
/// entries go into `entries`; a volume, store or pack that does not read,
/// or files that do not fit, are `None`.
pub fn volumes<'e>(
    bulk: &mut impl ReadWrite,
    entries: &'e mut [u8],
    sector: &mut volume::Sector,
) -> (Option<Files<'e>>, Option<Terrain>) {
    let Ok(Layout::Table(table)) = volume::read(bulk, sector) else {
        return (None, None);
    };
    let files = table.find(&volume::FILES).and_then(|extent| {
        let mut files = Volume::new(&mut *bulk, extent);
        let dir = crate::files::mount(&mut files, sector).ok()?;
        let mut len = 0;
        for index in 0..crate::files::DIR_SECTORS {
            for entry in dir
                .entries(&mut files, index, sector)
                .ok()?
                .iter()
                .flatten()
            {
                let entry = FileEntry {
                    name: entry.name.as_str(),
                    length: entry.length,
                    crc: entry.crc,
                    revision: entry.revision,
                };
                entry.append(entries, &mut len).then_some(())?;
            }
        }
        Some((dir.store_id(), dir.generation(), len))
    });
    let terrain = table.find(&volume::TERRAIN).and_then(|extent| {
        let header = crate::pack::status(&mut Volume::new(&mut *bulk, extent), sector).ok()??;
        Some(Terrain {
            ready: header.state == crate::pack::State::Ready,
            generation: header.generation,
            length: header.spec.length,
            tag: header.spec.tag,
            digest: header.spec.digest,
        })
    });
    let entries: &'e [u8] = entries;
    let files = files.map(|(store, generation, len)| Files {
        store,
        generation,
        entries: &entries[..len],
    });
    (files, terrain)
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
        self.next.put(w);
        self.last.map(u32::to_le_bytes).put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self {
            next: Wire::take(r),
            last: <Option<[u8; 4]> as Wire>::take(r).map(u32::from_le_bytes),
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
            journal: Some(Journal {
                next: 41,
                last: Some(0xdead_beef),
            }),
            rides: Some((0x1234, 900)),
            files: Some(Files {
                store: 5,
                generation: 77,
                entries: &entries[..used],
            }),
            facts: Some(0x2144_df1c),
            terrain: Some(Terrain {
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
        let listed: Vec<FileEntry<'_>> = decoded.files.unwrap().entries().collect();
        assert_eq!(listed, files);
        let none = Media {
            journal: None,
            rides: None,
            files: None,
            facts: None,
            terrain: None,
        };
        let len = none.encode(&mut out).unwrap();
        assert_eq!(Media::decode(&out[..len]), Some(none));
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
