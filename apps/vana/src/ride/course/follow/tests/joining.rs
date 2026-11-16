use super::*;

fn signs_along(
    rider: &mut Rider,
    course: &[course::Point],
    points: &[(f32, f32)],
    now: &mut u64,
) -> Vec<Sign> {
    let mut signs = Vec::new();
    for point in points.windows(1) {
        let events = ride_course(rider, course, point, now);
        assert!(events.is_empty(), "{events:?}");
        signs.push(rider.follower.sign(*now));
    }
    signs
}

#[test]
fn riding_against_the_course_before_joining_says_turn_around_until_lined_up() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let against = path(&[(600.0, 3.0), (300.0, 3.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &against, &mut now);
    let first = signs.iter().position(|&s| s == Sign::TurnAround);
    assert!(first.is_some_and(|k| k <= 6), "{signs:?}");
    assert!(
        signs[first.unwrap()..]
            .iter()
            .all(|&s| s == Sign::TurnAround),
        "{signs:?}"
    );
    let standing = [(300.0, 3.0); 40];
    let signs = signs_along(&mut rider, &course, &standing, &mut now);
    assert!(
        signs.iter().all(|&s| s == Sign::TurnAround),
        "standing still: {signs:?}"
    );
    let across = path(&[(300.0, 3.0), (300.0, -17.0)], 4.0);
    let signs = signs_along(&mut rider, &course, &across, &mut now);
    assert!(
        signs.iter().all(|&s| s == Sign::TurnAround),
        "crossing the road: {signs:?}"
    );
    let lined_up = path(&[(300.0, -17.0), (300.0, 0.0), (700.0, 0.0)], 7.0);
    let (_, events) = ride_course_warned(&mut rider, &course, &lined_up, &mut now);
    assert_eq!(events, [Event::Following]);
    assert!(matches!(rider.follower.sign(now), Sign::Chip { .. }));
}

#[test]
fn crossing_a_course_or_riding_far_from_it_never_says_turn_around() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let across = path(&[(500.0, -300.0), (500.0, 300.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &across, &mut now);
    assert!(signs.iter().all(|&s| s == Sign::None), "{signs:?}");
    let parallel = path(&[(900.0, -40.0), (100.0, -40.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &parallel, &mut now);
    assert!(
        signs.iter().all(|&s| s == Sign::None),
        "40 m off: {signs:?}"
    );
}

#[test]
fn turn_around_clears_on_leaving_the_course_and_never_shows_once_stopped() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let against = path(&[(600.0, 0.0), (300.0, 0.0)], 7.0);
    signs_along(&mut rider, &course, &against, &mut now);
    assert_eq!(rider.follower.sign(now), Sign::TurnAround);
    let away = path(&[(300.0, 0.0), (300.0, -200.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &away, &mut now);
    assert_eq!(signs.last(), Some(&Sign::None), "{signs:?}");
    now += nav::LOST_MS + 1_000;
    assert_eq!(rider.follower.sign(now), Sign::None);
    rider.reset();
    let on = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &on, &mut now),
        [Event::Following]
    );
    rider.follower.stop();
    let back = path(&[(500.0, 0.0), (100.0, 0.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &back, &mut now);
    assert!(
        signs.iter().all(|&s| s == Sign::None),
        "stopped from the map: {signs:?}"
    );
}

#[test]
fn a_lost_fix_takes_turn_around_down() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let against = path(&[(600.0, 0.0), (300.0, 0.0)], 7.0);
    signs_along(&mut rider, &course, &against, &mut now);
    assert_eq!(rider.follower.sign(now), Sign::TurnAround);
    for _ in 0..6 {
        now += 1_000;
        rider.observe(None, now);
    }
    assert_eq!(rider.follower.sign(now), Sign::None);
}
