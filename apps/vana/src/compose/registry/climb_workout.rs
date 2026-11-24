//! Ids 32–35: the climb page's tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{climb_effort, climb_profile, road, steepness};

pub const CLIMB_ROAD: u8 = 32;
pub const CLIMB_EFFORT: u8 = 33;
pub const STEEPNESS: u8 = 34;
pub const CLIMB_PROFILE: u8 = 35;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<road::Road>::NEW,
    &Erased::<climb_effort::ClimbEffort>::NEW,
    &Erased::<steepness::Steepness>::NEW,
    &Erased::<climb_profile::ClimbProfile>::NEW,
];
