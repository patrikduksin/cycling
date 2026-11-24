//! Ids 32–35: the climb page's road tiles.
use crate::compose::kit::{profile, road_field};
use crate::compose::tile::{Component, Erased};

pub const ROAD_FIELD: u8 = 32;
pub const ROAD_PROFILE: u8 = 33;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<road_field::RoadField>::NEW,
    &Erased::<profile::Profile>::NEW,
];
