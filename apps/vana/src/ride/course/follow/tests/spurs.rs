use super::*;
use crate::ride::course::stored::tests::uploaded;
use crate::ride::course::turns::tests::join;
use turns::{Kind, Side};

/// North 700 m with a 60 m dead end east at 300 m, and the course as
/// uploaded with its turns.
fn with_spur(drawn: &[(f32, f32)], ridden: &[(f32, f32)]) -> (Vec<Said>, Vec<Event>) {
    let (course, stored) = uploaded(drawn);
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut turns = vec![Turn::default(); turns::TURNS];
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
    rider.set_course(course.points());
    let mut now = 0;
    let ridden = path(ridden, 7.0);
    ride_course_stored(
        &mut rider,
        course.points(),
        Some(&stored),
        &ridden,
        &mut now,
    )
}

fn dead_end() -> Vec<(f32, f32)> {
    join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), (60.0, 300.0)], 5.0),
        path(&[(60.0, 302.0), (0.0, 302.0), (0.0, 700.0)], 5.0),
    ])
}

#[test]
fn riding_straight_past_a_short_spur_says_no_turn_and_stays_on_course() {
    let (warnings, events) = with_spur(&dead_end(), &[(0.0, 0.0), (0.0, 700.0)]);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(events, [Event::Following, Event::Arrived]);
}

#[test]
fn riding_into_a_short_spur_and_back_says_no_turn_and_stays_on_course() {
    let ridden = [
        (0.0, 0.0),
        (0.0, 300.0),
        (60.0, 300.0),
        (60.0, 302.0),
        (0.0, 302.0),
        (0.0, 700.0),
    ];
    let (warnings, events) = with_spur(&dead_end(), &ridden);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(events, [Event::Following, Event::Arrived]);
}

#[test]
fn turning_at_a_spur_past_a_corner_is_told_left_and_stays_on_course() {
    let drawn = path(
        &[(0.0, 0.0), (0.0, 330.0), (0.0, 300.0), (-300.0, 300.0)],
        5.0,
    );
    for ridden in [
        [(0.0, 0.0), (0.0, 300.0), (0.0, 300.0), (-300.0, 300.0)],
        [(0.0, 0.0), (0.0, 330.0), (0.0, 300.0), (-300.0, 300.0)],
    ] {
        let (warnings, events) = with_spur(&drawn, &ridden);
        let left = Kind::Turn(Side::Left);
        assert_eq!(warnings, [Said::Soon(left), Said::Now(left)], "{ridden:?}");
        assert_eq!(events, [Event::Following, Event::Arrived], "{ridden:?}");
    }
}
