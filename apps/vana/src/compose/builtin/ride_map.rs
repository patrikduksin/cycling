//! The map page: the status bar, the map growing into every row between it
//! and the foot, and the foot's scale and distance. Beside a radar the map
//! starts right of the strip; the foot runs under the strip, which ends
//! above it. Holding MOVE stops following the course.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{MAP, SCALE, SPACE, STATUS};
use crate::screens::ride::RADAR_WIDTH;
use crate::screens::ride::map::{BAR_UNITS, FOOT_UNITS};
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_| TREE.into_iter().collect::<Nodes>(),
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
        held: Some(Action::CourseOff),
    },
});

const TREE: [Node; 6] = [
    Node::col(3, Size::grow(1)),
    Node::leaf(STATUS, 0, Size::units(BAR_UNITS as u8)),
    Node::row(2, Size::grow(1)),
    Node::leaf(SPACE, 0, Size::units(RADAR_WIDTH as u8)).when(When::new(Flags::RADAR, Flags::NONE)),
    Node::leaf(MAP, 0, Size::grow(1)),
    Node::leaf(SCALE, 0, Size::units(FOOT_UNITS as u8)),
];
