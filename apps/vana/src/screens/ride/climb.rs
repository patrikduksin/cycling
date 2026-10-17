//! The climb page: the grade now with its steepness colour, the climb under
//! way (or altitude, ascent and the last climb) and the last 4 km of
//! elevation as a filled profile coloured by local grade.
//!
//! `frame` precomputes every chart column once per frame; `paint` only looks
//! them up.
use super::{
    CONTENT_BOTTOM, CONTENT_LEFT, CONTENT_RIGHT, CONTENT_TOP, CYAN, Cell, DIM, GRID, Values, WHITE,
    clock, grade, kilometres, whole,
};
use crate::ride::climb::{PROFILE_LEN, ProfileSample, Steepness, Summary};
use crate::screens::workout::View;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

/// Inner margin from the content rectangle's edges.
const MARGIN: usize = 6;
const LEFT: usize = CONTENT_LEFT + MARGIN;
/// One chart column per 20 m profile sample, right-aligned; the rider's
/// marker takes the last four columns of the margin.
const RIGHT: usize = CONTENT_RIGHT - MARGIN;
const CHART_LEFT: usize = RIGHT - PROFILE_LEN;
const _: () = assert!(CHART_LEFT >= CONTENT_LEFT, "the profile needs 200 columns");
const DIVIDER: usize = (CONTENT_LEFT + CONTENT_RIGHT) / 2;
const COLUMNS: [usize; 2] = [LEFT, DIVIDER + MARGIN];
const GRADE_LABEL_TOP: usize = CONTENT_TOP + 4;
const GRADE_TOP: usize = GRADE_LABEL_TOP + 18;
const GRADE_SCALE: usize = 5;
/// "CLIMBING" and "NO CLIMB" are eight scale-2 characters, right-aligned.
const STATUS_LEFT: usize = RIGHT - 8 * 12;
/// The steepness band under the grade.
const BAND: core::ops::Range<usize> =
    GRADE_TOP + 7 * GRADE_SCALE + 3..GRADE_TOP + 7 * GRADE_SCALE + 7;
const FIRST_ROW: usize = BAND.end + 3;
const ROW_PITCH: usize = 33;
const ROWS: usize = 3;
const CHART_TOP: usize = FIRST_ROW + ROWS * ROW_PITCH + 3;
/// The baseline row; the fill ends above it.
const CHART_BOTTOM: usize = CONTENT_BOTTOM - 3;
const CHART_HEIGHT: usize = CHART_BOTTOM - CHART_TOP;
const _: () = assert!(CHART_HEIGHT >= 24 && CHART_HEIGHT < EMPTY as usize);
/// A flat road spans at least 20 m of chart height, so it stays flat.
const MIN_SPAN_DM: i32 = 200;
/// A column with no profile sample yet.
const EMPTY: u8 = u8::MAX;

/// Steepness colours: blue down, then green, yellow, orange, red, crimson.
pub(super) const fn color(steepness: Steepness) -> u16 {
    match steepness {
        Steepness::Descent => 0x3dbf,
        Steepness::Easy => 0x07e0,
        Steepness::Moderate => 0xffe0,
        Steepness::Steep => 0xfd20,
        Steepness::Hard => 0xf800,
        Steepness::Severe => 0xc80f,
    }
}

const GRADE: usize = 0;
/// Values 1..=6 fill the cells, two per row.
const FIRST_CELL: usize = 1;

/// Cell labels; `true` marks a clock, drawn smaller to fit.
const CLIMBING: [(&[u8], bool); 6] = [
    (b"GAIN M", false),
    (b"LENGTH KM", false),
    (b"AVG GRADE", false),
    (b"VAM M/H", false),
    (b"CLIMB TIME", true),
    (b"ALTITUDE M", false),
];
const IDLE: [(&[u8], bool); 6] = [
    (b"ALTITUDE M", false),
    (b"ASCENT M", false),
    (b"LAST GAIN M", false),
    (b"LAST KM", false),
    (b"LAST AVG", false),
    (b"LAST TIME", true),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Layout {
    /// No climb yet: altitude and ascent.
    #[default]
    Idle,
    /// Not climbing: altitude, ascent and the last climb.
    Last,
    /// On a confirmed climb.
    Climbing,
}

/// What the climb page precomputes for one frame.
#[derive(Clone)]
pub struct Frame {
    layout: Layout,
    grade: Option<Steepness>,
    /// Rows below `CHART_TOP` where each column's fill starts, or `EMPTY`.
    tops: [u8; PROFILE_LEN],
    bands: [Steepness; PROFILE_LEN],
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            layout: Layout::Idle,
            grade: None,
            tops: [EMPTY; PROFILE_LEN],
            bands: [Steepness::Easy; PROFILE_LEN],
        }
    }
}

fn metres(value: &mut super::Text<12>, dm: i32) {
    whole(value, Some((dm + 5).div_euclid(10)));
}

/// A climb's gain, length and average grade into the first three cells, and
/// its time into cell `time`.
fn summary(cells: &mut [super::Text<12>], climb: &Summary, time: usize) {
    metres(&mut cells[0], climb.gain_dm as i32);
    kilometres(&mut cells[1], climb.length_mm);
    grade(&mut cells[2], climb.grade_tenths());
    clock(&mut cells[time], climb.time_ms / 1000);
}

pub(super) fn prepare(view: &View, values: &mut Values) {
    grade(
        &mut values[GRADE],
        view.sample.gradient_tenths.map(i32::from),
    );
    let altitude = view.totals.altitude_dm();
    if let Some(climb) = view.climbs.climbing() {
        summary(&mut values[FIRST_CELL..], &climb, 4);
        whole(&mut values[FIRST_CELL + 3], view.climbs.vam_m_h());
        match altitude {
            Some(dm) => metres(&mut values[FIRST_CELL + 5], dm),
            None => whole(&mut values[FIRST_CELL + 5], None::<u8>),
        }
        return;
    }
    match altitude {
        Some(dm) => metres(&mut values[FIRST_CELL], dm),
        None => whole(&mut values[FIRST_CELL], None::<u8>),
    }
    metres(&mut values[FIRST_CELL + 1], view.totals.ascent_dm() as i32);
    if let Some(last) = view.climbs.last_climb() {
        // The last climb fills the cells after altitude and ascent.
        summary(&mut values[FIRST_CELL + 2..], &last, 3);
    }
}

/// Precompute the layout, grade colour and every chart column.
pub(super) fn frame(view: &View) -> Frame {
    let climbs = &view.climbs;
    let mut frame = Frame {
        layout: if climbs.climbing().is_some() {
            Layout::Climbing
        } else if climbs.last_climb().is_some() {
            Layout::Last
        } else {
            Layout::Idle
        },
        grade: view
            .sample
            .gradient_tenths
            .map(|tenths| Steepness::of(i32::from(tenths))),
        ..Frame::default()
    };
    let mut samples = [ProfileSample::default(); PROFILE_LEN];
    let len = climbs.profile(&mut samples);
    let samples = &samples[..len];
    let (Some(low), Some(high)) = (
        samples.iter().map(|s| i32::from(s.altitude_dm)).min(),
        samples.iter().map(|s| i32::from(s.altitude_dm)).max(),
    ) else {
        return frame;
    };
    let range = high - low;
    let span = range.max(MIN_SPAN_DM);
    // A range smaller than the minimum span sits in the lower part, as a road.
    let floor = low - (span - range) / 4;
    // Leave two rows above the highest point for the marker.
    let rows = CHART_HEIGHT as i32 - 3;
    let first = PROFILE_LEN - len;
    let mut band = Steepness::of(samples.first().map_or(0, |s| i32::from(s.grade_tenths)));
    for (index, sample) in samples.iter().enumerate() {
        band = band.after(i32::from(sample.grade_tenths));
        let height = 1 + (i32::from(sample.altitude_dm) - floor) * rows / span;
        frame.tops[first + index] = (CHART_HEIGHT as i32 - height) as u8;
        frame.bands[first + index] = band;
    }
    frame
}

/// Paint one content row: the grade and its band, the cells, or the chart.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    let frame = &view.climb;
    if y >= CHART_TOP {
        paint_chart(frame, row);
        return;
    }
    if BAND.contains(&y) {
        row.span(LEFT..RIGHT, frame.grade.map_or(GRID, color));
        return;
    }
    if y < FIRST_ROW {
        row.text(
            LEFT,
            GRADE_TOP,
            GRADE_SCALE,
            view.values[GRADE].as_bytes(),
            WHITE,
        );
        if frame.layout == Layout::Climbing {
            row.text(STATUS_LEFT, GRADE_LABEL_TOP, 2, b"CLIMBING", CYAN);
        } else {
            row.text(STATUS_LEFT, GRADE_LABEL_TOP, 2, b"NO CLIMB", DIM);
        }
        row.text(LEFT, GRADE_LABEL_TOP, 2, b"GRADE", DIM);
        return;
    }
    let band = (y - FIRST_ROW) / ROW_PITCH;
    let (labels, cells) = match frame.layout {
        Layout::Climbing => (&CLIMBING, 6),
        Layout::Last => (&IDLE, 6),
        Layout::Idle => (&IDLE, 2),
    };
    if band >= ROWS || band * 2 >= cells {
        return;
    }
    let top = FIRST_ROW + band * ROW_PITCH;
    if y == top + ROW_PITCH - 1 {
        row.fill(GRID);
        return;
    }
    for (column, columns) in [0..DIVIDER + 1, DIVIDER + 1..usize::MAX]
        .into_iter()
        .enumerate()
    {
        let index = band * 2 + column;
        let (label, time) = labels[index];
        let cell = Cell {
            left: COLUMNS[column],
            top: top + 2,
            label,
            label_scale: 1,
            value_top: top + if time { 14 } else { 11 },
            value_scale: if time { 2 } else { 3 },
        };
        cell.paint(
            &mut row.within(columns),
            &view.values[FIRST_CELL + index],
            WHITE,
        );
    }
    row.set(DIVIDER, GRID);
}

/// The profile fill, its baseline and the rider's marker.
fn paint_chart(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if y == CHART_BOTTOM {
        row.span(CHART_LEFT..RIGHT, GRID);
    }
    if y >= CHART_BOTTOM {
        return;
    }
    let line = y - CHART_TOP;
    for (column, (&top, &band)) in frame.tops.iter().zip(&frame.bands).enumerate() {
        if top != EMPTY && line >= usize::from(top) {
            row.set(CHART_LEFT + column, color(band));
        }
    }
    // The rider, at the right edge: a white arrow at the newest altitude.
    let newest = frame.tops[PROFILE_LEN - 1];
    if newest != EMPTY {
        let reach = line.abs_diff(usize::from(newest));
        row.span(RIGHT + reach..RIGHT + MARGIN - 2, WHITE);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    let frame = &view.climb;
    if y >= CHART_TOP {
        return chart_pixel(frame, x, y);
    }
    if BAND.contains(&y) {
        return match ((LEFT..RIGHT).contains(&x), frame.grade) {
            (true, Some(steepness)) => color(steepness),
            (true, None) => GRID,
            _ => 0,
        };
    }
    if y < FIRST_ROW {
        if text(x, y, LEFT, GRADE_LABEL_TOP, 2, b"GRADE") {
            return DIM;
        }
        if frame.layout == Layout::Climbing {
            if text(x, y, STATUS_LEFT, GRADE_LABEL_TOP, 2, b"CLIMBING") {
                return CYAN;
            }
        } else if text(x, y, STATUS_LEFT, GRADE_LABEL_TOP, 2, b"NO CLIMB") {
            return DIM;
        }
        let value = view.values[GRADE].as_bytes();
        return if text(x, y, LEFT, GRADE_TOP, GRADE_SCALE, value) {
            WHITE
        } else {
            0
        };
    }
    let row = (y - FIRST_ROW) / ROW_PITCH;
    if row >= ROWS {
        return 0;
    }
    let top = FIRST_ROW + row * ROW_PITCH;
    let (labels, cells) = match frame.layout {
        Layout::Climbing => (&CLIMBING, 6),
        Layout::Last => (&IDLE, 6),
        Layout::Idle => (&IDLE, 2),
    };
    if row * 2 >= cells {
        return 0;
    }
    if y == top + ROW_PITCH - 1 || x == DIVIDER {
        return GRID;
    }
    let column = usize::from(x > DIVIDER);
    let index = row * 2 + column;
    let (label, time) = labels[index];
    let cell = Cell {
        left: COLUMNS[column],
        top: top + 2,
        label,
        label_scale: 1,
        value_top: top + if time { 14 } else { 11 },
        value_scale: if time { 2 } else { 3 },
    };
    cell.pixel(&view.values[FIRST_CELL + index], x, y)
        .unwrap_or(0)
}

#[cfg(test)]
fn chart_pixel(frame: &Frame, x: usize, y: usize) -> u16 {
    if y == CHART_BOTTOM {
        return if (CHART_LEFT..RIGHT).contains(&x) {
            GRID
        } else {
            0
        };
    }
    if y > CHART_BOTTOM || x < CHART_LEFT {
        return 0;
    }
    let row = y - CHART_TOP;
    let column = x - CHART_LEFT;
    if let Some(&top) = frame.tops.get(column) {
        return if top != EMPTY && row >= usize::from(top) {
            color(frame.bands[column])
        } else {
            0
        };
    }
    // The rider, at the right edge: a white arrow at the newest altitude.
    let newest = frame.tops[PROFILE_LEN - 1];
    let reach = x - RIGHT;
    if newest != EMPTY && reach < MARGIN - 2 && row.abs_diff(usize::from(newest)) <= reach {
        return WHITE;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::climb::Climbs;
    use crate::screens::workout::Page;

    /// A view of the climb page after riding `metres` along `altitude_dm(m)`.
    fn ridden(metres: u32, altitude_dm: impl Fn(u32) -> i32) -> View {
        let mut view = View::new(Page::Ride, 0, None);
        view.radar_paired = true;
        view.ride_page = crate::screens::ride::RidePage::Climb;
        let mut climbs = Climbs::default();
        for m in (5..=metres).step_by(5) {
            climbs.add(1000, 5000, Some(altitude_dm(m)));
        }
        view.climbs = climbs;
        view
    }

    #[test]
    fn a_flat_road_stays_low_and_green() {
        let mut view = ridden(2000, |_| 5000);
        view.prepare();
        let frame = &view.climb;
        // 2 km fills the right half; the older half is empty.
        assert_eq!(frame.tops[99], EMPTY);
        let flat = frame.tops[PROFILE_LEN - 1];
        assert!(frame.tops[100..].iter().all(|&top| top == flat));
        // Low in the minimum span: about a quarter of the chart.
        let height = CHART_HEIGHT - usize::from(flat);
        let quarter = CHART_HEIGHT / 4;
        assert!((quarter - 4..=quarter + 4).contains(&height), "{height}");
        assert!(frame.bands[100..].iter().all(|&b| b == Steepness::Easy));
        assert_eq!(
            view.pixel(CHART_LEFT + 150, CHART_BOTTOM - 1),
            color(Steepness::Easy)
        );
        assert_eq!(view.pixel(CHART_LEFT + 50, CHART_BOTTOM - 1), 0);
        assert_eq!(view.pixel(CHART_LEFT + 50, CHART_BOTTOM), GRID);
        // The rider's marker points at the newest altitude.
        assert_eq!(view.pixel(RIGHT + 2, CHART_TOP + usize::from(flat)), WHITE);
    }

    #[test]
    fn a_climb_fills_the_chart_and_colours_by_grade() {
        // 1 km flat, 1 km at 7%, 1 km at 10%, then 400 m down at 5%.
        let mut view = ridden(3400, |m| match m {
            ..1000 => 0,
            1000..2000 => (m as i32 - 1000) * 7 / 10,
            2000..3000 => 700 + (m as i32 - 2000),
            _ => 1700 - (m as i32 - 3000) / 2,
        });
        view.prepare();
        let frame = &view.climb;
        // 170 samples, right-aligned: the flat starts at column 30.
        assert_eq!(frame.tops[29], EMPTY);
        assert_eq!(usize::from(frame.tops[60]), CHART_HEIGHT - 1);
        // Highest at the top of the 10%, two rows below the chart top.
        assert_eq!(frame.tops[180], 2);
        // Each steady stretch is one solid colour.
        assert!(frame.bands[30..75].iter().all(|&b| b == Steepness::Easy));
        assert!(frame.bands[85..125].iter().all(|&b| b == Steepness::Steep));
        assert!(frame.bands[135..175].iter().all(|&b| b == Steepness::Hard));
        assert!(frame.bands[185..].iter().all(|&b| b == Steepness::Descent));
        assert_eq!(
            view.pixel(CHART_LEFT + 150, CHART_BOTTOM - 1),
            color(Steepness::Hard)
        );
        assert_eq!(
            view.pixel(CHART_LEFT + 195, CHART_BOTTOM - 1),
            color(Steepness::Descent)
        );
    }

    #[test]
    fn a_steady_grade_on_a_band_boundary_keeps_one_colour() {
        // 2 km at exactly 6%, sampled with the rounding a real trace has.
        let mut view = ridden(2000, |m| m as i32 * 6 / 10);
        view.prepare();
        let bands = &view.climb.bands[100..];
        assert!(bands.iter().all(|&b| b == bands[0]), "{bands:?}");
    }
}
