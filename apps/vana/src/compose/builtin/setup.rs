//! A first-run setup step: its title, two lines of text, then its two
//! cards. The three groups share the page's spare room in even gaps. MOVE
//! steps the cards, ACT or a tap takes one, BACK leaves setup for Home.
use super::{BAR_UNITS, Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CHOICE, SAYS, SPACE};
use crate::compose::tiles::says::Says;

pub const BUILTIN: &Builtin = &Builtin {
    tree,
    buttons: Buttons {
        act: None,
        back: Some(Button {
            action: Action::Page(OnPage::Back),
            cue: None,
        }),
        move_: None,
        held: None,
    },
};

const SIDE_UNITS: u8 = 10;
const LINES_UNITS: u8 = 6;
const CARDS_UNITS: u8 = 8;

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn words(says: Says) -> Node {
    Node::leaf(SAYS, says as u8, Size::CONTENT)
}

fn tree(_: &Choices) -> Nodes {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    [
        Node::col(2, grow),
        space(BAR_UNITS),
        Node::row(3, grow),
        space(SIDE_UNITS),
        Node::col(3, grow),
        words(Says::SetupTitle),
        Node::col(3, content),
        words(Says::SetupFirst),
        space(LINES_UNITS),
        words(Says::SetupSecond),
        Node::col(3, content),
        Node::leaf(CHOICE, 0, content),
        space(CARDS_UNITS),
        Node::leaf(CHOICE, 1, content),
        space(SIDE_UNITS),
    ]
    .into_iter()
    .collect()
}
