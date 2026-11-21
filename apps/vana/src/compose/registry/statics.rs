//! Ids 48–63: the static pages' tiles: home, settings, sheets and questions.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{best, check, choice, code, form, home, line, mark, power, says};

pub const MARK: u8 = 48;
pub const SAYS: u8 = 49;
pub const CODE: u8 = 50;
pub const CHOICE: u8 = 51;
pub const CHECK: u8 = 52;
pub const RIDING: u8 = 53;
pub const HOME_CARD: u8 = 54;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<mark::Mark>::NEW,
    &Erased::<says::Words>::NEW,
    &Erased::<code::Code>::NEW,
    &Erased::<choice::Choice>::NEW,
    &Erased::<check::Check>::NEW,
    &Erased::<check::Riding>::NEW,
    &Erased::<home::HomeCard>::NEW,
];

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
