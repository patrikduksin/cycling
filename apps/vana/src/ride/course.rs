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
//! Persisted version 1, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VCRS` |
//! | 4 | version, 1 |
//! | 5 | header length `h`: 34 in version 1 |
//! | 6 | point length `s`: 8 in version 1 |
//! | 7 | name length, up to 24 |
//! | 8..10 | points `n`, 2 to 2048 |
//! | 10..34 | the name: `A-Z`, `0-9`, space and `-._'`, zero padded |
//! | h.. | `n` points of `s` bytes: latitude, then longitude, in 1e-7 degrees (i32 each) |
//! | h+n*s.. | CRC32 of every byte before it |
//!
//! Later versions may lengthen the header or each point and raise the
//! version; a reader takes the fields it knows from any version at least 1.
//! A file with more points than a reader holds reads as damaged, never as
//! part of a course. Firmware from before courses never opens `course.dat`,
//! so the file disturbs neither it nor the rides and files beside it.
use crate::ride::thin;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::Crc32;
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};
use firmware_services::{files, volume};
use libm::{cosf, sqrtf};

pub mod gpx;

/// Points a course keeps, start and end included.
pub const POINTS: usize = 2048;
/// Bytes of a course name.
pub const NAME: usize = 24;
/// A point this close to the last one kept adds nothing.
const MIN_STEP_M: f32 = 5.0;
/// A point may sit this far off the line that replaces it on the first
/// thinning; doubled on each one after.
const TOLERANCE_M: f32 = 4.0;
/// Metres per 1e-7 degree of latitude, as in `ride::homing`.
const M_PER_E7: f32 = 0.011_119_508;
const LAT_MAX_E7: u32 = 900_000_000;
const LON_MAX_E7: u32 = 1_800_000_000;

const FILE: &str = "course.dat";
const MAGIC: &[u8; 4] = b"VCRS";
pub const VERSION: u8 = 1;
const HEADER_V1: usize = 34;
const POINT_V1: usize = 8;
/// The longest file this version writes.
pub const MAX_FILE: usize = HEADER_V1 + POINTS * POINT_V1 + 4;

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
        let scale = east_per_e7(first);
        let total: f32 = points
            .windows(2)
            .map(|pair| {
                let (east, north) = metres(pair[0], pair[1], scale);
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

/// Metres east per 1e-7 degree of longitude at `at`'s latitude.
fn east_per_e7(at: Point) -> f32 {
    let radians = at.lat_e7 as f32 * 1e-7 * core::f32::consts::PI / 180.0;
    M_PER_E7 * cosf(radians)
}

/// Metres east and north from `a` to `b`, the short way round.
fn metres(a: Point, b: Point, east_per_e7: f32) -> (f32, f32) {
    let mut east = i64::from(b.lon_e7) - i64::from(a.lon_e7);
    if east > 1_800_000_000 {
        east -= 3_600_000_000;
    } else if east < -1_800_000_000 {
        east += 3_600_000_000;
    }
    let north = i64::from(b.lat_e7) - i64::from(a.lat_e7);
    (east as f32 * east_per_e7, north as f32 * M_PER_E7)
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
    pub fn start(&mut self, course: &mut Course) {
        *self = Self::new();
        course.clear();
    }

    pub fn push(&mut self, course: &mut Course, point: Point) {
        self.last = Some(point);
        let len = course.len as usize;
        if len == 0 {
            self.east_per_e7 = east_per_e7(point);
        } else {
            let (east, north) = metres(course.points[len - 1], point, self.east_per_e7);
            if east * east + north * north < MIN_STEP_M * MIN_STEP_M {
                return;
            }
        }
        self.keep(course, point);
    }

    /// End the course at the newest point given.
    pub fn finish(&mut self, course: &mut Course) {
        if let Some(last) = self.last.take()
            && course.points().last() != Some(&last)
        {
            self.keep(course, last);
        }
    }

    fn keep(&mut self, course: &mut Course, point: Point) {
        if course.len as usize == POINTS {
            let (origin, scale) = (course.points[0], self.east_per_e7);
            let at = |p: &Point| metres(origin, *p, scale);
            course.len = thin::thin(&mut course.points, POINTS, &mut self.tolerance, at) as u32;
        }
        course.points[course.len as usize] = point;
        course.len += 1;
    }
}

/// Why a course could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No card, no FILES volume or no formatted file store.
    Unavailable,
    /// The medium failed this time; trying again may work.
    Media,
    /// A file is there but does not decode.
    Damaged,
    /// The store has no room for the file.
    Full,
}

impl From<files::Error> for Error {
    fn from(error: files::Error) -> Self {
        use device_api::bulk::Error as Media;
        match error {
            files::Error::Media(Media::Failed) | files::Error::Stale => Self::Media,
            files::Error::Full | files::Error::TooLarge => Self::Full,
            files::Error::Mismatch => Self::Damaged,
            _ => Self::Unavailable,
        }
    }
}

/// Every byte of `course`'s file, in order, in a few pieces.
pub fn encode(course: &Course, mut put: impl FnMut(&[u8])) {
    let mut crc = Crc32::new();
    let mut emit = |bytes: &[u8]| {
        crc = crc.update(bytes);
        put(bytes);
    };
    let mut header = [0u8; HEADER_V1];
    header[..4].copy_from_slice(MAGIC);
    header[4] = VERSION;
    header[5] = HEADER_V1 as u8;
    header[6] = POINT_V1 as u8;
    header[7] = course.name_len as u8;
    put_u16(&mut header, 8, course.len as u16);
    header[10..10 + course.name_len as usize].copy_from_slice(course.name().as_bytes());
    emit(&header);
    for point in course.points() {
        let mut bytes = [0u8; POINT_V1];
        put_u32(&mut bytes, 0, point.lat_e7 as u32);
        put_u32(&mut bytes, 4, point.lon_e7 as u32);
        emit(&bytes);
    }
    let sum = crc.finish();
    put(&sum.to_le_bytes());
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
    let mut head = [0u8; 255];
    if (length as usize) < HEADER_V1 + 4 {
        return Err(DecodeError::Damaged);
    }
    read(0, &mut head[..HEADER_V1]).map_err(DecodeError::Read)?;
    let (header, point, name_len) = (usize::from(head[5]), usize::from(head[6]), head[7]);
    let count = usize::from(get_u16(&head, 8));
    let expected = header as u64 + count as u64 * point as u64 + 4;
    if &head[..4] != MAGIC
        || head[4] < 1
        || header < HEADER_V1
        || point < POINT_V1
        || usize::from(name_len) > NAME
        || !(2..=POINTS).contains(&count)
        || expected != u64::from(length)
    {
        return Err(DecodeError::Damaged);
    }
    read(0, &mut head[..header]).map_err(DecodeError::Read)?;
    let mut crc = Crc32::new().update(&head[..header]);
    let name = &head[10..10 + usize::from(name_len)];
    if !name.iter().all(|&b| name_byte(b)) {
        return Err(DecodeError::Damaged);
    }
    course.name[..name.len()].copy_from_slice(name);
    course.name_len = u32::from(name_len);
    // Whole points per read, about a sector's worth; a point is at most
    // 255 bytes.
    let mut chunk = [0u8; SECTOR_SIZE];
    let per = SECTOR_SIZE / point;
    let mut index = 0;
    while index < count {
        let n = per.min(count - index);
        let bytes = &mut chunk[..n * point];
        read((header + index * point) as u32, bytes).map_err(DecodeError::Read)?;
        crc = crc.update(bytes);
        for (slot, bytes) in course.points[index..index + n]
            .iter_mut()
            .zip(bytes.chunks_exact(point))
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

fn open<'a, M: ReadWrite>(
    media: &'a mut M,
    sector: &mut files::Sector,
) -> Result<volume::Volume<'a, M>, Error> {
    volume::open(media, &volume::FILES, sector).map_err(|error| match error {
        volume::Error::Media(device_api::bulk::Error::Failed) => Error::Media,
        _ => Error::Unavailable,
    })
}

fn name() -> files::Name {
    files::Name::new(FILE).expect("a valid file name")
}

/// The stored course into `course`; empty when the store has none.
pub fn load(media: &mut impl ReadWrite, course: &mut Course) -> Result<(), Error> {
    course.clear();
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let Some(entry) = dir.find(&mut store, &name(), &mut sector)? else {
        return Ok(());
    };
    let read = |offset: u32, out: &mut [u8]| -> Result<(), files::Error> {
        let mut sector = [0; SECTOR_SIZE];
        let n = dir.read(&mut store, &entry, offset, out, &mut sector)?;
        if n == out.len() {
            Ok(())
        } else {
            Err(files::Error::Range)
        }
    };
    match decode(entry.length, read, course) {
        Ok(()) => Ok(()),
        Err(DecodeError::Read(files::Error::Range)) | Err(DecodeError::Damaged) => {
            Err(Error::Damaged)
        }
        Err(DecodeError::Read(error)) => Err(error.into()),
    }
}

/// Store `course`, replacing the file there in one atomic commit. A store
/// the host has not formatted is not formatted here.
pub fn save(media: &mut impl ReadWrite, course: &Course) -> Result<(), Error> {
    let mut length = 0u32;
    let mut crc = Crc32::new();
    encode(course, |bytes| {
        length += bytes.len() as u32;
        crc = crc.update(bytes);
    });
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let mut writer = dir.begin(&mut store, &name(), length, &mut sector)?;
    // Whole sectors to the writer, and the short last one at the end.
    let mut held = [0u8; SECTOR_SIZE];
    let mut filled = 0;
    let mut failed = None;
    encode(course, |mut bytes| {
        while !bytes.is_empty() && failed.is_none() {
            let n = bytes.len().min(SECTOR_SIZE - filled);
            held[filled..filled + n].copy_from_slice(&bytes[..n]);
            filled += n;
            bytes = &bytes[n..];
            if filled == SECTOR_SIZE {
                failed = writer.write(&mut store, &held).err();
                filled = 0;
            }
        }
    });
    if let Some(error) = failed {
        return Err(error.into());
    }
    writer.write(&mut store, &held[..filled])?;
    writer.commit(&mut store, crc.finish(), &mut sector)?;
    Ok(())
}

pub fn delete(media: &mut impl ReadWrite) -> Result<(), Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = open(media, &mut sector)?;
    match files::delete(&mut store, &name(), &mut sector) {
        Ok(()) | Err(files::Error::NotFound) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
