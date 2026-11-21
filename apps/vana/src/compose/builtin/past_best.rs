//! A saved ride's best page: as a ride just saved shows it, with the
//! saved ride's buttons.
use super::past_overview::SEND;
use super::{Builtin, summary_best};

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| summary_best::tree(),
    buttons: SEND,
};
