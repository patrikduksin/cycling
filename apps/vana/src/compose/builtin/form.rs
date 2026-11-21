//! FORM: fitness, fatigue and form under the status bar, then the trend of
//! the last six weeks with its ends named under it, and a note when rides
//! without power are left out. While the rides are read, or without a date
//! or a ride with power, a note shows instead. BACK returns to Settings.
use super::bests::{bar, note, side};
use super::question::{gap, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{FORM_BAND, LINE, TREND};
use crate::compose::tiles::form::Reads;
use crate::compose::tiles::line::Say;

const BUTTONS: Buttons = Buttons {
    act: None,
    back: Some(Button {
        action: Action::Page(OnPage::Back),
        cue: None,
    }),
    move_: None,
    held: None,
};

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| TREE.into_iter().collect::<Nodes>(),
    buttons: BUTTONS,
});

pub const NOTE: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| {
        note(Say::FormNoteTitle, Say::FormNoteLine)
            .into_iter()
            .collect()
    },
    buttons: BUTTONS,
});

const fn band(reads: Reads) -> Node {
    Node::leaf(FORM_BAND, reads as u8, Size::CONTENT)
}

const TREE: [Node; 25] = [
    Node::col(2, Size::grow(1)),
    bar(),
    Node::row(3, Size::grow(1)),
    side(),
    Node::col(17, Size::grow(1)),
    gap(10),
    say(Say::FormTitle),
    gap(10),
    band(Reads::Fitness),
    gap(6),
    band(Reads::Fatigue),
    gap(6),
    band(Reads::Form),
    gap(28),
    Node::leaf(TREND, 0, Size::grow(1)),
    gap(10),
    Node::row(2, Size::CONTENT),
    Node::leaf(LINE, Say::WeeksAgo as u8, Size::grow(1)),
    say(Say::Now),
    gap(8),
    say(Say::LeftOut),
    gap(2),
    say(Say::LeftOutLine),
    gap(14),
    side(),
];
