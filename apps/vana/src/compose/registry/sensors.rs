use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{sensor_cells, sensor_line, sensor_swap};

pub const SENSOR_LINE: u8 = 96;
pub const SENSOR_CELLS: u8 = 97;
pub const SENSOR_SWAP: u8 = 98;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<sensor_line::SensorLine>::NEW,
    &Erased::<sensor_cells::SensorCells>::NEW,
    &Erased::<sensor_swap::SensorSwap>::NEW,
];
