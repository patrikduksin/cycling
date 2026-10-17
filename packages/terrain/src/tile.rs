//! One tile's road graph, read in place from its bytes.
//!
//! | Part | Record | Fields |
//! | --- | --- | --- |
//! | Header | 32 B | magic `VT`, version 1, flags, row, col, nodes u16, edges u16, vertices u32, lon µm per µdeg u32, reserved, CRC32 (computed with itself zeroed), reserved |
//! | Nodes | 8 B | x, y (µdeg from the tile's south-west corner), z, first adjacency index |
//! | Adjacency | 2 B | edge << 1, plus 1 when the edge ends at the node |
//! | Edges | 16 B | from node, to node, first vertex u32, vertices u16, class, flags, road group, reserved |
//! | Vertices | 6 B | x, y, z: the edge's interior points, from `from` to `to` |
//!
//! Elevation `z` is fifths of a metre above -500 m. Nodes are sorted by (y, x). A
//! node on the tile boundary continues in the neighbouring tile at the same
//! point (a portal).
use crate::pack::{Error, TileKey, u16_at, u32_at};

pub const HEADER: usize = 32;
const NODE: usize = 8;
const ADJ: usize = 2;
const EDGE: usize = 16;
const VERTEX: usize = 6;
/// `z` counts steps of `Z_STEP_DM` (0.2 m) from -500 m: up to 12,607 m.
pub const Z_STEP_DM: i32 = 2;
pub const Z_OFFSET_DM: i32 = 5000;

/// Road classes, most major first.
pub mod class {
    pub const TRUNK: u8 = 1;
    pub const PRIMARY: u8 = 2;
    pub const SECONDARY: u8 = 3;
    pub const TERTIARY: u8 = 4;
    pub const UNCLASSIFIED: u8 = 5;
    pub const RESIDENTIAL: u8 = 6;
    pub const SERVICE: u8 = 7;
    pub const TRACK: u8 = 8;
    pub const CYCLEWAY: u8 = 9;
    pub const PATH: u8 = 10;
}

/// Edge flags.
pub mod flag {
    /// Travel only from `from` to `to`.
    pub const ONEWAY: u8 = 1;
    /// A slip road between two roads.
    pub const LINK: u8 = 2;
    pub const UNPAVED: u8 = 4;
    pub const ROUNDABOUT: u8 = 8;
    pub const BRIDGE: u8 = 16;
    pub const TUNNEL: u8 = 32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub first_vertex: u32,
    pub vertices: u16,
    pub class: u8,
    pub flags: u8,
    pub group: u16,
}

impl Edge {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// A checked view of a tile's bytes.
#[derive(Clone, Copy, Debug)]
pub struct Tile<'a> {
    bytes: &'a [u8],
    pub key: TileKey,
    nodes: usize,
    edges: usize,
    vertices: usize,
    /// Micrometres per micro-degree of longitude across this tile.
    pub lon_um: u32,
}

impl<'a> Tile<'a> {
    /// Check the CRC, the key and every count against the length. Node and
    /// vertex indices inside are checked too, so accessors never panic on
    /// a pack that passes.
    pub fn parse(bytes: &'a [u8], key: TileKey, size: u32) -> Result<Self, Error> {
        if bytes.len() < HEADER || &bytes[..2] != b"VT" || bytes[2] != 1 {
            return Err(Error::Corrupt);
        }
        if crc32_skipping(bytes, 24) != u32_at(bytes, 24) {
            return Err(Error::Corrupt);
        }
        let tile = Self::view(bytes);
        let length = HEADER
            + tile.nodes * NODE
            + 2 * tile.edges * ADJ
            + tile.edges * EDGE
            + tile.vertices * VERTEX;
        if tile.key != key || length != bytes.len() || tile.lon_um == 0 {
            return Err(Error::Corrupt);
        }
        let size = size.min(u32::from(u16::MAX)) as u16;
        for index in 0..tile.nodes {
            let node = tile.node(index as u16);
            let start = tile.adjacency_start(index);
            if node.x > size || node.y > size || start > 2 * tile.edges {
                return Err(Error::Corrupt);
            }
        }
        for index in 0..tile.edges {
            let edge = tile.edge(index as u16);
            if usize::from(edge.from) >= tile.nodes
                || usize::from(edge.to) >= tile.nodes
                || edge.first_vertex as usize + usize::from(edge.vertices) > tile.vertices
            {
                return Err(Error::Corrupt);
            }
        }
        for entry in 0..2 * tile.edges {
            if usize::from(tile.adjacency_entry(entry) >> 1) >= tile.edges {
                return Err(Error::Corrupt);
            }
        }
        Ok(tile)
    }

    /// The header fields of bytes `parse` has already accepted.
    pub(crate) fn view(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            key: TileKey {
                row: u16_at(bytes, 4),
                col: u16_at(bytes, 6),
            },
            nodes: usize::from(u16_at(bytes, 8)),
            edges: usize::from(u16_at(bytes, 10)),
            vertices: u32_at(bytes, 12) as usize,
            lon_um: u32_at(bytes, 16),
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes
    }

    pub fn edge_count(&self) -> usize {
        self.edges
    }

    pub fn node(&self, index: u16) -> Node {
        let at = HEADER + usize::from(index) * NODE;
        Node {
            x: u16_at(self.bytes, at),
            y: u16_at(self.bytes, at + 2),
            z: u16_at(self.bytes, at + 4),
        }
    }

    fn adjacency_start(&self, index: usize) -> usize {
        usize::from(u16_at(self.bytes, HEADER + index * NODE + 6))
    }

    fn adjacency_entry(&self, entry: usize) -> u16 {
        u16_at(self.bytes, HEADER + self.nodes * NODE + entry * ADJ)
    }

    /// Edges at `node`: (edge, true when the edge ends here).
    pub fn incident(&self, node: u16) -> impl Iterator<Item = (u16, bool)> + '_ {
        let index = usize::from(node);
        let start = self.adjacency_start(index);
        let end = if index + 1 < self.nodes {
            self.adjacency_start(index + 1)
        } else {
            2 * self.edges
        };
        (start..end.max(start)).map(move |entry| {
            let value = self.adjacency_entry(entry);
            (value >> 1, value & 1 == 1)
        })
    }

    pub fn edge(&self, index: u16) -> Edge {
        let at = HEADER + self.nodes * NODE + 2 * self.edges * ADJ + usize::from(index) * EDGE;
        let b = self.bytes;
        Edge {
            from: u16_at(b, at),
            to: u16_at(b, at + 2),
            first_vertex: u32_at(b, at + 4),
            vertices: u16_at(b, at + 8),
            class: b[at + 10],
            flags: b[at + 11],
            group: u16_at(b, at + 12),
        }
    }

    /// Point `index` of an edge from its `from` node (0) to its `to` node
    /// (`points(edge) - 1`), as a node.
    pub fn point(&self, edge: &Edge, index: usize) -> Node {
        if index == 0 {
            return self.node(edge.from);
        }
        if index > usize::from(edge.vertices) {
            return self.node(edge.to);
        }
        let base = HEADER + self.nodes * NODE + 2 * self.edges * ADJ + self.edges * EDGE;
        let at = base + (edge.first_vertex as usize + index - 1) * VERTEX;
        Node {
            x: u16_at(self.bytes, at),
            y: u16_at(self.bytes, at + 2),
            z: u16_at(self.bytes, at + 4),
        }
    }

    pub fn points(edge: &Edge) -> usize {
        usize::from(edge.vertices) + 2
    }

    /// The node at exactly (x, y), by binary search over the (y, x) order.
    pub fn node_at(&self, x: u16, y: u16) -> Option<u16> {
        let (mut low, mut high) = (0usize, self.nodes);
        while low < high {
            let middle = (low + high) / 2;
            let node = self.node(middle as u16);
            match (node.y, node.x).cmp(&(y, x)) {
                core::cmp::Ordering::Less => low = middle + 1,
                core::cmp::Ordering::Greater => high = middle,
                core::cmp::Ordering::Equal => return Some(middle as u16),
            }
        }
        None
    }
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

fn crc_update(mut value: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        value = (value >> 8) ^ CRC_TABLE[((value as u8) ^ byte) as usize];
    }
    value
}

/// CRC-32 (IEEE), as zlib computes it.
pub fn crc32(bytes: &[u8]) -> u32 {
    !crc_update(u32::MAX, bytes)
}

/// CRC-32 of `bytes` with the four bytes at `at` read as zero.
fn crc32_skipping(bytes: &[u8], at: usize) -> u32 {
    let value = crc_update(u32::MAX, &bytes[..at]);
    let value = crc_update(value, &[0; 4]);
    !crc_update(value, &bytes[at + 4..])
}
