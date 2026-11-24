use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{edit_field, entry, heading, pad, save_status, section};

pub const HEADING: u8 = 64;
pub const ENTRY: u8 = 65;
pub const SAVE_STATUS: u8 = 66;
pub const EDIT_FIELD: u8 = 67;
pub const PAD: u8 = 68;
pub const SECTION: u8 = 69;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<heading::Heading>::NEW,
    &Erased::<entry::EntryLine>::NEW,
    &Erased::<save_status::SaveStatus>::NEW,
    &Erased::<edit_field::EditField>::NEW,
    &Erased::<pad::Pad>::NEW,
    &Erased::<section::Section>::NEW,
];
