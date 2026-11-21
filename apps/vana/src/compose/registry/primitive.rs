//! Ids 0–14: tiles any screen draws.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{caption, gauge, icon, space, status, unit, value, zone};

pub const SPACE: u8 = 0;
pub const VALUE: u8 = 1;
pub const CAPTION: u8 = 2;
pub const UNIT: u8 = 3;
pub const GAUGE: u8 = 4;
pub const ZONE: u8 = 5;
pub const ICON: u8 = 6;
pub const STATUS: u8 = 7;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<space::Space>::NEW,
    &Erased::<value::Value>::NEW,
    &Erased::<caption::Caption>::NEW,
    &Erased::<unit::Unit>::NEW,
    &Erased::<gauge::Gauge>::NEW,
    &Erased::<zone::ZoneChip>::NEW,
    &Erased::<icon::Icon>::NEW,
    &Erased::<status::StatusBar>::NEW,
];
