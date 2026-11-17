use libm::{atan2f, cosf, sinf, sqrtf};

/// Metres per 1e-7 degree of latitude, on a sphere of the mean Earth radius.
pub const M_PER_E7: f32 = 0.011_119_508;
/// Angles are in 4096ths of a turn, clockwise from north.
pub const TURN: u16 = 4096;

type Point = (f32, f32);

/// Where a ride's positions are measured from: its first fix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    origin: (i32, i32),
    east_per_e7: f32,
}

impl Frame {
    pub(crate) fn at((lat, lon): (i32, i32)) -> Self {
        Self {
            origin: (lat, lon),
            east_per_e7: east_per_e7(lat),
        }
    }

    /// Metres east and north of the origin.
    pub fn place(&self, lat: i32, lon: i32) -> Point {
        metres(self.origin, (lat, lon), self.east_per_e7)
    }

    /// The `(lat_e7, lon_e7)` that `place` puts at `(east, north)`.
    pub fn locate(&self, (east, north): Point) -> (i32, i32) {
        let (lat0, lon0) = self.origin;
        (
            lat0.saturating_add((north / M_PER_E7) as i32),
            lon0.saturating_add((east / self.east_per_e7) as i32),
        )
    }

    /// Metres per 1e-7 degree east and north.
    pub fn metres_per_e7(&self) -> (f32, f32) {
        (self.east_per_e7, M_PER_E7)
    }
}

/// Metres east per 1e-7 degree of longitude at this latitude.
pub(crate) fn east_per_e7(lat_e7: i32) -> f32 {
    let radians = lat_e7 as f32 * 1e-7 * core::f32::consts::PI / 180.0;
    M_PER_E7 * cosf(radians)
}

/// Metres east and north from `(lat, lon)` `a` to `b`; across the
/// antimeridian the short way round is the way ridden.
pub(crate) fn metres(a: (i32, i32), b: (i32, i32), east_per_e7: f32) -> Point {
    let mut east = i64::from(b.1) - i64::from(a.1);
    if east > 1_800_000_000 {
        east -= 3_600_000_000;
    } else if east < -1_800_000_000 {
        east += 3_600_000_000;
    }
    let north = i64::from(b.0) - i64::from(a.0);
    (east as f32 * east_per_e7, north as f32 * M_PER_E7)
}

pub(crate) fn distance(a: Point, b: Point) -> f32 {
    let (east, north) = (b.0 - a.0, b.1 - a.1);
    sqrtf(east * east + north * north)
}

/// Margin on a box test for the rounding of a projection, which can land
/// outside its segment's box by a few ulps of the coordinates involved: a
/// metre, plus a bound that grows with their size, so a course far from the
/// ride's origin is never pruned wrongly.
const ROUNDING_M: f32 = 1.0;
const ROUNDING_ULPS: f32 = 64.0 * f32::EPSILON;

#[inline(always)]
pub(crate) fn box_gap(p: Point, a: Point, b: Point) -> (f32, f32) {
    let (west, east) = if a.0 < b.0 { (a.0, b.0) } else { (b.0, a.0) };
    let (south, north) = if a.1 < b.1 { (a.1, b.1) } else { (b.1, a.1) };
    let off = larger(
        larger(west - p.0, p.0 - east),
        larger(south - p.1, p.1 - north),
    );
    let scale = larger(
        larger(larger(a.0.abs(), a.1.abs()), larger(b.0.abs(), b.1.abs())),
        larger(p.0.abs(), p.1.abs()),
    );
    (off, ROUNDING_M + ROUNDING_ULPS * scale)
}

/// `f32::max` also handles NaN, which costs the C606 a compare and a branch
/// more each time; no coordinate is NaN.
#[inline(always)]
fn larger(x: f32, y: f32) -> f32 {
    if x > y { x } else { y }
}

/// The bearing from `from` to `to`, in `TURN`ths clockwise from north.
pub(crate) fn bearing(from: Point, to: Point) -> u16 {
    let radians = atan2f(to.0 - from.0, to.1 - from.1);
    let turns = radians / core::f32::consts::TAU;
    let turns = if turns < 0.0 { turns + 1.0 } else { turns };
    ((turns * f32::from(TURN)) as u32 % u32::from(TURN)) as u16
}

/// The sine and cosine of `heading`, in `TURN`ths clockwise from north.
pub(crate) fn rotation(heading: u16) -> (f32, f32) {
    let turns = f32::from(heading % TURN) / f32::from(TURN);
    let radians = turns * core::f32::consts::TAU;
    (sinf(radians), cosf(radians))
}

/// Millimetres; a float cast saturates, so no length overflows.
pub(crate) fn mm(metres: f32) -> u32 {
    (metres * 1000.0) as u32
}
