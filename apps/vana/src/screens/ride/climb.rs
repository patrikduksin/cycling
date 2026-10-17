//! The climb page: the grade now in its steepness colour, the climb under
//! way (or altitude, ascent and the last climb) and the last 4 km of
//! elevation as a solid profile coloured by local grade, with a white disc
//! for the rider.
//!
//! `frame` precomputes every chart column once per frame; `paint_chart` only
//! looks them up, and the canvas paints the text and the rider over it.
use super::{BODY, CELL_PITCH, GRID, LEFT, RIGHT, cell, grade, kilometres, lap_time, whole};
use crate::ride::climb::{PROFILE_LEN, ProfileSample, Steepness, Summary};
use crate::screens::canvas::{Canvas, Shape};
#[cfg(test)]
use crate::screens::chrome::BLACK;
use crate::screens::chrome::{ACCENT, Align, GRADE, HERO, LARGE, Rect, Row, VALUE, WHITE, ZONES};
use crate::screens::workout::{Text, View};

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
/// A flat road spans at least 20 m of chart height, so it stays flat.
const MIN_SPAN_DM: i32 = 200;
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

    let newest = frame.tops[PROFILE_LEN - 1];
    if newest != EMPTY {
        canvas.shape(Shape::Disc {
            cx: 4 * (CHART_RIGHT - 1) as i32 + 2,
            cy: 4 * (CHART_TOP + usize::from(newest)) as i32 + 2,
            radius: RIDER_RADIUS,
            color: WHITE,
        });
    }
    canvas
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
    frame
}

/// The profile fill and its baseline.
pub(super) fn paint_chart(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if y == CHART_BOTTOM {
        row.span(CHART_LEFT..CHART_RIGHT, GRID);
    }
    if !(CHART_TOP..CHART_BOTTOM).contains(&y) {
        return;
    }
    let line = y - CHART_TOP;
    // Runs of one colour fill as spans.
    let mut column = 0;
    while column < PROFILE_LEN {
        let filled = |c: usize| frame.tops[c] != EMPTY && line >= usize::from(frame.tops[c]);
        if !filled(column) {
            column += 1;
            continue;
        }
        let band = frame.bands[column];
        let start = column;
        while column < PROFILE_LEN && filled(column) && frame.bands[column] == band {
            column += 1;
        }
        row.span(CHART_LEFT + start..CHART_LEFT + column, color(band));
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
    if !(CHART_TOP..CHART_BOTTOM).contains(&y) {
        return BLACK;
    }
    let column = x - CHART_LEFT;
    let top = frame.tops[column];
    if top != EMPTY && y - CHART_TOP >= usize::from(top) {
        color(frame.bands[column])
    } else {
        BLACK
    }
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
                view.canvas
                    .assert_laid_out(BODY, &["GRADE"], &std::format!("{what} {tenths:?}"));
            }
        }
    }

    #[test]
    fn the_grade_band_takes_the_steepness_colour() {
        let mut view = ridden(1000, |_| 0);
        view.sample.gradient_tenths = Some(105);
        view.prepare();
        assert!(view.canvas.shapes.iter().any(|shape| matches!(
            shape,
            Shape::Bar { color, .. } if *color == ZONES[5]
        )));
        assert_eq!(view.canvas.find("10.5%").map(|run| run.color), Some(GRADE));
    }
}
