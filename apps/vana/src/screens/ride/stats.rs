//! The ride totals page: what this ride has added up so far, kept after stop,
//! above bars of the time spent in each power and heart rate zone.
use super::zones::{BAR_WIDTH, bar_pixel};
use super::{Cell, DIM, GRID, Values, clock, kilometres, speed, whole};
use crate::screens::workout::View;
use firmware_shell::rendering::text::text;

const COLUMNS: [usize; 2] = [34, 141];
const DIVIDER: usize = 135;
const FIRST_ROW: usize = 33;
const ROW_PITCH: usize = 34;
const ROWS: usize = 6;
/// Time-in-zone bars, below the totals: label, then the bar.
const ZONES_TOP: usize = FIRST_ROW + ROWS * ROW_PITCH;
const BAR_LEFT: usize = 58;
const POWER_BAR: usize = ZONES_TOP + 3;
const HEART_BAR: usize = ZONES_TOP + 12;
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

pub(super) fn pixel(view: &View, x: usize, y: usize) -> u16 {
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
        value_top: top + if index == TIME { 15 } else { 12 },
        value_scale: if index == TIME { 2 } else { 3 },
    };
    cell.pixel(&view.values[index], x, y).unwrap_or(0)
}

fn zones_pixel(view: &View, x: usize, y: usize) -> u16 {
    let marks = &view.marks;
    for (top, label) in [(POWER_BAR, b"PWR".as_slice()), (HEART_BAR, b"HR")] {
        if !(top..top + BAR_HEIGHT).contains(&y) {
            continue;
        }
        if text(x, y, COLUMNS[0], top, 1, label) {
            return DIM;
        }
        if (BAR_LEFT..BAR_LEFT + BAR_WIDTH).contains(&x) {
            return if top == POWER_BAR {
                bar_pixel(marks.power_time, BAR_LEFT, x)
            } else {
                bar_pixel(marks.heart_time, BAR_LEFT, x)
            };
        }
    }
    0
}
