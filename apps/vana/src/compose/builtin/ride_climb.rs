//! The climb page as a tree of tiles: the status bar, then the road's title,
//! the grade as the hero, power and heart rate with their zones side by
//! side, the steepness band and the road's two readings, each in the page's
//! text margins, and the road's profile under them. The text and the profile share the room
//! by weight, and the text's share becomes even gaps between its tiles.
use super::{Builtin, Choices};
use crate::compose::data::{Effort, Metric};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, MAX_NODES, Node, Size, When};
use crate::compose::registry::{
    CLIMB_EFFORT, CLIMB_PROFILE, CLIMB_ROAD, READING, SPACE, STATUS, STEEPNESS,
};
use crate::compose::tiles::reading::{Inline, Shown};
use crate::compose::tiles::road::Part;
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| tree().into_iter().collect(),
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

const BAR_UNITS: u8 = 24;
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const LEFT_UNITS: u8 = crate::screens::ride::TEXT_LEFT as u8;
const RIGHT_UNITS: u8 = crate::screens::ride::TEXT_RIGHT as u8;
/// How the text and the profile share the rows they leave.
const TEXT_WEIGHT: u8 = 1;
const PROFILE_WEIGHT: u8 = 1;
/// The grade's largest look, on the readings' ladder of faces.
const LARGE: u8 = 1;
pub const NODES: usize = 18;
const _: () = assert!(NODES <= MAX_NODES);

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn radar() -> Node {
    space(RADAR_UNITS).when(When::new(Flags::RADAR, Flags::NONE))
}

const fn road(part: Part) -> Node {
    Node::leaf(CLIMB_ROAD, part as u8, Size::CONTENT)
}

const fn effort(effort: Effort) -> Node {
    Node::leaf(CLIMB_EFFORT, effort.id(), Size::grow(1))
}

pub const fn tree() -> [Node; NODES] {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    let grade = Inline(Shown::left(Metric::Grade, LARGE)).byte();
    [
        Node::col(3, grow),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(4, Size::grow(TEXT_WEIGHT)),
        radar(),
        space(LEFT_UNITS),
        Node::col(5, grow),
        road(Part::Title),
        Node::leaf(READING, grade, content).hero(),
        Node::row(2, content),
        effort(Effort::Power),
        effort(Effort::Heart),
        Node::leaf(STEEPNESS, 0, content),
        road(Part::Facts),
        space(RIGHT_UNITS),
        Node::row(2, Size::grow(PROFILE_WEIGHT)),
        radar(),
        Node::row(1, grow),
        Node::leaf(CLIMB_PROFILE, 0, content),
    ]
}
