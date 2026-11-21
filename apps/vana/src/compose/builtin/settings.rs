//! Settings: one section a page, its title over its rows, which scroll
//! when they do not fit, and whether the changes are saved at the foot.
//! Each row says what it sets over its value, with a note at the right.
//! MOVE moves to the next row, into the next section past the last, ACT
//! acts on it, a tap acts on the row under the finger, and BACK goes Home.
//!
//! While a number is edited the screen is the editor: the section's name,
//! the number on an accent field, and a pad each side to step it down or
//! up. MOVE lowers, ACT raises and BACK finishes, which the editor's own
//! owner carries out, holding repeats included.
use super::workouts::{HEADING_UNITS, list};
use super::{Builtin, Choices, Nodes};
use crate::compose::data::ListKind;
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{EDIT_FIELD, HEADING, PAD, SAVE_STATUS, SECTION, SPACE, STATUS};
use crate::compose::tiles::entry::Line;
use crate::compose::tiles::pad::Way;
use crate::screens::workout::Page;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |choices: &Choices| if choices.editing { editor() } else { rows() },
    buttons: Buttons {
        act: None,
        back: Some(Button {
            action: Action::Go(Go::Open(Page::Home)),
            cue: None,
        }),
        move_: None,
        held: None,
    },
};

/// A row: what it sets over its value.
const ROW_UNITS: u8 = 46;
/// The save status's band, and the black under it.
const STATUS_UNITS: u8 = 12;
const FOOT_UNITS: u8 = 20;
const BAR_UNITS: u8 = 24;
const SIDE_UNITS: u8 = 10;
const BETWEEN_PADS_UNITS: u8 = 8;

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

fn rows() -> Nodes {
    let heading = Node::leaf(
        HEADING,
        ListKind::Settings as u8,
        Size::units(HEADING_UNITS),
    );
    let foot = [
        Node::leaf(SAVE_STATUS, 0, Size::units(STATUS_UNITS)),
        space(FOOT_UNITS),
    ];
    let lines = [Line::Detail, Line::Title];
    list(heading, ListKind::Settings, ROW_UNITS, &lines, &foot)
}

/// The editor's column holds nothing that grows, so its room becomes even
/// gaps round the section's name, the field, the pads and the status.
fn editor() -> Nodes {
    let content = Size::CONTENT;
    [
        Node::col(2, Size::grow(1)),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(3, Size::grow(1)),
        space(SIDE_UNITS),
        Node::col(4, Size::grow(1)),
        Node::leaf(SECTION, ListKind::Settings as u8, content),
        Node::leaf(EDIT_FIELD, 0, content),
        Node::row(3, content),
        Node::leaf(PAD, Way::Lower as u8, Size::grow(1)),
        space(BETWEEN_PADS_UNITS),
        Node::leaf(PAD, Way::Raise as u8, Size::grow(1)),
        Node::leaf(SAVE_STATUS, 0, content),
        space(SIDE_UNITS),
    ]
    .into_iter()
    .collect()
}
