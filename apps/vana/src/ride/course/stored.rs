//! A course's turns, found while it uploads from every point of the file,
//! and stored beside `course.dat` as `course-turns.dat`.
//!
//! Thinning a long course to fit (`course::Builder`) cuts its corners and
//! roundabouts into a few long segments, and a scan of what is kept can only
//! guess at them (`turns`). `Finder` follows the upload instead: it scans
//! every point given, a metre apart or more, as they stream in, keeping only
//! the last `RING` of them, which is all the scan still reads. Where the points
//! jitter, as a recorded GPS track's do, each is first moved to the mean of
//! those about it, so jitter reads as no bend. Each turn is
//! found in metres along the full track; once the course is complete, it is
//! moved to the same place along the course as kept, by the full track's
//! distance to each kept point, so the rider's place on the stored course
//! finds it.
//!
//! The file store commits one file at a time, so the turns cannot commit
//! with the course. Instead they name their course by its key
//! (`course::key`), the CRC32 of the course encoded as version 1 of
//! `course.dat`: turns of any other course, left by a cut
//! between the two writes or by firmware that writes only `course.dat`, are
//! not this course's, and its turns are found by the scan as before. A
//! course without the file, or with one that does not decode, is scanned
//! the same way.
//!
//! Persisted version 1, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VTRN` |
//! | 4 | version, 1 |
//! | 5 | header length `h`: 16 in version 1 |
//! | 6 | turn length `s`: 12 in version 1 |
//! | 7 | flags: 1, the course has more turns than are stored |
//! | 8..12 | the course's key |
//! | 12..14 | turns `n`, up to 1024 |
//! | 14..16 | zero |
//! | h.. | `n` turns of `s` bytes, in order along the course (`Turn::record`) |
//! | h+n*s.. | CRC32 of every byte before it |
//!
//! A turn is from and to in millimetres along the course as stored (u32
//! each), its kind (0 none, 1 left, 2 right, 3 sharp left, 4 sharp right, 5
//! U-turn) and flags (1 two corners, 2 cornered). Later versions may lengthen
//! the header or each turn, and add flags, and raise the version; a reader
//! takes what it knows from any version at least 1.
use super::turns::{RECORD, Scan, TURNS, Turn};
use super::{Error, Follow, POINTS, Point, east_per_e7, metres};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::Crc32;
use firmware_services::files;
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};
use libm::sqrtf;

const FILE: &str = "course-turns.dat";
const MAGIC: &[u8; 4] = b"VTRN";
pub const VERSION: u8 = 1;
const HEADER_V1: usize = 16;
/// The course has more turns than the file holds.
const FULL: u8 = 1;
/// The longest file this version writes.
pub const MAX_FILE: usize = HEADER_V1 + TURNS * RECORD + 4;

/// Points the scan reads that it keeps. It reads back only as far as the
/// group of bends it is in and the bend after it: at most 109 points on the
/// gates' courses, and 393 on a track that circles for kilometres.
pub const RING: usize = 512;
/// A point this close to the last one taken adds nothing. Points farther
/// apart are all taken, so no corner of a drawn course is cut.
const RING_STEP_M: f32 = 1.0;
/// Where the points jitter, each the scan reads is the mean of the points
/// taken within this far along either side of it: GPS jitter would
/// otherwise read as bends, where a course thinned to fit has none.
const SMOOTH_M: f32 = 6.0;
/// The points jitter where, over this many before one and those the mean
/// takes after it, how far each lies to the side of the line between its
/// neighbours swings by this much from one to the next at most of them.
const NOISE_SPAN: usize = 24;
const NOISE_M: f32 = 1.0;
/// Points taken that the mean and the jitter may still need.
const RAW: usize = 48;
/// The scan steps only while the track is in this far beyond its chord, so
/// it never takes the end of what has come in so far for the course's end.
const LEAD_M: f32 = 12.0;

/// Finds a course's turns while it uploads. All zero bytes are a valid
/// `Finder`, so composition may hand over zeroed memory; `Follow::start`
/// begins each course.
pub struct Finder {
    /// The first point and metres east per 1e-7 degree there: the frame
    /// the course's lengths are measured in.
    origin: Point,
    east_per_e7: f32,
    started: bool,
    /// The last points taken, in metres from the origin, and metres along
    /// the track to each: point `k` is `raw[k % RAW]`. `smoothed` of them
    /// are in the ring.
    raw: [((f32, f32), f32); RAW],
    raw_taken: usize,
    /// Whether, at point `k` taken, how far it lies to the side of the line
    /// between its neighbours swung by `NOISE_M` or more from the point
    /// before (`swung[k % RAW]`), and that for the newest point it is known.
    swung: [bool; RAW],
    side: f32,
    smoothed: usize,
    /// The last `RING` points as the scan reads them, each a point taken or
    /// the mean about one: point `k` is at `ring[k % RING]`.
    ring: [(f32, f32); RING],
    taken: usize,
    /// Metres along the track as the scan reads it to the last point in the
    /// ring, and to the last point given.
    taken_m: f32,
    given_m: f32,
    /// Millimetres along the full track to each point the course keeps.
    kept_mm: [u32; POINTS],
    scan: Scan,
    found: usize,
    /// The scan needed a point the ring no longer held: what it found is
    /// not to be trusted, and the course is scanned as before.
    lost: bool,
    /// The list filled before the track ended: the scan stopped there.
    full: bool,
    /// The most points back the scan read, for the record.
    reach: usize,
}

impl Finder {
    /// What zeroed memory holds, for a host that builds one in place.
    pub fn new() -> Self {
        Self {
            origin: Point::default(),
            east_per_e7: 0.0,
            started: false,
            raw: [((0.0, 0.0), 0.0); RAW],
            raw_taken: 0,
            swung: [false; RAW],
            side: 0.0,
            smoothed: 0,
            ring: [(0.0, 0.0); RING],
            taken: 0,
            taken_m: 0.0,
            given_m: 0.0,
            kept_mm: [0; POINTS],
            scan: Scan::default(),
            found: 0,
            lost: false,
            full: false,
            reach: 0,
        }
    }
}

impl Default for Finder {
    fn default() -> Self {
        Self::new()
    }
}

/// A `Finder` and the list it finds turns into, while a course uploads.
pub struct Finding<'a> {
    pub finder: &'a mut Finder,
    pub turns: &'a mut [Turn],
}

/// The turns found, moved onto the course as kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub count: usize,
    /// The list filled before the course ended.
    pub full: bool,
    /// The most points back the scan read.
    pub reach: usize,
}

impl Finding<'_> {
    /// Put the points taken into the ring, or the means about them where
    /// they jitter, each once the points `SMOOTH_M` after it are in, or at
    /// the `end` with what there is.
    fn smooth(&mut self, end: bool) {
        loop {
            let finder = &mut *self.finder;
            let (i, last) = (finder.smoothed, finder.raw_taken);
            if i >= last {
                return;
            }
            let at_m = finder.raw[i % RAW].1;
            if !end && finder.raw[(last - 1) % RAW].1 - at_m < SMOOTH_M {
                return;
            }
            let first = last.saturating_sub(RAW);
            let (mut sum, mut n) = ((0.0, 0.0), 0.0);
            for k in first..last {
                let (p, k_m) = finder.raw[k % RAW];
                if (k_m - at_m).abs() <= SMOOTH_M {
                    (sum, n) = ((sum.0 + p.0, sum.1 + p.1), n + 1.0);
                }
            }
            let from = first.max(i.saturating_sub(NOISE_SPAN)) + 2;
            let to = last.saturating_sub(1);
            let swung = (from..to).filter(|&k| finder.swung[k % RAW]).count();
            let noisy = to > from && swung * 2 > to - from;
            let here = match noisy {
                true => (sum.0 / n, sum.1 / n),
                false => finder.raw[i % RAW].0,
            };
            finder.smoothed += 1;
            self.take(here);
        }
    }

    /// A point for the scan to read.
    fn take(&mut self, here: (f32, f32)) {
        let finder = &mut *self.finder;
        // The slot this takes holds a point the scan may still read.
        finder.lost |=
            !finder.scan.done() && finder.taken - finder.scan.oldest().min(finder.taken) >= RING;
        if let Some(last) = finder.taken.checked_sub(1) {
            let last = finder.ring[last % RING];
            let (east, north) = (here.0 - last.0, here.1 - last.1);
            finder.taken_m += sqrtf(east * east + north * north);
        }
        finder.ring[finder.taken % RING] = here;
        finder.taken += 1;
        self.step(false);
    }

    fn step(&mut self, end: bool) {
        let finder = &mut *self.finder;
        let (ring, taken) = (&finder.ring, finder.taken);
        let at = |k: usize| ring[k % RING];
        while !finder.scan.done()
            && !finder.lost
            && (end || finder.scan.along_m() + LEAD_M < finder.taken_m)
        {
            finder.found = finder.scan.step(taken, at, 1, self.turns, finder.found);
            let back = taken.saturating_sub(finder.scan.oldest());
            finder.reach = finder.reach.max(back);
            // A full list waits for room no upload makes.
            if finder.scan.waiting() {
                finder.full = true;
                finder.scan = Scan::finished();
            }
        }
    }

    /// The track has ended: how many turns were found, in millimetres
    /// along it, or `None` when they cannot be trusted.
    pub(crate) fn scan_to_end(&mut self) -> Option<usize> {
        self.smooth(true);
        self.step(true);
        let finder = &*self.finder;
        (!finder.lost && finder.started).then_some(finder.found)
    }

    /// The course is complete: the turns found, in millimetres along
    /// `course` as it is kept, or `None` when they cannot be trusted.
    pub fn finish(&mut self, course: &super::Course) -> Option<Found> {
        self.scan_to_end()?;
        let finder = &*self.finder;
        let points = course.points();
        if points.len() < 2 {
            return None;
        }
        let kept_mm = &finder.kept_mm[..points.len()];
        let turns = &mut self.turns[..finder.found];
        let mut starts = Onto::new(points, kept_mm);
        let mut ends = Onto::new(points, kept_mm);
        for turn in turns {
            turn.start_mm = starts.at(turn.start_mm);
            turn.end_mm = ends.at(turn.end_mm);
        }
        Some(Found {
            count: finder.found,
            full: finder.full,
            reach: finder.reach,
        })
    }
}

/// Distances along the full track onto the course as kept, `points`, whose
/// point `k` is `kept_mm[k]` along the full track: each in proportion
/// between the kept points either side. Walks on from the last asked, so a
/// run of distances in order costs one pass.
struct Onto<'a> {
    points: &'a [Point],
    kept_mm: &'a [u32],
    scale: f32,
    /// The kept point at or before the last asked, and metres along the
    /// course to it.
    k: usize,
    along: f32,
}

impl<'a> Onto<'a> {
    fn new(points: &'a [Point], kept_mm: &'a [u32]) -> Self {
        Self {
            points,
            kept_mm,
            scale: east_per_e7(points[0]),
            k: 0,
            along: 0.0,
        }
    }

    fn segment(&self, k: usize) -> f32 {
        let (east, north) = metres(self.points[k], self.points[k + 1], self.scale);
        sqrtf(east * east + north * north)
    }

    fn at(&mut self, full_mm: u32) -> u32 {
        if self.kept_mm[self.k] > full_mm {
            (self.k, self.along) = (0, 0.0);
        }
        while self.k + 1 < self.points.len() && self.kept_mm[self.k + 1] <= full_mm {
            self.along += self.segment(self.k);
            self.k += 1;
        }
        let k = self.k;
        if k + 1 >= self.points.len() {
            return mm(self.along);
        }
        let (from, to) = (self.kept_mm[k], self.kept_mm[k + 1]);
        let t = match to.saturating_sub(from) {
            0 => 0.0,
            span => full_mm.saturating_sub(from) as f32 / span as f32,
        };
        mm(self.along + t * self.segment(k))
    }
}

/// How far `p` lies to the side of the line from `a` to `b`, left
/// positive.
fn side(a: (f32, f32), p: (f32, f32), b: (f32, f32)) -> f32 {
    let (east, north) = (b.0 - a.0, b.1 - a.1);
    let length = sqrtf(east * east + north * north).max(0.01);
    (east * (p.1 - a.1) - north * (p.0 - a.0)) / length
}

/// A float cast saturates, so no length overflows.
fn mm(metres: f32) -> u32 {
    (metres * 1000.0) as u32
}

impl Follow for Finding<'_> {
    fn start(&mut self) {
        let finder = &mut *self.finder;
        finder.started = false;
        (finder.raw_taken, finder.smoothed, finder.taken) = (0, 0, 0);
        (finder.taken_m, finder.given_m) = (0.0, 0.0);
        finder.scan = Scan::default();
        (finder.found, finder.lost, finder.full, finder.reach) = (0, false, false, 0);
    }

    fn given(&mut self, point: Point) {
        let finder = &mut *self.finder;
        if !finder.started {
            finder.started = true;
            (finder.origin, finder.east_per_e7) = (point, east_per_e7(point));
        }
        let here = metres(finder.origin, point, finder.east_per_e7);
        let off = |from: (f32, f32)| {
            let (east, north) = (here.0 - from.0, here.1 - from.1);
            sqrtf(east * east + north * north)
        };
        let last_raw = finder.raw_taken.checked_sub(1).map(|k| finder.raw[k % RAW]);
        let here_m = last_raw.map_or(0.0, |(p, p_m)| p_m + off(p));
        // Along the track the scan reads to the last point in the ring, and
        // on from the point taken it stands for.
        finder.given_m = match finder.smoothed.checked_sub(1) {
            Some(k) => finder.taken_m + here_m - finder.raw[k % RAW].1,
            None => here_m,
        };
        let step = last_raw.map_or(f32::INFINITY, |(p, _)| off(p));
        if step < RING_STEP_M {
            return;
        }
        let k = finder.raw_taken;
        finder.raw[k % RAW] = (here, here_m);
        finder.raw_taken += 1;
        if k >= 2 {
            let (a, p) = (finder.raw[(k - 2) % RAW].0, finder.raw[(k - 1) % RAW].0);
            let now = side(a, p, here);
            finder.swung[(k - 1) % RAW] = k >= 3 && (now - finder.side).abs() >= NOISE_M;
            finder.side = now;
        }
        self.smooth(false);
    }

    fn kept(&mut self, index: usize) {
        if let Some(slot) = self.finder.kept_mm.get_mut(index) {
            *slot = mm(self.finder.given_m);
        }
    }

    fn moved(&mut self, to: usize, from: usize) {
        let kept = &mut self.finder.kept_mm;
        if let (true, Some(&from)) = (to < kept.len(), kept.get(from)) {
            kept[to] = from;
        }
    }
}

fn name() -> files::Name {
    files::Name::new(FILE).expect("a valid file name")
}

/// Every byte of the file for `turns` of the course `key`, in a few pieces.
pub fn encode(key: u32, turns: &[Turn], full: bool, mut put: impl FnMut(&[u8])) {
    let turns = &turns[..turns.len().min(TURNS)];
    let mut crc = Crc32::new();
    let mut emit = |bytes: &[u8]| {
        crc = crc.update(bytes);
        put(bytes);
    };
    let mut header = [0u8; HEADER_V1];
    header[..4].copy_from_slice(MAGIC);
    header[4] = VERSION;
    header[5] = HEADER_V1 as u8;
    header[6] = RECORD as u8;
    header[7] = if full { FULL } else { 0 };
    put_u32(&mut header, 8, key);
    put_u16(&mut header, 12, turns.len() as u16);
    emit(&header);
    for turn in turns {
        emit(&turn.record());
    }
    let sum = crc.finish();
    put(&sum.to_le_bytes());
}

/// The stored turns read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub key: u32,
    pub count: usize,
    pub full: bool,
}

/// Read a file of `length` bytes into `turns` through `read`, which fills a
/// buffer from an offset. A file that does not decode reads as `None`, and
/// what it left in `turns` is not a list.
pub fn decode<E>(
    length: u32,
    mut read: impl FnMut(u32, &mut [u8]) -> Result<(), E>,
    turns: &mut [Turn],
) -> Result<Option<Stored>, E> {
    let mut head = [0u8; 255];
    if (length as usize) < HEADER_V1 + 4 {
        return Ok(None);
    }
    read(0, &mut head[..HEADER_V1])?;
    let (header, record) = (usize::from(head[5]), usize::from(head[6]));
    let count = usize::from(get_u16(&head, 12));
    let expected = header as u64 + count as u64 * record as u64 + 4;
    if &head[..4] != MAGIC
        || head[4] < 1
        || header < HEADER_V1
        || record < RECORD
        || count > turns.len()
        || expected != u64::from(length)
    {
        return Ok(None);
    }
    read(0, &mut head[..header])?;
    let mut crc = Crc32::new().update(&head[..header]);
    let stored = Stored {
        key: get_u32(&head, 8),
        count,
        full: head[7] & FULL != 0,
    };
    // Whole turns per read, about a sector's worth; a turn is at most 255
    // bytes.
    let mut chunk = [0u8; SECTOR_SIZE];
    let per = SECTOR_SIZE / record;
    let (mut index, mut valid) = (0, true);
    let mut last = Turn::default();
    while index < count {
        let n = per.min(count - index);
        let bytes = &mut chunk[..n * record];
        read((header + index * record) as u32, bytes)?;
        crc = crc.update(bytes);
        for (slot, bytes) in turns[index..index + n]
            .iter_mut()
            .zip(bytes.chunks_exact(record))
        {
            let Some(turn) = Turn::from_record(bytes) else {
                valid = false;
                continue;
            };
            // In order along the course, each from before it is to.
            valid &= turn.start_mm <= turn.end_mm
                && turn.start_mm >= last.start_mm
                && turn.end_mm >= last.end_mm;
            (*slot, last) = (turn, turn);
        }
        index += n;
    }
    let mut sum = [0u8; 4];
    read(length - 4, &mut sum)?;
    Ok((valid && crc.finish().to_le_bytes() == sum).then_some(stored))
}

/// The turns stored for the course `key` into `turns`: how many, or `None`
/// when there are none of this course's that decode.
pub fn load(
    media: &mut impl ReadWrite,
    key: u32,
    turns: &mut [Turn],
) -> Result<Option<Stored>, Error> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = super::open(media, &mut sector)?;
    let dir = files::mount(&mut store, &mut sector)?;
    let Some(entry) = dir.find(&mut store, &name(), &mut sector)? else {
        return Ok(None);
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
    match decode(entry.length, read, turns) {
        Ok(stored) => Ok(stored.filter(|stored| stored.key == key)),
        Err(files::Error::Range) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Store `turns` of the course `key`, replacing the file there in one
/// atomic commit.
pub fn save(media: &mut impl ReadWrite, key: u32, turns: &[Turn], full: bool) -> Result<(), Error> {
    super::write(media, &name(), |put| encode(key, turns, full, put))
}

/// Remove the stored turns.
pub fn delete(media: &mut impl ReadWrite) -> Result<(), Error> {
    super::remove(media, &name())
}

#[cfg(test)]
pub(crate) mod tests;
