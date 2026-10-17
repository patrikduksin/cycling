//! The main ride page: live speed, power and heart rate with their zones,
//! then cadence, distance, active time and grade.
use super::{CYAN, Cell, DIM, GRID, Values, clock, kilometres, speed, whole};
use crate::ride::metrics::{heart_zone, power_zone};
use crate::screens::workout::View;
use core::fmt::Write;
use firmware_shell::rendering::text::text;

const SPEED: usize = 0;
const POWER: usize = 1;
const HEART: usize = 2;
const POWER_ZONE: usize = 3;
const HEART_ZONE: usize = 4;
const CADENCE: usize = 5;
const DISTANCE: usize = 6;
const TIME: usize = 7;
const GRADE: usize = 8;

/// Row dividers below speed, power/heart and cadence/distance.
const ROWS: [usize; 3] = [101, 176, 217];
const COLUMN: usize = 135;

/// Each cell and the value it shows, left column then right.
const CELLS: [(Cell, usize); 7] = [
    (cell(38, 36, b"SPEED", 2, 54, 6), SPEED),
    (cell(38, 106, b"WATTS", 2, 124, 4), POWER),
    (cell(146, 106, b"BPM", 2, 124, 4), HEART),
    (cell(38, 181, b"CADENCE", 1, 192, 3), CADENCE),
    (cell(146, 181, b"DISTANCE", 1, 192, 3), DISTANCE),
    (cell(38, 222, b"ACTIVE TIME", 1, 234, 2), TIME),
    (cell(150, 222, b"GRADE", 1, 234, 2), GRADE),
];

const fn cell(
    left: usize,
    top: usize,
    label: &'static [u8],
    label_scale: usize,
    value_top: usize,
    value_scale: usize,
) -> Cell {
    Cell {
        left,
        top,
        label,
        label_scale,
        value_top,
        value_scale,
    }
}

pub(super) fn prepare(view: &View, values: &mut Values) {
    let sample = view.sample;
    speed(&mut values[SPEED], sample.speed_mm_s);
    whole(&mut values[POWER], sample.power_watts);
    whole(&mut values[HEART], sample.heart_bpm);
    let _ = match sample.power_watts {
        Some(v) => write!(values[POWER_ZONE], "Z{}", power_zone(v)),
        None => values[POWER_ZONE].push_str("Z-"),
    };
    let _ = match sample.heart_bpm {
        Some(v) => write!(values[HEART_ZONE], "Z{}", heart_zone(v)),
        None => values[HEART_ZONE].push_str("Z-"),
    };
    whole(
        &mut values[CADENCE],
        sample.cadence_tenths.map(|tenths| (tenths + 5) / 10),
    );
    kilometres(&mut values[DISTANCE], view.totals.distance_mm());
    clock(&mut values[TIME], view.active_secs);
    let _ = match sample.gradient_tenths {
        Some(v) => write!(
            values[GRADE],
            "{}{}.{}%",
            if v < 0 { "-" } else { "" },
            v.unsigned_abs() / 10,
            v.unsigned_abs() % 10
        ),
        None => values[GRADE].push_str("--%"),
    };
}

pub(super) fn pixel(view: &View, x: usize, y: usize) -> u16 {
    if ROWS.contains(&y) || (x == COLUMN && y > ROWS[0]) {
        return GRID;
    }
    let values = &view.values;
    if text(x, y, 178, 89, 1, b"KM/H") {
        return DIM;
    }
    for (cell, value) in &CELLS {
        if let Some(color) = cell.pixel(&values[*value], x, y) {
            return color;
        }
    }
    if text(x, y, 38, 157, 2, values[POWER_ZONE].as_bytes())
        || text(x, y, 146, 157, 2, values[HEART_ZONE].as_bytes())
    {
        return CYAN;
    }
    0
}
