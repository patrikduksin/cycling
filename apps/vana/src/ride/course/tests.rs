use super::*;
use crate::ride::thin::off_segment;
use crate::testing::Rng;
use crate::workout::library::tests::{Region, formatted, put};
use std::boxed::Box;
use std::vec::Vec;

pub(crate) fn empty() -> Box<Course> {
    Box::new(Course::empty())
}

/// About Santiago, metres east and north of a fixed start.
pub(crate) fn at(east: f64, north: f64) -> Point {
    let (lat, lon) = (-33.45f64, -70.66f64);
    let lat_e7 = ((lat + north / 111_195.0) * 1e7).round() as i32;
    let lon_e7 = ((lon + east / (111_195.0 * lat.to_radians().cos())) * 1e7).round() as i32;
    Point::new(lat_e7, lon_e7).unwrap()
}

fn built(points: impl IntoIterator<Item = Point>) -> Box<Course> {
    let mut course = empty();
    let mut builder = Builder::new();
    builder.start(&mut course, &mut ());
    for point in points {
        builder.push(&mut course, point, &mut ());
    }
    builder.finish(&mut course, &mut ());
    course
}

fn file(course: &Course) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode(course, |piece| bytes.extend_from_slice(piece));
    bytes
}

fn decoded(bytes: &[u8]) -> Result<Box<Course>, DecodeError<()>> {
    let mut course = empty();
    let read = |offset: u32, out: &mut [u8]| {
        let start = offset as usize;
        let piece = bytes.get(start..start + out.len()).ok_or(())?;
        out.copy_from_slice(piece);
        Ok(())
    };
    decode(bytes.len() as u32, read, &mut course).map(|()| course)
}

fn same(a: &Course, b: &Course) -> bool {
    a.points() == b.points() && a.name() == b.name()
}

/// A square loop of `side` metres, a fix every `step` metres.
fn square(side: f64, step: f64) -> Vec<Point> {
    let corners = [
        (0.0, 0.0),
        (side, 0.0),
        (side, side),
        (0.0, side),
        (0.0, 0.0),
    ];
    let mut points = Vec::new();
    for pair in corners.windows(2) {
        let n = (side / step) as usize;
        for i in 0..n {
            let t = i as f64 / n as f64;
            points.push(at(
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            ));
        }
    }
    points.push(at(0.0, 0.0));
    points
}

#[test]
fn zeroed_memory_is_an_empty_course() {
    let course = empty();
    assert!(course.is_empty());
    assert_eq!(course.name(), "");
    assert_eq!(course.length_m(), 0);
    // SAFETY: all zero bytes are a valid `Course`, which this checks.
    let zeroed: Box<Course> = unsafe { Box::new_zeroed().assume_init() };
    assert!(zeroed.is_empty() && zeroed.name().is_empty());
}

#[test]
fn a_dense_course_keeps_its_corners_and_its_length() {
    // 4 km round a square, a fix every metre.
    let course = built(square(1_000.0, 1.0));
    let points = course.points();
    assert!(points.len() < 900, "{} points", points.len());
    assert_eq!(points.first(), Some(&at(0.0, 0.0)));
    assert_eq!(points.last(), Some(&at(0.0, 0.0)));
    let scale = east_per_e7(points[0]);
    for corner in [at(1_000.0, 0.0), at(1_000.0, 1_000.0), at(0.0, 1_000.0)] {
        let near = points.iter().any(|&p| {
            let (east, north) = metres(p, corner, scale);
            east * east + north * north <= MIN_STEP_M * MIN_STEP_M
        });
        assert!(near, "{corner:?} kept within a step");
    }
    assert!(
        (3_990..=4_010).contains(&course.length_m()),
        "{}",
        course.length_m()
    );
}

#[test]
fn the_last_point_ends_the_course_even_close_to_the_one_before() {
    let course = built([at(0.0, 0.0), at(0.0, 100.0), at(0.0, 102.0)]);
    assert_eq!(
        course.points(),
        &[at(0.0, 0.0), at(0.0, 100.0), at(0.0, 102.0)]
    );
}

#[test]
fn names_keep_capitals_digits_and_a_few_marks_and_are_cut() {
    let mut course = empty();
    course.set_name(b"  Cerro San Crist\xc3\xb3bal <loop> #2, v1.0 extra long tail");
    assert_eq!(course.name(), "CERRO SAN CRISTBAL LOOP");
    course.set_name(b"r\xc3\xa9seau  ");
    assert_eq!(course.name(), "RSEAU");
}

#[test]
fn a_course_file_round_trips() {
    let mut course = built(square(500.0, 10.0));
    course.set_name(b"Bend loop");
    let bytes = file(&course);
    assert_eq!(&bytes[..4], b"VCRS");
    assert_eq!(
        bytes.len(),
        HEADER_V1 + course.points().len() * POINT_V1 + 4
    );
    let back = decoded(&bytes).unwrap();
    assert!(same(&course, &back));
    assert_eq!(back.name(), "BEND LOOP");
}

/// A later version may lengthen the header and each point; this reader
/// takes the fields it knows.
#[test]
fn a_later_version_with_longer_headers_and_points_still_reads() {
    let mut course = built(square(500.0, 25.0));
    course.set_name(b"LATER");
    let v1 = file(&course);
    let n = course.points().len();
    let mut v2 = Vec::new();
    v2.extend_from_slice(&v1[..HEADER_V1]);
    v2[4] = 2;
    v2[5] = (HEADER_V1 + 6) as u8;
    v2[6] = (POINT_V1 + 4) as u8;
    v2.extend_from_slice(&[9; 6]);
    for i in 0..n {
        let at = HEADER_V1 + i * POINT_V1;
        v2.extend_from_slice(&v1[at..at + POINT_V1]);
        v2.extend_from_slice(&[0xee; 4]);
    }
    let crc = firmware_services::checksum::crc32(&v2);
    v2.extend_from_slice(&crc.to_le_bytes());
    assert!(same(&course, &decoded(&v2).unwrap()));
}

#[test]
fn files_that_are_not_courses_read_as_damaged() {
    let course = built(square(300.0, 10.0));
    let good = file(&course);
    let mut too_many = good.clone();
    put_u16(&mut too_many, 8, (POINTS + 1) as u16);
    let mut off_earth = good.clone();
    put_u32(&mut off_earth, HEADER_V1, 900_000_001);
    let body = off_earth.len() - 4;
    let crc = firmware_services::checksum::crc32(&off_earth[..body]);
    off_earth[body..].copy_from_slice(&crc.to_le_bytes());
    let mut far_south = good.clone();
    put_u32(&mut far_south, HEADER_V1, i32::MIN as u32);
    let mut foreign = good.clone();
    foreign[0] = b'X';
    let mut old = good.clone();
    old[4] = 0;
    for bytes in [
        &good[..10],
        &too_many,
        &off_earth,
        &far_south,
        &foreign,
        &old,
        &good[..good.len() - 1],
    ] {
        assert_eq!(decoded(bytes).err(), Some(DecodeError::Damaged));
    }
}

/// Damaged files, mostly re-sealed with a fresh CRC so the fields behind it
/// are tried, never panic, and whatever loads is a course within bounds that
/// writes back the same.
#[test]
fn damaged_course_files_never_panic_and_only_courses_load() {
    let (mut loaded, mut refused) = (0, 0);
    for seed in 0..3_000 {
        let mut rng = Rng::new(seed);
        let n = rng.range(2, 60) as usize;
        let points = (0..n).map(|i| at(i as f64 * 37.0, (i % 7) as f64 * 11.0));
        let mut course = built(points);
        course.set_name(&rng.bytes(30));
        let mut bytes = rng.mutate(file(&course));
        if bytes.len() >= 4 && !rng.one_in(4) {
            let body = bytes.len() - 4;
            let crc = firmware_services::checksum::crc32(&bytes[..body]);
            bytes[body..].copy_from_slice(&crc.to_le_bytes());
        }
        let Ok(back) = decoded(&bytes) else {
            refused += 1;
            continue;
        };
        loaded += 1;
        assert!((2..=POINTS).contains(&back.points().len()), "seed {seed}");
        assert!(back.name().len() <= NAME, "seed {seed}");
        assert!(
            back.points()
                .iter()
                .all(|p| Point::new(p.lat_e7, p.lon_e7).is_some())
        );
        assert!(same(&back, &decoded(&file(&back)).unwrap()), "seed {seed}");
    }
    assert!(
        loaded > 300 && refused > 1_000,
        "{loaded} loaded, {refused} refused"
    );
}

#[test]
fn the_course_is_kept_on_files_beside_what_is_there() {
    let mut region = formatted();
    put(&mut region, "bests.dat", b"not touched");
    put(&mut region, "workouts/a.vw", b"nor this");
    let mut course = built(square(800.0, 3.0));
    course.set_name(b"Morning loop");
    save(&mut region, &course).unwrap();
    let mut back = empty();
    load(&mut region, &mut back).unwrap();
    assert!(same(&course, &back));
    // Replaced in one commit, and the other files are as they were.
    let other = built(square(200.0, 10.0));
    save(&mut region, &other).unwrap();
    load(&mut region, &mut back).unwrap();
    assert!(same(&other, &back));
    assert_eq!(read(&mut region, "bests.dat"), b"not touched");
    assert_eq!(read(&mut region, "workouts/a.vw"), b"nor this");
    delete(&mut region).unwrap();
    load(&mut region, &mut back).unwrap();
    assert!(back.is_empty());
    delete(&mut region).unwrap();
    assert_eq!(read(&mut region, "bests.dat"), b"not touched");
}

#[test]
fn a_damaged_file_or_a_missing_store_says_so() {
    let mut region = formatted();
    let course = built(square(300.0, 10.0));
    let mut bytes = file(&course);
    bytes[HEADER_V1 + 3] ^= 0x40;
    put(&mut region, "course.dat", &bytes);
    let mut back = built(square(100.0, 10.0));
    assert_eq!(load(&mut region, &mut back), Err(Error::Damaged));
    assert!(back.is_empty(), "a damaged file loads nothing");
    let mut blank = Region::default();
    assert_eq!(load(&mut blank, &mut back), Err(Error::Unavailable));
    assert_eq!(save(&mut blank, &course), Err(Error::Unavailable));
}

fn read(region: &mut Region, name: &str) -> Vec<u8> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    let name = files::Name::new(name).unwrap();
    let entry = dir.find(&mut store, &name, &mut sector).unwrap().unwrap();
    let mut bytes = std::vec![0; entry.length as usize];
    dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
        .unwrap();
    bytes
}

/// A winding road of `km` with a fix every `step` metres: the heading turns
/// by a curvature that wanders, with a hairpin now and then, as roads in the
/// hills do.
pub(crate) fn road(km: f64, step: f64, seed: u64) -> Vec<Point> {
    let mut rng = Rng::new(seed);
    let (mut east, mut north, mut heading, mut curve) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut points = std::vec![at(0.0, 0.0)];
    for _ in 0..(km * 1000.0 / step) as usize {
        // Curvature in radians a metre: straights, bends and the odd hairpin.
        curve += (rng.below(2001) as f64 - 1000.0) * 2e-7 * step;
        curve = curve.clamp(-0.02, 0.02) * 0.995;
        if rng.one_in((3000.0 / step) as u64) {
            curve = if rng.one_in(2) { 0.03 } else { -0.03 };
        }
        heading += curve * step;
        east += heading.sin() * step;
        north += heading.cos() * step;
        points.push(at(east, north));
    }
    points
}

/// How far the road strays from the stored course: the worst distance, in
/// metres, of any fix from the stretch of course that replaced it, and the
/// share of fixes more than 50 m off. The course keeps a subsequence of the
/// fixes, so each fix lies between two kept ones.
fn deviation(road: &[Point], course: &Course) -> (f64, f64) {
    let kept = course.points();
    let scale = f64::from(east_per_e7(road[0]));
    let xy = |p: Point| {
        let (east, north) = metres(road[0], p, scale as f32);
        (f64::from(east), f64::from(north))
    };
    let (mut worst, mut far, mut k) = (0.0f64, 0usize, 0usize);
    for &fix in road {
        if k + 1 < kept.len() && fix == kept[k + 1] {
            k += 1;
        }
        let (a, b) = (xy(kept[k]), xy(kept[(k + 1).min(kept.len() - 1)]));
        let p = xy(fix);
        let off = f64::from(off_segment(
            (p.0 as f32, p.1 as f32),
            (a.0 as f32, a.1 as f32),
            (b.0 as f32, b.1 as f32),
        ));
        worst = worst.max(off);
        far += usize::from(off > 50.0);
    }
    (worst, far as f64 / road.len() as f64)
}

/// Long winding roads and a staircase fit in `POINTS` and stay within a few
/// metres of the road everywhere.
#[test]
fn a_long_course_stays_on_the_road() {
    // 160 stairs of 2.5 km, east then north, a fix every 5 m.
    let (mut east, mut north) = (0.0, 0.0);
    let mut staircase = std::vec![at(0.0, 0.0)];
    for i in 0..80_000u32 {
        if (i / 500) % 2 == 0 {
            east += 5.0;
        } else {
            north += 5.0;
        }
        staircase.push(at(east, north));
    }
    let cases = [
        ("20 km @ 5 m", road(20.0, 5.0, 1), 10.0),
        ("60 km @ 5 m", road(60.0, 5.0, 2), 15.0),
        ("150 km @ 10 m", road(150.0, 10.0, 3), 30.0),
        ("200 km @ 5 m", road(200.0, 5.0, 4), 40.0),
        ("400 km staircase", staircase, 20.0),
    ];
    for (name, road, limit) in cases {
        let course = built(road.iter().copied());
        let (worst, far) = deviation(&road, &course);
        std::eprintln!(
            "| {name} | {} | {worst:.0} m | {:.0}% |",
            course.points().len(),
            far * 100.0
        );
        assert!(course.points().len() <= POINTS, "{name}");
        assert_eq!(course.points().first(), road.first(), "{name}");
        assert_eq!(course.points().last(), road.last(), "{name}");
        assert!(worst <= limit, "{name}: {worst:.0} m off the road");
    }
}
