use super::*;
use crate::ride::course::POINTS;
use crate::ride::course::tests::{at, empty};
use crate::testing::Rng;
use std::boxed::Box;
use std::format;
use std::string::String;
use std::vec::Vec;

fn read(bytes: &[u8]) -> (Result<(), Refusal>, Box<Course>) {
    let mut course = empty();
    let mut gpx = Gpx::new();
    gpx.start(&mut course);
    gpx.feed(bytes, &mut course);
    let result = gpx.finish(&mut course);
    (result, course)
}

/// The same file in pieces of random sizes.
fn read_in_pieces(bytes: &[u8], rng: &mut Rng) -> (Result<(), Refusal>, Box<Course>) {
    let mut course = empty();
    let mut gpx = Gpx::new();
    gpx.start(&mut course);
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = (rng.range(1, 300) as usize).min(rest.len());
        gpx.feed(&rest[..n], &mut course);
        rest = &rest[n..];
    }
    let result = gpx.finish(&mut course);
    (result, course)
}

fn point(lat: &str, lon: &str) -> Point {
    let e7 = |text: &str| {
        let mut decimal = Decimal::new();
        text.bytes().for_each(|b| decimal.push(b));
        decimal.e7().unwrap()
    };
    Point::new(e7(lat), e7(lon)).unwrap()
}

const TRACK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- exported <trkpt lat="1" lon="1"/> by hand -->
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata><name>Sunday &amp; loop</name><time>2026-10-24T08:00:00Z</time></metadata>
  <wpt lat="-33.5" lon="-70.5"><name>Cafe</name></wpt>
  <trk>
    <name>Not this one</name>
    <trkseg>
      <trkpt lat="-33.4500000" lon="-70.6600000"><ele>520.4</ele><time>2026-10-24T08:00:00Z</time></trkpt>
      <trkpt lon='-70.6590000' lat='-33.4500000'/>
    </trkseg>
    <trkseg>
      <trkpt lat="-33.449" lon="-70.659"></trkpt>
    </trkseg>
  </trk>
  <trk><trkseg><trkpt lat="10" lon="10"/></trkseg></trk>
  <rte><rtept lat="11" lon="11"/></rte>
</gpx>
"#;

#[test]
fn the_first_track_joins_its_segments_and_names_the_course() {
    let (result, course) = read(TRACK.as_bytes());
    assert_eq!(result, Ok(()));
    assert_eq!(
        course.points(),
        &[
            point("-33.45", "-70.66"),
            point("-33.45", "-70.659"),
            point("-33.449", "-70.659"),
        ]
    );
    assert_eq!(course.name(), "SUNDAY LOOP");
}

#[test]
fn a_route_reads_like_a_track_with_prefixes_cdata_and_odd_spacing() {
    let gpx = r#"<gpx:gpx><gpx:rte><gpx:name><![CDATA[Ruta ]] 7]]></gpx:name>
        <gpx:rtept   lat = "45.1"
            lon="7.25" /><gpx:rtept lat="+45.2" lon=" 7.3 "/></gpx:rte></gpx:gpx>"#;
    let (result, course) = read(gpx.as_bytes());
    assert_eq!(result, Ok(()));
    assert_eq!(
        course.points(),
        &[point("45.1", "7.25"), point("45.2", "7.3")]
    );
    assert_eq!(course.name(), "RUTA 7");
}

#[test]
fn an_empty_name_or_a_broken_entity_keeps_the_real_name() {
    let gpx = r#"<gpx><metadata><name></name></metadata><trk><name>Fish &chips loop</name>
        <trkseg><trkpt lat="1" lon="1"/><trkpt lat="1.001" lon="1"/></trkseg></trk></gpx>"#;
    let (result, course) = read(gpx.as_bytes());
    assert_eq!(result, Ok(()));
    assert_eq!(course.name(), "FISH LOOP");
}

#[test]
fn decimals_read_exactly_to_a_ten_millionth() {
    let cases = [
        ("0", Some(0)),
        ("-0.0000001", Some(-1)),
        ("89.99999999999", Some(899_999_999)),
        ("-180", Some(-1_800_000_000)),
        ("007.5", Some(75_000_000)),
        (".5", Some(5_000_000)),
        ("5.", Some(50_000_000)),
        (" 1.25 ", Some(12_500_000)),
        ("1000", None),
        ("1e5", None),
        ("1.2.3", None),
        ("--1", None),
        ("1 2", None),
        ("", None),
        ("-", None),
        ("0x10", None),
    ];
    for (text, want) in cases {
        let mut decimal = Decimal::new();
        text.bytes().for_each(|b| decimal.push(b));
        assert_eq!(decimal.e7(), want, "{text:?}");
    }
}

#[test]
fn a_point_without_real_coordinates_refuses_the_file() {
    for bad in [
        r#"lat="91" lon="0""#,
        r#"lat="0" lon="-180.0000001""#,
        r#"lat="abc" lon="0""#,
        r#"lon="0""#,
        r#"lat="1e1" lon="0""#,
        r#"lat="-214.7483648" lon="0""#,
        r#"lat="0" lon="-214.7483648""#,
    ] {
        let gpx = format!(
            r#"<gpx><trk><trkseg><trkpt lat="1" lon="1"/><trkpt {bad}/><trkpt lat="2" lon="2"/></trkseg></trk></gpx>"#
        );
        let (result, course) = read(gpx.as_bytes());
        assert_eq!(result, Err(Refusal::Coordinates), "{bad}");
        assert!(course.is_empty());
    }
}

#[test]
fn waypoints_alone_or_one_point_are_no_course() {
    for gpx in [
        r#"<gpx><wpt lat="1" lon="1"/><wpt lat="2" lon="2"/></gpx>"#,
        r#"<gpx><trk><trkseg><trkpt lat="1" lon="1"/></trkseg></trk></gpx>"#,
        "",
        "not a gpx at all",
        "\u{feff}<html><body>hello</body></html>",
    ] {
        assert_eq!(read(gpx.as_bytes()).0, Err(Refusal::Empty), "{gpx}");
    }
}

fn long_track(points: usize) -> String {
    let mut gpx = String::from("<gpx><trk><name>Long</name><trkseg>\n");
    for i in 0..points {
        let p = at(((i / 40) % 2) as f64 * 200.0, i as f64 * 3.0);
        gpx.push_str(&format!(
            "<trkpt lat=\"{:.7}\" lon=\"{:.7}\"><ele>{}</ele></trkpt>\n",
            f64::from(p.lat_e7) / 1e7,
            f64::from(p.lon_e7) / 1e7,
            i % 900
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>");
    gpx
}

#[test]
fn a_long_course_streams_in_and_is_decimated_to_fit() {
    // 60,000 points, about 3.5 MB of GPX.
    let gpx = long_track(60_000);
    let (result, course) = read_in_pieces(gpx.as_bytes(), &mut Rng::new(1));
    assert_eq!(result, Ok(()));
    assert!(course.points().len() <= POINTS, "{}", course.points().len());
    assert!(course.points().len() > 100);
    assert_eq!(course.points()[0], at(0.0, 0.0));
    assert_eq!(course.name(), "LONG");
}

/// The reader's state is the same however the bytes arrive.
#[test]
fn any_split_of_a_file_reads_the_same() {
    let files = [TRACK.as_bytes().to_vec(), long_track(300).into_bytes()];
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let bytes = &files[seed as usize % files.len()];
        let (whole, course) = read(bytes);
        let (pieces, again) = read_in_pieces(bytes, &mut rng);
        assert_eq!(whole, pieces, "seed {seed}");
        assert_eq!(course.points(), again.points(), "seed {seed}");
        assert_eq!(course.name(), again.name(), "seed {seed}");
    }
}

/// Hostile files: random bytes and damaged real files never panic, whatever
/// loads is a course within bounds, and a refused file leaves none.
#[test]
fn hostile_gpx_never_panics_and_only_real_points_load() {
    let samples = [TRACK.as_bytes().to_vec(), long_track(40).into_bytes()];
    let (mut loaded, mut refused) = (0, 0);
    for seed in 0..4_000 {
        let mut rng = Rng::new(seed);
        let bytes: Vec<u8> = if rng.one_in(4) {
            rng.bytes(2_000)
        } else {
            let mut bytes = samples[rng.below(2) as usize].clone();
            for _ in 0..rng.range(1, 6) {
                bytes = rng.mutate(bytes);
            }
            bytes
        };
        let (result, course) = read_in_pieces(&bytes, &mut rng);
        match result {
            Ok(()) => {
                loaded += 1;
                assert!((2..=POINTS).contains(&course.points().len()), "seed {seed}");
                for p in course.points() {
                    assert!(Point::new(p.lat_e7, p.lon_e7).is_some(), "seed {seed}");
                }
                assert!(course.name().len() <= crate::ride::course::NAME);
            }
            Err(_) => {
                refused += 1;
                assert!(course.is_empty(), "seed {seed}");
            }
        }
    }
    assert!(
        loaded > 300 && refused > 1_000,
        "{loaded} loaded, {refused} refused"
    );
}

#[test]
fn the_first_track_of_two_points_wins_and_a_route_stands_in_without_one() {
    let p = |lat: &str| std::format!(r#"<trkpt lat="{lat}" lon="1"/>"#);
    let r = |lat: &str| std::format!(r#"<rtept lat="{lat}" lon="1"/>"#);
    let cases = [
        // An empty track, then a route: the route.
        (
            std::format!("<gpx><trk></trk><rte>{}{}</rte></gpx>", r("2"), r("2.01")),
            point("2", "1"),
        ),
        // A route, then a track: the track.
        (
            std::format!(
                "<gpx><rte>{}{}</rte><trk><trkseg>{}{}</trkseg></trk></gpx>",
                r("2"),
                r("2.01"),
                p("3"),
                p("3.01")
            ),
            point("3", "1"),
        ),
        // A track of one point, then one of two: the second.
        (
            std::format!(
                "<gpx><trk><trkseg>{}</trkseg></trk><trk><trkseg>{}{}</trkseg></trk></gpx>",
                p("4"),
                p("5"),
                p("5.01")
            ),
            point("5", "1"),
        ),
        // A route, then a track of one point: the route stays.
        (
            std::format!(
                "<gpx><rte>{}{}</rte><trk><trkseg>{}</trkseg></trk></gpx>",
                r("7"),
                r("7.01"),
                p("8")
            ),
            point("7", "1"),
        ),
        // A route, then an empty track: the route stays.
        (
            std::format!("<gpx><rte>{}{}</rte><trk></trk></gpx>", r("6"), r("6.01")),
            point("6", "1"),
        ),
    ];
    for (gpx, first) in cases {
        let (result, course) = read(gpx.as_bytes());
        assert_eq!(result, Ok(()), "{gpx}");
        assert_eq!(course.points()[0], first, "{gpx}");
        assert_eq!(course.points().len(), 2, "{gpx}");
    }
}
