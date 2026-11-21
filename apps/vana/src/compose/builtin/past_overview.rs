//! A saved ride's first page, opened from the ride list: SAVED RIDE over
//! its start, then its totals as a ride just saved shows them. ACT (send)
//! sends it to the phone, BACK returns to the list.
use super::{Builtin, summary_overview};
use crate::compose::input::{Action, Button, Buttons, Go, OnPage};
use crate::screens::cue::Icon;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| summary_overview::tree(),
    buttons: SEND,
};

/// ACT (send) sends the saved ride to the phone, BACK returns to the list,
/// MOVE steps its pages. The send cue hides where the ride cannot be sent.
pub(super) const SEND: Buttons = Buttons {
    act: Some(Button {
        action: Action::Page(OnPage::Send),
        cue: Some(Icon::Send),
    }),
    back: Some(Button {
        action: Action::Page(OnPage::Back),
        cue: None,
    }),
    move_: Some(Button {
        action: Action::Go(Go::Next),
        cue: None,
    }),
    held: None,
};
