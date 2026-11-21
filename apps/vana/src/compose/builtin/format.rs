//! FORMAT STORAGE?: what boot found made by another version and what
//! formatting takes. ACT (trash) formats, BACK declines. While storage is
//! checked the wait shows alone and the buttons rest.
use super::question::{answer, block, gap, panel, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::Buttons;
use crate::compose::node::Node;
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| QUESTION.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: answer(true, Icon::Trash),
        back: answer(false, Icon::Back),
        move_: None,
        held: None,
    },
};

pub const WAIT: &Builtin = &Builtin {
    tree: |_: &Choices| {
        [panel(1), say(Say::FormatWait)]
            .into_iter()
            .collect::<Nodes>()
    },
    buttons: Buttons::FREE,
};

const QUESTION: [Node; 7] = [
    panel(1),
    block(5),
    say(Say::FormatTitle),
    gap(34),
    say(Say::FormatMade),
    gap(30),
    say(Say::FormatGoes),
];
