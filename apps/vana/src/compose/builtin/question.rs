//! What the questions, notes and sheets share: a line of words, the room
//! between lines, the block a question's lines make, and the buttons that
//! answer it.
use crate::compose::input::{Action, Button, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{LINE, SPACE};
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;

pub const fn say(say: Say) -> Node {
    Node::leaf(LINE, say as u8, Size::CONTENT)
}

pub const fn gap(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

pub const fn button(action: Action, cue: Icon) -> Option<Button> {
    Some(Button {
        action,
        cue: Some(cue),
    })
}

pub const fn answer(yes: bool, cue: Icon) -> Option<Button> {
    button(Action::Page(OnPage::Answer(yes)), cue)
}

/// A column the height of its lines, which its parent centres.
pub const fn block(children: u8) -> Node {
    Node::col(children, Size::CONTENT)
}

/// The panel, whose spare rows go evenly above, between and below its
/// children.
pub const fn panel(children: u8) -> Node {
    Node::col(children, Size::grow(1))
}
