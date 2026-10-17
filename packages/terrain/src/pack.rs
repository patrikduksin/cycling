//! Pack header, tile directory and the byte source they are read from.
//!
//! Layout (little-endian; `docs/terrain.md` is the reference):
//!
//! | Offset | Bytes | Content |
//! | --- | --- | --- |
//! | 0 | 512 | Header: magic, version, grid, offsets, CRC32 in its last 4 bytes |
//! | 512 | n × 512 | Attribution text (UTF-8), zero padded |
//! | `directory` | rows × cols × 8 | Per tile, row-major from the south-west: byte offset u32, length u32 (0 = no roads) |
//! | `data` | | Tiles, back to back |

/// Where pack bytes come from: a volume, a file or memory.
pub trait Source {
    /// Fill `buf` from `offset` bytes into the pack. A short or failed read
    /// is an error; the caller retries later or gives up.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
}

impl Source for &[u8] {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Range)?;
        let end = start.checked_add(buf.len()).ok_or(Error::Range)?;
        buf.copy_from_slice(self.get(start..end).ok_or(Error::Range)?);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The source failed to read.
    Media,
    /// Outside the pack or a value out of its range.
    Range,
    /// Not a pack this firmware reads (magic, version or geometry).
    Unsupported,
    /// A checksum or structural check failed.
    Corrupt,
}

pub const MAGIC: &[u8; 16] = b"VANA-TERRAIN\0\0\0\0";
pub const VERSION: u16 = 2;
pub const HEADER_BYTES: usize = 512;
pub const ENTRY_BYTES: usize = 8;

/// A tile by grid row (from the south) and column (from the west).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Tile side in micro-degrees.
    pub tile: u32,
    /// South-west corner of the grid, micro-degrees.
    pub south: i32,
    pub west: i32,
    pub rows: u16,
    pub cols: u16,
    pub directory: u32,
    pub data: u32,
    pub total: u32,
    pub tiles: u32,
    /// Seconds since 1970 of the OSM data.
    pub created: u64,
    pub attribution_offset: u32,
    pub attribution_len: u32,
}

impl Header {
    pub fn parse(sector: &[u8; HEADER_BYTES]) -> Result<Self, Error> {
        if crate::tile::crc32(&sector[..HEADER_BYTES - 4]) != u32_at(sector, HEADER_BYTES - 4) {
            return Err(Error::Corrupt);
        }
        if &sector[..16] != MAGIC || u16_at(sector, 16) != VERSION {
            return Err(Error::Unsupported);
        }
        let header = Self {
            tile: u32_at(sector, 20),
            south: u32_at(sector, 24) as i32,
            west: u32_at(sector, 28) as i32,
            rows: u16_at(sector, 32),
            cols: u16_at(sector, 34),
            directory: u32_at(sector, 36),
            data: u32_at(sector, 40),
            total: u32_at(sector, 44),
            tiles: u32_at(sector, 48),
            created: u64::from(u32_at(sector, 52)) | u64::from(u32_at(sector, 56)) << 32,
            attribution_offset: u32_at(sector, 60),
            attribution_len: u32_at(sector, 64),
        };
        let entries = u64::from(header.rows) * u64::from(header.cols) * ENTRY_BYTES as u64;
        // Tile-local coordinates are u16; the grid must fit the pack.
        if header.tile == 0
            || header.tile > u32::from(u16::MAX)
            || header.rows == 0
            || header.cols == 0
            || u64::from(header.directory) + entries != u64::from(header.data)
            || header.data > header.total
        {
            return Err(Error::Unsupported);
        }
        Ok(header)
    }

    /// The tile holding a point, if it lies in the grid.
    pub fn key(&self, lat: i32, lon: i32) -> Option<TileKey> {
        let row = (i64::from(lat) - i64::from(self.south)).div_euclid(i64::from(self.tile));
        let col = (i64::from(lon) - i64::from(self.west)).div_euclid(i64::from(self.tile));
        self.checked(row, col)
    }

    pub fn checked(&self, row: i64, col: i64) -> Option<TileKey> {
        ((0..i64::from(self.rows)).contains(&row) && (0..i64::from(self.cols)).contains(&col))
            .then_some(TileKey {
                row: row as u16,
                col: col as u16,
            })
    }

    /// South-west corner of a tile, micro-degrees.
    pub fn origin(&self, key: TileKey) -> (i32, i32) {
        (
            self.south + i32::from(key.row) * self.tile as i32,
            self.west + i32::from(key.col) * self.tile as i32,
        )
    }

    /// Byte offset of a tile's directory entry.
    pub fn entry(&self, key: TileKey) -> u64 {
        u64::from(self.directory)
            + (u64::from(key.row) * u64::from(self.cols) + u64::from(key.col)) * ENTRY_BYTES as u64
    }

    /// A directory entry's tile: (offset, length), `None` for no roads.
    pub fn locate(&self, entry: &[u8; ENTRY_BYTES]) -> Result<Option<(u32, u32)>, Error> {
        let offset = u32_at(entry, 0);
        let length = u32_at(entry, 4);
        if length == 0 {
            return Ok(None);
        }
        if offset < self.data || u64::from(offset) + u64::from(length) > u64::from(self.total) {
            return Err(Error::Corrupt);
        }
        Ok(Some((offset, length)))
    }
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
