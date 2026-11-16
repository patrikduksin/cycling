use super::super::tests::{Said, hairpin, join, line_of, path, ride_with, ring, turns_of};
use super::super::{Kind, SCAN_STEPS, Scan, Side, TURNS, Turn, distance};
use crate::ride::course::Course;
use crate::ride::course::stored::tests::{found, uploaded};
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

const LEFT: Kind = Kind::Turn(Side::Left);
const RIGHT: Kind = Kind::Turn(Side::Right);
const SHARP_LEFT: Kind = Kind::Sharp(Side::Left);

fn length(line: &[(f32, f32)]) -> f32 {
    line.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

/// The turns a rider riding `drawn` start to end is told to take as they
/// reach them: by the scan of the course as kept, and from the turns found
/// while it uploaded.
fn told(drawn: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    let (course, _) = uploaded(drawn);
    told_kept(drawn, &line_of(&course))
}

/// The same, where the device scans `kept`.
fn told_kept(drawn: &[(f32, f32)], kept: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    heard_kept(drawn, kept).map(|said| {
        said.into_iter()
            .filter_map(|(_, said)| match said {
                Said::Now(kind) => Some(kind),
                Said::Soon(_) => None,
            })
            .collect()
    })
}

/// `told_kept`, with every `Said` and the metre it is heard at.
fn heard_kept(drawn: &[(f32, f32)], kept: &[(f32, f32)]) -> [Vec<(u32, Said)>; 2] {
    let (course, stored) = uploaded(drawn);
    let (kept_m, drawn_m) = (length(&line_of(&course)), length(drawn));
    assert!(
        (kept_m - drawn_m).abs() < drawn_m * 0.02,
        "{kept_m} m kept of {drawn_m} m"
    );
    let positions = || (0..=(kept_m / 5.0) as usize).map(|k| k as f32 * 5.0);
    [
        ride_with(&turns_of(kept), positions()),
        ride_with(&stored, positions()),
    ]
}

/// North 300 m, `depth` m east and back `apart` m north of the way out,
/// and on north.
fn side_visit(depth: f32, apart: f32) -> Vec<(f32, f32)> {
    join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), (depth, 300.0)], 5.0),
        path(
            &[(depth, 300.0 + apart), (0.0, 300.0 + apart), (0.0, 700.0)],
            5.0,
        ),
    ])
}

#[test]
fn a_dead_end_visit_of_up_to_150_m_says_nothing_where_the_road_runs_on() {
    for depth in [12.0, 30.0, 60.0, 92.0, 100.0, 145.0] {
        for apart in [0.0, 2.0] {
            let told = told(&side_visit(depth, apart));
            assert_eq!(told, [[], []], "{depth} m deep, {apart} m apart");
        }
    }
}

#[test]
fn a_longer_out_and_back_keeps_its_turns() {
    for apart in [0.0, 2.0] {
        let told = told(&side_visit(400.0, apart));
        assert_eq!(told, [[RIGHT, Kind::UTurn, RIGHT]; 2], "{apart} m apart");
    }
}

#[test]
fn roads_8_m_apart_are_a_switchback_whose_u_turns_stay() {
    let corners = [
        (0.0, 0.0),
        (0.0, 300.0),
        (0.0, 400.0),
        (8.0, 400.0),
        (8.0, 300.0),
        (16.0, 300.0),
        (16.0, 600.0),
    ];
    let told = told(&path(&corners, 5.0));
    assert_eq!(told, [[Kind::UTurn, Kind::UTurn]; 2]);
}

#[test]
fn a_spur_past_a_corner_leaves_the_corner_named() {
    for overshoot in [12.0, 30.0, 90.0] {
        let drawn = path(
            &[
                (0.0, 0.0),
                (0.0, 300.0 + overshoot),
                (0.0, 300.0),
                (-300.0, 300.0),
            ],
            5.0,
        );
        assert_eq!(told(&drawn), [[LEFT]; 2], "{overshoot} m past");
    }
}

/// The turns told on `corners`, scanned as given rather than as kept.
fn kept_about(corners: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    told_kept(corners, corners)
}

#[test]
fn a_spur_whose_way_back_overshoots_its_base_says_nothing_where_the_course_runs_on() {
    // km 9.78: in at 152 degrees, a 27 m spur, and back on a line 7
    // degrees off the way out, which passes the base 3.3 m off and meets
    // the course 11 m on.
    let corners = [
        (406.8, 293.8),
        (-76.7, 145.9),
        (0.0, 0.0),
        (-24.5, -12.3),
        (10.9, 0.4),
        (111.0, -279.8),
        (171.7, -462.3),
    ];
    assert_eq!(kept_about(&corners), [[LEFT]; 2]);
}

#[test]
fn a_spur_at_the_end_of_a_long_out_and_back_is_one_u_turn() {
    // km 51.46: a 420 m out-and-back whose far end turns left into a 17 m
    // spur. As kept, its way back leaves the way out by up to 3.6 m for 30
    // m after the spur, so the spur ends there. Uploaded again, thinning
    // takes that in, and the way back retraces it all: the spur is then
    // what is past the corner 17 m from the tip.
    let out = [
        (-145.5, 141.0),
        (-122.2, 165.5),
        (13.4, 198.2),
        (94.3, 183.9),
        (0.0, 0.0),
        (66.7, -21.6),
        (37.4, -53.3),
        (52.1, -61.0),
    ];
    let back = [(37.4, -53.3), (55.4, -28.3), (66.7, -21.6), (0.0, 0.0)];
    let corners: Vec<_> = out
        .iter()
        .chain(&back)
        .chain(out[..4].iter().rev())
        .copied()
        .collect();
    let u_turn_alone = [RIGHT, LEFT, RIGHT, Kind::UTurn, SHARP_LEFT, RIGHT, LEFT];
    assert_eq!(kept_about(&corners), [u_turn_alone; 2]);
}

#[test]
fn hairpins_and_roundabouts_keep_their_turns() {
    for radius in [4.0, 8.0] {
        assert_eq!(told(&hairpin(radius, 5.0)), [[Kind::UTurn]; 2], "r{radius}");
    }
    for radius in [10.0, 20.0] {
        assert_eq!(told(&ring(radius, 90.0, 5.0)), [[RIGHT]; 2], "r{radius}");
        assert_eq!(told(&ring(radius, 270.0, 5.0)), [[LEFT]; 2], "r{radius}");
    }
}

#[test]
fn a_dogleg_just_past_a_spur_holds_back_the_next_sign_as_it_would_without_it() {
    // km 87.2: in at 18 degrees, a hook into an 82 m spur, then a dogleg
    // east that the scan takes for one turn group, ending 86 m past where
    // the way back meets the course, and a left 150 m on.
    let way_in = [
        (-90.0, -300.0),
        (0.0, -30.0),
        (4.0, -18.0),
        (8.0, -6.0),
        (0.0, 0.0),
    ];
    let spur = [(-78.0, 25.0), (0.0, 0.0)];
    let on = [
        (10.0, -3.0),
        (12.0, 5.0),
        (16.0, 14.0),
        (27.0, 10.0),
        (33.0, 10.0),
        (33.0, 4.0),
        (60.0, -4.0),
        (80.0, -7.0),
        (81.0, -1.0),
        (115.0, 145.0),
        (-185.0, 145.0),
    ];
    let without: Vec<_> = way_in.iter().chain(&on).copied().collect();
    let with: Vec<_> = way_in.iter().chain(&spur).chain(&on).copied().collect();
    let base_m = length(&way_in);
    let spur_m = length(&with) - length(&without);
    assert!(as_without(
        &turns_of(&with),
        &turns_of(&without),
        base_m,
        spur_m
    ));
    assert!(as_without(
        &found(&with).1,
        &found(&without).1,
        base_m,
        spur_m
    ));
    let said = |line: &[(f32, f32)]| {
        heard_kept(line, line)
            .map(|heard| heard.into_iter().map(|(_, said)| said).collect::<Vec<_>>())
    };
    assert_eq!(said(&with), said(&without));
}

#[test]
fn a_jog_just_past_a_spur_keeps_both_its_turns() {
    for jog in [10.0, 15.0, 25.0] {
        let corners = [
            (0.0, 0.0),
            (0.0, 300.0),
            (60.0, 300.0),
            (0.0, 300.0),
            (0.0, 300.0 + jog),
            (15.0, 300.0 + jog),
            (15.0, 700.0),
        ];
        assert_eq!(told(&path(&corners, 5.0)), [[RIGHT, LEFT]; 2], "{jog} m");
    }
}

/// Metres along `line` to its point nearest `p`, and how far off it is.
fn nearest(line: &[(f32, f32)], p: (f32, f32)) -> (f32, f32) {
    let (mut off, mut off_m, mut from_m) = (f32::INFINITY, 0.0, 0.0);
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let along = distance(a, b);
        let t = match along > 0.0 {
            true => {
                let dot = (p.0 - a.0) * (b.0 - a.0) + (p.1 - a.1) * (b.1 - a.1);
                (dot / (along * along)).clamp(0.0, 1.0)
            }
            false => 0.0,
        };
        let foot = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        if distance(foot, p) < off {
            (off, off_m) = (distance(foot, p), from_m + t * along);
        }
        from_m += along;
    }
    (off_m, off)
}

/// A course with a spur `depth` deep at its base, and the same course
/// without it.
struct Shape {
    name: String,
    way_in: Vec<(f32, f32)>,
    depth: f32,
    tip: (f32, f32),
    on: Vec<(f32, f32)>,
}

impl Shape {
    fn base(&self) -> (f32, f32) {
        self.way_in[self.way_in.len() - 1]
    }

    fn corners(&self, spur: bool) -> Vec<(f32, f32)> {
        let spur = match spur {
            true => vec![self.tip, self.base()],
            false => vec![],
        };
        self.way_in
            .iter()
            .chain(&spur)
            .chain(&self.on)
            .copied()
            .collect()
    }

    /// The spur leaves the course or comes back onto it other than at its
    /// base: what is drawn is not a dead end.
    fn tangled(&self) -> bool {
        let base = self.base();
        let way = |p: (f32, f32)| {
            let (east, north) = (p.0 - base.0, p.1 - base.1);
            east.atan2(north)
        };
        let came = self.way_in[self.way_in.len() - 2];
        let ways = [way(self.tip), way(came), way(self.on[0])];
        let same = |a: f32, b: f32| libm::cosf(a - b) > 0.99;
        let overlaps = same(ways[0], ways[1]) || same(ways[0], ways[2]) || same(ways[1], ways[2]);
        let course: Vec<_> = self.way_in.iter().chain(&self.on).copied().collect();
        let away: Vec<_> = course
            .windows(2)
            .filter(|pair| pair[0] != base && pair[1] != base)
            .map(|pair| pair.to_vec())
            .collect();
        let near = (8..=self.depth as usize).any(|m| {
            let t = m as f32 / self.depth;
            let p = (
                base.0 + t * (self.tip.0 - base.0),
                base.1 + t * (self.tip.1 - base.1),
            );
            away.iter().any(|segment| nearest(segment, p).1 < 6.0)
        });
        overlaps || near
    }
}

const DEPTHS: [f32; 8] = [10.0, 12.0, 20.0, 35.0, 60.0, 100.0, 145.0, 150.0];
/// Spurs this deep are marked whatever their shape, as shallower and
/// deeper ones, at the edges of what is a spur, may not be.
const MARKED_M: core::ops::RangeInclusive<f32> = 20.0..=145.0;

fn shapes() -> Vec<Shape> {
    let base = (0.0, 300.0);
    let ways_in = [
        ("straight in", vec![(0.0, 0.0), base]),
        (
            "a corner 25 m before",
            vec![(-300.0, 275.0), (0.0, 275.0), base],
        ),
        ("a corner at the base", vec![(-300.0, 300.0), base]),
    ];
    let ways = [
        ("right", 90.0f32),
        ("left", -90.0),
        ("a V back right", 150.0),
        ("a V back left", -150.0),
        ("a V ahead", 30.0),
    ];
    let mut ons: Vec<(String, Vec<(f32, f32)>)> = vec![
        ("straight on".to_string(), vec![(0.0, 700.0)]),
        ("left at the base".to_string(), vec![(-400.0, 300.0)]),
    ];
    for jog in [10.0, 15.0, 25.0, 40.0, 60.0] {
        let at = base.1 + jog;
        let dogleg = vec![(0.0, at), (15.0, at), (15.0, 700.0)];
        ons.push((format!("a {jog} m jog right"), dogleg));
        ons.push((format!("left {jog} m on"), vec![(0.0, at), (-400.0, at)]));
    }
    let mut shapes = Vec::new();
    for (in_name, way_in) in &ways_in {
        for (way_name, degrees) in ways {
            for depth in DEPTHS {
                for (on_name, on) in &ons {
                    let a = degrees.to_radians();
                    let shape = Shape {
                        name: format!("{in_name}, {depth} m {way_name}, {on_name}"),
                        way_in: way_in.clone(),
                        depth,
                        tip: (
                            base.0 + depth * libm::sinf(a),
                            base.1 + depth * libm::cosf(a),
                        ),
                        on: on.clone(),
                    };
                    if !shape.tangled() {
                        shapes.push(shape);
                    }
                }
            }
        }
    }
    shapes
}

/// The turns a rider is told of or held back by.
fn heeded(turns: &[Turn]) -> Vec<Turn> {
    turns
        .iter()
        .filter(|turn| turn.kind.is_some() || turn.cornered)
        .copied()
        .collect()
}

/// Whether `marked` says what `reference` says, as the course without the
/// `spur_m` of it after `base_m` sees it, each where it is give or take
/// `NEAR_M`.
fn as_without(marked: &[Turn], reference: &[Turn], base_m: f32, spur_m: f32) -> bool {
    const NEAR_M: f32 = 6.0;
    let (marked, reference) = (heeded(marked), heeded(reference));
    let without = |mm: u32| {
        let m = mm as f32 / 1000.0;
        match m {
            m if m <= base_m => m,
            m if m >= base_m + spur_m => m - spur_m,
            _ => base_m,
        }
    };
    let near = |mm: u32, want_mm: u32| (without(mm) - want_mm as f32 / 1000.0).abs() <= NEAR_M;
    marked.len() == reference.len()
        && marked.iter().zip(&reference).all(|(turn, want)| {
            (turn.kind, turn.twice, turn.cornered) == (want.kind, want.twice, want.cornered)
                && near(turn.start_mm, want.start_mm)
                && near(turn.end_mm, want.end_mm)
        })
}

/// `line`'s turns as a placed course's are found, and metres of it read
/// past as spurs.
fn scanned(line: &[(f32, f32)]) -> (Vec<Turn>, f32) {
    let mut turns = vec![Turn::default(); TURNS];
    let mut scan = Scan::default();
    let mut found = 0;
    while !scan.done() {
        found = scan.step(line.len(), |k| line[k], SCAN_STEPS, &mut turns, found);
    }
    turns.truncate(found);
    (turns, scan.spurs_m())
}

#[test]
fn a_marked_spur_says_what_the_course_without_it_says() {
    let (mut failed, mut checked) = (Vec::new(), 0);
    for shape in shapes() {
        for step in [1.0, 3.0, 5.0, 8.0, 15.0, 10_000.0] {
            let name = format!("{}, every {step} m", shape.name);
            let with = path(&shape.corners(true), step);
            let without = path(&shape.corners(false), step);
            let (marked, spur_m) = scanned(&with);
            if spur_m == 0.0 {
                assert!(!MARKED_M.contains(&shape.depth), "unmarked: {name}");
                continue;
            }
            checked += 1;
            let base_m = nearest(&without, shape.base()).0;
            let drawn_m = length(&with) - length(&without);
            let scan = as_without(&marked, &turns_of(&without), base_m, drawn_m);
            let ((kept_with, with, _), (kept_without, without, _)) =
                (found(&with), found(&without));
            let kept_m = |course: &Course| length(&line_of(course));
            let kept_spur_m = kept_m(&kept_with) - kept_m(&kept_without);
            let upload = as_without(&with, &without, base_m, kept_spur_m);
            if !scan || !upload {
                failed.push(format!("{name}: scan {scan}, upload {upload}"));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {checked}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
