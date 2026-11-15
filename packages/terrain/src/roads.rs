//! The roads of a cached tile, for drawing a map.
//!
//! A map asks [`Header::tiles_under`] which tiles its view touches, asks
//! [`Lookahead::want`](crate::Lookahead::want) for them, and draws each
//! tile [`Lookahead::roads`](crate::Lookahead::roads) returns. Roads come as
//! a class, flags and absolute points; the tile records stay private.
use crate::pack::{Header, TileKey};
use crate::tile::{Edge, Tile, class};

/// What kind of road, most major first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoadClass {
    Trunk,
    Primary,
    Secondary,
    Tertiary,
    Unclassified,
    Residential,
    Service,
    Track,
    Cycleway,
    /// Paths and bridleways, and any class byte this firmware does not know.
    Path,
}

impl RoadClass {
    fn parse(byte: u8) -> Self {
        match byte {
            class::TRUNK => Self::Trunk,
            class::PRIMARY => Self::Primary,
            class::SECONDARY => Self::Secondary,
            class::TERTIARY => Self::Tertiary,
            class::UNCLASSIFIED => Self::Unclassified,
            class::RESIDENTIAL => Self::Residential,
            class::SERVICE => Self::Service,
            class::TRACK => Self::Track,
            class::CYCLEWAY => Self::Cycleway,
            _ => Self::Path,
        }
    }

    /// A major road on the map: trunk, primary and secondary.
    pub fn major(self) -> bool {
        self <= Self::Secondary
    }
}

/// One road between two junctions (or a junction and a tile edge).
#[derive(Clone, Copy, Debug)]
pub struct Road<'t> {
    tile: Tile<'t>,
    edge: Edge,
    /// The tile's south-west corner, micro-degrees.
    origin: (i32, i32),
}

impl<'t> Road<'t> {
    pub(crate) fn all(tile: Tile<'t>, origin: (i32, i32)) -> impl Iterator<Item = Road<'t>> {
        (0..tile.edge_count() as u16).map(move |index| Road {
            tile,
            edge: tile.edge(index),
            origin,
        })
    }

    pub fn class(&self) -> RoadClass {
        RoadClass::parse(self.edge.class)
    }

    /// The [`flag`](crate::tile::flag) bits: oneway, link, unpaved and so on.
    pub fn flags(&self) -> u8 {
        self.edge.flags
    }

    /// Absolute `(lat_e7, lon_e7)` from the first node through the shape
    /// to the last node.
    pub fn points(&self) -> impl Iterator<Item = (i32, i32)> + 't {
        let Road { tile, edge, origin } = *self;
        tile.shape(&edge).map(move |point| {
            (
                e7(origin.0 + i32::from(point.y)),
                e7(origin.1 + i32::from(point.x)),
            )
        })
    }
}

/// Micro-degrees to 1e-7 degrees. A grid may reach 360°, past what e7
/// holds in i32; no real road is there.
fn e7(micro: i32) -> i32 {
    micro.saturating_mul(10)
}

impl Header {
    /// The tiles of the grid a convex polygon touches, row by row from the
    /// south-west. `corners` are `(lat_e7, lon_e7)` in order around it, as
    /// a map view rotated to the heading.
    pub fn tiles_under(&self, corners: &[(i32, i32); 4]) -> impl Iterator<Item = TileKey> + '_ {
        let corners = corners.map(|(lat, lon)| (i128::from(lat), i128::from(lon)));
        let (south, west, north, east) = corners.iter().fold(
            (i128::MAX, i128::MAX, i128::MIN, i128::MIN),
            |(s, w, n, e), &(lat, lon)| (s.min(lat), w.min(lon), n.max(lat), e.max(lon)),
        );
        let side = i128::from(self.tile) * 10;
        let (grid_south, grid_west) = (i128::from(self.south) * 10, i128::from(self.west) * 10);
        let row = |lat: i128| (lat - grid_south).div_euclid(side);
        let col = |lon: i128| (lon - grid_west).div_euclid(side);
        let rows = row(south).max(0)..=row(north).min(i128::from(self.rows) - 1);
        let cols = col(west).max(0)..=col(east).min(i128::from(self.cols) - 1);
        rows.flat_map(move |row| cols.clone().map(move |col| (row, col)))
            .filter(move |&(row, col)| {
                let (lat, lon) = (grid_south + row * side, grid_west + col * side);
                let square = [
                    (lat, lon),
                    (lat + side, lon),
                    (lat + side, lon + side),
                    (lat, lon + side),
                ];
                !separated(&corners, &square)
            })
            .filter_map(move |(row, col)| self.checked(row as i64, col as i64))
    }
}

/// Some side of the convex polygon `corners` has the whole `square` beyond
/// it. The square's own sides are the bounding box the caller iterates.
fn separated(corners: &[(i128, i128); 4], square: &[(i128, i128); 4]) -> bool {
    let cross = |a: (i128, i128), b: (i128, i128), p: (i128, i128)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let turn = (0..4)
        .map(|i| cross(corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4]).signum())
        .find(|&sign| sign != 0)
        .unwrap_or(0);
    (0..4).any(|i| {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        square.iter().all(|&p| cross(a, b, p) * turn < 0)
    })
}
