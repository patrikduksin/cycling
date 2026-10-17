//! The lap page: the open lap's time, distance and averages, then the last
//! finished lap with how this one compares. A delta carries its sign as well
//! as its colour: green and "+" for faster or harder, red and "-" for slower
//! or easier.
use super::{
    CONTENT_LEFT, CONTENT_RIGHT, CONTENT_TOP, CYAN, Cell, DIM, GREEN, GRID, RED, Values, WHITE,
    kilometres, lap_time, speed, whole,
};
use crate::screens::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

const NUMBER: usize = 0;
const TIME: usize = 1;
const DISTANCE: usize = 2;
const POWER: usize = 3;
const SPEED: usize = 4;
const HEART: usize = 5;
const PREVIOUS: usize = 6;
const PREVIOUS_TIME: usize = 7;
const PREVIOUS_POWER: usize = 8;
const SPEED_DELTA: usize = 9;
const POWER_DELTA: usize = 10;

/// Everything is placed from the content rectangle, so the shared chrome can
/// move without this page changing.
const TOP: usize = CONTENT_TOP;
/// The column divider halves the content width.
const COLUMN: usize = CONTENT_LEFT + (CONTENT_RIGHT - CONTENT_LEFT) / 2 + 1;
const LEFT: usize = CONTENT_LEFT + 10;
const RIGHT: usize = COLUMN + 11;
/// Row dividers below the lap time, the first two rows and the lap averages.
const ROWS: [usize; 3] = [TOP + 74, TOP + 122, TOP + 170];

/// Cells with fixed labels, and the value each shows.
const CELLS: [(Cell, usize); 7] = [
    (cell(LEFT, TOP + 80, b"LAP KM", TOP + 92, 3), DISTANCE),
    (cell(RIGHT, TOP + 80, b"LAP AVG W", TOP + 92, 3), POWER),
    (cell(LEFT, TOP + 128, b"LAP KM/H", TOP + 140, 3), SPEED),
    (cell(RIGHT, TOP + 128, b"LAP AVG BPM", TOP + 140, 3), HEART),
    (
        cell(RIGHT, TOP + 177, b"PREV AVG W", TOP + 189, 2),
        PREVIOUS_POWER,
    ),
    (
        cell(LEFT, TOP + 212, b"KM/H VS PREV", TOP + 224, 3),
        SPEED_DELTA,
    ),
    (
        cell(RIGHT, TOP + 212, b"W VS PREV", TOP + 224, 3),
        POWER_DELTA,
    ),
];

const fn cell(
    left: usize,
    top: usize,
    label: &'static [u8],
    value_top: usize,
    value_scale: usize,
) -> Cell {
    Cell {
        left,
        top,
        label,
        label_scale: 1,
        value_top,
        value_scale,
    }
}

pub(super) fn prepare(view: &View, values: &mut Values) {
    let lap = view.lap;
    let _ = write!(values[NUMBER], "LAP {}", lap.number);
    lap_time(&mut values[TIME], lap.time_ms / 1000);
    kilometres(&mut values[DISTANCE], lap.distance_mm);
    whole(&mut values[POWER], lap.avg_power());
    speed(&mut values[SPEED], lap.avg_speed_mm_s());
    whole(&mut values[HEART], lap.avg_heart_bpm());
    let Some(previous) = view.previous_lap else {
        let _ = values[PREVIOUS].push_str("PREV LAP");
        for index in [PREVIOUS_TIME, PREVIOUS_POWER, SPEED_DELTA, POWER_DELTA] {
            let _ = values[index].push_str("--");
        }
        return;
    };
    let _ = write!(values[PREVIOUS], "PREV LAP {}", previous.number);
    lap_time(&mut values[PREVIOUS_TIME], previous.time_ms / 1000);
    whole(&mut values[PREVIOUS_POWER], previous.avg_power());
    // Tenths of km/h, truncated per side as the speed readings are.
    let tenths = |mm_s: u32| i64::from(mm_s) * 36 / 1000;
    match (lap.avg_speed_mm_s(), previous.avg_speed_mm_s()) {
        (Some(now), Some(before)) => {
            let delta = tenths(now) - tenths(before);
            signed(&mut values[SPEED_DELTA], delta, |value, magnitude| {
                write!(value, "{}.{}", magnitude / 10, magnitude % 10)
            });
        }
        _ => {
            let _ = values[SPEED_DELTA].push_str("--");
        }
    }
    match (lap.avg_power(), previous.avg_power()) {
        (Some(now), Some(before)) => {
            let delta = i64::from(now) - i64::from(before);
            signed(&mut values[POWER_DELTA], delta, |value, magnitude| {
                write!(value, "{magnitude}")
            });
        }
        _ => {
            let _ = values[POWER_DELTA].push_str("--");
        }
    }
}

/// `delta` with a leading "+" or "-", and no sign for zero.
fn signed(
    value: &mut Text<12>,
    delta: i64,
    magnitude: impl FnOnce(&mut Text<12>, u64) -> core::fmt::Result,
) {
    let _ = value.push_str(match delta {
        1.. => "+",
        ..0 => "-",
        0 => "",
    });
    let _ = magnitude(value, delta.unsigned_abs());
}

/// Green for a gain, red for a loss, white for no change or no reading.
fn delta_color(value: &Text<12>) -> u16 {
    match value.as_bytes().first() {
        Some(b'+') => GREEN,
        Some(b'-') if value.as_bytes() != b"--" => RED,
        _ => WHITE,
    }
}

/// Paint one content row; earlier cells in `CELLS` cover later ones.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    let values = &view.values;
    if y < ROWS[0] {
        // Under an hour the time takes five characters at scale 6.
        let time = values[TIME].as_bytes();
        let (scale, top) = if time.len() <= 5 {
            (6, TOP + 24)
        } else {
            (4, TOP + 32)
        };
        row.text(LEFT, top, scale, time, WHITE);
        row.text(RIGHT, TOP + 4, 2, values[NUMBER].as_bytes(), CYAN);
        row.text(LEFT, TOP + 8, 1, b"LAP TIME", DIM);
    } else {
        for (cell, value) in CELLS.iter().rev() {
            let color = if matches!(*value, SPEED_DELTA | POWER_DELTA) {
                delta_color(&values[*value])
            } else {
                WHITE
            };
            cell.paint(row, &values[*value], color);
        }
        row.text(LEFT, TOP + 189, 2, values[PREVIOUS_TIME].as_bytes(), WHITE);
        row.text(LEFT, TOP + 177, 1, values[PREVIOUS].as_bytes(), DIM);
    }
    if ROWS.contains(&y) {
        row.fill(GRID);
    } else if y > ROWS[0] {
        row.set(COLUMN, GRID);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    if ROWS.contains(&y) || (x == COLUMN && y > ROWS[0]) {
        return GRID;
    }
    let values = &view.values;
    if y < ROWS[0] {
        if text(x, y, LEFT, TOP + 8, 1, b"LAP TIME") {
            return DIM;
        }
        if text(x, y, RIGHT, TOP + 4, 2, values[NUMBER].as_bytes()) {
            return CYAN;
        }
        // Under an hour the time takes five characters at scale 6.
        let time = values[TIME].as_bytes();
        let (scale, top) = if time.len() <= 5 {
            (6, TOP + 24)
        } else {
            (4, TOP + 32)
        };
        return if text(x, y, LEFT, top, scale, time) {
            WHITE
        } else {
            0
        };
    }
    if text(x, y, LEFT, TOP + 177, 1, values[PREVIOUS].as_bytes()) {
        return DIM;
    }
    if text(x, y, LEFT, TOP + 189, 2, values[PREVIOUS_TIME].as_bytes()) {
        return WHITE;
    }
    for (cell, value) in &CELLS {
        if let Some(color) = cell.pixel(&values[*value], x, y) {
            return if color == WHITE && matches!(*value, SPEED_DELTA | POWER_DELTA) {
                delta_color(&values[*value])
            } else {
                color
            };
        }
    }
    0
}
