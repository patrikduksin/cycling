//! While a ride is paused the whole screen says so: the pause mark, PAUSED
//! or AUTO PAUSED and for how long, then the ride so far, and the hint to
//! ride on while auto-paused, whose room stays when it does not show. ACT
//! resumes, or holds an auto-pause; BACK asks END RIDE?; MOVE shows the
//! ride pages.
use super::{Builtin, Choices};
use crate::compose::data::Metric;
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{PAUSE, RIDE_CELL, RIDE_LINE, SPACE};
use crate::compose::tiles::ride_cell::Celled;
use crate::compose::tiles::ride_line::Says;
use crate::screens::cue::Icon;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| TREE.into_iter().collect(),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Play),
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: Some(Icon::Stop),
        }),
        move_: Some(Button {
            action: Action::Go(Go::Next),
            cue: None,
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

const RIDE_TIME: Celled = Celled {
    metric: Metric::Elapsed,
    large: true,
    centre: true,
};

/// The pause, the ride so far and the hint read as one block, centred down
/// the screen so the cues' corners stay clear of it.
const TREE: [Node; 13] = [
    Node::col(1, Size::grow(1)),
    Node::col(11, Size::CONTENT),
    Node::leaf(PAUSE, 0, Size::CONTENT),
    space(10),
    line(Says::Paused),
    space(8),
    line(Says::PausedFor),
    space(24),
    Node::leaf(RIDE_CELL, RIDE_TIME.byte(), Size::CONTENT),
    space(10),
    line(Says::RideKm),
    space(24),
    line(Says::Resume),
];
