use super::*;
use crate::ride::course::nav::ARRIVED_MS;
use crate::testing::{Holds, Legal};

macro_rules! holds {
    ($($name:expr => $rule:path),* $(,)?) => {
        pub(super) const HOLDS: &[Holds<World>] = &[
            $(($name, |world| $rule(&world.follower.view())),)*
            (
                "every state is a row of the old-to-new table",
                super::table::is_a_row_of_the_old_to_new_table,
            ),
        ];
    };
}
turn_rules!(holds);

fn observe_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::Observe { .. } = step else {
        return true;
    };
    use LifecycleKind::*;
    match (before.kind(), after.kind()) {
        (Joining, Following | Arrived) | (Following, Arrived) => true,
        (from, to) => from == to,
    }
}

fn course_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::Course { full } = step else {
        return true;
    };
    let want = match before.kind() {
        LifecycleKind::Following | LifecycleKind::Arrived | LifecycleKind::LetGo => {
            LifecycleKind::LetGo
        }
        _ if full => LifecycleKind::Joining,
        _ => LifecycleKind::NoCourse,
    };
    after.kind() == want
}

fn new_ride_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::NewRide = step else {
        return true;
    };
    let waits = before.follower.len >= 2 || before.follower.life.unplaced();
    after.kind()
        == match waits {
            true => LifecycleKind::Unplaced,
            false => LifecycleKind::NoCourse,
        }
}

fn let_go_ways(before: &World, step: Step, after: &World) -> bool {
    let Step::LetGo = step else {
        return true;
    };
    match before.kind() {
        LifecycleKind::Following => after.kind() == LifecycleKind::LetGo,
        _ => before.follower.encoding() == after.follower.encoding(),
    }
}

fn arrived_sign_only_while_arrived(_: &World, _: Step, after: &World) -> bool {
    let arrived_ms = latches(&after.follower.nav()).arrived_ms;
    after.edge.shown != Sign::Arrived
        || after.kind() == LifecycleKind::Arrived
            && arrived_ms.is_some_and(|at| after.now < at + ARRIVED_MS)
}

fn turn_around_sign_only_while_joining(_: &World, _: Step, after: &World) -> bool {
    after.edge.shown != Sign::TurnAround || after.kind() == LifecycleKind::Joining
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
    let Some(k) = latches(&after.follower.nav()).last_beeped else {
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
