use super::*;
use std::string::String;

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
        signs.push(rider.sign(*now));
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
    assert!(matches!(rider.sign(now), Sign::Chip { .. }));
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
    assert_eq!(rider.sign(now), Sign::TurnAround);
    let away = path(&[(300.0, 0.0), (300.0, -200.0)], 7.0);
    let signs = signs_along(&mut rider, &course, &away, &mut now);
    assert_eq!(signs.last(), Some(&Sign::None), "{signs:?}");
    now += nav::LOST_MS + 1_000;
    assert_eq!(rider.sign(now), Sign::None);
    rider.reset();
    let on = path(&[(0.0, 0.0), (500.0, 0.0)], 7.0);
    assert_eq!(
        ride_course(&mut rider, &course, &on, &mut now),
        [Event::Following]
    );
    rider.let_go();
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
    assert_eq!(rider.sign(now), Sign::TurnAround);
    for _ in 0..6 {
        now += 1_000;
        rider.observe(None, now);
    }
    assert_eq!(rider.sign(now), Sign::None);
}

#[test]
fn standing_beside_the_course_with_a_jittering_fix_never_says_turn_around() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let jitter = [
        (600.0, 3.0),
        (584.0, 3.0),
        (583.9, 3.0),
        (584.1, 3.0),
        (584.0, 3.2),
        (583.8, 3.0),
        (584.1, 2.9),
    ];
    let signs = signs_along(&mut rider, &course, &jitter, &mut now);
    assert!(signs.iter().all(|&s| s != Sign::TurnAround), "{signs:?}");
}

#[test]
fn turn_around_says_why_by_the_against_fixes_that_raised_it() {
    use firmware_services::why;
    use std::string::ToString;
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let _collecting = why::collect();
    let mut now = 0;
    for point in path(&[(600.0, 3.0), (300.0, 3.0)], 7.0) {
        why::pass(now + 1_000, |_| {});
        ride_course(&mut rider, &course, &[point], &mut now);
        if rider.sign(now) == Sign::TurnAround {
            break;
        }
    }
    let decided: Vec<why::Why> = why::take()
        .into_iter()
        .filter_map(|entry| match entry {
            why::Entry::Decision(why) => Some(why),
            why::Entry::Delta(_) => None,
        })
        .collect();
    assert_eq!(rider.sign(now), Sign::TurnAround, "{decided:#?}");
    let operand = |why: &why::Why, name: &str| {
        why.operands
            .iter()
            .find(|(named, _)| *named == name)
            .map(|(_, value)| value.clone())
    };
    let (raised, before) = decided.split_last().expect("decisions");
    assert_eq!(raised.tag, "follow.turn_around", "{decided:#?}");
    assert_eq!(raised.at_ms, now);
    let threshold = TURN_AROUND_FIXES.to_string();
    assert_eq!(operand(raised, "against_fixes"), Some(threshold.clone()));
    assert_eq!(operand(raised, "turn_around_fixes"), Some(threshold));
    assert_eq!(operand(raised, "fresh"), Some("true".into()));
    let joining: Vec<_> = before
        .iter()
        .filter(|why| why.tag == "follow.joining")
        .collect();
    let against: Vec<_> = joining
        .iter()
        .filter(|why| operand(why, "riding").as_deref() == Some("Against"))
        .map(|why| operand(why, "against_fixes").unwrap())
        .collect();
    assert_eq!(against, ["1", "2", "3"], "{joining:#?}");
    let last = joining.last().unwrap();
    assert_eq!(operand(last, "fixes"), Some("0".into()));
    assert_eq!(operand(last, "moving"), Some("true".into()));
    assert!(
        before.iter().any(|why| why.tag == "follow.no_turn_around"),
        "{decided:#?}"
    );
}

fn fields(follower: &Follower) -> Vec<(&'static str, &'static str, String)> {
    use firmware_services::why::{Fields, Value};
    let mut named = Vec::new();
    let mut sink = |owner, field, value: Value<'_>| {
        let value = match value {
            Value::Debug(value) => std::format!("{value:?}"),
            Value::Buffer { len, .. } => std::format!("{len} items"),
        };
        named.push((owner, field, value));
    };
    Fields::new(&mut sink).owner("follow", follower);
    named
}

fn field(named: &[(&str, &str, String)], owner: &str, name: &str) -> Option<String> {
    named
        .iter()
        .find(|(o, n, _)| *o == owner && *n == name)
        .map(|(_, _, value)| value.clone())
}

#[test]
fn the_follower_shows_its_state_and_its_navigator_s_once_following() {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&SQUARE);
    rider.set_course(&course);
    let mut now = 0;
    let against = path(&[(600.0, 0.0), (300.0, 0.0)], 7.0);
    signs_along(&mut rider, &course, &against, &mut now);
    let named = fields(&rider.follower);
    assert_eq!(field(&named, "follow", "life").as_deref(), Some("Joining"));
    assert_eq!(
        field(&named, "follow", "course").as_deref(),
        Some("401 items")
    );
    assert!(
        field(&named, "follow", "against_fixes")
            .is_some_and(|fixes| fixes != "Some(0)" && fixes != "None"),
        "{named:?}"
    );
    assert_eq!(field(&named, "follow", "segment").as_deref(), Some("None"));
    assert!(named.iter().all(|(owner, ..)| *owner == "follow"));
    let lined_up = path(
        &[(300.0, 0.0), (300.0, -17.0), (300.0, 0.0), (700.0, 0.0)],
        7.0,
    );
    ride_course(&mut rider, &course, &lined_up, &mut now);
    let named = fields(&rider.follower);
    assert_eq!(
        field(&named, "follow", "life").as_deref(),
        Some("Following")
    );
    assert_eq!(
        field(&named, "follow", "against_fixes").as_deref(),
        Some("None")
    );
    assert!(field(&named, "follow", "segment").is_some_and(|s| s.starts_with("Some(")));
    assert_eq!(field(&named, "nav", "place").as_deref(), Some("On"));
    assert_eq!(field(&named, "nav", "lost").as_deref(), Some("false"));
}
