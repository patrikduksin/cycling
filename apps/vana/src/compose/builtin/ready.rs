//! Ready: the headline and what the ride will be, then a line per check,
//! evenly apart from the top. ACT starts the ride beside the play cue;
//! MOVE steps to the workout line (ACT opens Workouts), then the sensors
//! line (ACT opens the sensor menu), then back to the start. BACK returns
//! Home, and rolling off starts the ride too.
use super::{BAR_UNITS, Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CHECK, RIDING, SAYS, SPACE};
use crate::compose::tiles::says::Says;
use crate::screens::cue::Icon;
use crate::screens::ready::Line;
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree,
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Play),
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: None,
        }),
        move_: None,
        held: None,
    },
};

/// The lines keep their own inset inside these; the headline matches it.
const SIDE_UNITS: u8 = 4;
const INSET_UNITS: u8 = 6;
/// The rows between the groups. The spare room goes under the last line,
/// where the start cue sits, so storage's line keeps clear of it.
const GAP_UNITS: u8 = 4;

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

fn tree(_: &Choices) -> Nodes {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    let check = |line: Line| Node::leaf(CHECK, line as u8, content);
    [
        Node::col(2, grow),
        space(BAR_UNITS),
        Node::row(3, grow),
        space(SIDE_UNITS),
        Node::col(11, grow),
        space(GAP_UNITS),
        Node::col(2, content),
        Node::row(2, content),
        space(INSET_UNITS),
        Node::leaf(SAYS, Says::ReadyHeadline as u8, grow),
        Node::leaf(RIDING, 0, content),
        space(GAP_UNITS),
        check(Line::Gps),
        space(GAP_UNITS),
        check(Line::Sensors),
        space(GAP_UNITS),
        check(Line::Battery),
        space(GAP_UNITS),
        check(Line::Storage),
        Node::leaf(SPACE, 0, grow),
        space(SIDE_UNITS),
    ]
    .into_iter()
    .collect()
}
