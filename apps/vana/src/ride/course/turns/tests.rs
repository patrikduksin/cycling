use super::*;
use crate::ride::course::tests::{at as point_at, empty};
use crate::ride::course::{Builder, Course, east_per_e7, metres};
use std::vec;
use std::vec::Vec;

/// Points every `step` metres along straight legs between `corners`.
fn path(corners: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
    let mut points = vec![corners[0]];
    for pair in corners.windows(2) {
        let steps = (distance(pair[0], pair[1]) / step).ceil() as usize;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            points.push((
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            ));
        }
    }
    points
}

/// Points every `step` metres round a circle about `centre`, from `from`
/// degrees clockwise from north, through `sweep` degrees: clockwise when
/// positive.
fn arc(centre: (f32, f32), radius: f32, from: f32, sweep: f32, step: f32) -> Vec<(f32, f32)> {
    let steps = (fabsf(sweep).to_radians() * radius / step).ceil() as usize;
    (0..=steps)
        .map(|i| {
            let a = (from + sweep * i as f32 / steps as f32).to_radians();
            (
                centre.0 + radius * libm::sinf(a),
                centre.1 + radius * libm::cosf(a),
            )
        })
        .collect()
}

fn join(parts: &[Vec<(f32, f32)>]) -> Vec<(f32, f32)> {
    let mut line: Vec<(f32, f32)> = Vec::new();
    for part in parts {
        let skip = usize::from(line.last() == part.first());
        line.extend_from_slice(&part[skip..]);
    }
    line
}

/// Ride `line` start to end, a fix every `step` metres, and say where along
/// it each warning came.
fn ride(line: &[(f32, f32)], step: f32) -> Vec<(u32, Warning)> {
    let mut warner = Warner::default();
    let mut said = Vec::new();
    let (mut walked, mut ridden) = (0.0, 0.0);
    for next in 1..line.len() {
        let (a, b) = (line[next - 1], line[next]);
        let length = distance(a, b);
        while ridden <= walked + length {
            let t = if length > 0.0 {
                (ridden - walked) / length
            } else {
                0.0
            };
            let here = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
            let at = |k: usize| line[k];
            if let Some(warning) = warner.step(line.len(), at, Place { next, here }) {
                said.push((ridden as u32, warning));
            }
            ridden += step;
        }
        walked += length;
    }
    said
}

fn kinds(said: &[(u32, Warning)]) -> Vec<Warning> {
    said.iter().map(|&(_, warning)| warning).collect()
}

fn ahead(kind: Kind) -> Warning {
    Warning::Ahead { kind, metres: 150 }
}

const RIGHT: Kind = Kind::Turn(Side::Right);
const LEFT: Kind = Kind::Turn(Side::Left);

#[test]
fn a_corner_is_warned_of_150_m_before_and_again_at_it() {
    let line = path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 400.0)], 10.0);
    let said = ride(&line, 7.0);
    assert_eq!(
        kinds(&said),
        [ahead(RIGHT), Warning::Now(RIGHT)],
        "{said:?}"
    );
    assert!((245..=255).contains(&said[0].0), "{said:?}");
    assert!((370..=377).contains(&said[1].0), "{said:?}");
    let left = path(&[(0.0, 0.0), (0.0, 400.0), (-300.0, 400.0)], 10.0);
    assert_eq!(kinds(&ride(&left, 7.0)), [ahead(LEFT), Warning::Now(LEFT)]);
}

#[test]
fn sharp_turns_and_u_turns_say_so() {
    let sharp = path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 100.0)], 10.0);
    let kind = Kind::Sharp(Side::Right);
    assert_eq!(kinds(&ride(&sharp, 7.0)), [ahead(kind), Warning::Now(kind)]);
    let back = path(&[(0.0, 0.0), (0.0, 400.0), (0.0, 100.0)], 10.0);
    let kind = Kind::UTurn;
    assert_eq!(kinds(&ride(&back, 7.0)), [ahead(kind), Warning::Now(kind)]);
}

#[test]
fn a_gentle_bend_says_nothing() {
    // 45 degrees right over 160 m, between straights.
    let line = join(&[
        path(&[(0.0, 0.0), (0.0, 300.0)], 10.0),
        arc((200.0, 300.0), 200.0, 270.0, 45.0, 5.0),
        path(&[(58.6, 441.4), (270.0, 653.0)], 10.0),
    ]);
    assert!(ride(&line, 7.0).is_empty());
}

#[test]
fn a_hairpin_is_a_u_turn_whether_drawn_fine_or_as_one_point() {
    // Up, round a 10 m radius to the left, and back down 20 m to the west.
    let fine = join(&[
        path(&[(0.0, 0.0), (0.0, 400.0)], 5.0),
        arc((-10.0, 400.0), 10.0, 90.0, -180.0, 2.0),
        path(&[(-20.0, 400.0), (-20.0, 0.0)], 5.0),
    ]);
    let rough = path(&[(0.0, 0.0), (-10.0, 410.0), (-20.0, 0.0)], 50.0);
    for line in [fine, rough] {
        let said = ride(&line, 7.0);
        assert_eq!(
            kinds(&said),
            [ahead(Kind::UTurn), Warning::Now(Kind::UTurn)],
            "{said:?}"
        );
    }
}

/// A roundabout of 12 m radius about (0, 0), ridden anticlockwise from the
/// south, leaving after `sweep` degrees round it.
fn roundabout(sweep: f32) -> Vec<(f32, f32)> {
    let ring = arc((0.0, 0.0), 12.0, 180.0, -sweep, 3.0);
    let out = *ring.last().unwrap();
    let away = (out.0 / 12.0 * 300.0, out.1 / 12.0 * 300.0);
    join(&[
        path(&[(0.0, -300.0), (0.0, -12.0)], 10.0),
        ring,
        path(&[out, away], 10.0),
    ])
}

#[test]
fn a_roundabout_is_one_turn_the_way_it_leaves() {
    // Entering from the south, anticlockwise: the first exit is east, the
    // second north, the third west.
    let first = ride(&roundabout(90.0), 7.0);
    assert_eq!(
        kinds(&first),
        [ahead(RIGHT), Warning::Now(RIGHT)],
        "{first:?}"
    );
    let third = ride(&roundabout(270.0), 7.0);
    assert_eq!(
        kinds(&third),
        [ahead(LEFT), Warning::Now(LEFT)],
        "{third:?}"
    );
    let straight = ride(&roundabout(180.0), 7.0);
    assert!(straight.is_empty(), "{straight:?}");
}

#[test]
fn a_jog_left_and_right_is_no_turn_but_two_far_apart_are_two() {
    let jog = path(
        &[(0.0, 0.0), (0.0, 300.0), (-10.0, 300.0), (-10.0, 600.0)],
        5.0,
    );
    let said = ride(&jog, 7.0);
    assert!(said.is_empty(), "{said:?}");
    let block = path(
        &[(0.0, 0.0), (0.0, 300.0), (-200.0, 300.0), (-200.0, 600.0)],
        10.0,
    );
    assert_eq!(
        kinds(&ride(&block, 7.0)),
        [
            ahead(LEFT),
            Warning::Now(LEFT),
            ahead(RIGHT),
            Warning::Now(RIGHT)
        ]
    );
    // 50 m apart, the second is warned of only at it.
    let close = path(
        &[(0.0, 0.0), (0.0, 300.0), (-50.0, 300.0), (-50.0, 600.0)],
        10.0,
    );
    assert_eq!(
        kinds(&ride(&close, 7.0)),
        [ahead(LEFT), Warning::Now(LEFT), Warning::Now(RIGHT)]
    );
}

/// `course`'s points in metres east and north of its start.
fn line_of(course: &Course) -> Vec<(f32, f32)> {
    let points = course.points();
    let scale = east_per_e7(points[0]);
    points
        .iter()
        .map(|&p| metres(points[0], p, scale))
        .collect()
}

#[test]
fn a_long_course_thinned_to_fit_warns_of_its_hairpins_as_drawn() {
    // 200 km meandering 25 m either side every 500 m, then ten switchbacks
    // of 300 m, a point every 5 m: far more than a course keeps.
    let mut road = Vec::new();
    for k in 0..40_000 {
        let north = k as f32 * 5.0;
        let east = 25.0 * libm::sinf(north / 500.0 * core::f32::consts::TAU);
        road.push((east, north));
    }
    let top = road.last().unwrap().1;
    let mut switchbacks = vec![road.clone()];
    for k in 0..10 {
        let (east, side) = (-20.0 * k as f32, if k % 2 == 0 { 1.0 } else { -1.0 });
        let (from, to) = if k % 2 == 0 {
            (top, top + 300.0)
        } else {
            (top + 300.0, top)
        };
        switchbacks.push(path(&[(east, from), (east, to)], 5.0));
        switchbacks.push(arc((east - 10.0, to), 10.0, 90.0, -180.0 * side, 2.0));
    }
    switchbacks.push(path(&[(-200.0, top), (-200.0, top + 300.0)], 5.0));
    let full = join(&switchbacks);
    let mut course = empty();
    let mut builder = Builder::new();
    builder.start(&mut course);
    for &(east, north) in &full {
        builder.push(&mut course, point_at(f64::from(east), f64::from(north)));
    }
    builder.finish(&mut course);
    let thinned = line_of(&course);
    assert!(
        builder.tolerance >= 16.0,
        "thinned hard: {}",
        builder.tolerance
    );
    assert!(thinned.len() * 20 < full.len(), "{} points", thinned.len());

    let want = [ahead(Kind::UTurn), Warning::Now(Kind::UTurn)].repeat(10);
    let full_said = ride(&full, 7.0);
    assert_eq!(kinds(&full_said), want, "{full_said:?}");
    let thinned_said = ride(&thinned, 7.0);
    assert_eq!(kinds(&thinned_said), want, "{thinned_said:?}");
    // The thinned meander is shorter; from the first hairpin on they agree.
    let (fine_0, thin_0) = (full_said[0].0, thinned_said[0].0);
    for (fine, thin) in full_said.iter().zip(&thinned_said) {
        let (fine_m, thin_m) = (fine.0 - fine_0, thin.0 - thin_0);
        assert!(fine_m.abs_diff(thin_m) <= 30, "{fine:?} {thin:?}");
    }
}

#[test]
fn a_rider_past_the_end_or_on_a_tiny_course_hears_nothing() {
    let line = path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 400.0)], 10.0);
    let at = |k: usize| line[k];
    let mut warner = Warner::default();
    let off_end = Place {
        next: line.len(),
        here: (300.0, 400.0),
    };
    assert_eq!(warner.step(line.len(), at, off_end), None);
    let start = Place {
        next: 0,
        here: (0.0, 0.0),
    };
    assert_eq!(warner.step(line.len(), at, start), None);
    let two = [(0.0, 0.0), (0.0, 0.0)];
    let place = Place {
        next: 1,
        here: (0.0, 0.0),
    };
    assert_eq!(warner.step(2, |k| two[k], place), None);
    // Points kilometres apart, and on top of one another.
    let far = [(0.0, 0.0), (0.0, 4.0e6), (4.0e6, 4.0e6), (4.0e6, 4.0e6)];
    let place = Place {
        next: 1,
        here: (0.0, 3.99999e6),
    };
    assert_eq!(
        next(4, |k| far[k], place, 0, u32::MAX).map(|turn| turn.kind),
        Some(RIGHT)
    );
}
