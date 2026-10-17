//! Named volumes inside the owned block region.
//!
//! Relative owned sectors 0 and 1 hold two copies of a volume table: magic,
//! version, generation, the region size it was planned for and up to
//! `MAX_VOLUMES` named extents, sealed by a CRC32. Readers take the valid copy
//! of the newest generation. Writers write copy 1 and then copy 0, so a cut
//! leaves either table whole. Each volume is an independent block device
//! ([`Volume`]) whose relative sector 0 is its first sector, so a mechanism
//! written for the whole region (the sector log) runs unchanged inside it, and
//! no access through a volume can reach another volume or the table.
//!
//! Before volumes existed the sector log owned the whole region; its
//! superblocks sit where the table now does. Reading reports such a region as
//! [`Layout::Legacy`] and never as a blank one. Only [`create`], an explicit
//! initialize, writes a table, and it refuses a table it does not understand.

use crate::checksum::crc32;
use crate::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use device_api::bulk::{Error as MediaError, OwnedInfo, ReadWrite, SECTOR_SIZE};

pub type Sector = [u8; SECTOR_SIZE];
pub type Name = [u8; 8];

pub const MAGIC: &[u8; 16] = b"CYCLING-VOLUMES\0";
pub const VERSION: u16 = 1;
pub const MAX_VOLUMES: usize = 8;
/// Sectors before any volume: both table copies.
pub const TABLE: u64 = 2;
/// The default plan starts volumes on 1 MiB boundaries.
pub const ALIGN: u64 = 2048;

/// Rides: the sector log of VANA ride records.
pub const RIDES: Name = *b"RIDES\0\0\0";
/// Host-written read-mostly terrain pack (`crate::pack`).
pub const TERRAIN: Name = *b"TERRAIN\0";
/// Small named files (`crate::files`).
pub const FILES: Name = *b"FILES\0\0\0";

const MIB: u64 = 2048;
/// Default sizes: about 2,000 h of rides, a terrain pack of several hundred MB
/// with room to grow, and a small file store. The rest stays unallocated.
pub const RIDES_SECTORS: u64 = 1024 * MIB;
pub const FILES_SECTORS: u64 = 256 * MIB;
pub const TERRAIN_SECTORS: u64 = 1280 * MIB;

const CRC: usize = SECTOR_SIZE - 4;
const ENTRIES: usize = 64;
const ENTRY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Media(MediaError),
    /// A valid table this version cannot use: newer version, unknown kinds or
    /// extents that do not fit this region. Never overwritten.
    Foreign,
    /// The named volume is not in the table.
    Missing,
    /// The region is too small for any plan.
    Small,
}

impl From<MediaError> for Error {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

/// What a volume holds; the owning mechanism checks its own format inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Log = 1,
    Pack = 2,
    Files = 3,
}

impl Kind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Log),
            2 => Some(Self::Pack),
            3 => Some(Self::Files),
            _ => None,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Pack => "pack",
            Self::Files => "files",
        }
    }
}

/// A named extent of owned sectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub name: Name,
    pub kind: Kind,
    pub start: u64,
    pub sectors: u64,
}

impl Extent {
    /// The name without padding.
    pub fn label(&self) -> &str {
        label(&self.name)
    }
}

pub fn label(name: &Name) -> &str {
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    core::str::from_utf8(&name[..end]).unwrap_or("?")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Table {
    pub generation: u64,
    /// Region sectors when the table was planned. The region may grow later.
    pub planned: u64,
    volumes: [Option<Extent>; MAX_VOLUMES],
}

impl Table {
    pub fn volumes(&self) -> impl Iterator<Item = &Extent> {
        self.volumes.iter().flatten()
    }

    pub fn find(&self, name: &Name) -> Option<Extent> {
        self.volumes().find(|v| v.name == *name).copied()
    }

    /// Sectors after the last volume, reserved for later volumes.
    pub fn unallocated(&self, sectors: u64) -> u64 {
        let end = self.volumes().map(|v| v.start + v.sectors).max();
        sectors.saturating_sub(end.unwrap_or(TABLE))
    }

    fn encode(&self, sector: &mut Sector) {
        sector.fill(0);
        sector[..16].copy_from_slice(MAGIC);
        put_u16(sector, 16, VERSION);
        put_u16(sector, 18, self.volumes().count() as u16);
        put_u32(sector, 20, SECTOR_SIZE as u32);
        put_u64(sector, 24, self.generation);
        put_u64(sector, 32, self.planned);
        for (index, volume) in self.volumes().enumerate() {
            let entry = &mut sector[ENTRIES + index * ENTRY..][..ENTRY];
            entry[..8].copy_from_slice(&volume.name);
            entry[8] = volume.kind as u8;
            put_u64(entry, 16, volume.start);
            put_u64(entry, 24, volume.sectors);
        }
        let checksum = crc32(&sector[..CRC]);
        put_u32(sector, CRC, checksum);
    }
}

/// What the table sectors hold.
// Without an allocator there is no boxing; a layout is read once per use.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Table(Table),
    /// Neither copy is a table or a superblock: an unformatted region.
    Blank,
    /// The sector log from before volumes owns the whole region.
    Legacy,
    /// Neither table copy validates, but the default plan's RIDES volume
    /// holds a log: the table was damaged. [`create`] rewrites the default
    /// table, which restores access when the region used it.
    Lost,
}

impl Layout {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Table(_) => "volumes",
            Self::Blank => "blank",
            Self::Legacy => "legacy",
            Self::Lost => "lost",
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum Replica {
    Table(Table),
    Foreign,
    Legacy,
    Invalid,
}

fn decode(sector: &Sector, region: u64) -> Replica {
    if sector[..16] != MAGIC[..] {
        return if crate::sector_log::is_superblock(sector) {
            Replica::Legacy
        } else {
            Replica::Invalid
        };
    }
    if get_u32(sector, CRC) != crc32(&sector[..CRC]) {
        return Replica::Invalid;
    }
    let count = usize::from(get_u16(sector, 18));
    if get_u16(sector, 16) != VERSION
        || get_u32(sector, 20) != SECTOR_SIZE as u32
        || count > MAX_VOLUMES
        || sector[40..ENTRIES].iter().any(|b| *b != 0)
        || sector[ENTRIES + count * ENTRY..CRC].iter().any(|b| *b != 0)
    {
        return Replica::Foreign;
    }
    let mut volumes = [None; MAX_VOLUMES];
    // Extents are sorted, disjoint, after the table area and inside the region.
    let mut end = TABLE;
    for (index, slot) in volumes.iter_mut().take(count).enumerate() {
        let entry = &sector[ENTRIES + index * ENTRY..][..ENTRY];
        let mut name = [0; 8];
        name.copy_from_slice(&entry[..8]);
        let start = get_u64(entry, 16);
        let sectors = get_u64(entry, 24);
        let Some(kind) = Kind::from_u8(entry[8]) else {
            return Replica::Foreign;
        };
        if name[0] == 0
            || entry[9..16].iter().any(|b| *b != 0)
            || start < end
            || sectors == 0
            || start.checked_add(sectors).is_none_or(|e| e > region)
        {
            return Replica::Foreign;
        }
        end = start + sectors;
        *slot = Some(Extent {
            name,
            kind,
            start,
            sectors,
        });
    }
    for (index, a) in volumes.iter().flatten().enumerate() {
        if volumes
            .iter()
            .flatten()
            .skip(index + 1)
            .any(|b| b.name == a.name)
        {
            return Replica::Foreign;
        }
    }
    Replica::Table(Table {
        generation: get_u64(sector, 24),
        planned: get_u64(sector, 32),
        volumes,
    })
}

/// Read both table copies: two sector reads, two more when neither is valid.
pub fn read(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Layout, Error> {
    let region = media.owned_info()?.sectors;
    let mut copies = [Replica::Invalid, Replica::Invalid];
    for (index, copy) in copies.iter_mut().enumerate() {
        media.owned_read(index as u64, sector)?;
        *copy = decode(sector, region);
    }
    match copies {
        [Replica::Foreign, _] | [_, Replica::Foreign] => Err(Error::Foreign),
        [Replica::Table(a), Replica::Table(b)] => {
            Ok(Layout::Table(if b.generation > a.generation {
                b
            } else {
                a
            }))
        }
        [Replica::Table(t), _] | [_, Replica::Table(t)] => Ok(Layout::Table(t)),
        [Replica::Legacy, _] | [_, Replica::Legacy] => Ok(Layout::Legacy),
        [Replica::Invalid, Replica::Invalid] => {
            // Records of a lost table's log must not look like a blank region.
            let Ok(table) = plan(region) else {
                return Ok(Layout::Blank);
            };
            let rides = table.find(&RIDES).ok_or(Error::Small)?;
            for copy in 0..2 {
                media.owned_read(rides.start + copy, sector)?;
                if crate::sector_log::is_superblock(sector) {
                    return Ok(Layout::Lost);
                }
            }
            Ok(Layout::Blank)
        }
    }
}

/// The default layout for a region of `sectors`: RIDES, FILES and TERRAIN at
/// their default sizes, or RIDES alone on a region too small for them (test
/// and simulator media).
pub fn plan(sectors: u64) -> Result<Table, Error> {
    let mut volumes = [None; MAX_VOLUMES];
    if sectors >= ALIGN + RIDES_SECTORS + FILES_SECTORS + TERRAIN_SECTORS {
        let mut start = ALIGN;
        for (slot, (name, kind, size)) in volumes.iter_mut().zip([
            (RIDES, Kind::Log, RIDES_SECTORS),
            (FILES, Kind::Files, FILES_SECTORS),
            (TERRAIN, Kind::Pack, TERRAIN_SECTORS),
        ]) {
            *slot = Some(Extent {
                name,
                kind,
                start,
                sectors: size,
            });
            start += size;
        }
    } else {
        // Small media keep the table area minimal and give rides the rest.
        let rides = sectors.checked_sub(TABLE).filter(|s| *s > 0);
        volumes[0] = Some(Extent {
            name: RIDES,
            kind: Kind::Log,
            start: TABLE,
            sectors: rides.ok_or(Error::Small)?,
        });
    }
    Ok(Table {
        generation: 1,
        planned: sectors,
        volumes,
    })
}

/// Write the default table over a blank, legacy or lost one and return it.
/// An existing table is returned unchanged; a foreign one is refused. Writes
/// two sectors when it creates a table; the legacy log's sectors stay
/// unreadable behind it: its superblocks are gone and its store id never
/// validates in a new log.
pub fn create(media: &mut impl ReadWrite, sector: &mut Sector) -> Result<Table, Error> {
    let layout = read(media, sector)?;
    if let Layout::Table(table) = layout {
        return Ok(table);
    }
    let table = plan(media.owned_info()?.sectors)?;
    table.encode(sector);
    for copy in [1, 0] {
        media.owned_write(copy, sector)?;
    }
    Ok(table)
}

/// The volume named `name`, from the current table.
pub fn open<'a, M: ReadWrite>(
    media: &'a mut M,
    name: &Name,
    sector: &mut Sector,
) -> Result<Volume<'a, M>, Error> {
    match read(media, sector)? {
        Layout::Table(table) => {
            let extent = table.find(name).ok_or(Error::Missing)?;
            Ok(Volume::new(media, extent))
        }
        Layout::Blank | Layout::Legacy | Layout::Lost => Err(Error::Missing),
    }
}

/// One volume as its own block device: relative sector 0 is its first sector
/// and every access past its end is out of range.
pub struct Volume<'a, M> {
    media: &'a mut M,
    extent: Extent,
}

impl<'a, M: ReadWrite> Volume<'a, M> {
    pub fn new(media: &'a mut M, extent: Extent) -> Self {
        Self { media, extent }
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    fn address(&self, sector: u64) -> Result<u64, MediaError> {
        if sector >= self.extent.sectors {
            return Err(MediaError::Range);
        }
        Ok(self.extent.start + sector)
    }
}

impl<M: ReadWrite> ReadWrite for Volume<'_, M> {
    fn owned_info(&mut self) -> Result<OwnedInfo, MediaError> {
        let base = self.media.owned_info()?;
        if self.extent.start + self.extent.sectors > base.sectors {
            return Err(MediaError::Range);
        }
        Ok(OwnedInfo {
            start_sector: base.start_sector + self.extent.start,
            sectors: self.extent.sectors,
            sector_size: base.sector_size,
        })
    }

    fn owned_read(&mut self, sector: u64, output: &mut Sector) -> Result<(), MediaError> {
        let address = self.address(sector)?;
        self.media.owned_read(address, output)
    }

    fn owned_write(&mut self, sector: u64, input: &Sector) -> Result<(), MediaError> {
        let address = self.address(sector)?;
        self.media.owned_write(address, input)
    }

    fn entropy(&mut self) -> u64 {
        self.media.entropy()
    }
}

#[cfg(test)]
mod tests;
