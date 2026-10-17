//! The main ride page: live speed, power and heart rate with their zones,
//! then cadence, distance, active time and grade.
#[cfg(test)]
use super::zones::gauge_pixel;
use super::zones::{GAUGE_WIDTH, Mark, paint_gauge};
use super::{Cell, DIM, GRID, Values, WHITE, clock, grade, kilometres, speed, whole};
#[cfg(test)]
use crate::screens::rounded;
use crate::screens::rounded_span;
use crate::screens::workout::View;
use core::fmt::Write;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
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

/// Zone chips: the Z label on a band of the zone colour, then a gauge of the
/// reading's position within its zone. Power left, heart rate right.
const CHIP_TOP: usize = 154;
const CHIP_BOTTOM: usize = 175;
const CHIP_WIDTH: usize = 34;
const GAUGE_TOP: usize = 161;
const GAUGE_BOTTOM: usize = 168;
type MarkOf = fn(&View) -> Option<Mark>;
const ZONES: [(usize, usize, MarkOf); 2] = [
    (34, POWER_ZONE, |view| view.marks.power),
    (142, HEART_ZONE, |view| view.marks.heart),
];

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
    let profile = view.profile;
    let _ = match sample.power_watts {
        Some(v) => write!(values[POWER_ZONE], "Z{}", profile.power_zone(v)),
        None => values[POWER_ZONE].push_str("Z-"),
    };
    let _ = match sample.heart_bpm {
        Some(v) => write!(values[HEART_ZONE], "Z{}", profile.heart_zone(v)),
        None => values[HEART_ZONE].push_str("Z-"),
    };
    whole(
        &mut values[CADENCE],
        sample.cadence_tenths.map(|tenths| (tenths + 5) / 10),
    );
    kilometres(&mut values[DISTANCE], view.totals.distance_mm());
    clock(&mut values[TIME], view.active_secs);
    grade(&mut values[GRADE], sample.gradient_tenths.map(i32::from));
}

/// Paint one content row: the cells of its band, or the zone chips, then the
/// grid on top.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    let values = &view.values;
    let band = ROWS
        .iter()
        .position(|&bottom| y < bottom)
        .unwrap_or(ROWS.len());
    if band == 0 {
        // Speed spans the first band.
        let (cell, value) = &CELLS[0];
        cell.paint(row, &values[*value], WHITE);
        row.text(178, 89, 1, b"KM/H", DIM);
    } else {
        // Each later band has a left and a right cell, split after the column.
        for (right, columns) in [(false, 0..COLUMN + 1), (true, COLUMN + 1..usize::MAX)] {
            let mut half = row.within(columns);
            if band == 1 && (CHIP_TOP..CHIP_BOTTOM).contains(&y) {
                let (left, zone, mark) = ZONES[usize::from(right)];
                paint_zone(&mut half, mark(view), left, values[zone].as_bytes());
            } else {
                let (cell, value) = &CELLS[2 * band - 1 + usize::from(right)];
                cell.paint(&mut half, &values[*value], WHITE);
            }
        }
    }
    if ROWS.contains(&y) {
        row.fill(GRID);
    } else if y > ROWS[0] {
        row.set(COLUMN, GRID);
    }
}

/// The zone chip and gauge starting at `left`.
fn paint_zone(row: &mut Row, mark: Option<Mark>, left: usize, label: &[u8]) {
    let y = row.y();
    let Some(mark) = mark else {
        row.text(left + 4, CHIP_TOP + 3, 2, label, DIM);
        return;
    };
    // A pill of the zone colour behind the black zone number.
    let pill = rounded_span(y, (left, CHIP_TOP, left + CHIP_WIDTH, CHIP_BOTTOM), 7);
    row.span(pill.clone(), mark.colour);
    row.within(pill).text(left + 4, CHIP_TOP + 3, 2, label, 0);
    let gauge = left + CHIP_WIDTH + 6;
    let bounds = (gauge, GAUGE_TOP, gauge + GAUGE_WIDTH, GAUGE_BOTTOM);
    paint_gauge(row, mark, gauge, rounded_span(y, bounds, 3));
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    if ROWS.contains(&y) || (x == COLUMN && y > ROWS[0]) {
        return GRID;
    }
    let values = &view.values;
    let row = ROWS
        .iter()
        .position(|&bottom| y < bottom)
        .unwrap_or(ROWS.len());
    let right = x > COLUMN;
    if row == 0 && text(x, y, 178, 89, 1, b"KM/H") {
        return DIM;
    }
    if row == 1 && (CHIP_TOP..CHIP_BOTTOM).contains(&y) {
        let (left, zone, mark) = ZONES[usize::from(right)];
        return zone_pixel(mark(view), left, values[zone].as_bytes(), x, y).unwrap_or(0);
    }
    // Speed spans the first row; then each row has a left and a right cell.
    let index = if row == 0 {
        0
    } else {
        2 * row - 1 + usize::from(right)
    };
    let (cell, value) = &CELLS[index];
    cell.pixel(&values[*value], x, y).unwrap_or(0)
}

/// The zone chip and gauge starting at `left`, or none of their pixels.
#[cfg(test)]
fn zone_pixel(mark: Option<Mark>, left: usize, label: &[u8], x: usize, y: usize) -> Option<u16> {
    let ink = text(x, y, left + 4, CHIP_TOP + 3, 2, label);
    let Some(mark) = mark else {
        return ink.then_some(DIM);
    };
    if (left..left + CHIP_WIDTH).contains(&x) {
        // A pill of the zone colour behind the black zone number.
        let pill = rounded(x, y, (left, CHIP_TOP, left + CHIP_WIDTH, CHIP_BOTTOM), 7);
        return pill.then_some(if ink { 0 } else { mark.colour });
    }
    let gauge = left + CHIP_WIDTH + 6;
    let bounds = (gauge, GAUGE_TOP, gauge + GAUGE_WIDTH, GAUGE_BOTTOM);
    rounded(x, y, bounds, 3).then(|| gauge_pixel(mark, gauge, x))
}
