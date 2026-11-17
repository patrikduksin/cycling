//! A course: a planned route the rider loads from the phone to follow.
//!
//! A course is up to `POINTS` points in 1e-7 degrees, start first, and a
//! short name. It lives beside the breadcrumb (PSRAM on the device), never on
//! a stack; all zero bytes are an empty course, so composition may hand over
//! zeroed memory. One course is kept, on the FILES volume as `course.dat`, and
//! loading another replaces it. Where there is no formatted FILES store, no
//! course is kept: formatting is the host's decision.
//!
//! A course of any length fits. While it is built, a point closer than
//! `MIN_STEP_M` to the last one kept adds nothing, and a full list is thinned
//! in place until a quarter of it is free: each kept point reaches on to the
//! farthest point whose chord passes within a tolerance of every point it
//! replaces. The tolerance starts at `TOLERANCE_M` and doubles only when a
//! pass frees less than a quarter, so a long course keeps its shape to within
//! a few times the tolerance its length needs. The last point always stays.
//!
//! Persisted, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VCRS` |
//! | 4 | version, 2 |
//! | 5 | header length, 34 |
//! | 6 | point length, 8 |
//! | 7 | name length, up to 24 |
//! | 8..10 | points `n`, 2 to 2048 |
//! | 10..34 | the name: `A-Z`, `0-9`, space and `-._'`, zero padded |
//! | 34.. | `n` points of 8 bytes: latitude, then longitude, in 1e-7 degrees (i32 each) |
//! | 34+8n.. | CRC32 of every byte before it |
//!
//! A file of another version or shape reads as damaged, never as part of a
//! course, and is left alone until an upload replaces it.
use crate::ride::geo::{M_PER_E7, east_per_e7, metres};
use crate::ride::thin;
use device_api::bulk::SECTOR_SIZE;
use firmware_services::checksum::Crc32;
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};
use libm::sqrtf;

pub mod follow;
pub mod gpx;
pub mod nav;
pub mod stored;
pub mod turns;

/// Points a course keeps, start and end included.
pub const POINTS: usize = 2048;
/// Bytes of a course name.
pub const NAME: usize = 24;
/// A point this close to the last one kept adds nothing.
const MIN_STEP_M: f32 = 5.0;
/// A point may sit this far off the line that replaces it on the first
/// thinning; doubled on each one after.
const TOLERANCE_M: f32 = 4.0;
const LAT_MAX_E7: u32 = 900_000_000;
const LON_MAX_E7: u32 = 1_800_000_000;

const MAGIC: &[u8; 4] = b"VCRS";
pub const VERSION: u8 = 2;
const HEADER: usize = 34;
const POINT: usize = 8;
/// The longest file this version writes.
pub const MAX_FILE: usize = HEADER + POINTS * POINT + 4;

/// A position in 1e-7 degrees, within the ranges of latitude and longitude.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub lat_e7: i32,
    pub lon_e7: i32,
}

impl Point {
    /// The point, if both are real coordinates.
    pub fn new(lat_e7: i32, lon_e7: i32) -> Option<Self> {
        let within = lat_e7.unsigned_abs() <= LAT_MAX_E7 && lon_e7.unsigned_abs() <= LON_MAX_E7;
        within.then_some(Self { lat_e7, lon_e7 })
    }
}

impl From<Point> for (i32, i32) {
    fn from(point: Point) -> Self {
        (point.lat_e7, point.lon_e7)
    }
}

/// The course: `len` points in order and its name. All zero bytes are an
/// empty course.
#[repr(C)]
pub struct Course {
    len: u32,
    name_len: u32,
    name: [u8; NAME],
    points: [Point; POINTS],
}

impl Course {
    /// What zeroed memory holds, for a host that builds a course in place.
    pub const fn empty() -> Self {
        Self {
            len: 0,
            name_len: 0,
            name: [0; NAME],
            points: [Point {
                lat_e7: 0,
                lon_e7: 0,
            }; POINTS],
        }
    }

    /// The points, start first.
    pub fn points(&self) -> &[Point] {
        &self.points[..self.len as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.name_len = 0;
    }

    /// Take a name, keeping what a course name may hold, in capitals, with
    /// single spaces.
    pub fn set_name(&mut self, text: &[u8]) {
        self.name_len = 0;
        for &byte in text {
            let byte = byte.to_ascii_uppercase();
            let after_space = self.name_len == 0 || self.name[self.name_len as usize - 1] == b' ';
            if (after_space && byte == b' ') || !name_byte(byte) || self.name_len as usize == NAME {
                continue;
            }
            self.name[self.name_len as usize] = byte;
            self.name_len += 1;
        }
        while self.name_len > 0 && self.name[self.name_len as usize - 1] == b' ' {
            self.name_len -= 1;
        }
    }

    /// Metres along the course, start to end.
    pub fn length_m(&self) -> u32 {
        let points = self.points();
        let Some(&first) = points.first() else {
            return 0;
        };
        let scale = east_per_e7(first.lat_e7);
        let total: f32 = points
            .windows(2)
            .map(|pair| {
                let (east, north) = metres(pair[0].into(), pair[1].into(), scale);
                sqrtf(east * east + north * north)
            })
            .sum();
        // A float cast saturates, so no course length overflows.
        total as u32
    }
}

fn name_byte(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit() || b" -._'".contains(&byte)
}

/// Builds a course from points as they stream in, of any count.
#[derive(Clone, Copy, Debug)]
pub struct Builder {
    tolerance: f32,
    east_per_e7: f32,
    /// The newest point given, which ends the course even when too close
    /// to the last one kept.
    last: Option<Point>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub const fn new() -> Self {
        Self {
            tolerance: TOLERANCE_M,
            east_per_e7: M_PER_E7,
            last: None,
        }
    }

    /// Start `course` over.
    pub fn start(&mut self, course: &mut Course, follow: &mut impl Follow) {
        *self = Self::new();
        course.clear();
        follow.start();
    }

    pub fn push(&mut self, course: &mut Course, point: Point, follow: &mut impl Follow) {
        follow.given(point);
        self.last = Some(point);
        let len = course.len as usize;
        if len == 0 {
            self.east_per_e7 = east_per_e7(point.lat_e7);
        } else {
            let (east, north) = metres(
                course.points[len - 1].into(),
                point.into(),
                self.east_per_e7,
            );
            if east * east + north * north < MIN_STEP_M * MIN_STEP_M {
                return;
            }
        }
        self.keep(course, point, follow);
    }

    /// End the course at the newest point given.
    pub fn finish(&mut self, course: &mut Course, follow: &mut impl Follow) {
        if let Some(last) = self.last.take()
            && course.points().last() != Some(&last)
        {
            self.keep(course, last, follow);
        }
    }

    fn keep(&mut self, course: &mut Course, point: Point, follow: &mut impl Follow) {
        if course.len as usize == POINTS {
            let (origin, scale) = (course.points[0], self.east_per_e7);
            let at = |p: &Point| metres(origin.into(), (*p).into(), scale);
            let moved = |to, from| follow.moved(to, from);
            course.len =
                thin::thin_moving(&mut course.points, POINTS, &mut self.tolerance, at, moved)
                    as u32;
        }
        follow.kept(course.len as usize);
        course.points[course.len as usize] = point;
        course.len += 1;
    }
}

/// What follows a course as it is built: every point given, kept or not,
/// and where the kept ones go, so a list beside the course keeps up.
pub trait Follow {
    /// The course starts over.
    fn start(&mut self) {}
    /// The next point, in order.
    fn given(&mut self, _point: Point) {}
    /// The newest point given is kept, at `index`.
    fn kept(&mut self, _index: usize) {}
    /// Thinning moved the kept point at `from` to `to`.
    fn moved(&mut self, _to: usize, _from: usize) {}
}

/// Nothing follows.
impl Follow for () {}

/// Metres along `points` from the first to each, in turn, in the course's
/// own frame: what distances along a course are measured in.
pub(crate) fn along_m(points: &[Point]) -> impl Iterator<Item = f32> + '_ {
    let scale = points
        .first()
        .map_or(M_PER_E7, |&first| east_per_e7(first.lat_e7));
    let mut along = 0.0f32;
    points.iter().enumerate().map(move |(k, &p)| {
        if k > 0 {
            let (east, north) = metres(points[k - 1].into(), p.into(), scale);
            along += sqrtf(east * east + north * north);
        }
        along
    })
}

/// Every byte of `course`'s file, in order, in a few pieces.
pub fn encode(course: &Course, mut put: impl FnMut(&[u8])) {
    let sum = sealed(course, &mut put);
    put(&sum.to_le_bytes());
}

/// The CRC that ends `course`'s file: the turns stored beside it name their
/// course by it.
pub fn key(course: &Course) -> u32 {
    sealed(course, &mut |_| {})
}

/// The file's bytes before its CRC, to `put`; returns the CRC.
fn sealed(course: &Course, put: &mut impl FnMut(&[u8])) -> u32 {
    let mut crc = Crc32::new();
    let mut emit = |bytes: &[u8]| {
        crc = crc.update(bytes);
        put(bytes);
    };
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(MAGIC);
    header[4] = VERSION;
    header[5] = HEADER as u8;
    header[6] = POINT as u8;
    header[7] = course.name_len as u8;
    put_u16(&mut header, 8, course.len as u16);
    header[10..10 + course.name_len as usize].copy_from_slice(course.name().as_bytes());
    emit(&header);
    for point in course.points() {
        let mut bytes = [0u8; POINT];
        put_u32(&mut bytes, 0, point.lat_e7 as u32);
        put_u32(&mut bytes, 4, point.lon_e7 as u32);
        emit(&bytes);
    }
    crc.finish()
}

/// Read a file of `length` bytes into `course` through `read`, which fills
/// a buffer from an offset. A file that does not decode leaves `course`
/// empty and reads as `Damaged`.
pub fn decode<E>(
    length: u32,
    mut read: impl FnMut(u32, &mut [u8]) -> Result<(), E>,
    course: &mut Course,
) -> Result<(), DecodeError<E>> {
    course.clear();
    let result = decode_into(length, &mut read, course);
    if result.is_err() {
        course.clear();
    }
    result
}

/// A read failed, or the bytes are not a course.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError<E> {
    Read(E),
    Damaged,
}

fn decode_into<E>(
    length: u32,
    read: &mut impl FnMut(u32, &mut [u8]) -> Result<(), E>,
    course: &mut Course,
) -> Result<(), DecodeError<E>> {
    let mut head = [0u8; HEADER];
    if (length as usize) < HEADER + 4 {
        return Err(DecodeError::Damaged);
    }
    read(0, &mut head).map_err(DecodeError::Read)?;
    let name_len = head[7];
    let count = usize::from(get_u16(&head, 8));
    if &head[..4] != MAGIC
        || head[4] != VERSION
        || usize::from(head[5]) != HEADER
        || usize::from(head[6]) != POINT
        || usize::from(name_len) > NAME
        || !(2..=POINTS).contains(&count)
        || HEADER + count * POINT + 4 != length as usize
    {
        return Err(DecodeError::Damaged);
    }
    let mut crc = Crc32::new().update(&head);
    let name = &head[10..10 + usize::from(name_len)];
    if !name.iter().all(|&b| name_byte(b)) {
        return Err(DecodeError::Damaged);
    }
    course.name[..name.len()].copy_from_slice(name);
    course.name_len = u32::from(name_len);
    let mut chunk = [0u8; SECTOR_SIZE];
    let per = SECTOR_SIZE / POINT;
    let mut index = 0;
    while index < count {
        let n = per.min(count - index);
        let bytes = &mut chunk[..n * POINT];
        read((HEADER + index * POINT) as u32, bytes).map_err(DecodeError::Read)?;
        crc = crc.update(bytes);
        for (slot, bytes) in course.points[index..index + n]
            .iter_mut()
            .zip(bytes.as_chunks::<POINT>().0)
        {
            let (lat, lon) = (get_u32(bytes, 0) as i32, get_u32(bytes, 4) as i32);
            *slot = Point::new(lat, lon).ok_or(DecodeError::Damaged)?;
        }
        index += n;
    }
    let mut sum = [0u8; 4];
    read(length - 4, &mut sum).map_err(DecodeError::Read)?;
    if crc.finish().to_le_bytes() != sum {
        return Err(DecodeError::Damaged);
    }
    course.len = count as u32;
    Ok(())
}

#[cfg(test)]
pub(crate) mod bits;
#[cfg(test)]
pub(crate) mod tests;
