//! The Climb ride page (#517).
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::kit::field::Field;
use crate::compose::kit::road_field::RoadSlot;
use crate::compose::kit::{field, grid, rule};
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{ROAD_FIELD, ROAD_PROFILE, SPACE, TOP_LINE};
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| tree(),
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
};

const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;

const fn road(slot: RoadSlot) -> Node {
    Node::leaf(ROAD_FIELD, slot as u8, Size::grow(1))
}

pub fn tree() -> Nodes {
    let grow = Size::grow(1);
    let radar = When::new(Flags::RADAR, Flags::NONE);
    let rows = [
        [field(Field::ClimbGrade), field(Field::ClimbPwr)],
        [field(Field::ClimbHeart), field(Field::ClimbCad)],
        [road(RoadSlot::First), road(RoadSlot::Second)],
    ];
    [
        Node::col(3, grow),
        Node::leaf(TOP_LINE, 0, Size::CONTENT),
        rule(),
        Node::row(2, grow),
        Node::leaf(SPACE, 0, Size::units(RADAR_UNITS)).when(radar),
        Node::col(3, grow),
    ]
    .into_iter()
    .chain(grid(&rows, Size::CONTENT))
    .chain([rule(), Node::leaf(ROAD_PROFILE, 0, grow)])
    .collect()
}
