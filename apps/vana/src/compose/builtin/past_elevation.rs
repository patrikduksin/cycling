//! A saved ride's elevation page: as a ride just saved shows it, with the
//! saved ride's buttons.
use super::past_overview::SEND;
use super::{Builtin, summary_elevation};

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_| summary_elevation::tree(),
    buttons: SEND,
});
