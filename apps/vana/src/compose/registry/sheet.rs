use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::sheet;

pub const LEVEL: u8 = 104;
pub const SHEET_SENSORS: u8 = 105;
pub const DAY_NIGHT: u8 = 106;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<sheet::Level>::NEW,
    &Erased::<sheet::Sensors>::NEW,
    &Erased::<sheet::DayNight>::NEW,
];
