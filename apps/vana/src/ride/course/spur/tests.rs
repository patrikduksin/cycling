use crate::ride::course::stored::tests::found;
use crate::ride::course::stored::{Finder, Finding};
use crate::ride::course::turns::tests::{build, hairpin, join, path, turns_of};
use crate::ride::course::turns::{Kind, Side, TURNS, Turn};
use crate::ride::course::{Point, east_per_e7, metres};
use std::vec::Vec;

fn road_with_spur(depth: f32) -> Vec<(f32, f32)> {
    join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), (depth, 300.0)], 5.0),
        path(&[(depth, 304.0), (0.0, 304.0), (0.0, 700.0)], 5.0),
    ])
}

fn framed(points: &[Point]) -> Vec<(f32, f32)> {
    let scale = east_per_e7(points[0]);
    points
        .iter()
        .map(|&p| metres(points[0], p, scale))
        .collect()
}

fn kinds(turns: &[Turn]) -> Vec<Kind> {
    turns.iter().filter_map(|turn| turn.kind).collect()
}

#[test]
fn a_short_spur_is_dropped_with_its_turns() {
    for depth in [30.0, 60.0, 92.0, 145.0] {
        let (course, turns, _) = found(&road_with_spur(depth));
        let line = framed(course.points());
        let widest = line.iter().map(|p| p.0.abs()).fold(0.0, f32::max);
        assert!(widest < 5.0, "{depth} m spur kept: {widest} m east");
        assert!(kinds(&turns).is_empty(), "{depth} m: {turns:?}");
        assert!(kinds(&turns_of(&line)).is_empty(), "{depth} m scanned");
    }
}

#[test]
fn a_longer_out_and_back_keeps_its_u_turn() {
    let (course, turns, _) = found(&road_with_spur(400.0));
    let line = framed(course.points());
    let widest = line.iter().map(|p| p.0).fold(0.0, f32::max);
    assert!(widest > 395.0, "{widest}");
    assert!(kinds(&turns).contains(&Kind::UTurn), "{turns:?}");
    assert!(kinds(&turns_of(&line)).contains(&Kind::UTurn));
}

#[test]
fn a_hairpin_keeps_its_points_and_its_u_turn() {
    for radius in [4.0, 8.0, 20.0] {
        let full = hairpin(radius, 5.0);
        let (course, turns, _) = found(&full);
        let line = framed(course.points());
        let top = line.iter().map(|p| p.1).fold(0.0, f32::max);
        assert!(top > 400.0, "{radius} m hairpin cut at {top} m");
        assert_eq!(kinds(&turns), [Kind::UTurn], "{radius} m");
    }
}

#[test]
fn a_spur_from_a_corner_leaves_the_corner_named() {
    let full = join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), (0.0, 360.0)], 5.0),
        path(&[(3.0, 360.0), (3.0, 304.0), (-373.0, 440.0)], 5.0),
    ]);
    let mut finder = std::boxed::Box::new(Finder::new());
    let mut list = std::vec![Turn::default(); TURNS];
    let mut finding = Finding {
        finder: &mut finder,
        turns: &mut list,
    };
    let (course, _) = build(&full, &mut finding);
    let found = finding.finish(&course).expect("turns found");
    let line = framed(course.points());
    assert!(line.iter().all(|p| p.1 < 310.0 || p.0 < -5.0), "{line:?}");
    assert_eq!(kinds(&list[..found.count]), [Kind::Turn(Side::Left)]);
    assert_eq!(kinds(&turns_of(&line)), [Kind::Turn(Side::Left)]);
}
