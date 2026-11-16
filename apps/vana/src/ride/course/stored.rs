//! A course's turns, found while it uploads from every point of the file,
//! and stored beside `course.dat` as `course-turns.dat`.
//!
//! Thinning a long course to fit (`course::Builder`) cuts its corners and
//! roundabouts into a few long segments, and a scan of what is kept can only
//! guess at them (`turns`). `Finder` follows the upload instead. It first
//! reads every point given for out-and-back spurs (`turns::Spurs`), and
//! passes on the course as `Spurs` reads it, each spur cut at its base, so
//! what follows reads the course as though its spurs were never drawn. Its
//! scan finds no spurs of its own. It scans the points a metre apart or more
//! as they stream in, keeping only the last `RING` of them, which is all the
//! scan still reads. Where the points jitter, as a recorded GPS track's do,
//! each is first moved to the mean of those about it, so jitter reads as no
//! bend. Whether they jitter is read from the points as given, spurs and
//! all, so a spur cut never changes it. Each turn is found in metres along
//! the full track, spurs and all; once the course is complete, it is moved
//! to the same place along the course as kept, by the full track's distance
//! to each kept point, so the rider's place on the stored course finds it.
//!
//! The file store commits one file at a time, so the turns cannot commit
//! with the course. Instead they name their course by its key
//! (`course::key`), the CRC that ends `course.dat`: turns of any other
//! course, left by a cut between the two writes, are not this course's, and
//! its turns are found by the scan as before. A
//! course without the file, or with one that does not decode, is scanned
//! the same way.
//!
//! Persisted, little-endian:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | magic `VTRN` |
//! | 4 | version, 2 |
//! | 5 | header length, 16 |
//! | 6 | turn length, 12 |
//! | 7 | flags: 1, the course has more turns than are stored; other bits zero |
//! | 8..12 | the course's key |
//! | 12..14 | turns `n`, up to 1024 |
//! | 14..16 | zero |
//! | 16.. | `n` turns of 12 bytes, in order along the course (`Turn::record`) |
//! | 16+12n.. | CRC32 of every byte before it |
//!
//! A turn is from and to in millimetres along the course as stored (u32
//! each), its kind (0 none, 1 left, 2 right, 3 sharp left, 4 sharp right, 5
//! U-turn) and flags (1 two corners, 2 cornered). A file of another version
//! or shape reads as one that does not decode.
use super::turns::{RECORD, Scan, Spurs, TURNS, Turn, distance, starts_at};
use super::{Error, Follow, POINTS, Point, east_per_e7, metres};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::Crc32;
use firmware_services::files;
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};
use libm::sqrtf;

const FILE: &str = "course-turns.dat";
const MAGIC: &[u8; 4] = b"VTRN";
pub const VERSION: u8 = 2;
const HEADER: usize = 16;
/// The course has more turns than the file holds.
const FULL: u8 = 1;
/// The longest file this version writes.
pub const MAX_FILE: usize = HEADER + TURNS * RECORD + 4;

/// Capacity of the input-point buffer used during turn finding.
pub const GIVEN: usize = 2048;
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
/// The scan and `Spurs` step only while the track is in this far beyond
/// the point their chord heads for, which is more than their first step
/// reads, so neither takes the end of what has come in so far for the
/// course's end.
const LEAD_M: f32 = 20.0;
/// Spurs in the ring that the scan may still read before.
const CUTS: usize = 8;

/// A point taken, metres along the track as the scan reads it, and of
/// spurs before it.
#[derive(Clone, Copy, Debug, Default)]
struct Taken {
    at: (f32, f32),
    m: f32,
    spurs_m: f32,
    jitter_k: usize,
}

/// The last point passed on outside a spur, metres along the points given
/// to it and along the full track.
#[derive(Clone, Copy, Debug, Default)]
struct Passed {
    given_m: f32,
    track_m: f32,
}

/// The ring's point `first`, the first after a spur, and the millimetres
/// along the ring before it and of the spur.
#[derive(Clone, Copy, Debug, Default)]
struct Cut {
    first: usize,
    from_mm: u32,
    spur_mm: u32,
}

/// Finds a course's turns while it uploads. All zero bytes are a valid
/// `Finder`, so composition may hand over zeroed memory; `Follow::start`
/// begins each course.
pub struct Finder {
    /// The first point and metres east per 1e-7 degree there: the frame
    /// the course's lengths are measured in.
    origin: Point,
    east_per_e7: f32,
    started: bool,
    /// The last `GIVEN` points given, in metres from the origin, and metres
    /// along them to each: point `k` is `given[k % GIVEN]`. The first
    /// `passed` have been passed on.
    given: [((f32, f32), f32); GIVEN],
    given_count: usize,
    passed: usize,
    spurs: Spurs,
    last_passed: Passed,
    /// Metres of spurs passed since the last point taken.
    spur_m: f32,
    /// The last points taken: point `k` is `raw[k % RAW]`. `smoothed` of
    /// them are in the ring.
    raw: [Taken; RAW],
    raw_taken: usize,
    jitter: [(f32, f32); 2],
    jitter_taken: usize,
    /// Whether, at point `k` the jitter test took, how far it lies to the
    /// side of the line between its neighbours swung by `NOISE_M` or more
    /// from the point before (`swung[k % RAW]`), and that for the newest
    /// point it is known.
    swung: [bool; RAW],
    side: f32,
    smoothed: usize,
    /// The last `RING` points as the scan reads them, each a point taken or
    /// the mean about one: point `k` is at `ring[k % RING]`.
    ring: [(f32, f32); RING],
    /// Metres along the track as the scan reads it to each point in the
    /// ring.
    ring_m: [f32; RING],
    taken: usize,
    /// Metres along the track as the scan reads it to the last point in the
    /// ring.
    taken_m: f32,
    /// Metres of spurs before the last point in the ring.
    ring_spurs_m: f32,
    /// The spurs in the ring the scan may still read before, and the
    /// millimetres of those it has read past.
    cuts: [Cut; CUTS],
    cut_count: usize,
    passed_spurs_mm: u32,
    /// Millimetres along the full track to each point the course keeps,
    /// once it is passed on, and which point given each is.
    kept_mm: [u32; POINTS],
    kept_given: [u32; POINTS],
    kept_len: usize,
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
    fn jitter(&mut self, p: (f32, f32)) -> usize {
        let k = self.jitter_taken;
        if k > 0 && distance(self.jitter[1], p) < RING_STEP_M {
            return k - 1;
        }
        if k >= 2 {
            let now = side(self.jitter[0], self.jitter[1], p);
            self.swung[(k - 1) % RAW] = k >= 3 && (now - self.side).abs() >= NOISE_M;
            self.side = now;
        }
        self.jitter = [self.jitter[1], p];
        self.jitter_taken += 1;
        k
    }

    /// What zeroed memory holds, for a host that builds one in place.
    pub fn new() -> Self {
        Self {
            origin: Point::default(),
            east_per_e7: 0.0,
            started: false,
            given: [((0.0, 0.0), 0.0); GIVEN],
            given_count: 0,
            passed: 0,
            spurs: Spurs::default(),
            last_passed: Passed::default(),
            spur_m: 0.0,
            raw: [Taken::default(); RAW],
            raw_taken: 0,
            jitter: [(0.0, 0.0); 2],
            jitter_taken: 0,
            swung: [false; RAW],
            side: 0.0,
            smoothed: 0,
            ring: [(0.0, 0.0); RING],
            ring_m: [0.0; RING],
            taken: 0,
            taken_m: 0.0,
            ring_spurs_m: 0.0,
            cuts: [Cut::default(); CUTS],
            cut_count: 0,
            passed_spurs_mm: 0,
            kept_mm: [0; POINTS],
            kept_given: [0; POINTS],
            kept_len: 0,
            scan: Scan::default(),
            found: 0,
            lost: false,
            full: false,
            reach: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn spurs_m(&self) -> f32 {
        let last = self.raw_taken.checked_sub(1).map(|k| self.raw[k % RAW]);
        last.map_or(0.0, |last| last.spurs_m) + self.spur_m
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
    /// Read the points given for spurs, and pass on or drop each once no
    /// spur still to be found can take it; at the `end`, every one left.
    fn unspur(&mut self, end: bool) {
        loop {
            let finder = &mut *self.finder;
            let (given, count) = (&finder.given, finder.given_count);
            let at = |k: usize| given[k % GIVEN].0;
            let newest_m = count.checked_sub(1).map_or(0.0, |k| given[k % GIVEN].1);
            let lead = |k: usize| k < count && newest_m - given[k % GIVEN].1 > LEAD_M;
            while !finder.spurs.done() && (end || lead(finder.spurs.reads_to())) {
                finder.spurs.step(count, &at);
            }
            let settled = match finder.spurs.done() {
                true => count,
                false => finder.spurs.settled(),
            };
            let k = finder.passed;
            if k >= settled {
                return;
            }
            self.pass_on(k);
            let finder = &mut *self.finder;
            finder.passed += 1;
            finder.spurs.passed(finder.passed);
        }
    }

    fn pass_on(&mut self, k: usize) {
        let finder = &mut *self.finder;
        let given = &finder.given;
        let here = finder.spurs.at(&|j: usize| given[j % GIVEN].0, k);
        let (given_here, here_m) = given[k % GIVEN];
        let jitter_k = finder.jitter(given_here);
        finder.spur_m += finder.spurs.cut_at(k);
        let last = finder.raw_taken.checked_sub(1).map(|j| finder.raw[j % RAW]);
        let step = last.map_or(f32::INFINITY, |last| distance(last.at, here));
        let (m, spurs_m) = last.map_or((0.0, 0.0), |last| {
            (last.m + step, last.spurs_m + finder.spur_m)
        });
        let track_m = match finder.spurs.skipped(k) {
            true => finder.last_passed.track_m + here_m - finder.last_passed.given_m,
            false => {
                finder.last_passed = Passed {
                    given_m: here_m,
                    track_m: m + spurs_m,
                };
                m + spurs_m
            }
        };
        if step >= RING_STEP_M {
            finder.spur_m = 0.0;
            self.take_raw(Taken {
                at: here,
                m,
                spurs_m,
                jitter_k,
            });
        }
        self.place(k, track_m);
    }

    /// Point `k` given, if the course keeps it, lies `track_m` along the
    /// full track: so far along the track as the scan's turns are measured.
    fn place(&mut self, k: usize, track_m: f32) {
        let finder = &mut *self.finder;
        let kept = &finder.kept_given[..finder.kept_len];
        let Ok(index) = kept.binary_search(&(k as u32)) else {
            return;
        };
        let along = match finder.smoothed.checked_sub(1) {
            Some(j) => finder.taken_m + track_m - finder.raw[j % RAW].m,
            None => track_m,
        };
        finder.kept_mm[index] = mm(along);
    }

    fn take_raw(&mut self, point: Taken) {
        let finder = &mut *self.finder;
        let k = finder.raw_taken;
        finder.raw[k % RAW] = point;
        finder.raw_taken += 1;
        self.smooth(false);
    }

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
            let at_m = finder.raw[i % RAW].m;
            if !end && finder.raw[(last - 1) % RAW].m - at_m < SMOOTH_M {
                return;
            }
            let first = last.saturating_sub(RAW);
            let (mut sum, mut n) = ((0.0, 0.0), 0.0);
            for k in first..last {
                let Taken { at: p, m: k_m, .. } = finder.raw[k % RAW];
                if (k_m - at_m).abs() <= SMOOTH_M {
                    (sum, n) = ((sum.0 + p.0, sum.1 + p.1), n + 1.0);
                }
            }
            let jitter_i = finder.raw[i % RAW].jitter_k;
            let jitter_last = finder.raw[(last - 1) % RAW].jitter_k + 1;
            let jitter_first = jitter_last.saturating_sub(RAW);
            let from = jitter_first.max(jitter_i.saturating_sub(NOISE_SPAN)) + 2;
            let to = jitter_last.saturating_sub(1);
            let swung = (from..to).filter(|&k| finder.swung[k % RAW]).count();
            let noisy = to > from && swung * 2 > to - from;
            let here = match noisy {
                true => (sum.0 / n, sum.1 / n),
                false => finder.raw[i % RAW].at,
            };
            let spurs_m = finder.raw[i % RAW].spurs_m;
            finder.smoothed += 1;
            self.take(here, spurs_m);
        }
    }

    /// A point for the scan to read, with `spurs_m` of spurs before it.
    fn take(&mut self, here: (f32, f32), spurs_m: f32) {
        let finder = &mut *self.finder;
        // The slot this takes holds a point the scan may still read.
        finder.lost |=
            !finder.scan.done() && finder.taken - finder.scan.oldest().min(finder.taken) >= RING;
        if spurs_m > finder.ring_spurs_m {
            let cut = Cut {
                first: finder.taken,
                from_mm: mm(finder.taken_m),
                spur_mm: mm(spurs_m) - mm(finder.ring_spurs_m),
            };
            match finder.cuts.get_mut(finder.cut_count) {
                Some(slot) => {
                    *slot = cut;
                    finder.cut_count += 1;
                }
                None => finder.lost = true,
            }
            finder.ring_spurs_m = spurs_m;
        }
        if let Some(last) = finder.taken.checked_sub(1) {
            finder.taken_m += distance(finder.ring[last % RING], here);
        }
        finder.ring[finder.taken % RING] = here;
        finder.ring_m[finder.taken % RING] = finder.taken_m;
        finder.taken += 1;
        self.step(false);
    }

    fn step(&mut self, end: bool) {
        let finder = &mut *self.finder;
        let (ring, ring_m, taken) = (&finder.ring, &finder.ring_m, finder.taken);
        let at = |k: usize| ring[k % RING];
        let lead = |k: usize| k < taken && finder.taken_m - ring_m[k % RING] > LEAD_M;
        while !finder.scan.done() && !finder.lost && (end || lead(finder.scan.reads_to())) {
            let listed = finder.found;
            finder.found = finder.scan.step(taken, at, 1, self.turns, finder.found);
            let cuts = &finder.cuts[..finder.cut_count];
            let onto_track = |mm: u32| {
                let spurs_mm: u32 = cuts
                    .iter()
                    .filter(|cut| mm > cut.from_mm)
                    .map(|cut| cut.spur_mm)
                    .sum();
                mm.saturating_add(finder.passed_spurs_mm + spurs_mm)
            };
            for turn in &mut self.turns[listed..finder.found] {
                turn.start_mm = starts_at(turn.start_mm, onto_track);
                turn.end_mm = onto_track(turn.end_mm);
            }
            let oldest = finder.scan.oldest();
            let read_past = cuts.iter().take_while(|cut| cut.first <= oldest).count();
            finder.passed_spurs_mm += cuts[..read_past].iter().map(|cut| cut.spur_mm).sum::<u32>();
            finder.cuts.copy_within(read_past..finder.cut_count, 0);
            finder.cut_count -= read_past;
            let back = taken.saturating_sub(oldest);
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
        self.unspur(true);
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
        (finder.given_count, finder.passed) = (0, 0);
        finder.spurs = Spurs::default();
        (finder.last_passed, finder.spur_m) = (Passed::default(), 0.0);
        (finder.raw_taken, finder.smoothed, finder.taken) = (0, 0, 0);
        finder.jitter_taken = 0;
        (finder.taken_m, finder.ring_spurs_m) = (0.0, 0.0);
        (finder.cut_count, finder.passed_spurs_mm, finder.kept_len) = (0, 0, 0);
        finder.scan = Scan::default();
        (finder.found, finder.lost, finder.full, finder.reach) = (0, false, false, 0);
    }

    fn given(&mut self, point: Point) {
        let finder = &mut *self.finder;
        if finder.lost {
            return;
        }
        if !finder.started {
            finder.started = true;
            (finder.origin, finder.east_per_e7) = (point, east_per_e7(point));
        }
        let here = metres(finder.origin, point, finder.east_per_e7);
        let k = finder.given_count;
        let here_m = k.checked_sub(1).map_or(0.0, |last| {
            let (p, p_m) = finder.given[last % GIVEN];
            p_m + distance(p, here)
        });
        // The slot this takes holds a point still to pass on or read.
        let oldest = match finder.spurs.done() {
            true => finder.passed,
            false => finder.passed.min(finder.spurs.oldest()),
        };
        if k - oldest.min(k) >= GIVEN {
            finder.lost = true;
            return;
        }
        finder.given[k % GIVEN] = (here, here_m);
        finder.given_count += 1;
        self.unspur(false);
    }

    fn kept(&mut self, index: usize) {
        let finder = &mut *self.finder;
        if let Some(slot) = finder.kept_given.get_mut(index) {
            *slot = finder.given_count.saturating_sub(1) as u32;
            finder.kept_len = index + 1;
        }
    }

    fn moved(&mut self, to: usize, from: usize) {
        let finder = &mut *self.finder;
        if to < POINTS && from < POINTS {
            finder.kept_mm[to] = finder.kept_mm[from];
            finder.kept_given[to] = finder.kept_given[from];
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
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(MAGIC);
    header[4] = VERSION;
    header[5] = HEADER as u8;
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
    let mut head = [0u8; HEADER];
    if (length as usize) < HEADER + 4 {
        return Ok(None);
    }
    read(0, &mut head)?;
    let count = usize::from(get_u16(&head, 12));
    if &head[..4] != MAGIC
        || head[4] != VERSION
        || usize::from(head[5]) != HEADER
        || usize::from(head[6]) != RECORD
        || head[7] & !FULL != 0
        || count > turns.len()
        || HEADER + count * RECORD + 4 != length as usize
    {
        return Ok(None);
    }
    let mut crc = Crc32::new().update(&head);
    let stored = Stored {
        key: get_u32(&head, 8),
        count,
        full: head[7] & FULL != 0,
    };
    let mut chunk = [0u8; SECTOR_SIZE];
    let per = SECTOR_SIZE / RECORD;
    let (mut index, mut valid) = (0, true);
    let mut last = Turn::default();
    while index < count {
        let n = per.min(count - index);
        let bytes = &mut chunk[..n * RECORD];
        read((HEADER + index * RECORD) as u32, bytes)?;
        crc = crc.update(bytes);
        for (slot, bytes) in turns[index..index + n]
            .iter_mut()
            .zip(bytes.as_chunks::<RECORD>().0)
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
