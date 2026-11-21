//! READING while a saved ride is read back, or RIDE UNREADABLE, alone under
//! the bar, with the saved ride's buttons: its send cue hides while it is
//! read.
use super::past_overview::SEND;
use super::{Builtin, summary_message};

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| summary_message::tree(),
    buttons: SEND,
};
