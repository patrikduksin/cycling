//! A saved ride's effort page: as a ride just saved shows it, with the
//! saved ride's buttons.
use super::past_overview::SEND;
use super::{Builtin, summary_effort};

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| summary_effort::tree(),
    buttons: SEND,
};
