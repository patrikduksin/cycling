//! DELETE RIDE?, the page after a saved ride's others when FILES can note
//! the deletion, reached only going forward and centred without the bar:
//! the ride's start, time and distance. ACT (trash) deletes it, BACK
//! returns to the list.
use super::Builtin;
use super::summary_overview::{CENTRED, MARGIN_UNITS, page};
use crate::compose::input::{Action, Button, Buttons, Go, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::WORDS;
use crate::screens::cue::Icon;
use crate::screens::ride::summary::Words;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| {
        let words = Node::leaf(WORDS, Words::Delete as u8, Size::CONTENT);
        page(MARGIN_UNITS, CENTRED, 1, &[words])
    },
    buttons: Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Delete),
            cue: Some(Icon::Trash),
        }),
        back: Some(Button {
            action: Action::Page(OnPage::Back),
            cue: Some(Icon::Back),
        }),
        move_: Some(Button {
            action: Action::Go(Go::Next),
            cue: None,
        }),
        held: None,
    },
};
