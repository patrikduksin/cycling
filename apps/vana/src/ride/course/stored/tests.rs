use super::*;
use crate::ride::course::tests::{at as point_at, road};
use crate::ride::course::turns::Warner;
use crate::ride::course::turns::tests::{
    arc, bends, block, build, curved_ring, extended, hairpin, join, path, ring,
};
use crate::ride::course::{Course, east_per_e7, key, metres, save as save_course};
use crate::testing::Rng;
use crate::workout::library::tests::{Region, formatted, put};
use device_api::bulk::{Error as MediaError, OwnedInfo};
use firmware_services::volume;
use libm::fabsf;
use std::boxed::Box;
use std::vec;
use std::vec::Vec;

/// `full`, metres east and north of a start, uploaded: the course kept,
/// and its turns as stored and read back.
pub(crate) fn uploaded(full: &[(f32, f32)]) -> (Box<Course>, Vec<Turn>) {
    let (course, turns, _) = found(full);
    let bytes = file(key(&course), &turns, false);
    let mut back = vec![Turn::default(); TURNS];
    let stored = decoded(&bytes, &mut back).expect("the turns decode");
    back.truncate(stored.count);
    assert_eq!(records(&back), records(&turns));
    (course, back)
}

/// `full` uploaded, its turns as found, before they are stored.
pub(crate) fn found(full: &[(f32, f32)]) -> (Box<Course>, Vec<Turn>, Found) {
    let mut finder = Box::new(Finder::new());
    let mut turns = vec![Turn::default(); TURNS];
    let mut finding = Finding {
        finder: &mut finder,
        turns: &mut turns,
    };
    let (course, _) = build(full, &mut finding);
    let found = finding.finish(&course).expect("turns found");
    turns.truncate(found.count);
    (course, turns, found)
}

pub(crate) fn file(key: u32, turns: &[Turn], full: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode(key, turns, full, |piece| bytes.extend_from_slice(piece));
    bytes
}

pub(crate) fn decoded(bytes: &[u8], turns: &mut [Turn]) -> Option<Stored> {
    let read = |offset: u32, out: &mut [u8]| {
        let start = offset as usize;
        let piece = bytes.get(start..start + out.len()).ok_or(())?;
        out.copy_from_slice(piece);
        Ok::<(), ()>(())
    };
    decode(bytes.len() as u32, read, turns).ok().flatten()
}

fn records(turns: &[Turn]) -> Vec<[u8; RECORD]> {
    turns.iter().map(Turn::record).collect()
}

/// Courses the gates ride: roundabouts, hairpins, city blocks and mixed
/// corners, as drawn and with 12 or 150 km after them.
fn gate_courses() -> Vec<Vec<(f32, f32)>> {
    let mut drawn = Vec::new();
    for spacing in [3.0, 5.0, 8.0] {
        for radius in [15.0, 25.0, 40.0] {
            for sweep in [90.0, 180.0, 270.0, 330.0] {
                drawn.push(ring(radius, sweep, spacing));
                drawn.push(curved_ring(radius, sweep, spacing));
            }
        }
        for radius in [20.0, 30.0, 40.0] {
            drawn.push(hairpin(radius, spacing));
        }
        for apart in [20.0, 40.0, 80.0] {
            drawn.push(block(spacing, apart, 4.0, true));
            let rll = [
                (400.0, 90.0),
                (400.0 + apart, -90.0),
                (400.0 + 2.0 * apart, -90.0),
            ];
            drawn.push(bends(spacing, &rll));
        }
    }
    let mut courses = Vec::new();
    for line in &drawn {
        for after in [0.0, 12_000.0, 150_000.0] {
            courses.push(extended(line, after));
        }
    }
    courses
}

/// `points` in metres from the first, as the course's frame measures.
fn framed(points: &[Point]) -> Vec<(f32, f32)> {
    let scale = east_per_e7(points[0]);
    points
        .iter()
        .map(|&p| metres(points[0], p, scale))
        .collect()
}

fn length(east: f32, north: f32) -> f32 {
    sqrtf(east * east + north * north)
}

/// The point `metres` along `line`.
fn along(line: &[(f32, f32)], metres: f32) -> (f32, f32) {
    let mut walked = 0.0;
    for pair in line.windows(2) {
        let (east, north) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
        let step = length(east, north);
        if walked + step >= metres && step > 0.0 {
            let t = (metres - walked) / step;
            return (pair[0].0 + t * east, pair[0].1 + t * north);
        }
        walked += step;
    }
    *line.last().unwrap()
}

/// Metres along `line` to the point nearest `p`, within `window` metres
/// of `near` along it, and how far off it `p` is.
fn projected(line: &[(f32, f32)], p: (f32, f32), near: f32, window: f32) -> (f32, f32) {
    let (mut best, mut walked) = ((f32::MAX, 0.0), 0.0);
    for pair in line.windows(2) {
        let (east, north) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
        let step = length(east, north);
        if step > 0.0 && walked + step >= near - window && walked <= near + window {
            let t = (((p.0 - pair[0].0) * east + (p.1 - pair[0].1) * north) / (step * step))
                .clamp(0.0, 1.0);
            let off = length(pair[0].0 + t * east - p.0, pair[0].1 + t * north - p.1);
            if off < best.0 {
                best = (off, walked + t * step);
            }
        }
        walked += step;
    }
    (best.1, best.0)
}

/// Each turn is found at a distance along the full track and stored at a
/// distance along the course as kept. The full track's point there,
/// projected onto the kept course, lands within a few metres of where the
/// stored distance says, so the rider's place along the kept course meets
/// the turn where it is.
#[test]
fn a_turn_stored_along_the_kept_course_is_where_it_is_on_the_full_track() {
    let (mut errors, mut offs) = (Vec::new(), Vec::new());
    for full in gate_courses() {
        let mut finder = Box::new(Finder::new());
        let mut turns = vec![Turn::default(); TURNS];
        let mut finding = Finding {
            finder: &mut finder,
            turns: &mut turns,
        };
        let (course, _) = build(&full, &mut finding);
        let count = finding.scan_to_end().unwrap();
        let on_full = finding.turns[..count].to_vec();
        let found = finding.finish(&course).unwrap();
        let points: Vec<Point> = full
            .iter()
            .map(|&(east, north)| point_at(f64::from(east), f64::from(north)))
            .collect();
        let (track, kept) = (framed(&points), framed(course.points()));
        for (fine, stored) in on_full.iter().zip(&turns[..found.count]) {
            for (fine_mm, stored_mm) in [
                (fine.start_mm, stored.start_mm),
                (fine.end_mm, stored.end_mm),
            ] {
                let p = along(&track, fine_mm as f32 / 1000.0);
                let stored_m = stored_mm as f32 / 1000.0;
                let (on_kept, off) = projected(&kept, p, stored_m, 100.0);
                errors.push(fabsf(on_kept - stored_m));
                offs.push(off);
            }
        }
    }
    errors.sort_by(f32::total_cmp);
    offs.sort_by(f32::total_cmp);
    let at = |list: &[f32], q: f32| list[((list.len() - 1) as f32 * q) as usize];
    std::println!(
        "{} turn ends: error median {:.2} m, 95th {:.2} m, max {:.2} m; the full track off the kept course: median {:.2} m, max {:.2} m",
        errors.len(),
        at(&errors, 0.5),
        at(&errors, 0.95),
        at(&errors, 1.0),
        at(&offs, 0.5),
        at(&offs, 1.0)
    );
    assert!(errors.len() > 500);
    assert!(at(&errors, 1.0) <= 8.0, "{:?}", &errors[errors.len() - 8..]);
}

/// A medium that loses power after `writes` more sector writes: none after
/// lands.
struct Cut {
    region: Region,
    writes: usize,
}

impl ReadWrite for Cut {
    fn owned_info(&mut self) -> Result<OwnedInfo, MediaError> {
        self.region.owned_info()
    }

    fn owned_read(&mut self, index: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), MediaError> {
        self.region.owned_read(index, out)
    }

    fn owned_write(&mut self, index: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), MediaError> {
        if self.writes == 0 {
            return Err(MediaError::Failed);
        }
        self.writes -= 1;
        self.region.owned_write(index, input)
    }
}

/// What the device reads after a restart: the course, and its turns when
/// they are stored and its own.
fn reread(region: &mut Region) -> (Box<Course>, Option<Vec<[u8; RECORD]>>) {
    let mut course = Box::new(Course::empty());
    crate::ride::course::load(region, &mut course).unwrap();
    let mut turns = vec![Turn::default(); TURNS];
    let stored = load(region, key(&course), &mut turns).unwrap();
    (course, stored.map(|stored| records(&turns[..stored.count])))
}

/// An upload cut off at any sector write leaves the old course with its
/// turns, or the new one with its turns or none: never a course with
/// another course's turns.
#[test]
fn a_cut_at_any_write_never_gives_a_course_another_courses_turns() {
    let (old, old_turns) = uploaded(&extended(&ring(20.0, 270.0, 5.0), 12_000.0));
    let (new, new_turns) = uploaded(&extended(&hairpin(30.0, 5.0), 3_000.0));
    let mut region = formatted();
    save_course(&mut region, &old).unwrap();
    save(&mut region, key(&old), &old_turns, false).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for writes in 0.. {
        let mut media = Cut {
            region: region.clone(),
            writes,
        };
        let done = save_course(&mut media, &new).is_ok()
            && save(&mut media, key(&new), &new_turns, false).is_ok();
        let (course, turns) = reread(&mut media.region);
        let is_new = course.points() == new.points();
        assert!(is_new || course.points() == old.points(), "{writes} writes");
        let own = records(if is_new { &new_turns } else { &old_turns });
        assert!(
            turns.as_ref().is_none_or(|turns| *turns == own),
            "{writes} writes"
        );
        seen.insert((is_new, turns.is_some()));
        if done {
            assert_eq!((is_new, turns.is_some()), (true, true));
            std::println!("{writes} writes, cut after each");
            break;
        }
    }
    let both = [(false, true), (true, false), (true, true)];
    assert!(both.iter().all(|state| seen.contains(state)), "{seen:?}");
}

fn turns_file(region: &mut Region) -> Option<files::Entry> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    dir.find(&mut store, &name(), &mut sector).unwrap()
}

/// A course older firmware stored has no turns beside it, and one it
/// stored over a newer course's leaves turns that are not its own: either
/// reads as none, and is scanned. Deleting a course deletes its turns.
#[test]
fn a_course_without_its_own_turns_reads_none() {
    let (first, first_turns) = uploaded(&extended(&ring(20.0, 270.0, 5.0), 12_000.0));
    let mut region = formatted();
    save_course(&mut region, &first).unwrap();
    assert_eq!(reread(&mut region).1, None);
    save(&mut region, key(&first), &first_turns, false).unwrap();
    assert_eq!(reread(&mut region).1, Some(records(&first_turns)));
    let (second, _) = uploaded(&extended(&hairpin(30.0, 5.0), 3_000.0));
    save_course(&mut region, &second).unwrap();
    assert_eq!(reread(&mut region).1, None);
    crate::ride::course::delete(&mut region).unwrap();
    assert_eq!(turns_file(&mut region), None);
    crate::ride::course::delete(&mut region).unwrap();
}

/// A turns file damaged in any way reads as none: a byte changed anywhere,
/// cut short, or longer.
#[test]
fn a_damaged_turns_file_reads_none() {
    let (course, turns) = uploaded(&extended(&ring(20.0, 270.0, 5.0), 0.0));
    assert!(!turns.is_empty());
    let bytes = file(key(&course), &turns, false);
    let mut list = vec![Turn::default(); TURNS];
    for at in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[at] ^= 0x10;
        let mut region = formatted();
        put(&mut region, FILE, &damaged);
        let read = load(&mut region, key(&course), &mut list);
        assert_eq!(read, Ok(None), "byte {at}");
    }
    for length in [0, 1, HEADER_V1, bytes.len() - 1] {
        assert_eq!(decoded(&bytes[..length], &mut list), None, "{length} bytes");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(decoded(&longer, &mut list), None);
}

/// Hostile files, random and mutated from a real one, some sealed with a
/// right CRC, never panic, and any that decodes holds a list in order.
#[test]
fn hostile_turns_files_never_panic() {
    let (course, turns) = uploaded(&extended(&ring(20.0, 270.0, 5.0), 0.0));
    let real = file(key(&course), &turns, false);
    let mut rng = Rng::new(297);
    let mut list = vec![Turn::default(); TURNS];
    let (cases, mut decoded_some) = (20_000, 0);
    for case in 0..cases {
        let mut bytes: Vec<u8> = match case % 3 {
            0 => (0..rng.below(80)).map(|_| rng.below(256) as u8).collect(),
            _ => real.clone(),
        };
        for _ in 0..rng.range(1, 6) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len() as u64) as usize;
            match rng.below(4) {
                0 => bytes[at] = rng.below(256) as u8,
                1 => bytes.truncate(at),
                2 => bytes.insert(at, rng.below(256) as u8),
                _ => bytes[at.min(HEADER_V1 - 1)] = rng.below(256) as u8,
            }
        }
        if case % 2 == 0 && bytes.len() >= 4 {
            let n = bytes.len() - 4;
            let sum = firmware_services::checksum::crc32(&bytes[..n]);
            bytes[n..].copy_from_slice(&sum.to_le_bytes());
        }
        if let Some(stored) = decoded(&bytes, &mut list) {
            decoded_some += 1;
            let got = &list[..stored.count];
            assert!(got.iter().all(|t| t.start_mm <= t.end_mm), "case {case}");
            let ordered = got
                .windows(2)
                .all(|p| p[0].start_mm <= p[1].start_mm && p[0].end_mm <= p[1].end_mm);
            assert!(ordered, "case {case}");
        }
    }
    std::println!("{decoded_some} of {cases} hostile files decoded");
}

/// A course of more turns than the list holds keeps the first 1,024, says
/// so, stores and reads them back, and warns of none past them.
#[test]
fn a_course_of_more_turns_than_the_list_holds_keeps_the_first() {
    let corners: Vec<(f32, f32)> = (0..1_500)
        .map(|k| {
            (
                400.0 + 120.0 * k as f32,
                if k % 2 == 0 { 90.0 } else { -90.0 },
            )
        })
        .collect();
    let (course, turns, found) = found(&bends(5.0, &corners));
    assert!(found.full, "{found:?}");
    assert_eq!(found.count, TURNS);
    let bytes = file(key(&course), &turns, found.full);
    assert!(bytes.len() <= MAX_FILE);
    let mut list = vec![Turn::default(); TURNS];
    let stored = decoded(&bytes, &mut list).unwrap();
    assert_eq!((stored.count, stored.full), (TURNS, true));
    let mut warner = Warner::default();
    let past = list[TURNS - 1].end_mm + 50_000;
    let said: Vec<_> = (0..2_000)
        .filter_map(|k| warner.step(&list, past + k * 7_000))
        .collect();
    assert!(said.is_empty(), "{said:?}");
}

/// The scan reads back no further than the ring keeps on every gate
/// course, a long winding road, and a track that circles for kilometres,
/// which keeps a turn open longest.
#[test]
fn the_scan_never_reads_past_what_the_ring_keeps() {
    let mut most = 0;
    for full in gate_courses() {
        most = most.max(found(&full).2.reach);
    }
    let winding = framed(&road(60.0, 2.0, 5));
    most = most.max(found(&winding).2.reach);
    std::println!("the scan read at most {most} points back, of {RING}");
    assert!(most < RING);
    let circles = join(&[
        path(&[(0.0, -400.0), (0.0, -30.0)], 1.0),
        arc((30.0, -30.0), 30.0, 270.0, 360.0 * 26.0, 1.0),
    ]);
    let mut finder = Box::new(Finder::new());
    let mut list = vec![Turn::default(); TURNS];
    let mut finding = Finding {
        finder: &mut finder,
        turns: &mut list,
    };
    let (course, _) = build(&circles, &mut finding);
    let circled = finding.finish(&course);
    std::println!("circling: {circled:?}");
    assert!(circled.is_none_or(|found| found.reach < RING));
}

/// What finding the turns at upload costs: a 300 km course of a point every
/// 5 m, built alone and with its turns found, and the file.
/// `cargo test -p vana --lib --release upload_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement"]
fn upload_cost() {
    let mut rng = Rng::new(9);
    let corners: Vec<(f32, f32)> = (0..300)
        .map(|k| {
            (
                400.0 + 1_000.0 * k as f32,
                if rng.below(2) == 0 { 90.0 } else { -90.0 },
            )
        })
        .collect();
    let full = bends(5.0, &corners);
    let points: Vec<Point> = full
        .iter()
        .map(|&(east, north)| point_at(f64::from(east), f64::from(north)))
        .collect();
    fn build(points: &[Point], follow: &mut impl Follow) -> Box<Course> {
        let mut course = Box::new(Course::empty());
        let mut builder = crate::ride::course::Builder::new();
        builder.start(&mut course, follow);
        for &point in points {
            builder.push(&mut course, point, follow);
        }
        builder.finish(&mut course, follow);
        course
    }
    let mut best = [u128::MAX; 3];
    let mut finder = Box::new(Finder::new());
    let mut turns = vec![Turn::default(); TURNS];
    for _ in 0..5 {
        let start = std::time::Instant::now();
        build(&points, &mut ());
        best[0] = best[0].min(start.elapsed().as_micros());
        let start = std::time::Instant::now();
        let mut finding = Finding {
            finder: &mut finder,
            turns: &mut turns,
        };
        let course = build(&points, &mut finding);
        let built = start.elapsed().as_micros();
        let found = finding.finish(&course).unwrap();
        best[1] = best[1].min(built);
        best[2] = best[2].min(start.elapsed().as_micros() - built);
        assert!(found.count >= 290, "{found:?}");
    }
    std::println!(
        "{} points: built alone {} us, with turns found {} us ({:.0} ns more a point), moved onto the kept course {} us; Finder {} bytes, a turn {} bytes, the largest file {} bytes",
        points.len(),
        best[0],
        best[1],
        (best[1] - best[0]) as f64 * 1000.0 / points.len() as f64,
        best[2],
        core::mem::size_of::<Finder>(),
        RECORD,
        MAX_FILE
    );
}
