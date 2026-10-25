//! The climb page: the grade now in its steepness colour, the climb under
//! way (or altitude, ascent and the last climb) and the last 4 km of
//! elevation as a solid profile coloured by local grade, with a white disc
//! for the rider.
//!
//! With a terrain pack and a confident look-ahead (`ride::climb_ahead`) the
//! page looks forward instead:
//!
//! - **Climb ahead**, before a climb starts: the distance to its foot as the
//!   hero, its category, length, average grade and gain, and the road from
//!   the rider to its top, the climb in full steepness colour and the road
//!   before it dimmed.
//! - **To the top**, on a climb: the distance left as the hero, the gain
//!   left, the grade now and over the next 500 m, and the climb from its
//!   foot to its top. What is ahead is the subject, in full colour; the part
//!   already climbed (from the barometer) is dimmed; the rider's disc sits
//!   where they meet.
//!
//! Off the map, unmatched or unsure, it is the map-free page above.
//!
//! `frame` precomputes every chart column once per frame; `paint_chart` only
//! looks them up, and the canvas paints the text and the rider over it.
use super::{BODY, CELL_PITCH, GRID, LEFT, RIGHT, cell, grade, kilometres, lap_time, whole};
use crate::ride::climb::{Category, PROFILE_LEN, ProfileSample, STEP_MM, Steepness, Summary};
use crate::ride::climb_ahead::ClimbAhead;
use crate::screens::canvas::{Canvas, Shape};
#[cfg(test)]
use crate::screens::chrome::BLACK;
use crate::screens::chrome::{
    ACCENT, Align, BLACK as DIM_UNDER, CAPTION, GRADE, HERO, LARGE, Rect, Row, SECONDARY, VALUE,
    WHITE, ZONES, blend,
};
use crate::screens::workout::{Text, View};
use core::fmt::Write;

/// "GRADE" and the climb state share this baseline.
const HEAD: usize = 40;
/// The grade's baseline, under its caption.
const GRADE_BASE: usize = HEAD + 4 + HERO.cap as usize;
/// The steepness band under the grade.
const BAND_TOP: usize = GRADE_BASE + 6;
const BAND_HEIGHT: usize = 5;
/// The first band of cells; up to three follow.
const FIRST: usize = BAND_TOP + BAND_HEIGHT + 4;
const ROWS: usize = 3;
/// One chart column per 20 m profile sample, ending at the body's right
/// margin; the rider's disc sits on the newest.
const CHART_RIGHT: usize = BODY.right - 6;
const CHART_LEFT: usize = CHART_RIGHT - PROFILE_LEN;
const _: () = assert!(CHART_LEFT >= BODY.left, "the profile needs 200 columns");
const CHART_TOP: usize = FIRST + ROWS * CELL_PITCH + 6;
/// The baseline row; the fill ends above it. The chart runs into the toast
/// band, where a toast covers it for a moment.
const CHART_BOTTOM: usize = BODY.bottom - 8;
const CHART_HEIGHT: usize = CHART_BOTTOM - CHART_TOP;
const _: () = assert!(CHART_HEIGHT >= 24 && CHART_HEIGHT < EMPTY as usize);
/// The map views have two rows of cells, so their profile takes the third.
const MAP_TOP: usize = FIRST + (ROWS - 1) * CELL_PITCH + 6;
const MAP_HEIGHT: usize = CHART_BOTTOM - MAP_TOP;
const _: () = assert!(MAP_HEIGHT < EMPTY as usize);
/// A flat road spans at least 20 m of chart height, so it stays flat.
pub(super) const MIN_SPAN_DM: i32 = 200;
/// A column with no profile sample yet.
const EMPTY: u8 = u8::MAX;
/// The rider's disc: 6 px across, in quarter pixels.
const RIDER_RADIUS: i32 = 12;

/// Steepness colours, from the zone ramp: blue down, then green, yellow,
/// orange, red and purple.
pub(super) const fn color(steepness: Steepness) -> u16 {
    ZONES[match steepness {
        Steepness::Descent => 1,
        Steepness::Easy => 2,
        Steepness::Moderate => 3,
        Steepness::Steep => 4,
        Steepness::Hard => 5,
        Steepness::Severe => 6,
    }]
}

/// Cell captions and value colours, two per band.
const CLIMBING: [(&[u8], u16); 6] = [
    (b"GAIN M", GRADE),
    (b"LENGTH KM", WHITE),
    (b"AVG GRADE", GRADE),
    (b"VAM M/H", GRADE),
    (b"CLIMB TIME", WHITE),
    (b"ALTITUDE M", GRADE),
];
const IDLE: [(&[u8], u16); 6] = [
    (b"ALTITUDE M", GRADE),
    (b"ASCENT M", GRADE),
    (b"LAST GAIN", GRADE),
    (b"LAST KM", WHITE),
    (b"LAST AVG", GRADE),
    (b"LAST TIME", WHITE),
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
    /// The map shows a climb ahead.
    Ahead,
    /// The map shows the rest of the climb under way.
    ToTop,
}

/// What the climb page precomputes for one frame.
#[derive(Clone)]
pub struct Frame {
    layout: Layout,
    /// The chart's first row: `CHART_TOP`, or `MAP_TOP` for the map views.
    top: usize,
    grade: Option<Steepness>,
    /// Rows below `top` where each column's fill starts, or `EMPTY`.
    tops: [u8; PROFILE_LEN],
    bands: [Steepness; PROFILE_LEN],
    /// Columns drawn at 25%: road outside the climb, or already climbed.
    dim: [bool; PROFILE_LEN],
    /// The rider's column, or `EMPTY`.
    rider: u8,
    /// The climb the map view is about.
    climb: Option<ClimbAhead>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            layout: Layout::Idle,
            top: CHART_TOP,
            grade: None,
            tops: [EMPTY; PROFILE_LEN],
            bands: [Steepness::Easy; PROFILE_LEN],
            dim: [false; PROFILE_LEN],
            rider: EMPTY,
            climb: None,
        }
    }
}

fn metres(value: &mut Text<12>, dm: i32) {
    whole(value, Some((dm + 5).div_euclid(10)));
}

fn altitude(value: &mut Text<12>, dm: Option<i32>) {
    match dm {
        Some(dm) => metres(value, dm),
        None => whole(value, None::<u8>),
    }
}

/// A climb's gain, length and average grade into the first three cells, and
/// its time into cell `time`.
fn summary(cells: &mut [Text<12>], climb: &Summary, time: usize) {
    metres(&mut cells[0], climb.gain_dm as i32);
    kilometres(&mut cells[1], climb.length_mm);
    grade(&mut cells[2], climb.grade_tenths());
    lap_time(&mut cells[time], climb.time_ms / 1000);
}

/// The text, the steepness band and the rider; `frame` must be prepared.
pub(super) fn prepare(view: &View) -> Canvas {
    let frame = &view.climb;
    if matches!(frame.layout, Layout::Ahead | Layout::ToTop) {
        return prepare_map(view);
    }
    let mut canvas = Canvas::default();
    let width = RIGHT - LEFT;
    let (state, state_color) = if frame.layout == Layout::Climbing {
        (b"CLIMBING".as_slice(), ACCENT)
    } else {
        (b"NO CLIMB".as_slice(), crate::screens::chrome::SECONDARY)
    };
    let state = canvas.fit(
        &[&crate::screens::chrome::CAPTION],
        state,
        width,
        RIGHT,
        HEAD,
        Align::Right,
        state_color,
    );
    canvas.caption(b"GRADE", state.left - LEFT, LEFT, HEAD, Align::Left);
    if !view.forecast.map {
        // No terrain pack (or no memory for it): only the barometer speaks.
        canvas.fit(
            &[&crate::screens::chrome::CAPTION],
            b"NO MAP",
            RIGHT - LEFT,
            LEFT,
            CHART_TOP + 12,
            Align::Left,
            crate::screens::chrome::SECONDARY,
        );
    }
    let mut now = Text::<12>::new();
    grade(&mut now, view.sample.gradient_tenths.map(i32::from));
    let faces = [&HERO, &LARGE, &VALUE];
    canvas.fit(
        &faces,
        now.as_bytes(),
        width,
        LEFT,
        GRADE_BASE,
        Align::Left,
        GRADE,
    );
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: LEFT,
            top: BAND_TOP,
            right: RIGHT,
            bottom: BAND_TOP + BAND_HEIGHT,
        },
        radius: 2,
        color: frame.grade.map_or(GRID, color),
    });

    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let altitude_dm = view.totals.altitude_dm();
    let (labels, cells) = if let Some(climb) = view.climbs.climbing() {
        summary(&mut values, &climb, 4);
        whole(&mut values[3], view.climbs.vam_m_h());
        altitude(&mut values[5], altitude_dm);
        (&CLIMBING, 6)
    } else {
        altitude(&mut values[0], altitude_dm);
        metres(&mut values[1], view.totals.ascent_dm() as i32);
        match view.climbs.last_climb() {
            // The last climb fills the cells after altitude and ascent.
            Some(last) => {
                summary(&mut values[2..], &last, 3);
                (&IDLE, 6)
            }
            None => (&IDLE, 2),
        }
    };
    for (index, ((caption, color), value)) in labels.iter().zip(&values).take(cells).enumerate() {
        let top = FIRST + index / 2 * CELL_PITCH;
        cell(
            &mut canvas,
            index % 2,
            top,
            caption,
            value.as_bytes(),
            *color,
        );
    }

    rider(&mut canvas, frame);
    canvas
}

/// The rider's disc on the chart.
fn rider(canvas: &mut Canvas, frame: &Frame) {
    if frame.rider == EMPTY {
        return;
    }
    let column = usize::from(frame.rider);
    let top = frame.tops[column];
    if top != EMPTY {
        canvas.shape(Shape::Disc {
            cx: 4 * (CHART_LEFT + column) as i32 + 2,
            cy: 4 * (frame.top + usize::from(top)) as i32 + 2,
            radius: RIDER_RADIUS,
            color: WHITE,
        });
    }
}

/// Kilometres with one decimal: `0.4`, `12.3`.
fn tenths_km(value: &mut Text<12>, mm: u64) {
    let tenths = (mm + 50_000) / 100_000;
    let _ = write!(value, "{}.{}", tenths / 10, tenths % 10);
}

/// The hero reading with its unit after it, on the grade's baseline.
fn hero(canvas: &mut Canvas, value: &[u8], unit: &[u8], color: u16) {
    let unit_width = CAPTION.width(unit) + 2;
    let width = RIGHT - LEFT - unit_width - 6;
    let run = canvas.fit(
        &[&HERO, &LARGE, &VALUE],
        value,
        width,
        LEFT,
        GRADE_BASE,
        Align::Left,
        color,
    );
    canvas.caption(unit, unit_width, run.right() + 6, GRADE_BASE, Align::Left);
}

/// Cell captions and value colours of the map views, two per band.
const AHEAD: [(&[u8], u16); 4] = [
    (b"LENGTH KM", WHITE),
    (b"AVG GRADE", GRADE),
    (b"GAIN M", GRADE),
    (b"NOW", GRADE),
];
const TO_TOP: [(&[u8], u16); 4] = [
    (b"TO GO M", GRADE),
    (b"NOW", GRADE),
    (b"NEXT 500 M", GRADE),
    (b"VAM M/H", GRADE),
];

/// A climb's category, in the colour of its average steepness.
fn category(canvas: &mut Canvas, climb: &Summary, left: usize) {
    let name = Category::of(climb.gain_dm).name();
    let color = color(Steepness::of(climb.grade_tenths().unwrap_or(0)));
    canvas.fit(
        &[&CAPTION],
        name,
        RIGHT - left,
        RIGHT,
        HEAD,
        Align::Right,
        color,
    );
}

/// The climb ahead, or the rest of the climb under way, from the map.
fn prepare_map(view: &View) -> Canvas {
    let frame = &view.climb;
    let mut canvas = Canvas::default();
    let Some(climb) = frame.climb else {
        return canvas;
    };
    let ahead = frame.layout == Layout::Ahead;
    let title: &[u8] = if ahead { b"CLIMB AHEAD" } else { b"TO THE TOP" };
    let title = canvas.fit(
        &[&CAPTION],
        title,
        RIGHT - LEFT,
        LEFT,
        HEAD,
        Align::Left,
        ACCENT,
    );
    category(&mut canvas, &climb.summary, title.right() + 8);
    let mut value = Text::<12>::new();
    let distance = if ahead {
        climb.summary.start_mm
    } else {
        climb.end_mm()
    };
    tenths_km(&mut value, distance);
    hero(&mut canvas, value.as_bytes(), b"KM", WHITE);
    let band = if ahead {
        Steepness::of(climb.summary.grade_tenths().unwrap_or(0))
    } else {
        frame.grade.unwrap_or_else(|| view.forecast.steepness(0))
    };
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: LEFT,
            top: BAND_TOP,
            right: RIGHT,
            bottom: BAND_TOP + BAND_HEIGHT,
        },
        radius: 2,
        color: color(band),
    });
    let now = view
        .sample
        .gradient_tenths
        .map(i32::from)
        .or_else(|| view.forecast.grade(0, 100_000));
    let mut values: [Text<12>; 4] = core::array::from_fn(|_| Text::new());
    let labels = if ahead {
        kilometres(&mut values[0], climb.summary.length_mm);
        grade(&mut values[1], climb.summary.grade_tenths());
        metres(&mut values[2], climb.summary.gain_dm as i32);
        grade(&mut values[3], now);
        &AHEAD
    } else {
        // Gain left: from the rider (0) to the top.
        let top = climb.summary.start_altitude_dm + climb.summary.gain_dm as i32;
        metres(&mut values[0], top.max(0));
        grade(&mut values[1], now);
        grade(&mut values[2], view.forecast.grade(0, 500_000));
        whole(&mut values[3], view.climbs.vam_m_h());
        &TO_TOP
    };
    for (index, ((caption, color), value)) in labels.iter().zip(&values).enumerate() {
        let top = FIRST + index / 2 * CELL_PITCH;
        cell(
            &mut canvas,
            index % 2,
            top,
            caption,
            value.as_bytes(),
            *color,
        );
    }
    if climb.open {
        // The map ends before the top: say so where the chart ends.
        canvas.fit(
            &[&CAPTION],
            b"AND MORE",
            RIGHT - LEFT,
            RIGHT,
            MAP_TOP + 12,
            Align::Right,
            SECONDARY,
        );
    }
    rider(&mut canvas, frame);
    canvas
}

/// The map's frame: the climb ahead or under way, when the forecast has one.
fn map_frame(view: &View) -> Option<Frame> {
    let forecast = &view.forecast;
    let (layout, climb) = match (forecast.current(), forecast.upcoming()) {
        (Some(current), _) => (Layout::ToTop, current),
        (None, Some(next)) => (Layout::Ahead, next),
        (None, None) => return None,
    };
    let mut frame = Frame {
        top: MAP_TOP,
        layout,
        grade: view
            .sample
            .gradient_tenths
            .map(|tenths| Steepness::of(i32::from(tenths))),
        climb: Some(climb),
        ..Frame::default()
    };
    let step = STEP_MM;
    let ahead_len = forecast.len;
    // Past: the barometric profile of the climb so far, while on one.
    let mut past = [ProfileSample::default(); PROFILE_LEN];
    let mut past_len = 0;
    let mut behind_mm = 0;
    if layout == Layout::ToTop
        && let Some(so_far) = view.climbs.climbing()
    {
        past_len = view.climbs.profile(&mut past);
        behind_mm = so_far
            .length_mm
            .min(past_len.saturating_sub(1) as u64 * step);
    }
    // Ahead: to the top, plus a little road after it for context.
    let end = climb.end_mm();
    let after = (climb.summary.length_mm / 10).max(200_000);
    let ahead_mm = (end + after)
        .min(ahead_len.saturating_sub(1) as u64 * step)
        .max(step);
    let total = behind_mm + ahead_mm;
    let columns = PROFILE_LEN as u64;
    let rider = (behind_mm * (columns - 1) / total) as usize;
    frame.rider = rider as u8;
    // Elevation and steepness at a column, relative to the rider.
    let at = |column: usize| -> (i32, Steepness, bool) {
        let distance = column as u64 * total / (columns - 1);
        if distance < behind_mm {
            let back = ((behind_mm - distance) / step) as usize;
            let newest = past_len - 1;
            let sample = past[newest.saturating_sub(back)];
            let z = i32::from(sample.altitude_dm) - i32::from(past[newest].altitude_dm);
            (z, Steepness::of(i32::from(sample.grade_tenths)), true)
        } else {
            let ahead = distance - behind_mm;
            let index = ((ahead / step) as usize).min(ahead_len - 1);
            let inside = ahead >= climb.summary.start_mm && ahead <= end;
            (
                i32::from(forecast.samples[index]),
                forecast.steepness(index),
                layout == Layout::Ahead && !inside || ahead > end,
            )
        }
    };
    let mut low = i32::MAX;
    let mut high = i32::MIN;
    let mut columns_z = [0i32; PROFILE_LEN];
    for (column, slot) in columns_z.iter_mut().enumerate() {
        // Steepness is already a 200 m grade, so a steady grade keeps one colour.
        let (z, steepness, dim) = at(column);
        *slot = z;
        low = low.min(z);
        high = high.max(z);
        frame.bands[column] = steepness;
        frame.dim[column] = dim;
    }
    let range = high - low;
    let span = range.max(MIN_SPAN_DM);
    let floor = low - (span - range) / 4;
    let rows = MAP_HEIGHT as i32 - 4;
    for (column, z) in columns_z.iter().enumerate() {
        let height = 1 + (z - floor) * rows / span;
        frame.tops[column] = (MAP_HEIGHT as i32 - height) as u8;
    }
    Some(frame)
}

/// A column's colour: its steepness, at 25% when dimmed.
fn shade(frame: &Frame, column: usize) -> u16 {
    let full = color(frame.bands[column]);
    if frame.dim[column] {
        blend(full, DIM_UNDER, 4)
    } else {
        full
    }
}

/// Precompute the layout, grade colour and every chart column.
pub(super) fn frame(view: &View) -> Frame {
    if let Some(frame) = map_frame(view) {
        return frame;
    }
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
    // Leave room above the highest point for the rider's disc.
    let rows = CHART_HEIGHT as i32 - 4;
    let first = PROFILE_LEN - len;
    let mut band = Steepness::of(samples.first().map_or(0, |s| i32::from(s.grade_tenths)));
    for (index, sample) in samples.iter().enumerate() {
        band = band.after(i32::from(sample.grade_tenths));
        let height = 1 + (i32::from(sample.altitude_dm) - floor) * rows / span;
        frame.tops[first + index] = (CHART_HEIGHT as i32 - height) as u8;
        frame.bands[first + index] = band;
    }
    frame.rider = (PROFILE_LEN - 1) as u8;
    frame
}

/// The profile fill and its baseline.
pub(super) fn paint_chart(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if y == CHART_BOTTOM {
        row.span(CHART_LEFT..CHART_RIGHT, GRID);
    }
    if !(frame.top..CHART_BOTTOM).contains(&y) {
        return;
    }
    let line = y - frame.top;
    // Runs of one colour fill as spans.
    let mut column = 0;
    while column < PROFILE_LEN {
        let filled = |c: usize| frame.tops[c] != EMPTY && line >= usize::from(frame.tops[c]);
        if !filled(column) {
            column += 1;
            continue;
        }
        let shade_of = shade(frame, column);
        let start = column;
        while column < PROFILE_LEN && filled(column) && shade(frame, column) == shade_of {
            column += 1;
        }
        row.span(CHART_LEFT + start..CHART_LEFT + column, shade_of);
    }
}

/// The per-pixel original of `paint_chart`.
#[cfg(test)]
pub(super) fn chart_pixel(frame: &Frame, x: usize, y: usize) -> u16 {
    if !(CHART_LEFT..CHART_RIGHT).contains(&x) {
        return BLACK;
    }
    if y == CHART_BOTTOM {
        return GRID;
    }
    if !(frame.top..CHART_BOTTOM).contains(&y) {
        return BLACK;
    }
    let column = x - CHART_LEFT;
    let top = frame.tops[column];
    if top != EMPTY && y - frame.top >= usize::from(top) {
        shade(frame, column)
    } else {
        BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::climb::Climbs;
    use crate::screens::workout::Page;

    /// A view of the climb page after riding `metres` along `altitude_dm(m)`;
    /// views borrow the climbs, so tests leak them.
    fn ridden(metres: u32, altitude_dm: impl Fn(u32) -> i32) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        view.radar_paired = true;
        view.ride_page = crate::screens::ride::RidePage::Climb;
        let mut climbs = Climbs::default();
        for m in (5..=metres).step_by(5) {
            climbs.add(1000, 5000, Some(altitude_dm(m)));
        }
        view.climbs = std::boxed::Box::leak(std::boxed::Box::new(climbs));
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
        assert_eq!(view.pixel(CHART_LEFT + 50, CHART_BOTTOM - 1), BLACK);
        assert_eq!(view.pixel(CHART_LEFT + 50, CHART_BOTTOM), GRID);
        // The rider's disc sits on the newest altitude.
        assert_eq!(
            view.pixel(CHART_RIGHT - 1, CHART_TOP + usize::from(flat)),
            WHITE
        );
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
        // Highest at the top of the 10%, three rows below the chart top.
        assert_eq!(frame.tops[180], 3);
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

    /// Every climb state, with the widest values, lays out whole.
    #[test]
    fn extreme_climbs_never_overlap_or_clip() {
        let steep = |m: u32| m as i32 * 25 / 10;
        let cases: [(&str, View); 4] = [
            ("nothing ridden", ridden(0, |_| 0)),
            ("flat", ridden(4000, |_| 5000)),
            ("climbing 25%", ridden(4000, steep)),
            (
                "after a climb",
                ridden(6000, |m| {
                    steep(m.min(3000)) - (m.saturating_sub(3000)) as i32
                }),
            ),
        ];
        for (what, mut view) in cases {
            for tenths in [None, Some(-125), Some(-5), Some(255), Some(999)] {
                view.sample.gradient_tenths = tenths;
                view.totals.add(1000, 0, &view.sample, Some(30_000));
                view.prepare();
                view.canvas()
                    .assert_laid_out(BODY, &["GRADE"], &std::format!("{what} {tenths:?}"));
            }
        }
    }

    #[test]
    fn the_grade_band_takes_the_steepness_colour() {
        let mut view = ridden(1000, |_| 0);
        view.sample.gradient_tenths = Some(105);
        view.prepare();
        assert!(view.canvas().shapes.iter().any(|shape| matches!(
            shape,
            Shape::Bar { color, .. } if *color == ZONES[5]
        )));
        assert_eq!(
            view.canvas().find("10.5%").map(|run| run.color),
            Some(GRADE)
        );
    }

    /// A climb page view with a map forecast of `road(metres)` over `km`.
    fn mapped(km: usize, road: impl Fn(f64) -> f64) -> View<'static> {
        let mut view = ridden(0, |_| 0);
        edit(&mut view, |forecast| {
            forecast.update(&crate::ride::climb_ahead::tests::ahead(1, km, road));
        });
        view
    }

    /// Change the view's forecast: views borrow it, so tests leak a copy.
    fn edit(
        view: &mut View<'static>,
        change: impl FnOnce(&mut crate::ride::climb_ahead::Forecast),
    ) {
        let forecast = std::boxed::Box::leak(std::boxed::Box::new(view.forecast.clone()));
        change(forecast);
        view.forecast = forecast;
    }

    /// Every painted row equals the per-pixel reference.
    fn assert_paints_reference(view: &View) {
        for y in 0..320 {
            let mut pixels = [0x5555; crate::screens::chrome::WIDTH];
            view.paint(&mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, view.reference_pixel(x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn a_climb_ahead_shows_its_foot_category_and_profile() {
        // 2 km flat, then 3 km at 7%, then flat.
        let mut view = mapped(10, |m| (m - 2000.0).clamp(0.0, 3000.0) * 0.07);
        view.prepare();
        let frame = &view.climb;
        assert_eq!(frame.layout, Layout::Ahead);
        for text in [
            "CLIMB AHEAD",
            "CAT 3",
            "2.0",
            "KM",
            "LENGTH KM",
            "3.00",
            "7.0%",
            "210",
        ] {
            assert!(view.canvas().shows(text), "{text}");
        }
        // The chart runs from the rider to past the top: 5.3 km over 200
        // columns. The flat before the foot is dim, the climb is full orange.
        assert_eq!(frame.rider, 0);
        let foot = 2_000 * (PROFILE_LEN - 1) / 5_300;
        assert!(frame.dim[..foot - 2].iter().all(|&d| d));
        assert!(frame.dim[foot + 2..foot + 100].iter().all(|&d| !d));
        assert!(
            frame.bands[foot + 10..foot + 100]
                .iter()
                .all(|&b| b == Steepness::Steep)
        );
        assert_eq!(
            view.pixel(CHART_LEFT + foot + 50, CHART_BOTTOM - 1),
            color(Steepness::Steep)
        );
        assert_eq!(
            view.pixel(CHART_LEFT + 20, CHART_BOTTOM - 1),
            blend(color(Steepness::Easy), BLACK, 4)
        );
        // The top is the highest column.
        let top = frame
            .tops
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| **t)
            .unwrap()
            .0;
        assert!(top > foot + 100, "{top}");
        assert_paints_reference(&view);
    }

    #[test]
    fn on_a_climb_the_page_counts_down_to_the_top() {
        // On a 9% climb with 1.5 km to go, then down.
        let road = |m: f64| {
            if m < 1500.0 {
                m * 0.09
            } else {
                135.0 - (m - 1500.0) * 0.05
            }
        };
        let mut view = mapped(5, road);
        view.sample.gradient_tenths = Some(88);
        view.prepare();
        assert_eq!(view.climb.layout, Layout::ToTop);
        for text in [
            "TO THE TOP",
            "1.5",
            "TO GO M",
            "135",
            "8.8%",
            "NEXT 500 M",
            "9.0%",
        ] {
            assert!(view.canvas().shows(text), "{text}");
        }
        // Without a climb so far from the barometer the rider is at the left.
        assert_eq!(view.climb.rider, 0);
        assert!(!view.climb.dim[..150].iter().any(|&d| d));
        assert_paints_reference(&view);
    }

    #[test]
    fn the_part_climbed_is_dimmed_behind_the_rider() {
        // The barometer has 1 km of 8% behind; the map 1 km more ahead.
        let mut view = ridden(2000, |m| (m as i32 - 1000).max(0) * 8 / 10);
        edit(&mut view, |forecast| {
            forecast.update(&crate::ride::climb_ahead::tests::ahead(1, 3, |m| {
                (m.min(1000.0)) * 0.08
            }));
        });
        view.prepare();
        let frame = &view.climb;
        assert_eq!(frame.layout, Layout::ToTop);
        let rider = usize::from(frame.rider);
        assert!((70..110).contains(&rider), "{rider}");
        assert!(frame.dim[..rider - 2].iter().all(|&d| d));
        assert!(frame.dim[rider + 2..rider + 50].iter().all(|&d| !d));
        // The rider's disc sits on the profile at the rider's column.
        let x = CHART_LEFT + rider;
        assert_eq!(
            view.pixel(x, MAP_TOP + usize::from(frame.tops[rider])),
            WHITE
        );
        assert_paints_reference(&view);
    }

    #[test]
    fn an_unsure_forecast_leaves_the_map_free_page() {
        let mut view = mapped(10, |m| (m - 1000.0).max(0.0) * 0.08);
        edit(&mut view, |forecast| forecast.confidence = 20);
        view.prepare();
        assert!(view.canvas().shows("GRADE"));
        assert!(!view.canvas().shows("CLIMB AHEAD"));
    }

    /// The map views lay out whole with the widest values.
    #[test]
    fn extreme_forecasts_never_overlap_or_clip() {
        let cases: [(&str, View); 4] = [
            // An HC wall 9.9 km away that runs past the horizon.
            ("far HC", mapped(10, |m| (m - 9_400.0).max(0.0) * 0.25)),
            // A 25% climb from here past the horizon.
            ("steep open", mapped(10, |m| m * 0.25)),
            // A short Cat 4 just ahead.
            ("short", mapped(10, |m| (m - 300.0).clamp(0.0, 900.0) * 0.1)),
            (
                "descent then climb",
                mapped(10, |m| {
                    if m < 3000.0 {
                        -m * 0.08
                    } else {
                        -240.0 + (m - 3000.0) * 0.12
                    }
                }),
            ),
        ];
        for (what, mut view) in cases {
            for tenths in [None, Some(-125), Some(255), Some(999)] {
                view.sample.gradient_tenths = tenths;
                view.prepare();
                assert!(
                    matches!(view.climb.layout, Layout::Ahead | Layout::ToTop),
                    "{what}"
                );
                view.canvas()
                    .assert_laid_out(BODY, &["KM"], &std::format!("{what} {tenths:?}"));
                assert_paints_reference(&view);
            }
        }
    }

    #[test]
    fn without_a_map_the_page_says_so() {
        let mut view = ridden(2000, |_| 5000);
        view.prepare();
        assert!(view.canvas().shows("NO MAP"));
        assert_paints_reference(&view);
        edit(&mut view, |forecast| forecast.map = true);
        view.prepare();
        assert!(!view.canvas().shows("NO MAP"));
    }
}
