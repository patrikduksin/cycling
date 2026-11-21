//! Connect phone: the step and what to do, the code, then the network or
//! address with its password, and a note. Spare room becomes even gaps
//! between those four. MOVE, ACT and a tap on the code switch codes, and a
//! tap on the note tries again once a session ended; BACK ends the session.
use super::{BAR_UNITS, Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CODE, SAYS, SPACE};
use crate::compose::tiles::says::Says;

const SWITCH: Option<Button> = Some(Button {
    action: Action::Page(OnPage::PhoneCode),
    cue: None,
});

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree,
    buttons: Buttons {
        act: SWITCH,
        back: Some(Button {
            action: Action::Page(OnPage::Back),
            cue: None,
        }),
        move_: SWITCH,
        held: None,
    },
});

const SIDE_UNITS: u8 = 12;
/// Rows between a line and the one it goes with.
const PAIR_UNITS: u8 = 6;

const fn words(says: Says) -> Node {
    Node::leaf(SAYS, says as u8, Size::CONTENT)
}

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

fn tree(_: &Choices) -> Nodes {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    [
        Node::col(2, grow),
        space(BAR_UNITS),
        Node::row(3, grow),
        space(SIDE_UNITS),
        Node::col(4, grow),
        Node::col(3, content),
        words(Says::PhoneStep),
        space(PAIR_UNITS),
        words(Says::PhoneTitle),
        Node::leaf(CODE, 0, content),
        Node::col(3, content),
        words(Says::PhoneName),
        space(PAIR_UNITS),
        words(Says::PhoneSecret),
        words(Says::PhoneNote),
        space(SIDE_UNITS),
    ]
    .into_iter()
    .collect()
}
