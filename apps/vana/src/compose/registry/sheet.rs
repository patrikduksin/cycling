use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{home, sheet};

pub const LEVEL: u8 = 104;
pub const SHEET_SENSORS: u8 = 105;
pub const DAY_NIGHT: u8 = 106;
pub const FACE: u8 = 107;
pub const WIDGET: u8 = 108;
pub const TODAY: u8 = 109;
pub const START: u8 = 110;
pub const SHEET_PHONE: u8 = 111;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<sheet::Level>::NEW,
    &Erased::<sheet::Sensors>::NEW,
    &Erased::<sheet::DayNight>::NEW,
    &Erased::<home::Face>::NEW,
    &Erased::<home::Widget>::NEW,
    &Erased::<home::TodayCard>::NEW,
    &Erased::<home::Start>::NEW,
    &Erased::<sheet::Phone>::NEW,
];

const _: () = assert!(TILES.len() == (SHEET_PHONE - LEVEL) as usize + 1);
