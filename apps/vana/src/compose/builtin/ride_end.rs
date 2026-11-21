//! END RIDE?, which BACK opens from the paused screen: the question, the
//! ride it is about, and what the answer does. ACT saves and ends the ride,
//! BACK returns to the paused ride, and MOVE asks DISCARD RIDE? where a ride
//! can be discarded.
use super::{Builtin, Choices};
use crate::compose::input::{Action, Button, Buttons};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{RIDE_LINE, SPACE};
use crate::compose::tiles::ride_line::Says;
use crate::screens::cue::Icon;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| TREE.into_iter().collect(),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Check),
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: Some(Icon::Back),
        }),
        move_: Some(Button {
            action: Action::Ride(Role::Move),
            cue: Some(Icon::Trash),
        }),
        held: None,
    },
});

const fn line(says: Says) -> Node {
    Node::leaf(RIDE_LINE, says.byte(), Size::CONTENT)
}

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

/// The question and the ride it is about read as one block, centred down
/// the screen so the cues' corners stay clear of it.
const TREE: [Node; 9] = [
    Node::col(1, Size::grow(1)),
    Node::col(7, Size::CONTENT),
    line(Says::EndRide),
    space(28),
    line(Says::RideTime),
    space(10),
    line(Says::RideKm),
    space(28),
    line(Says::Saved),
];
