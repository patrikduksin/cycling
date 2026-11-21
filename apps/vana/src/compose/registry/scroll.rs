//! Ids 64–79: the lists' tiles. 64–75 are Workouts', History's and
//! Settings'; 76–79 are Sensors'.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{
    edit_field, entry, glance, heading, pad, save_status, section, sensor_icon, sensor_row,
    sensor_search, sensor_title,
};

pub const HEADING: u8 = 64;
pub const ENTRY: u8 = 65;
pub const GLANCE: u8 = 66;
pub const SAVE_STATUS: u8 = 67;
pub const EDIT_FIELD: u8 = 68;
pub const PAD: u8 = 69;
pub const SECTION: u8 = 70;
pub const SENSOR_ROW: u8 = 76;
pub const SENSOR_TITLE: u8 = 77;
pub const SENSOR_SEARCH: u8 = 78;
pub const SENSOR_ICON: u8 = 79;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<heading::Heading>::NEW,
    &Erased::<entry::EntryLine>::NEW,
    &Erased::<glance::WeekGlance>::NEW,
    &Erased::<save_status::SaveStatus>::NEW,
    &Erased::<edit_field::EditField>::NEW,
    &Erased::<pad::Pad>::NEW,
    &Erased::<section::Section>::NEW,
];

/// Ids 76–79: Sensors' tiles; the pages under the lists go on at 96.
pub(super) static SENSORS: &[&dyn Component] = &[
    &Erased::<sensor_row::SensorRow>::NEW,
    &Erased::<sensor_title::SensorTitle>::NEW,
    &Erased::<sensor_search::SensorSearch>::NEW,
    &Erased::<sensor_icon::SensorIcon>::NEW,
];
