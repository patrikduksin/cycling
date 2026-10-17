//! Flat-earth geometry around a point, good to a few centimetres over the
//! few kilometres one computation spans.
use crate::pack::{Header, TileKey};
use crate::tile::{Node, Tile};

/// Micrometres per micro-degree of latitude (spherical Earth, as the
/// builder uses).
pub const LAT_UM: f32 = 111_195.0;

/// A point in micro-degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub lat: i32,
    pub lon: i32,
}

impl Point {
    pub fn of(header: &Header, key: TileKey, node: Node) -> Self {
        let (south, west) = header.origin(key);
        Self {
            lat: south + i32::from(node.y),
            lon: west + i32::from(node.x),
        }
    }

    /// Metres east and north of `origin`, with `lon_um` micrometres per
    /// micro-degree of longitude.
    pub fn metres_from(self, origin: Point, lon_um: u32) -> (f32, f32) {
        (
            (self.lon - origin.lon) as f32 * lon_um as f32 * 1e-6,
            (self.lat - origin.lat) as f32 * LAT_UM * 1e-6,
        )
    }
}

/// A tile point with its elevation in decimetres (absolute).
pub fn z_dm(node: Node) -> i32 {
    i32::from(node.z) - crate::tile::Z_OFFSET_DM
}

/// Compass bearing of (east, north), degrees clockwise from north, 0..360.
pub fn bearing(east: f32, north: f32) -> f32 {
    let degrees = libm::atan2f(east, north).to_degrees();
    if degrees < 0.0 {
        degrees + 360.0
    } else {
        degrees
    }
}

/// Signed turn from bearing `from` to bearing `to`, in -180..180.
pub fn turn(from: f32, to: f32) -> f32 {
    let mut delta = to - from;
    while delta > 180.0 {
        delta -= 360.0;
    }
    while delta <= -180.0 {
        delta += 360.0;
    }
    delta
}

pub fn length(east: f32, north: f32) -> f32 {
    libm::sqrtf(east * east + north * north)
}

/// Where `p` projects onto segment a-b: (fraction 0..1, distance), all in
/// metres of one local frame.
pub fn project(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    (t, length(p.0 - x, p.1 - y))
}

/// The tile point at `index` of `edge`, as a global point and elevation.
pub fn edge_point(
    header: &Header,
    tile: &Tile<'_>,
    edge: &crate::tile::Edge,
    index: usize,
) -> (Point, i32) {
    let node = tile.point(edge, index);
    (Point::of(header, tile.key, node), z_dm(node))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_and_turns() {
        assert!((bearing(0.0, 1.0) - 0.0).abs() < 0.01);
        assert!((bearing(1.0, 0.0) - 90.0).abs() < 0.01);
        assert!((bearing(-1.0, 0.0) - 270.0).abs() < 0.01);
        assert!((turn(350.0, 10.0) - 20.0).abs() < 0.01);
        assert!((turn(10.0, 350.0) + 20.0).abs() < 0.01);
        assert!((turn(0.0, 180.0) - 180.0).abs() < 0.01);
    }

    #[test]
    fn projection_clamps_to_the_segment() {
        let (t, d) = project((5.0, 3.0), (0.0, 0.0), (10.0, 0.0));
        assert!((t - 0.5).abs() < 1e-6 && (d - 3.0).abs() < 1e-6);
        let (t, d) = project((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0));
        assert!(t == 0.0 && (d - 5.0).abs() < 1e-6);
    }
}
