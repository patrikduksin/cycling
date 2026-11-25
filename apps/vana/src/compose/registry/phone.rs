use crate::compose::kit::code;
use crate::compose::tile::{Component, Erased};

pub const CODE: u8 = 64;

pub(super) static TILES: &[&dyn Component] = &[&Erased::<code::CodeTile>::NEW];
