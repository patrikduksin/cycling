use super::*;
use crate::ride::course::stored::tests::found;
use crate::ride::course::turns::tests::join;

#[test]
fn riding_straight_past_a_short_spur_says_no_turn_and_stays_on_course() {
    let full = join(&[
        path(&[(0.0, 0.0), (0.0, 300.0), (60.0, 300.0)], 5.0),
        path(&[(60.0, 304.0), (0.0, 304.0), (0.0, 700.0)], 5.0),
    ]);
    let (course, stored, _) = found(&full);
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut turns = vec![Turn::default(); turns::TURNS];
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
    rider.set_course(course.points());
    let mut now = 0;
    let straight = path(&[(0.0, 0.0), (0.0, 700.0)], 7.0);
    let (warnings, events) = ride_course_stored(
        &mut rider,
        course.points(),
        Some(&stored),
        &straight,
        &mut now,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(events, [Event::Following, Event::Arrived]);
}
