use super::*;
use crate::ride::course::nav::tests::{Said, said};
use crate::ride::course::turns;
use crate::ride::homing::Homing;
use crate::ride::homing::tests::{at, path};
extern crate std;
use std::vec;
use std::vec::Vec;

mod many;
mod passes;

struct Rider<'a> {
    homing: Homing<'a>,
    follower: Follower<'a>,
}

impl<'a> Rider<'a> {
    fn new(crumbs: &'a mut [Crumb], line: &'a mut [Crumb], turns: &'a mut [Turn]) -> Self {
        Self {
            homing: Homing::new(crumbs),
            follower: Follower::new(line, turns),
        }
    }

    fn set_course(&mut self, course: &[course::Point]) {
        self.follower.set_course(course, self.homing.frame());
    }

    fn observe(&mut self, location_e7: Option<(i32, i32)>, now: u64) -> Option<Event> {
        let fix = self.homing.observe(location_e7, now);
        self.follower.observe(fix, now, None)
    }

    fn needs_course(&self) -> bool {
        self.follower.unplaced() && self.homing.frame().is_some()
    }

    fn reset(&mut self) {
        self.homing.reset();
        self.follower.reset();
    }
}

fn course_of(corners: &[(f32, f32)]) -> Vec<course::Point> {
    path(corners, 10.0)
        .into_iter()
        .map(|(east, north)| {
            let (lat, lon) = at(east, north);
            course::Point::new(lat, lon).unwrap()
        })
        .collect()
}

const SQUARE: [(f32, f32); 5] = [
    (0.0, 0.0),
    (1000.0, 0.0),
    (1000.0, 1000.0),
    (0.0, 1000.0),
    (0.0, 0.0),
];

fn ride_course_warned(
    rider: &mut Rider,
    course: &[course::Point],
    points: &[(f32, f32)],
    now: &mut u64,
) -> (Vec<Said>, Vec<Event>) {
    ride_course_stored(rider, course, None, points, now)
}

fn ride_course_stored(
    rider: &mut Rider,
    course: &[course::Point],
    stored: Option<&[Turn]>,
    points: &[(f32, f32)],
    now: &mut u64,
) -> (Vec<Said>, Vec<Event>) {
    let (mut warnings, mut events) = (Vec::new(), Vec::new());
    for &(east, north) in points {
        *now += 1_000;
        let event = rider.observe(Some(at(east, north)), *now);
        match event.and_then(|event| said(event, rider.follower.sign(*now))) {
            Some(told) => warnings.push(told),
            None => events.extend(event),
        }
        if rider.needs_course() {
            rider.set_course(course);
            if let Some(stored) = stored {
                rider.follower.take_turns(course, |turns| {
                    turns[..stored.len()].copy_from_slice(stored);
                    Some((stored.len(), false))
                });
            }
        }
    }
    (warnings, events)
}

fn ride_course(
    rider: &mut Rider,
    course: &[course::Point],
    points: &[(f32, f32)],
    now: &mut u64,
) -> Vec<Event> {
    ride_course_warned(rider, course, points, now).1
}

#[test]
fn following_a_course_warns_of_each_corner_ahead_and_at_it() {
    use turns::{Kind, Side};
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut turns = vec![Turn::default(); turns::TURNS];
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let lap = path(&SQUARE, 7.0);
    let (warnings, events) = ride_course_warned(&mut rider, &course, &lap, &mut now);
    assert_eq!(events, [Event::Following, Event::Arrived]);
    let left = Kind::Turn(Side::Left);
    let corner = [Said::Soon(left), Said::Now(left)];
    assert_eq!(warnings, corner.repeat(3), "{warnings:?}");
    let (warnings, _) = ride_course_warned(&mut rider, &course, &lap, &mut now);
    assert!(warnings.is_empty(), "{warnings:?}");
}

fn along_line(line: &[(f32, f32)], metres: f32) -> (f32, f32) {
    let mut walked = 0.0;
    for pair in line.windows(2) {
        let length = distance(pair[0], pair[1]);
        if walked + length >= metres && length > 0.0 {
            let t = (metres - walked) / length;
            let (a, b) = (pair[0], pair[1]);
            return (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        }
        walked += length;
    }
    *line.last().unwrap()
}

#[test]
fn stored_turns_move_into_the_frame_of_a_ride_started_far_away() {
    use crate::ride::course::stored::tests::uploaded;
    use crate::ride::course::turns::tests::bends;
    let corners: Vec<(f32, f32)> = (0..12)
        .map(|k| {
            (
                400.0 + 5_000.0 * k as f32,
                if k % 2 == 0 { 90.0 } else { -90.0 },
            )
        })
        .collect();
    let (course, stored) = uploaded(&bends(10.0, &corners));
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut turns = vec![Turn::default(); turns::TURNS];
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
    rider.observe(Some(at(0.0, 40_000.0)), 1_000);
    rider.set_course(course.points());
    assert_eq!(rider.follower.course().len(), course.points().len());
    while !rider.follower.scan.done() {
        rider.follower.scan_turns();
    }
    let named = |turns: &[Turn]| -> Vec<Turn> {
        turns
            .iter()
            .filter(|turn| turn.kind.is_some())
            .copied()
            .collect()
    };
    let scanned = named(&rider.follower.turns[..rider.follower.turn_count]);
    rider.follower.take_turns(course.points(), |list| {
        list[..stored.len()].copy_from_slice(&stored);
        Some((stored.len(), false))
    });
    assert!(rider.follower.scan.done());
    let placed = named(&rider.follower.turns[..rider.follower.turn_count]);
    assert_eq!(placed.len(), corners.len());
    assert_eq!(scanned.len(), corners.len());
    let apart = |a: &[Turn], b: &[Turn]| {
        a.iter()
            .zip(b)
            .map(|(a, b)| a.start_mm.abs_diff(b.start_mm))
            .max()
            .unwrap()
    };
    let (moved, as_stored) = (apart(&placed, &scanned), apart(&named(&stored), &scanned));
    std::println!("stored turns {moved} mm from the scan's once moved, {as_stored} mm as stored");
    assert!(moved <= 10_000, "{moved} mm");
    assert!(as_stored > 100_000, "{as_stored} mm");
    for (placed, stored) in placed.iter().zip(&named(&stored)) {
        assert_eq!((placed.kind, placed.twice), (stored.kind, stored.twice));
    }
}

#[test]
fn noisy_fixes_round_a_roundabout_never_give_a_wrong_word() {
    use crate::ride::course::stored::tests::uploaded;
    use crate::ride::course::turns::tests::{FOUND, Found, Verdict, built, extended, judge, ring};
    use crate::ride::course::turns::{TURNS, Turn};
    use crate::testing::Rng;
    for found in FOUND {
        let (mut runs, mut silent, mut wrong) = (0, 0, Vec::new());
        for radius in [15.0, 20.0, 30.0, 40.0] {
            for sweep in [90.0, 180.0, 270.0] {
                let road = ring(radius, sweep, 5.0);
                let degrees = turns::swing(0.0, 180.0 - sweep);
                for after in [0.0, 12_000.0] {
                    let (course, stored): (Vec<course::Point>, _) = match (found, after > 0.0) {
                        (Found::Uploaded, _) => {
                            let (course, turns) = uploaded(&extended(&road, after));
                            (course.points().to_vec(), Some(turns))
                        }
                        (Found::Scanned, true) => (built(&road, after).0.points().to_vec(), None),
                        (Found::Scanned, false) => (
                            road.iter()
                                .map(|&(east, north)| {
                                    course::tests::at(f64::from(east), f64::from(north))
                                })
                                .collect(),
                            None,
                        ),
                    };
                    for (noise, step) in [0.0f32, 3.0, 6.0]
                        .into_iter()
                        .flat_map(|noise| [5.0f32, 7.0, 9.0].map(|step| (noise, step)))
                    {
                        for seed in 0..39 {
                            let mut rng = Rng::new(seed + 1);
                            let mut jitter = || {
                                let span = (2.0 * noise * 1000.0) as u64;
                                rng.below(span + 1) as f32 / 1000.0 - noise
                            };
                            let fixes: Vec<(f32, f32)> = (0..(900.0 / step) as usize)
                                .map(|k| {
                                    let (east, north) = along_line(&road, k as f32 * step);
                                    (east + jitter(), north + jitter())
                                })
                                .collect();
                            let (mut crumbs, mut line) =
                                (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
                            let mut turns = vec![Turn::default(); TURNS];
                            let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
                            rider.set_course(&course);
                            let mut now = 0;
                            let (warnings, _) = ride_course_stored(
                                &mut rider,
                                &course,
                                stored.as_deref(),
                                &fixes,
                                &mut now,
                            );
                            let said: Vec<_> = warnings.iter().map(|&w| (0, w)).collect();
                            runs += 1;
                            match judge(&said, degrees) {
                            Verdict::Wrong => wrong.push(std::format!(
                                "{found:?}: r{radius} exit {sweep} after {after} noise {noise} step {step} seed {seed}: {warnings:?}"
                            )),
                            Verdict::Silent if sweep != 180.0 => silent += 1,
                            Verdict::Silent => {}
                            Verdict::Right => {}
                        }
                        }
                    }
                }
            }
        }
        std::println!(
            "{found:?}: {} of {runs} wrong, {silent} silent at a first or third exit",
            wrong.len()
        );
        assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(8)]);
    }
}

#[test]
fn riding_a_followed_course_the_wrong_way_or_stopped_warns_of_nothing() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut turns = vec![Turn::default(); turns::TURNS];
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut turns);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let out = path(&[(0.0, 0.0), (600.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &out, &mut now),
        [Event::Following]
    );
    let back = path(&[(600.0, 0.0), (200.0, 0.0)], 7.0);
    let (warnings, events) = ride_course_warned(&mut rider, &course, &back, &mut now);
    assert!(warnings.is_empty(), "{warnings:?} {events:?}");
    rider.reset();
    let out = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &out, &mut now),
        [Event::Following]
    );
    rider.follower.stop();
    let on = path(&[(500.0, 0.0), (1000.0, 0.0), (1000.0, 500.0)], 7.0);
    let (warnings, _) = ride_course_warned(&mut rider, &course, &on, &mut now);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_course_is_followed_along_it_off_course_once_to_its_end() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    assert!(
        rider.follower.unplaced() && rider.follower.course().is_empty(),
        "placed at the first fix"
    );
    let mut now = 0;
    let out = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &out, &mut now),
        [Event::Following]
    );
    let to_end = rider.follower.to_end_m().unwrap();
    assert!((3_480..=3_520).contains(&to_end), "{to_end}");
    let detour = path(
        &[(500.0, 0.0), (520.0, -150.0), (540.0, 0.0), (1000.0, 0.0)],
        2.0,
    );
    assert_eq!(
        ride_course(&mut rider, &course, &detour, &mut now),
        [Event::OffCourse, Event::StillOff, Event::OnCourse]
    );
    let rest = path(
        &[(1000.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0), (0.0, 0.0)],
        7.0,
    );
    assert_eq!(
        ride_course(&mut rider, &course, &rest, &mut now),
        [Event::Arrived]
    );
    assert!(!rider.follower.following());
    assert_eq!(rider.follower.to_end_m(), None);
    let lap = path(&[(0.0, 0.0), (600.0, 0.0)], 7.0);
    assert!(ride_course(&mut rider, &course, &lap, &mut now).is_empty());
    rider.reset();
    assert_eq!(
        ride_course(&mut rider, &course, &lap, &mut now),
        [Event::Following]
    );
}

#[test]
fn crossing_a_course_or_riding_it_backwards_follows_nothing() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let across = path(&[(500.0, -300.0), (500.0, 500.0)], 7.0);
    assert!(ride_course(&mut rider, &course, &across, &mut now).is_empty());
    rider.reset();
    let backwards = path(&[(1000.0, 500.0), (1000.0, 0.0), (300.0, 0.0)], 7.0);
    assert!(ride_course(&mut rider, &course, &backwards, &mut now).is_empty());
}

#[test]
fn sharing_a_kilometre_then_turning_off_is_off_course_until_back_on_it() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let ride = path(&[(0.0, 0.0), (1000.0, 0.0), (1000.0, -700.0)], 2.0);
    assert_eq!(
        ride_course(&mut rider, &course, &ride, &mut now),
        [Event::Following, Event::OffCourse, Event::StillOff]
    );
    assert!(rider.follower.following(), "700 m off");
    let off = rider.follower.sign(now);
    assert!(matches!(off, Sign::OffCourse { .. }), "{off:?}");
    let back = path(
        &[
            (1000.0, -700.0),
            (700.0, -700.0),
            (700.0, 0.0),
            (1000.0, 0.0),
            (1000.0, 600.0),
        ],
        7.0,
    );
    assert_eq!(
        ride_course(&mut rider, &course, &back, &mut now),
        [Event::OnCourse]
    );
}

#[test]
fn once_stopped_from_the_map_the_course_is_not_followed_again() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let on = path(&[(0.0, 0.0), (700.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &on, &mut now),
        [Event::Following]
    );
    rider.follower.stop();
    assert_eq!(rider.follower.ahead(), None);
    let more = path(&[(700.0, 0.0), (1000.0, 0.0), (1000.0, 300.0)], 7.0);
    assert!(ride_course(&mut rider, &course, &more, &mut now).is_empty());
    assert!(!rider.follower.following());
}

#[test]
fn an_out_and_back_course_keeps_to_the_leg_the_rider_is_on() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&[(0.0, 0.0), (0.0, 2500.0), (4.0, 2500.0), (4.0, 0.0)]);
    rider.set_course(&course);
    let mut now = 0;
    let mut last = u32::MAX;
    let mut events = Vec::new();
    for leg in [
        path(&[(0.0, 0.0), (0.0, 2500.0)], 7.0),
        path(&[(4.0, 2500.0), (4.0, 0.0)], 7.0),
    ] {
        for point in leg.windows(1) {
            events.extend(ride_course(&mut rider, &course, point, &mut now));
            if let Some(to_end) = rider.follower.to_end_m() {
                assert!(to_end <= last.saturating_add(30), "{to_end} after {last}");
                let sign = rider.follower.sign(now);
                assert!(!matches!(sign, Sign::OffCourse { .. }), "{sign:?}");
                last = to_end;
            }
        }
    }
    assert_eq!(events, [Event::Following, Event::Arrived]);
}

#[test]
fn a_course_set_mid_ride_is_placed_from_the_ride_s_first_fix() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let mut now = 0;
    let out = path(&[(1000.0, 1000.0), (1500.0, 1000.0)], 7.0);
    assert!(ride_course(&mut rider, &[], &out, &mut now).is_empty());
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    assert!(!rider.follower.unplaced());
    let first = *rider.follower.course().last().unwrap();
    assert!(
        distance((first.east, first.north), (-1000.0, -1000.0)) < 1.0,
        "{first:?}"
    );
    rider.set_course(&[]);
    assert!(rider.follower.course().is_empty() && !rider.follower.unplaced());
}
