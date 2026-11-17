use super::*;
use crate::testing::{Report, explore};
use std::string::String;

mod deep;
mod fixtures;
mod model;
mod rules;
mod snapshot;
mod table;

use super::turn_list::{View, turn_rules};
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
    let placed = shape.map(|shape| fixture(bounds.size, shape));
    let first = bounds.times.iter().copied().min().unwrap_or(0);
    let mut init = World::new(bounds, placed, first);
    let mut joining = 0..JOIN_FIXES;
    while bounds.starts_following && init.kind() != LifecycleKind::Following {
        assert!(
            joining.next().is_some(),
            "{:?} joins in {JOIN_FIXES} fixes",
            placed.map(|f| f.name)
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
        |world| world.kind().name(),
        HOLDS,
        &LEGAL,
        budget,
    );
    Searched {
        name: placed.map_or("no course", |placed| placed.name),
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

const OFF_AND_BACK: [Event; 3] = [Event::OffCourse, Event::StillOff, Event::OnCourse];

impl Bounds {
    fn raises(&self, shape: Shape) -> &'static [Event] {
        match (self.starts_following, shape) {
            (true, _) => &OFF_AND_BACK,
            (false, Shape::Straight) => &JOINS,
            (false, Shape::L | Shape::Hairpin) => &TURNS_SOUND,
        }
    }
}

fn closes(bounds: Bounds, shape: Option<Shape>) {
    let raises = shape.map_or(&[][..], |shape| bounds.raises(shape));
    let searched = search(&bounds, shape, BUDGET);
    let text = summary(&searched);
    std::println!("{text}");
    let report = &searched.report;
    assert!(report.closed, "{text}");
    let kinds: Vec<&str> = match shape {
        Some(_) => KINDS.iter().map(|kind| kind.name()).collect(),
        None => std::vec![LifecycleKind::NoCourse.name()],
    };
    for kind in kinds {
        assert!(report.reached.contains(&kind), "{kind} unreached\n{text}");
    }
    let raised = &searched.raised;
    for event in raises {
        assert!(
            raised.contains(event),
            "{event:?} unraised: {raised:?}\n{text}"
        );
    }
    assert!(report.broken.is_empty(), "{text}");
}

#[test]
fn places_without_a_course() {
    closes(Bounds::places(), None);
}

#[test]
fn times_without_a_course() {
    closes(Bounds::times(), None);
}

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

#[test]
fn an_upload_borrows_the_turn_list_while_the_line_is_empty() {
    let fixture = fixture(Size::Issue, Shape::L);
    let (mut line, mut turns) = (
        vec![Crumb::default(); fixture.course.points().len()],
        vec![Turn::default(); ROOM],
    );
    let mut follower = Follower::new(&mut line, &mut turns);
    place(&mut follower, fixture, true);
    place(&mut follower, fixture, false);
    follower.upload_room();
    follower.upload_room();
    place(&mut follower, fixture, true);
    assert!(follower.joinable());
}

#[test]
#[cfg(debug_assertions)]
#[should_panic = "the turn list is lent only while no course is placed"]
fn the_turn_list_is_never_lent_while_a_course_is_placed() {
    let fixture = fixture(Size::Issue, Shape::L);
    let (mut line, mut turns) = (
        vec![Crumb::default(); fixture.course.points().len()],
        vec![Turn::default(); ROOM],
    );
    let mut follower = Follower::new(&mut line, &mut turns);
    place(&mut follower, fixture, true);
    follower.upload_room();
}
