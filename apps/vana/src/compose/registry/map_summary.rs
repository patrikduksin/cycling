//! Ids 40–47: the map page's and the summary pages' tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{best_effort, cell, map, profile, scale, words, zone_time};

pub const MAP: u8 = 40;
pub const SCALE: u8 = 41;
pub const WORDS: u8 = 42;
pub const CELL: u8 = 43;
pub const ZONE_TIME: u8 = 44;
pub const BEST_EFFORT: u8 = 45;
pub const PROFILE: u8 = 46;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<map::Map>::NEW,
    &Erased::<scale::Scale>::NEW,
    &Erased::<words::Block>::NEW,
    &Erased::<cell::Cell>::NEW,
    &Erased::<zone_time::ZoneTime>::NEW,
    &Erased::<best_effort::BestEffort>::NEW,
    &Erased::<profile::Profile>::NEW,
];
