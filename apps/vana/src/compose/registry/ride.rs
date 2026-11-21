//! Ids 16–47: the ride pages' tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{chip, effort, reading, total};

pub const READING: u8 = 16;
pub const TOTAL: u8 = 17;
pub const EFFORT: u8 = 18;
pub const CHIP: u8 = 19;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<reading::Reading>::NEW,
    &Erased::<total::Total>::NEW,
    &Erased::<effort::Effort>::NEW,
    &Erased::<chip::Chip>::NEW,
];
