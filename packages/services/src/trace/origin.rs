//! What a trace's header holds of where the device started, and what
//! composition tells the trace as it opens: the terrain pack the session
//! found, and the working memory it could give VANA. A world log holds
//! every other medium in its own records.
use super::{Reader, Wire, Writer};
use crate::volume::{self, Layout, Volume};
use device_api::bulk::{Error as MediaError, ReadWrite};

/// The working memory composition gave VANA at power-on. Without each
/// block VANA does without its feature: no terrain cache is NO MAP on the
/// climb page, no breadcrumb is NO TRACK and no course.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub terrain: bool,
    pub track: bool,
    pub phone: bool,
    pub ride_index: bool,
    pub ride_facts: bool,
}

impl Memory {
    pub const ALL: Self = Self {
        terrain: true,
        track: true,
        phone: true,
        ride_index: true,
        ride_facts: true,
    };
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

/// The TERRAIN pack's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terrain {
    pub ready: bool,
    pub generation: u64,
    pub length: u64,
    pub tag: [u8; 16],
    pub digest: [u8; 32],
}

/// The TERRAIN pack on the owned region `bulk`, as a session finds it.
pub fn terrain(bulk: &mut impl ReadWrite, sector: &mut volume::Sector) -> Found<Terrain> {
    use crate::pack;
    let table = match volume::read(bulk, sector) {
        Ok(Layout::Table(table)) => table,
        Ok(Layout::Blank) | Err(volume::Error::Media(MediaError::Unsupported)) => {
            return Found::Absent;
        }
        Ok(Layout::Lost)
        | Err(volume::Error::Foreign | volume::Error::Missing | volume::Error::Small) => {
            return Found::Unreadable(Failure::Table);
        }
        Err(volume::Error::Media(_)) => return Found::Unreadable(Failure::Card),
    };
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
    terrain.into()
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

#[derive(Clone, Copy, Debug)]
pub struct Stamp<'a> {
    /// The build's content hash in hex, as `cycling_devtools.build_hash` prints it.
    pub build: &'a str,
    /// The device's manifest name.
    pub device: &'a str,
    pub boot: u32,
}

/// The header block of a trace this build records, in `out`; `None` when
/// it does not fit.
pub fn header(stamp: Stamp<'_>, terrain: Found<Terrain>, out: &mut [u8]) -> Option<usize> {
    let header = super::Header {
        build: build(stamp.build),
        device: stamp.device,
        boot: stamp.boot,
        terrain,
    };
    super::encode_header(&header, out)
}

/// A build hash as a header's `build`: its forty hex digits as bytes.
/// Anything else, such as no hash or an unstamped placeholder, is zero:
/// unknown.
pub fn build(hash: &str) -> [u8; 20] {
    let mut id = [0u8; 20];
    if hash.len() != 2 * id.len() {
        return [0; 20];
    }
    for (byte, pair) in id.iter_mut().zip(hash.as_bytes().chunks(2)) {
        let Some(parsed) = core::str::from_utf8(pair)
            .ok()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        else {
            return [0; 20];
        };
        *byte = parsed;
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

impl Wire<'_> for Memory {
    fn put(&self, w: &mut Writer) {
        let bits = [
            self.terrain,
            self.track,
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
            track: bit(1),
            phone: bit(2),
            ride_index: bit(3),
            ride_facts: bit(4),
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

    #[test]
    fn a_pack_that_does_not_read_is_unreadable_and_not_absent() {
        use crate::testing::Media as Card;
        use crate::{checksum::crc32, pack};
        let mut card = Card::new(5_777_407);
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let read = |card: &mut Card| {
            let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
            terrain(card, &mut sector).map(|pack| pack.length)
        };
        assert_eq!(read(&mut card), Found::Absent, "no volume table");
        let table = volume::create(&mut card, &mut sector).unwrap();
        assert_eq!(read(&mut card), Found::Absent, "nothing written");

        let mut volume = volume::open(&mut card, &volume::TERRAIN, &mut sector).unwrap();
        let spec = pack::Spec {
            length: 512,
            crc: 0,
            tag: [1; 16],
            digest: [2; 32],
            stamp: 0,
        };
        pack::begin(&mut volume, spec, &mut sector).unwrap();
        assert_eq!(read(&mut card), Found::Read(512));

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
            read(&mut card),
            Found::Unreadable(Failure::Volume),
            "a pack header of another version"
        );
        card.off = true;
        assert_eq!(read(&mut card), Found::Unreadable(Failure::Card));
    }

    #[test]
    fn a_build_is_its_hash_and_anything_else_is_zero() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(build(hash)[..3], [0x01, 0x23, 0x45]);
        assert_eq!(build(hash)[19], 0x67);
        assert_eq!(build(""), [0; 20]);
        assert_eq!(build(&hash[..12]), [0; 20]);
        assert_eq!(
            build("zcyclingzhashzplaceholderzzzzzzzzzzzzzzz"),
            [0; 20],
            "devices/build.sh's placeholder, in an image it did not stamp"
        );
    }
}
