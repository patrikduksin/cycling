//! Ids 64–79: the lists' tiles. 64–75 are Workouts', History's and
//! Settings'; 76–79 are left to Sensors.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{edit_field, entry, glance, heading, pad, save_status, section};

pub const HEADING: u8 = 64;
pub const ENTRY: u8 = 65;
pub const GLANCE: u8 = 66;
pub const SAVE_STATUS: u8 = 67;
pub const EDIT_FIELD: u8 = 68;
pub const PAD: u8 = 69;
pub const SECTION: u8 = 70;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<heading::Heading>::NEW,
    &Erased::<entry::EntryLine>::NEW,
    &Erased::<glance::WeekGlance>::NEW,
    &Erased::<save_status::SaveStatus>::NEW,
    &Erased::<edit_field::EditField>::NEW,
    &Erased::<pad::Pad>::NEW,
    &Erased::<section::Section>::NEW,
];

/// Ids 76–79: Sensors' tiles.
pub(super) static SENSORS: &[&dyn Component] = &[];
