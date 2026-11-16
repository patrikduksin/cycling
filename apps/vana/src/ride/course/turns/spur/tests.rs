use super::super::tests::{Said, hairpin, join, line_of, path, ride_with, ring, turns_of};
use super::super::{Kind, Side, distance};
use crate::ride::course::stored::tests::uploaded;
use std::vec::Vec;

const LEFT: Kind = Kind::Turn(Side::Left);
const RIGHT: Kind = Kind::Turn(Side::Right);
const SHARP_LEFT: Kind = Kind::Sharp(Side::Left);

fn length(line: &[(f32, f32)]) -> f32 {
    line.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

/// The turns a rider riding `drawn` start to end is told to take as they
/// reach them: by the scan of the course as kept, and from the turns found
/// while it uploaded. The course keeps its length, less what thinning cuts
/// from its corners.
fn told(drawn: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    let (course, _) = uploaded(drawn);
    told_kept(drawn, &line_of(&course))
}

/// The same, where the device scans `kept`.
fn told_kept(drawn: &[(f32, f32)], kept: &[(f32, f32)]) -> [Vec<Kind>; 2] {
    let (course, stored) = uploaded(drawn);
    let (kept_m, drawn_m) = (length(&line_of(&course)), length(drawn));
    assert!(
        (kept_m - drawn_m).abs() < drawn_m * 0.02,
        "{kept_m} m kept of {drawn_m} m"
    );
    let positions = || (0..=(kept_m / 5.0) as usize).map(|k| k as f32 * 5.0);
    let now = |said: Vec<(u32, Said)>| {
        said.into_iter()
            .filter_map(|(_, said)| match said {
                Said::Now(kind) => Some(kind),
                Said::Soon(_) => None,
            })
            .collect()
    };
    [
        now(ride_with(&turns_of(kept), positions())),
        now(ride_with(&stored, positions())),
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

/// The points of the first ride's course about a spur, as it was kept.
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

