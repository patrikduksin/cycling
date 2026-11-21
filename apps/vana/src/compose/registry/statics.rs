//! Ids 48–63: the static pages' tiles: home, settings, sheets and questions.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{best, form, line, power};

pub(super) static TILES: &[&dyn Component] = &[];

pub const LINE: u8 = 56;
pub const FTP: u8 = 57;
pub const BEST: u8 = 58;
pub const FORM_BAND: u8 = 59;
pub const TREND: u8 = 60;
pub const POWER_RING: u8 = 61;

pub(super) static QUESTIONS: &[&dyn Component] = &[
    &Erased::<line::Line>::NEW,
    &Erased::<line::FtpHero>::NEW,
    &Erased::<best::Best>::NEW,
    &Erased::<form::Band>::NEW,
    &Erased::<form::Trend>::NEW,
    &Erased::<power::PowerRing>::NEW,
];
