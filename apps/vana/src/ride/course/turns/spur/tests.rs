use super::super::tests::{Said, hairpin, join, line_of, path, ride_with, ring, turns_of};
use super::super::{Kind, PlacedScan, SCAN_STEPS, Side, TURNS, Turn, distance};
use crate::ride::course::stored::tests::{found, found_along, uploaded};
use core::cell::Cell;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

std::thread_local! {
    pub(super) static UNCUT: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn uncut<R>(f: impl FnOnce() -> R) -> R {
    UNCUT.set(true);
    let said = f();
    UNCUT.set(false);
    said
}

const LEFT: Kind = Kind::Turn(Side::Left);
const RIGHT: Kind = Kind::Turn(Side::Right);

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
    for depth in [15.0, 30.0, 60.0, 92.0, 100.0, 140.0] {
        for apart in [0.0, 0.5] {
            let told = told(&side_visit(depth, apart));
            assert_eq!(told, [[], []], "{depth} m deep, {apart} m apart");
        }
    }
}

#[test]
fn a_dead_end_too_shallow_or_drifting_to_be_sure_of_says_what_it_said_uncut() {
    for (depth, apart) in [(10.0, 0.0), (12.0, 0.0), (60.0, 1.5), (60.0, 3.0)] {
        let drawn = side_visit(depth, apart);
        let said = told(&drawn);
        assert_eq!(
            said,
            uncut(|| told(&drawn)),
            "{depth} m deep, {apart} m apart"
        );
        assert_ne!(said, [[], []], "{depth} m deep, {apart} m apart");
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
    for overshoot in [30.0, 60.0, 90.0] {
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

fn without_or_uncut(with: &[(f32, f32)], without: &[(f32, f32)], base_m: f32, spur_m: f32) -> bool {
    let scan = turns_of(with);
    let upload = found(with).1;
    let scan_right = as_without(&scan, &turns_of(without), base_m, spur_m)
        || heeded(&scan) == heeded(&uncut(|| turns_of(with)));
    let upload_right = as_without(&upload, &found(without).1, base_m, spur_m)
        || heeded(&upload) == heeded(&uncut(|| found(with).1));
    scan_right && upload_right
}

/// The turns told on `corners`, scanned as given rather than as kept.
fn kept_about(corners: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    told_kept(corners, corners)
}

#[test]
fn a_spur_whose_way_back_overshoots_its_base_says_what_it_said_uncut() {
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
    assert_eq!(kept_about(&corners), uncut(|| kept_about(&corners)));
}

#[test]
fn a_spur_at_the_end_of_a_long_out_and_back_says_what_it_said_with_or_without_it() {
    // km 51.46: a 420 m out-and-back whose far end turns left into a 17 m
    // spur; its way back strays 3.7 m from the way out 25 m on.
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
    let without: Vec<_> = out[..7]
        .iter()
        .chain(&back[1..])
        .chain(out[..4].iter().rev())
        .copied()
        .collect();
    let told = kept_about(&corners);
    assert!(
        told == kept_about(&without) || told == uncut(|| kept_about(&corners)),
        "{told:?}"
    );
}

#[test]
fn a_fork_15_degrees_off_the_way_in_says_what_it_said_uncut() {
    for depth in [60.0f32, 151.0, 160.0] {
        let out = 165f32.to_radians();
        let tip = (depth * libm::sinf(out), 300.0 + depth * libm::cosf(out));
        let back = 255f32.to_radians();
        let back = (2.9 * libm::sinf(back), 300.0 + 2.9 * libm::cosf(back));
        let drawn = path(&[(0.0, 0.0), (0.0, 300.0), tip, back, (400.0, 300.0)], 5.0);
        assert_eq!(told(&drawn), uncut(|| told(&drawn)), "{depth} m deep");
    }
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
fn a_dogleg_just_past_a_spur_holds_back_the_next_sign_as_without_it_or_uncut() {
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
    assert!(without_or_uncut(&with, &without, base_m, spur_m));
    let said = |line: &[(f32, f32)]| {
        heard_kept(line, line)
            .map(|heard| heard.into_iter().map(|(_, said)| said).collect::<Vec<_>>())
    };
    let said_with = said(&with);
    assert!(said_with == said(&without) || said_with == uncut(|| said(&with)));
}

#[test]
fn a_spur_whose_way_on_hooks_back_beside_the_way_in_says_what_it_says_without_it_or_uncut() {
    let way_in = [(0.0, 0.0), (0.0, 300.0)];
    let spur = [(-80.0, 320.0), (0.0, 300.0)];
    let on = [
        (3.0, 292.0),
        (6.0, 299.0),
        (10.0, 307.0),
        (20.0, 304.0),
        (26.0, 304.0),
        (26.0, 298.0),
        (55.0, 290.0),
        (400.0, 290.0),
    ];
    let without: Vec<_> = way_in.iter().chain(&on).copied().collect();
    let with: Vec<_> = way_in.iter().chain(&spur).chain(&on).copied().collect();
    let (base_m, spur_m) = (length(&way_in), length(&with) - length(&without));
    assert!(without_or_uncut(&with, &without, base_m, spur_m));
}

#[test]
fn a_spur_drawn_at_its_corners_whose_way_back_turns_off_along_its_way_out_keeps_the_u_turn() {
    let corners = [
        (0.0, 0.0),
        (0.0, 300.0),
        (60.0, 300.0),
        (8.0, 300.0),
        (8.0, 0.0),
        (300.0, 0.0),
    ];
    let without = [
        (0.0, 0.0),
        (0.0, 300.0),
        (8.0, 300.0),
        (8.0, 0.0),
        (300.0, 0.0),
    ];
    assert_eq!(kept_about(&without), [[Kind::UTurn, LEFT]; 2]);
    assert_eq!(kept_about(&corners), kept_about(&without));
}

#[test]
fn a_spur_drawn_at_its_corners_where_the_road_runs_on_says_nothing() {
    for (tip, on) in [
        ((60.0, 300.0), (0.0, 700.0)),
        ((-60.0, 300.0), (300.0, 300.0)),
    ] {
        let corners = [(0.0, 0.0), (0.0, 300.0), tip, (0.0, 300.0), on];
        let without = [(0.0, 0.0), (0.0, 300.0), on];
        assert_eq!(kept_about(&corners), kept_about(&without), "{tip:?}");
    }
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

fn upto(line: &[(f32, f32)], m: f32) -> Vec<(f32, f32)> {
    let mut kept = vec![line[0]];
    let mut kept_m = 0.0;
    for pair in line.windows(2) {
        let along = distance(pair[0], pair[1]);
        if kept_m + along >= m {
            let t = (m - kept_m) / along;
            let end = (
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            );
            kept.extend((distance(pair[0], end) > 0.0).then_some(end));
            return kept;
        }
        kept_m += along;
        kept.push(pair[1]);
    }
    kept
}

struct Drawn {
    with: Vec<(f32, f32)>,
    without: Vec<(f32, f32)>,
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

/// A course with a spur `depth` deep at its base, whose way back comes back
/// `drift` to the side of it, on from the way in, and the same course
/// without the spur.
struct Shape {
    name: String,
    way_in: Vec<(f32, f32)>,
    base: (f32, f32),
    depth: f32,
    tip: (f32, f32),
    drift: f32,
    on: Vec<(f32, f32)>,
}

impl Shape {
    fn base(&self) -> (f32, f32) {
        self.base
    }

    fn comes_from(&self) -> (f32, f32) {
        let n = self.way_in.len();
        match self.way_in[n - 1] == self.base {
            true => self.way_in[n - 2],
            false => self.way_in[n - 1],
        }
    }

    fn returns_to(&self) -> (f32, f32) {
        let (came, base, tip) = (self.comes_from(), self.base(), self.tip);
        let side = ((base.1 - tip.1) / self.depth, (tip.0 - base.0) / self.depth);
        let onward = side.0 * (base.0 - came.0) + side.1 * (base.1 - came.1);
        let drift = match onward < -0.01 {
            true => -self.drift,
            false => self.drift,
        };
        (base.0 + side.0 * drift, base.1 + side.1 * drift)
    }

    fn parted(&self) -> bool {
        let bearing = |from: (f32, f32), to: (f32, f32)| (to.0 - from.0).atan2(to.1 - from.1);
        let ways = [
            bearing(self.base(), self.comes_from()),
            bearing(self.base(), self.tip),
            bearing(self.returns_to(), self.on[0]),
        ];
        let apart = |a: f32, b: f32| libm::cosf(a - b) <= libm::cosf(CUT_DEG.to_radians());
        apart(ways[0], ways[1]) && apart(ways[1], ways[2]) && apart(ways[2], ways[0])
    }

    fn cut(&self) -> bool {
        CUT_M.contains(&self.depth) && self.drift <= CUT_DRIFT_M && self.parted()
    }

    fn corners(&self) -> Vec<(f32, f32)> {
        let spur = [self.tip, self.returns_to()];
        self.way_in
            .iter()
            .chain(&spur)
            .chain(&self.on)
            .copied()
            .collect()
    }

    fn drawn(&self, step: f32) -> Drawn {
        let out: Vec<_> = self.way_in.iter().chain([&self.tip]).copied().collect();
        let base_m = length(&self.way_in) + distance(self.way_in[self.way_in.len() - 1], self.base);
        let on: Vec<_> = [self.returns_to()]
            .iter()
            .chain(&self.on)
            .copied()
            .collect();
        let without = join(&[upto(&path(&out, step), base_m), path(&on, step)]);
        Drawn {
            with: path(&self.corners(), step),
            without,
        }
    }

    /// The spur leaves the course or runs beside it other than at its
    /// base: what is drawn is not a dead end.
    fn tangled(&self) -> bool {
        let base = self.base();
        let way = |p: (f32, f32)| {
            let (east, north) = (p.0 - base.0, p.1 - base.1);
            east.atan2(north)
        };
        let ways = [way(self.tip), way(self.comes_from()), way(self.on[0])];
        let same = |a: f32, b: f32| libm::cosf(a - b) > 0.99;
        let overlaps = same(ways[0], ways[1]) || same(ways[0], ways[2]) || same(ways[1], ways[2]);
        let mut course: Vec<_> = self
            .way_in
            .iter()
            .chain([&base])
            .chain(&self.on)
            .copied()
            .collect();
        course.dedup();
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
        let segments: Vec<_> = course.windows(2).collect();
        let crossed = segments.iter().enumerate().any(|(i, a)| {
            segments[i + 1..]
                .iter()
                .skip(1)
                .any(|b| path(b, 1.0).into_iter().any(|p| nearest(a, p).1 < 6.0))
        });
        overlaps || near || crossed
    }
}

const DEPTHS: [f32; 8] = [10.0, 12.0, 20.0, 35.0, 60.0, 100.0, 145.0, 150.0];
/// Either side of `CLOSE_M`, as a way back that ends exactly that far off
/// leaves the way out wherever its points round it to.
const DRIFTS: [f32; 6] = [0.5, 0.9, 1.1, 1.5, 2.0, 3.0];
const CUT_M: core::ops::RangeInclusive<f32> = 20.0..=100.0;
const CUT_DRIFT_M: f32 = 0.5;
const CUT_DEG: f32 = 45.0;

const BASE: (f32, f32) = (0.0, 300.0);
const BASE_ALONG_M: [f32; 3] = [0.0, 8.0, 25.0];

fn shapes(drifts: &[f32]) -> Vec<Shape> {
    let mut ons: Vec<(String, Vec<(f32, f32)>)> = vec![
        ("straight on".to_string(), vec![(0.0, 700.0)]),
        ("left at the base".to_string(), vec![(-400.0, 300.0)]),
    ];
    for jog in [10.0, 15.0, 25.0, 40.0, 60.0] {
        let at = BASE.1 + jog;
        let dogleg = vec![(0.0, at), (15.0, at), (15.0, 700.0)];
        ons.push((format!("a {jog} m jog right"), dogleg));
        ons.push((format!("left {jog} m on"), vec![(0.0, at), (-400.0, at)]));
    }
    let ways_in = [
        ("straight in", vec![(0.0, 0.0), BASE]),
        (
            "a corner 25 m before",
            vec![(-300.0, 275.0), (0.0, 275.0), BASE],
        ),
        ("a corner at the base", vec![(-300.0, 300.0), BASE]),
    ];
    let mut shapes = Vec::new();
    for (in_name, way_in) in &ways_in {
        for (on_name, on) in &ons {
            shapes.extend(spurs_between(
                in_name,
                way_in,
                on_name,
                on,
                drifts,
                &BASE_ALONG_M,
            ));
        }
    }
    shapes
}

/// km 87.2: in at 18 degrees, then a hook back towards the way in, 3.6 m
/// off it at its first corner, a dogleg east and a left 150 m on, with
/// each spur at its base retraced exactly.
fn hooks() -> Vec<Shape> {
    let from_base = |corners: &[(f32, f32)]| -> Vec<(f32, f32)> {
        let at = |&(east, north): &(f32, f32)| (BASE.0 + east, BASE.1 + north);
        corners.iter().map(at).collect()
    };
    let way_in = from_base(&[
        (-90.0, -300.0),
        (0.0, -30.0),
        (4.0, -18.0),
        (8.0, -6.0),
        (0.0, 0.0),
    ]);
    let on = from_base(&[
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
    ]);
    let on_name = "a hook, a dogleg and a left 150 m on";
    spurs_between("in at 18 degrees", &way_in, on_name, &on, &[0.0], &[0.0])
}

/// A spur of each way and depth, and way back `drifts` off, between
/// `way_in` and `on`, where what is drawn is a dead end.
fn spurs_between(
    in_name: &str,
    way_in: &[(f32, f32)],
    on_name: &str,
    on: &[(f32, f32)],
    drifts: &[f32],
    leads: &[f32],
) -> Vec<Shape> {
    let ways = [
        ("right", 90.0f32),
        ("left", -90.0),
        ("a V back right", 145.0),
        ("a V back left", -145.0),
        ("a V ahead", 35.0),
    ];
    let mut shapes = Vec::new();
    for (way_name, degrees) in ways {
        let a = degrees.to_radians();
        let along = |p: (f32, f32), m: f32| (p.0 + m * libm::sinf(a), p.1 + m * libm::cosf(a));
        let hooks_back_beside_its_way_out = degrees.abs() != 90.0;
        let leads = match hooks_back_beside_its_way_out {
            true => &[0.0][..],
            false => leads,
        };
        for &lead in leads {
            let base = along(BASE, lead);
            let way_name = match lead > 0.0 {
                true => format!("{way_name} {lead} m along it"),
                false => way_name.to_string(),
            };
            for depth in DEPTHS {
                for &drift in drifts {
                    let shape = Shape {
                        name: format!(
                            "{in_name}, {depth} m {way_name} back {drift} m off, {on_name}"
                        ),
                        way_in: way_in.to_vec(),
                        base,
                        depth,
                        tip: along(base, depth),
                        drift,
                        on: on.iter().map(|&p| along(p, lead)).collect(),
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
    let mut scan = PlacedScan::default();
    let mut found = 0;
    while !scan.done() {
        found = scan.step(line.len(), |k| line[k], SCAN_STEPS, &mut turns, found);
    }
    turns.truncate(found);
    (turns, scan.spurs_m())
}

/// Drawn a point every this many metres, or at the corners alone.
const EVERY_M: [f32; 6] = [1.0, 3.0, 5.0, 8.0, 15.0, 10_000.0];

fn unlike(shapes: Vec<Shape>, every_m: &[f32]) -> (Vec<String>, usize) {
    let (mut failed, mut checked) = (Vec::new(), 0);
    for shape in shapes {
        for &step in every_m {
            let name = format!("{}, every {step} m", shape.name);
            let Drawn { with, without } = shape.drawn(step);
            let base_m = nearest(&without, shape.returns_to()).0;
            let drawn_m = length(&with) - length(&without);
            let paths = [
                (
                    scanned(&with),
                    turns_of(&without),
                    uncut(|| scanned(&with).0),
                ),
                (
                    found_along(&with),
                    found_along(&without).0,
                    uncut(|| found_along(&with).0),
                ),
            ];
            for (path, ((said, cut_m), reference, uncut)) in ["scan", "upload"].iter().zip(paths) {
                checked += usize::from(cut_m > 0.0);
                let right = match cut_m > 0.0 {
                    true => as_without(&said, &reference, base_m, drawn_m),
                    false => heeded(&said) == heeded(&uncut) && !shape.cut(),
                };
                if !right {
                    failed.push(format!("{name}: {path}, {cut_m} m cut"));
                }
            }
        }
    }
    (failed, checked)
}

#[test]
fn a_marked_spur_says_what_the_course_without_it_says() {
    let (failed, checked) = unlike(shapes(&[0.0]), &EVERY_M);
    assert!(
        failed.is_empty(),
        "{} of {checked}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn a_spur_before_a_hook_back_towards_the_way_in_says_what_the_course_without_it_says() {
    let (failed, checked) = unlike(hooks(), &EVERY_M);
    assert!(
        failed.is_empty(),
        "{} of {checked}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn a_spur_whose_way_back_comes_back_beside_its_base_says_what_the_course_without_it_says() {
    let (failed, checked) = unlike(shapes(&DRIFTS), &EVERY_M);
    assert!(
        failed.is_empty(),
        "{} of {checked}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn a_spur_whose_way_back_drifts_off_its_way_out_leaves_the_corner_named_or_says_what_it_said_uncut()
{
    for drift in [0.0].into_iter().chain(DRIFTS) {
        let at = 300.0 + drift;
        let drawn = path(
            &[
                (0.0, 0.0),
                (0.0, 300.0),
                (60.0, 300.0),
                (0.0, at),
                (-300.0, at),
            ],
            5.0,
        );
        let said = told(&drawn);
        let sure = drift <= CUT_DRIFT_M;
        assert!(
            said == [[LEFT]; 2] || !sure && said == uncut(|| told(&drawn)),
            "{drift} m off: {said:?}"
        );
    }
}

fn out_and_back_with_a_stub(
    (leg, stub, side): (f32, f32, f32),
    (apart, stray): (f32, f32),
    every_m: f32,
) -> Vec<(f32, f32)> {
    let (corner, base) = ((377.0, 300.0), (377.0, 300.0 - leg));
    let tip = (base.0 + side * stub, base.1);
    let back = [
        tip,
        (base.0 + side * apart, base.1),
        (base.0 - side * stray, base.1 + leg.min(30.0)),
        corner,
        (0.0, 300.0),
        (0.0, 700.0),
    ];
    join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), corner, base, tip], every_m),
        path(&back, every_m),
    ])
}

#[test]
fn a_dead_end_at_the_far_end_of_a_long_out_and_back_says_what_it_said_uncut() {
    let mut failed = Vec::new();
    for leg in [25.0, 32.0, 43.0] {
        for stub in [5.0, 10.0, 17.0, 30.0] {
            for side in [1.0, -1.0] {
                for back in [(0.0, 0.0), (2.0, 0.0), (0.0, 3.7)] {
                    for every_m in [3.0, 5.0, 10_000.0] {
                        let drawn = out_and_back_with_a_stub((leg, stub, side), back, every_m);
                        let said = kept_about(&drawn);
                        if said != uncut(|| kept_about(&drawn)) {
                            failed.push(format!(
                                "{leg} m leg, {stub} m to the {side}, back {back:?}, every {every_m} m: {said:?}"
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn spurs_one_after_another_say_what_they_said_uncut() {
    let corners = [
        (0.0, 0.0),
        (0.0, 300.0),
        (60.0, 300.0),
        (0.0, 300.0),
        (-20.0, 300.0),
        (0.0, 300.0),
        (0.0, 700.0),
        (300.0, 700.0),
    ];
    let drawn = path(&corners, 5.0);
    assert_eq!(told(&drawn), uncut(|| told(&drawn)));
}

#[test]
fn a_spur_off_a_spur_says_what_they_said_uncut() {
    let corners = [
        (0.0, 0.0),
        (0.0, 300.0),
        (40.0, 300.0),
        (40.0, 330.0),
        (40.0, 300.0),
        (100.0, 300.0),
        (0.0, 300.0),
        (0.0, 700.0),
        (300.0, 700.0),
    ];
    let drawn = path(&corners, 5.0);
    assert_eq!(told(&drawn), uncut(|| told(&drawn)));
}

mod fuzz;

fn judged_at_random(seeds: core::ops::Range<u64>) {
    let judged = fuzz::judge(seeds);
    std::println!(
        "{} courses: {} spur stretches said as without them, {} as uncut, {} planned",
        judged.courses,
        judged.without,
        judged.uncut,
        judged.planned
    );
    assert!(
        judged.failed.is_empty(),
        "{}:\n{}",
        judged.failed.len(),
        judged.failed.join("\n")
    );
}

#[test]
fn spurs_drawn_at_random_say_what_they_say_without_them_or_uncut() {
    judged_at_random(0..2_000);
}

#[test]
#[ignore = "100,000 courses: cargo test -p vana --lib --release spurs_drawn -- --ignored --nocapture"]
fn spurs_drawn_at_random_say_what_they_say_without_them_or_uncut_at_length() {
    judged_at_random(0..100_000);
}
