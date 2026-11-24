//! Ids 16–31: the ride pages' tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{chip, effort, reading, ride_cell, total, zones};

pub const READING: u8 = 16;
pub const TOTAL: u8 = 17;
pub const EFFORT: u8 = 18;
pub const CHIP: u8 = 19;
pub const RIDE_CELL: u8 = 20;
pub const ZONES: u8 = 21;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<reading::Reading>::NEW,
    &Erased::<total::Total>::NEW,
    &Erased::<effort::Effort>::NEW,
    &Erased::<chip::Chip>::NEW,
    &Erased::<ride_cell::RideCell>::NEW,
    &Erased::<zones::Zones>::NEW,
];

const _: () = assert!(TILES.len() == (ZONES - READING) as usize + 1);
