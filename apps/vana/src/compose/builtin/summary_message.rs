//! SAVING while a ride's last samples are written, alone under the bar.
//! The press that finishes a ride waits for it; no cue shows meanwhile.
use super::summary_overview::{CENTRED, MARGIN_UNITS, page};
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::WORDS;
use crate::screens::ride::summary::Words;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_| tree(),
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
});

pub(super) fn tree() -> Nodes {
    let words = Node::leaf(WORDS, Words::Message as u8, Size::CONTENT);
    page(MARGIN_UNITS, CENTRED, 1, &[words])
}
