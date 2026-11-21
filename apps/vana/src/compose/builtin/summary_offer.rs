//! `FTP 245 W?`, the page after the others when a new 20 min best suggests
//! a higher FTP, centred without the bar. ACT (check) takes it and BACK
//! (back) keeps the FTP in force; either then finishes the ride.
use super::Builtin;
use super::summary_overview::{CENTRED, MARGIN_UNITS, page};
use crate::compose::input::{Action, Button, Buttons, Go, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::WORDS;
use crate::screens::cue::Icon;
use crate::screens::ride::summary::Words;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| {
        let words = Node::leaf(WORDS, Words::Offer as u8, Size::CONTENT);
        page(MARGIN_UNITS, CENTRED, 1, &[words])
    },
    buttons: Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Answer(true)),
            cue: Some(Icon::Check),
        }),
        back: Some(Button {
            action: Action::Page(OnPage::Answer(false)),
            cue: Some(Icon::Back),
        }),
        move_: Some(Button {
            action: Action::Go(Go::Next),
            cue: None,
        }),
        held: None,
    },
};
