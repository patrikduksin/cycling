//! The main ride page as a tree of tiles: the status bar, then elapsed time
//! and grade, the course chip on a course, speed (the hero off a course), power and
//! heart rate with their zone gauges, and the two readings the rider chose.
//! On arrival the ride's totals replace the readings. The tree fills the
//! panel and its spare room becomes even gaps, so nothing is placed by hand.
use super::{Builtin, Choices};
use crate::compose::data::{Effort, Metric};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, MAX_NODES, Node, Size, When};
use crate::compose::registry::{CHIP, EFFORT, READING, SPACE, STATUS, TOTAL};
use crate::compose::tiles::reading::{Inline, Shown, Side};
use crate::compose::tiles::total::Captioned;
use crate::screens::ride::slots::Slots;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |choices: &Choices| tree(choices.main_slots).into_iter().collect(),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: None,
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: None,
        }),
        move_: Some(Button {
            action: Action::Go(Go::Next),
            cue: None,
        }),
        held: None,
    },
});

/// The status bar and the black under it.
const BAR_UNITS: u8 = 24;
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const LEFT_UNITS: u8 = 10;
const RIGHT_UNITS: u8 = crate::screens::ride::TEXT_RIGHT as u8;
/// Columns between the lower line's two readings.
const BETWEEN_UNITS: u8 = 8;
/// Gliding tiles a page shows at most.
pub const GLIDES: usize = 4;
/// On arrival the banner stays, over the room the time and chip took.
const BANNER_UNITS: u8 = crate::screens::sign::BANNER_UNITS as u8;
pub const NODES: usize = 28;
const _: () = assert!(NODES <= MAX_NODES);

/// The largest look a reading takes, on the readings' ladder of faces.
const HERO: u8 = 0;
const LARGE: u8 = 1;
const VALUE: u8 = 2;
const CAPTION: u8 = 4;
/// On the totals' ladder.
const TOTAL_60: u8 = 0;
const TOTAL_VALUE: u8 = 2;
const TOTAL_30: u8 = 3;

const fn when(needs: Flags, forbids: Flags) -> When {
    When::new(needs, forbids)
}

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn reading(metric: Metric, largest: u8, side: Side, size: Size) -> Node {
    let shown = Shown::left(metric, largest).on(side);
    Node::leaf(READING, Inline(shown).byte(), size)
}

const fn total(metric: Metric, largest: u8, side: Side, size: Size) -> Node {
    let shown = Shown::left(metric, largest).on(side);
    Node::leaf(TOTAL, Captioned(shown).byte(), size).when(when(Flags::ARRIVED, Flags::NONE))
}

const fn effort(effort: Effort, target: bool) -> Node {
    let arg = effort.id() | (target as u8) << 1;
    let workout = if target {
        when(Flags::WORKOUT, Flags::ARRIVED)
    } else {
        when(Flags::NONE, Flags::WORKOUT.with(Flags::ARRIVED))
    };
    Node::leaf(EFFORT, arg, Size::CONTENT).when(workout)
}

/// The main page with the rider's lower line. Its middle column holds no
/// growing tile, so the room its tiles leave becomes even gaps. On arrival
/// the banner's room replaces the time and grade.
pub const fn tree(slots: Slots) -> [Node; NODES] {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    let riding = when(Flags::NONE, Flags::ARRIVED);
    let arrived = when(Flags::ARRIVED, Flags::NONE);
    [
        Node::col(2, grow),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(4, grow),
        space(RADAR_UNITS).when(when(Flags::RADAR, Flags::NONE)),
        space(LEFT_UNITS),
        Node::col(13, grow),
        space(BANNER_UNITS).when(arrived),
        Node::row(2, content).when(riding),
        reading(Metric::Elapsed, VALUE, Side::Left, grow),
        reading(Metric::Grade, CAPTION, Side::Right, content),
        Node::leaf(CHIP, 0, content).when(when(Flags::COURSE, Flags::ARRIVED)),
        reading(Metric::Speed, HERO, Side::Left, content)
            .hero()
            .when(when(Flags::NONE, Flags::COURSE.with(Flags::ARRIVED))),
        reading(Metric::Speed, LARGE, Side::Left, content)
            .when(when(Flags::COURSE, Flags::ARRIVED)),
        effort(Effort::Power, false),
        effort(Effort::Power, true),
        effort(Effort::Heart, false),
        effort(Effort::Heart, true),
        Node::row(3, content).when(riding),
        reading(slots.left, VALUE, Side::Left, grow),
        space(BETWEEN_UNITS),
        reading(slots.right, VALUE, Side::Right, grow),
        total(Metric::Distance, TOTAL_60, Side::Left, content),
        total(Metric::Elapsed, TOTAL_VALUE, Side::Left, content),
        Node::row(3, content).when(arrived),
        total(Metric::AvgSpeed, TOTAL_30, Side::Left, grow).when(When::ALWAYS),
        total(Metric::AvgPower, TOTAL_30, Side::Centre, grow).when(When::ALWAYS),
        total(Metric::AvgHeart, TOTAL_30, Side::Right, grow).when(When::ALWAYS),
        space(RIGHT_UNITS),
    ]
}
