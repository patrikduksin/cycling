//! Ids 48–63: the static pages' tiles: home, settings, sheets and questions.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{home, line, mark, power};

pub const MARK: u8 = 48;
pub const HOME_CARD: u8 = 49;

pub(super) static TILES: &[&dyn Component] =
    &[&Erased::<mark::Mark>::NEW, &Erased::<home::HomeCard>::NEW];

pub const LINE: u8 = 56;
pub const FTP: u8 = 57;
pub const POWER_RING: u8 = 58;

pub(super) static QUESTIONS: &[&dyn Component] = &[
    &Erased::<line::Line>::NEW,
    &Erased::<line::FtpHero>::NEW,
    &Erased::<power::PowerRing>::NEW,
];
