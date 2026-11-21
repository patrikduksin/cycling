//! BESTS: the rider's all-time bests under the status bar, spread evenly
//! down the page; ACT asks CLEAR BESTS?, which ACT (check) confirms and
//! BACK (back) cancels. Where bests cannot be kept, a note shows instead.
use super::question::{answer, block, gap, panel, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{BEST, SPACE, STATUS};
use crate::compose::tiles::best::Place;
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;

/// ACT asks to clear the bests where there are bests to clear; BACK
/// returns to Settings.
const LIST: Buttons = Buttons {
    act: Some(Button {
        action: Action::Page(OnPage::ClearBests),
        cue: None,
    }),
    back: Some(Button {
        action: Action::Page(OnPage::Back),
        cue: None,
    }),
    move_: None,
    held: None,
};

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| LIST_TREE.into_iter().collect::<Nodes>(),
    buttons: LIST,
});

pub const NOTE: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| {
        note(Say::BestsNoteTitle, Say::BestsNoteLine)
            .into_iter()
            .collect()
    },
    buttons: LIST,
});

pub const CLEAR: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| CLEAR_TREE.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: answer(true, Icon::Check),
        back: answer(false, Icon::Back),
        move_: None,
        held: None,
    },
});

/// Columns between the page's text and the panel's sides.
pub const SIDE_UNITS: u8 = 16;

pub const fn bar() -> Node {
    Node::leaf(STATUS, 0, Size::CONTENT)
}

pub const fn side() -> Node {
    Node::leaf(SPACE, 0, Size::units(SIDE_UNITS))
}

const fn best(index: u8) -> Node {
    Node::leaf(BEST, Place::of(index).byte(), Size::CONTENT)
}

const LIST_TREE: [Node; 14] = [
    Node::col(2, Size::grow(1)),
    bar(),
    Node::row(3, Size::grow(1)),
    side(),
    Node::col(8, Size::grow(1)),
    say(Say::BestsTitle),
    best(0),
    best(1),
    best(2),
    best(3),
    best(4),
    best(5),
    best(6),
    side(),
];

const CLEAR_TREE: [Node; 5] = [
    panel(1),
    block(3),
    say(Say::ClearTitle),
    gap(16),
    say(Say::ClearLine),
];

/// A note in place of a page's content: two lines centred under the bar.
pub const fn note(title: Say, line: Say) -> [Node; 7] {
    [
        Node::col(2, Size::grow(1)),
        bar(),
        panel(1),
        block(3),
        say(title),
        gap(12),
        say(line),
    ]
}
