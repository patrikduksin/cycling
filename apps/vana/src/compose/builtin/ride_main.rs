//! The Stats ride page: the top line, then
//! an even grid of two columns and four rows, power and heart rate filled
//! with their zones on top. The radar strip, while one is paired, takes
//! its room at the left and the grid shares what is left.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::kit::field::Field;
use crate::compose::kit::{field, grid, rule};
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{SPACE, TOP_LINE};
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

/// Gliding tiles a page shows at most.
pub const GLIDES: usize = 4;
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;

const CELLS: [[Field; 2]; 4] = [
    [Field::Pwr, Field::Heart],
    [Field::Spd, Field::Cad],
    [Field::Grade, Field::Climb],
    [Field::Dist, Field::Np],
];

pub fn tree() -> Nodes {
    let grow = Size::grow(1);
    let radar = When::new(Flags::RADAR, Flags::NONE);
    let rows = CELLS.map(|row| row.map(field));
    [
        Node::col(3, grow),
        Node::leaf(TOP_LINE, 0, Size::CONTENT),
        rule(),
        Node::row(2, grow),
        Node::leaf(SPACE, 0, Size::units(RADAR_UNITS)).when(radar),
    ]
    .into_iter()
    .chain(grid(&rows, grow))
    .collect()
}
