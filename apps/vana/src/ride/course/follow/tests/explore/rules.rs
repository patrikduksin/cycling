use super::*;
use crate::ride::course::nav::ARRIVED_MS;
use crate::testing::{Holds, Legal};

macro_rules! holds {
    ($($name:expr => $rule:ident),* $(,)?) => {
        pub(super) const HOLDS: &[Holds<World>] =
            &[$(($name, |world| $rule(&world.follower.view()))),*];
    };
}
turn_rules!(holds);

fn observe_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::Observe { .. } = step else {
        return true;
    };
    use Life::*;
    match (before.life(), after.life()) {
        (Joining, Following | Arrived) | (Following, Arrived) => true,
        (from, to) => from == to,
    }
}

fn course_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::Course { full } = step else {
        return true;
    };
    let want = match before.life() {
        Life::Following | Life::Arrived | Life::LetGo => Life::LetGo,
        _ if full => Life::Joining,
        _ => Life::NoCourse,
    };
    after.life() == want
}

fn new_ride_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::NewRide = step else {
        return true;
    };
    let waits = before.follower.len >= 2 || before.follower.unplaced;
    after.life()
        == match waits {
            true => Life::Unplaced,
            false => Life::NoCourse,
        }
}

fn let_go_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::LetGo = step else {
        return true;
    };
    match before.life() {
        Life::Following => after.life() == Life::LetGo,
        _ => before.follower.encoding() == after.follower.encoding(),
    }
}

fn arrived_sign_only_while_arrived(_: &World, _: Step, after: &World) -> bool {
    let arrived_ms = latches(&after.follower.nav).arrived_ms;
    after.edge.shown != Sign::Arrived
        || after.life() == Life::Arrived && arrived_ms.is_some_and(|at| after.now < at + ARRIVED_MS)
}

fn turn_around_sign_only_while_joining(_: &World, _: Step, after: &World) -> bool {
    after.edge.shown != Sign::TurnAround || after.life() == Life::Joining
}

fn paused_never_off_course(_: &World, step: Step, after: &World) -> bool {
    let Step::Observe { paused: true, .. } = step else {
        return true;
    };
    !matches!(after.edge.raised, Some(Event::OffCourse | Event::StillOff))
}

fn turn_now_once_per_turn(before: &World, step: Step, after: &World) -> bool {
    if after.edge.raised != Some(Event::TurnNow) {
        return true;
    }
    let Some(k) = latches(&after.follower.nav).last_beeped else {
        return false;
    };
    let heard = match started_over(before, step, after) {
        true => 0,
        false => before.turn_now_since_restart,
    };
    heard & 1 << k == 0
}

pub(super) const LEGAL: [Legal<World, Step>; 8] = [
    ("ways in and out: Observe", observe_ways),
    ("ways in and out: Course", course_ways),
    ("ways in and out: NewRide", new_ride_ways),
    ("ways in and out: LetGo", let_go_ways),
    (
        "the Arrived sign only while arrived, within ARRIVED_MS",
        arrived_sign_only_while_arrived,
    ),
    (
        "the TurnAround sign only while joining",
        turn_around_sign_only_while_joining,
    ),
    (
        "a paused Observe never raises OffCourse or StillOff",
        paused_never_off_course,
    ),
    ("TurnNow sounds once per turn", turn_now_once_per_turn),
];
