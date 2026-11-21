//! Ids 80–95: the scripted tiles: the brand reveal and other drawn motion.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::script;

pub const SCRIPT: u8 = 80;

pub(super) static TILES: &[&dyn Component] = &[&Erased::<script::Script>::NEW];
