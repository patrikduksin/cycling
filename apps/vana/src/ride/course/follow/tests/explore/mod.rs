//! Every state of today's `Follower` reachable in a bounded configuration,
//! with each candidate `holds` and `legal` rule of #399 checked on it.
use super::*;
use crate::testing::{Report, explore};
use std::string::String;

mod deep;
mod fixtures;
mod model;
pub(super) mod rules;
pub(super) mod snapshot;

use super::turn_list::*;
use crate::ride::course::nav::tests::latches;
use fixtures::*;
use model::*;
use rules::*;
use snapshot::*;

const BUDGET: usize = 750_000;

struct Searched {
    name: &'static str,
    report: Report<Step>,
    raised: Vec<Event>,
    elapsed: core::time::Duration,
}

fn search(bounds: &Bounds, shape: Option<Shape>, budget: usize) -> Searched {
    let start = std::time::Instant::now();
    let fixture = shape.map(|shape| fixture(bounds.size, shape));
    let first = bounds.times.iter().copied().min().unwrap_or(0);
    let mut init = World::new(bounds, fixture, first);
    let mut joining = 0..JOIN_FIXES;
    while bounds.starts_following && init.life() != Life::Following {
        assert!(
            joining.next().is_some(),
            "{:?} joins in {JOIN_FIXES} fixes",
            fixture.map(|f| f.name)
        );
        let ahead = Step::Observe {
            now: first,
            fix: Some((Move::Ahead, Way::With)),
            speed_mm_s: None,
            paused: false,
        };
        init = next(&init, ahead);
    }
    let raised = core::cell::RefCell::new(Vec::new());
    let report = explore(
        &[init],
        |world| alphabet(bounds, world),
        |world, step| {
            let after = next(world, step);
            if let Some(event) = after.edge.raised {
                let mut raised = raised.borrow_mut();
                if !raised.contains(&event) {
                    raised.push(event);
                }
            }
            after
        },
        |world| world.life().name(),
        HOLDS,
        &LEGAL,
        budget,
    );
    Searched {
        name: fixture.map_or("no course", |fixture| fixture.name),
        report,
        raised: raised.into_inner(),
        elapsed: start.elapsed(),
    }
}

fn summary(each: &Searched) -> String {
    use core::fmt::Write;
    let report = &each.report;
    let mut text = std::format!(
        "{}: {} states, {}, {:?}, reached {:?}\n",
        each.name,
        report.states,
        if report.closed { "closed" } else { "TRUNCATED" },
        each.elapsed,
        report.reached
    );
    for broken in &report.broken {
        writeln!(text, "  breaks {:?} by {:?}", broken.rule, broken.path).unwrap();
    }
    text
}

const JOINS: [Event; 2] = [Event::Following, Event::Arrived];
const TURNS_SOUND: [Event; 4] = [
    Event::Following,
    Event::TurnSoon,
    Event::TurnNow,
    Event::Arrived,
];

impl Shape {
    /// The events a search of this course raises, and the rules it breaks.
    fn expects(self) -> (&'static [Event], &'static [&'static str]) {
        match self {
            Shape::Straight => (&JOINS, &[]),
            Shape::L | Shape::Hairpin => (&TURNS_SOUND, &[TURN_LIST_OUTLIVES_THE_LINE]),
        }
    }
}

fn closes(bounds: Bounds, shape: Option<Shape>) {
    let (raises, breaks) = shape.map_or((&[][..], &[][..]), Shape::expects);
    closes_raising(bounds, shape, raises, breaks);
}

fn closes_raising(bounds: Bounds, shape: Option<Shape>, raises: &[Event], breaks: &[&str]) {
    let searched = search(&bounds, shape, BUDGET);
    let text = summary(&searched);
    std::println!("{text}");
    let report = &searched.report;
    assert!(report.closed, "{text}");
    let lives: Vec<&str> = match shape {
        Some(_) => LIVES.iter().map(|life| life.name()).collect(),
        None => std::vec![Life::NoCourse.name()],
    };
    for life in lives {
        assert!(report.reached.contains(&life), "{life} unreached\n{text}");
    }
    let raised = &searched.raised;
    for event in raises {
        assert!(
            raised.contains(event),
            "{event:?} unraised: {raised:?}\n{text}"
        );
    }
    let broken: Vec<&str> = report.broken.iter().map(|broken| broken.rule).collect();
    assert_eq!(broken, breaks, "\n{text}");
}

#[test]
fn places_without_a_course() {
    closes(Bounds::places(), None);
}

#[test]
fn times_without_a_course() {
    closes(Bounds::times(), None);
}

/// The straight has no turns, so its list never outlives the line.
#[test]
fn places_on_the_straight() {
    closes(Bounds::places(), Some(Shape::Straight));
}

#[test]
fn places_on_the_l() {
    closes(Bounds::places(), Some(Shape::L));
}

#[test]
fn speeds_on_the_l() {
    closes(Bounds::speeds(), Some(Shape::L));
}

#[test]
#[ignore = "does not close; run by hand to measure"]
fn follower_search_in_full() {
    let bounds = Bounds::full();
    for shape in core::iter::once(None).chain(Shape::ALL.map(Some)) {
        std::println!("{}", summary(&search(&bounds, shape, 100_000)));
    }
}

/// Not an `Input`, so the search never calls it.
fn upload_room_while_the_line_is_empty(follower: &Follower) -> bool {
    follower.len == 0
}

#[test]
#[ignore = "N3a finding: an upload lends the turn list while the old course is still placed"]
fn an_upload_borrows_the_turn_list_while_the_line_is_empty() {
    let fixture = fixture(Size::Issue, Shape::L);
    let (mut line, mut turns) = (
        vec![Crumb::default(); fixture.course.points().len()],
        vec![Turn::default(); ROOM],
    );
    let mut follower = Follower::new(&mut line, &mut turns);
    place(&mut follower, fixture, true);
    let lends = upload_room_while_the_line_is_empty(&follower);
    follower.turn_room();
    place(&mut follower, fixture, false);
    assert!(upload_room_while_the_line_is_empty(&follower));
    follower.turn_room();
    assert!(lends);
}
