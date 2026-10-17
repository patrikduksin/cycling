//! The ride totals page: what this ride has added up so far, kept after stop,
//! above bars of the time spent in each power and heart rate zone.
#[cfg(test)]
use super::zones::bar_pixel;
use super::zones::{BAR_WIDTH, paint_bar};
use super::{BODY_BOTTOM, Cell, DIM, GRID, Values, WHITE, clock, kilometres, speed, whole};
#[cfg(test)]
use crate::screens::rounded;
use crate::screens::rounded_span;
use crate::screens::workout::View;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

const COLUMNS: [usize; 2] = [34, 141];
const DIVIDER: usize = 135;
const FIRST_ROW: usize = 33;
const ROW_PITCH: usize = 37;
const ROWS: usize = 6;
/// Time-in-zone bars, below the totals: label, then the bar.
const ZONES_TOP: usize = FIRST_ROW + ROWS * ROW_PITCH;
const BAR_LEFT: usize = 58;
const POWER_BAR: usize = HEART_BAR - 12;
const HEART_BAR: usize = BODY_BOTTOM - 4 - BAR_HEIGHT;
const BAR_HEIGHT: usize = 8;

/// Labels in reading order, two per row; each reading has its own unit.
const LABELS: [&[u8]; 12] = [
    b"DISTANCE KM",
    b"MOVING TIME",
    b"AVG KM/H",
    b"MAX KM/H",
    b"AVG WATTS",
    b"MAX WATTS",
    b"NP WATTS",
    b"WORK KJ",
    b"AVG BPM",
    b"MAX BPM",
    b"AVG RPM",
    b"ASCENT M",
];
/// Moving time needs eight characters; everything else fits at scale 3.
const TIME: usize = 1;

pub(super) fn prepare(view: &View, values: &mut Values) {
    let totals = &view.totals;
    let [
        distance,
        moving,
        avg_speed,
        max_speed,
        avg_power,
        max_power,
        np,
        work,
        avg_heart,
        max_heart,
        cadence,
        ascent,
    ] = values;
    kilometres(distance, totals.distance_mm());
    clock(moving, totals.moving_ms() / 1000);
    speed(avg_speed, totals.avg_speed_mm_s());
    speed(max_speed, totals.max_speed_mm_s());
    whole(avg_power, totals.avg_power());
    whole(max_power, totals.max_power());
    whole(np, totals.normalized_power());
    whole(work, totals.work_kj());
    whole(avg_heart, totals.avg_heart_bpm());
    whole(max_heart, totals.max_heart_bpm());
    whole(cadence, totals.avg_cadence_rpm());
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));
}

/// The cell of `index` in the row band starting at `top`.
fn cell(index: usize, top: usize) -> Option<Cell> {
    let column = index % 2;
    Some(Cell {
        left: COLUMNS[column],
        top: top + 3,
        label: LABELS.get(index)?,
        label_scale: 1,
        value_top: top + if index == TIME { 16 } else { 13 },
        value_scale: if index == TIME { 2 } else { 3 },
    })
}

/// Paint one content row: two cells per band and rules between them, then
/// the time-in-zone bars.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    if y >= ZONES_TOP {
        paint_zones(view, row);
        return;
    }
    if let Some(band) = y.checked_sub(FIRST_ROW).map(|y| y / ROW_PITCH) {
        let top = FIRST_ROW + band * ROW_PITCH;
        if y == top + ROW_PITCH - 1 {
            row.fill(GRID);
        } else {
            for (column, columns) in [0..DIVIDER + 1, DIVIDER + 1..usize::MAX]
                .into_iter()
                .enumerate()
            {
                let index = band * 2 + column;
                if let Some(cell) = cell(index, top) {
                    cell.paint(&mut row.within(columns), &view.values[index], WHITE);
                }
            }
        }
    }
    row.set(DIVIDER, GRID);
}

fn paint_zones(view: &View, row: &mut Row) {
    let y = row.y();
    let marks = &view.marks;
    for (top, label) in [(POWER_BAR, b"PWR".as_slice()), (HEART_BAR, b"HR")] {
        if !(top..top + BAR_HEIGHT).contains(&y) {
            continue;
        }
        let bounds = (BAR_LEFT, top, BAR_LEFT + BAR_WIDTH, top + BAR_HEIGHT);
        let shape = rounded_span(y, bounds, BAR_HEIGHT / 2);
        if top == POWER_BAR {
            paint_bar(row, marks.power_time, BAR_LEFT, shape);
        } else {
            paint_bar(row, marks.heart_time, BAR_LEFT, shape);
        }
        row.text(COLUMNS[0], top, 1, label, DIM);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    if y >= ZONES_TOP {
        return zones_pixel(view, x, y);
    }
    if x == DIVIDER {
        return GRID;
    }
    let Some(row) = y.checked_sub(FIRST_ROW).map(|y| y / ROW_PITCH) else {
        return 0;
    };
    let top = FIRST_ROW + row * ROW_PITCH;
    if y == top + ROW_PITCH - 1 {
        return GRID;
    }
    let column = usize::from(x > DIVIDER);
    let index = row * 2 + column;
    let Some(label) = LABELS.get(index) else {
        return 0;
    };
    let cell = Cell {
        left: COLUMNS[column],
        top: top + 3,
        label,
        label_scale: 1,
        value_top: top + if index == TIME { 16 } else { 13 },
        value_scale: if index == TIME { 2 } else { 3 },
    };
    cell.pixel(&view.values[index], x, y).unwrap_or(0)
}

#[cfg(test)]
fn zones_pixel(view: &View, x: usize, y: usize) -> u16 {
    let marks = &view.marks;
    for (top, label) in [(POWER_BAR, b"PWR".as_slice()), (HEART_BAR, b"HR")] {
        if !(top..top + BAR_HEIGHT).contains(&y) {
            continue;
        }
        if text(x, y, COLUMNS[0], top, 1, label) {
            return DIM;
        }
        let bounds = (BAR_LEFT, top, BAR_LEFT + BAR_WIDTH, top + BAR_HEIGHT);
        if rounded(x, y, bounds, BAR_HEIGHT / 2) {
            return if top == POWER_BAR {
                bar_pixel(marks.power_time, BAR_LEFT, x)
            } else {
                bar_pixel(marks.heart_time, BAR_LEFT, x)
            };
        }
    }
    0
}
