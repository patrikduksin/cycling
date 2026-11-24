//! Ids 40–47: the map page's tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{map, scale};

pub const MAP: u8 = 40;
pub const SCALE: u8 = 41;

pub(super) static TILES: &[&dyn Component] =
    &[&Erased::<map::Map>::NEW, &Erased::<scale::Scale>::NEW];
