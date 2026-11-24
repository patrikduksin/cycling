//! Home, a watch face that starts the ride: one big Ride button, or with a
//! workout planned for today its card over "Just ride" (`TODAY`).
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{FACE, SPACE, START, TODAY as TODAY_CARD, WIDGET};
use crate::compose::tiles::home::{
    Begin, FACE_UNITS, JUST_RIDE_UNITS, RIDE_UNITS, TODAY_UNITS, WIDGET_UNITS,
};

const BUTTONS: Buttons = Buttons {
    act: None,
    back: Some(Button {
        action: Action::Go(Go::TurnOff),
        cue: None,
    }),
    move_: None,
    held: None,
};

pub const BUILTIN: &Builtin = &Builtin {
    tree: || {
        let units = Size::units;
        home(&[
            Node::col(3, Size::grow(1)),
            space(Size::grow(1)),
            Node::leaf(START, Begin::Ride as u8, units(RIDE_UNITS)),
            space(units(FOOT)),
        ])
    },
    buttons: BUTTONS,
};

/// Home with a workout planned for today.
pub const TODAY: &Builtin = &Builtin {
    tree: || {
        let units = Size::units;
        home(&[
            Node::col(5, Size::grow(1)),
            space(Size::grow(1)),
            Node::leaf(TODAY_CARD, 0, units(TODAY_UNITS)),
            space(units(BETWEEN)),
            Node::leaf(START, Begin::JustRide as u8, units(JUST_RIDE_UNITS)),
            space(units(FOOT)),
        ])
    },
    buttons: BUTTONS,
};

const WIDGET_TOP: u8 = 12;
const WIDGET_LEFT: u8 = 12;
const BETWEEN: u8 = 7;
const SIDE: u8 = 8;
const FOOT: u8 = 10;

const fn space(size: Size) -> Node {
    Node::leaf(SPACE, 0, size)
}

/// The face and the status widget over `start`, a column of the ways to
/// start a ride.
fn home(start: &[Node]) -> Nodes {
    let units = Size::units;
    [
        Node::col(4, Size::grow(1)),
        Node::leaf(FACE, 0, units(FACE_UNITS)),
        space(units(WIDGET_TOP)),
        Node::row(3, units(WIDGET_UNITS)),
        space(units(WIDGET_LEFT)),
        Node::leaf(WIDGET, 0, Size::CONTENT),
        space(Size::grow(1)),
        Node::row(3, Size::grow(1)),
        space(units(SIDE)),
    ]
    .into_iter()
    .chain(start.iter().copied())
    .chain([space(units(SIDE))])
    .collect()
}
