use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{
    sensor_back, sensor_facts, sensor_head, sensor_hint, sensor_list, sensor_search, sensor_title,
};

pub const SENSOR_TITLE: u8 = 96;
pub const SENSOR_SEARCH: u8 = 97;
pub const SENSOR_LIST: u8 = 98;
pub const SENSOR_HINT: u8 = 99;
pub const SENSOR_BACK: u8 = 100;
pub const SENSOR_HEAD: u8 = 101;
pub const SENSOR_FACTS: u8 = 102;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<sensor_title::SensorTitle>::NEW,
    &Erased::<sensor_search::SensorSearch>::NEW,
    &Erased::<sensor_list::SensorList>::NEW,
    &Erased::<sensor_hint::SensorHint>::NEW,
    &Erased::<sensor_back::SensorBack>::NEW,
    &Erased::<sensor_head::SensorHead>::NEW,
    &Erased::<sensor_facts::SensorFacts>::NEW,
];
