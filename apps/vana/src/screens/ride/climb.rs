//! The climb page: one layout for every road. The grade fills the top left in
//! its steepness colour, 3 s power and heart rate stack beside it with their
//! zones, and a band in the grade's colour runs under them. Two readings and
//! a profile, solid and coloured by local grade with a white disc for the
//! rider, follow. What they show depends on the road (`Situation`):
//!
//! - **Ahead** (the map pinned a climb whose foot is ahead): CLIMB IN, the
//!   distance to the foot; its length, average grade and gain; the road from
//!   the rider past the top, with the road before the foot and after the top
//!   dimmed, and an empty progress bar.
//! - **On the map's climb**: CLIMBING; the distance and gain to the top; the
//!   whole climb from its foot past its top, fixed while the rider's disc
//!   moves along it, what is behind dimmed, and the share climbed as a bar.
//! - **On a climb without a map** (the barometer's): CLIMBING; the length
//!   and gain so far; the last 4 km with the road before the climb dimmed.
//! - **Clear**: NO CLIMB; altitude and ascent over the whole ride's
//!   elevation, the rider at its end.
//!
//! `frame` precomputes every chart column once per frame; `paint_chart` only
//! looks them up, and the canvas paints the text, the bar and the rider.
use super::{GRID, cell, grade, kilometres, whole};
use crate::ride::climb::{Category, Climbs, PROFILE_LEN, ProfileSample, STEP_MM, Steepness};
use crate::ride::climb_ahead::{Forecast, Pinned};
use crate::ride::elevation::Trace;
use crate::screens::Layout;
use crate::screens::canvas::{Canvas, Shape};
#[cfg(test)]
use crate::screens::chrome::BLACK;
use crate::screens::chrome::{
    ACCENT, Align, BLACK as DIM_UNDER, GRADE, Rect, Row, SECONDARY, WHITE, ZONES, mix, zone_color,
};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, Line};
use firmware_shell::shell::Parts;

/// The climb page on one panel. A text row's bottom is its baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The title on the left and the category on the right.
    pub head: Rect,
    /// The grade's box, under the title, beside power and heart rate.
    pub grade: Rect,
    /// Power's and heart rate's captions, right-aligned across the text
    /// margins: a caption may reach left over the grade's box, above the
    /// grade itself.
    pub captions: [Rect; 2],
    /// Power's and heart rate's values, left of their zone chips.
    pub values: [Rect; 2],
    pub chips: [Rect; 2],
    /// The steepness band under the grade and the readings.
    pub band: Rect,
    /// The road's two readings: one band of cells.
    pub cells: Rect,
    /// One column per profile sample at scale 1, ending at the body's right
    /// margin. Without a progress bar the fill runs to the screen's edge,
    /// into the toast band, where a toast covers it for a moment. A taller
    /// panel gives the chart its spare rows.
    pub chart: Rect,
    /// With a progress bar the chart ends at `barred`'s bottom, the bar
    /// follows, and FOOT, the share and TOP under it.
    pub barred: Rect,
    pub bar: Rect,
    pub labels: Rect,
}

const VALUE_WIDTH: u16 = 36;
const CHIP_GAP: u16 = 4;
const CHIP_WIDTH: u16 = 22;
/// The grade's box starts left of the text margin, so a grade like 6.0%
/// keeps the large face beside the readings.
const GRADE_INSET: u16 = 6;
/// What the cells keep right of the grade's box: a gap, a value and its
/// zone chip, and the text's right margin.
const BESIDE_GRADE: u16 = 6 + VALUE_WIDTH + CHIP_GAP + CHIP_WIDTH + super::TEXT_RIGHT;

pub const fn regions(at: &Scaled) -> Regions {
    use FaceRole::{Body, Caption};
    use Track::{Cap, Fr, Unit};
    let frame = super::frame(at);
    let (body, text) = (frame.body, frame.text);
    let [
        head,
        _,
        power_caption,
        _,
        power,
        heart_caption,
        _,
        heart,
        _,
        under_grade,
        band,
        _,
        cells,
        chart,
    ] = body.rows(
        [
            Unit(16),
            Unit(6),
            Unit(12),
            Unit(8),
            Cap(Body),
            Unit(16),
            Unit(8),
            Cap(Body),
            Unit(2),
            Unit(4),
            Unit(5),
            Unit(4),
            Unit(super::CELL_PITCH as u16 - 4),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, values, _, chip] = text.cols(
        [Fr(1), Unit(VALUE_WIDTH), Unit(CHIP_GAP), Unit(CHIP_WIDTH)],
        Unit(0),
        at,
    );
    let [_, grade, _] = body.cols([Unit(GRADE_INSET), Fr(1), Unit(BESIDE_GRADE)], Unit(0), at);
    let chart = chart.with_columns(chart_columns(at));
    let [barred, _, bar, _, labels, _] = chart.rows(
        [Fr(1), Unit(5), Unit(6), Unit(4), Cap(Caption), Unit(3)],
        Unit(0),
        at,
    );
    assert!(
        chart.width() <= COLUMNS,
        "the chart has more columns than its frame"
    );
    assert!(
        chart.height() < EMPTY as usize && barred.height() >= at.unit(64),
        "the chart's rows do not fit a column's top, or leave no room over the bar"
    );
    Regions {
        head: head.with_columns(text),
        grade: Rect {
            top: power_caption.top,
            bottom: under_grade.top,
            ..grade
        },
        captions: [
            power_caption.with_columns(text),
            heart_caption.with_columns(text),
        ],
        values: [power.with_columns(values), heart.with_columns(values)],
        chips: [
            chip_beside(power.with_columns(chip), at),
            chip_beside(heart.with_columns(chip), at),
        ],
        band: band.with_columns(text),
        cells: cells.with_columns(text),
        chart,
        barred,
        bar,
        labels,
    }
}

const fn chip_beside(value: Rect, at: &Scaled) -> Rect {
    Rect {
        top: value.top - at.unit(2),
        bottom: value.bottom + at.unit(3),
        ..value
    }
}

const fn chart_columns(at: &Scaled) -> Rect {
    use Track::{Fr, Unit};
    let body = super::frame(at).body;
    let [_, columns, _] = body.cols([Fr(1), Unit(PROFILE_LEN as u16), Unit(6)], Unit(0), at);
    columns
}

/// The most columns the chart has on any of `panels`.
const fn columns(panels: &[(usize, usize)]) -> usize {
    crate::screens::scaled::largest!(panels, |at| chart_columns(&at).width())
}

const COLUMNS: usize = columns(crate::screens::limits::PANELS);
const _: () = assert!(COLUMNS < EMPTY as usize, "a column's index is a u8");
/// A flat road spans at least 20 m of chart height, so it stays flat.
pub(super) const MIN_SPAN_DM: i32 = 200;
/// A column with no profile sample yet.
const EMPTY: u8 = u8::MAX;
/// The rider's disc: 6 px across, in quarter pixels.
const RIDER_RADIUS: usize = 12;
/// The map's road past the top: a tenth of the climb, at least 200 m.
const AFTER_MIN_MM: u64 = 200_000;

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

/// What the climb page draws from, lent by the ride and the look-ahead.
#[derive(Clone, Copy)]
pub struct Lent<'a> {
    pub climbs: &'a Climbs,
    pub climbing: Option<crate::ride::climb::Summary>,
    /// The whole ride's elevation.
    pub trace: &'a Trace,
    pub forecast: &'a Forecast,
    /// The map's climb near or under way, on the ride's odometer.
    pub pinned: Option<&'a Pinned>,
}

impl Lent<'static> {
    /// Nothing ridden and no map.
    pub const NONE: Self = Self {
        climbs: &Climbs::NONE,
        climbing: None,
        trace: &Trace::EMPTY,
        forecast: &Forecast::NONE,
        pinned: None,
    };
}

/// The road, as the page draws it; `app::climb_page` reads it the same way.
#[derive(Clone, Copy)]
enum Situation<'a> {
    Ahead(&'a Pinned),
    OnMap(&'a Pinned),
    /// The barometer's climb under way: its length and gain so far.
    OnClimb {
        length_mm: u64,
        gain_dm: u32,
    },
    Clear,
}

fn situation<'a>(view: &View<'a>) -> Situation<'a> {
    let lent = view.climb_lent();
    let odometer = view.totals.distance_mm();
    match (lent.pinned, lent.climbing) {
        (Some(pin), _) if odometer < pin.foot_mm => Situation::Ahead(pin),
        (Some(pin), _) => Situation::OnMap(pin),
        (None, Some(climb)) => Situation::OnClimb {
            length_mm: climb.length_mm,
            gain_dm: climb.gain_dm,
        },
        (None, None) => Situation::Clear,
    }
}

/// What the climb page precomputes for one frame.
#[derive(Clone, Hash)]
pub struct Frame {
    bottom: usize,
    tops: [u8; COLUMNS],
    bands: [Steepness; COLUMNS],
    /// Columns drawn at 25%: road outside the climb, or already climbed.
    dim: [bool; COLUMNS],
    /// The rider's column, or `EMPTY`.
    rider: u8,
    /// The share of the climb climbed, 0..=1000, for the progress bar.
    done: Option<u16>,
}

impl Frame {
    pub const EMPTY: Self = Self {
        bottom: regions(&crate::screens::scaled::BASE).chart.bottom,
        tops: [EMPTY; COLUMNS],
        bands: [Steepness::Easy; COLUMNS],
        dim: [false; COLUMNS],
        rider: EMPTY,
        done: None,
    };
}

fn metres(value: &mut Text<12>, dm: i32) {
    whole(value, Some((dm + 5).div_euclid(10)));
}

/// Kilometres with one decimal: `0.4`, `12.3`.
fn tenths_km(value: &mut Text<12>, mm: u64) {
    let tenths = (mm + 50_000) / 100_000;
    let _ = write!(value, "{}.{}", tenths / 10, tenths % 10);
}

/// The text, the band, the bar and the rider; `frame` must be prepared.
pub(super) fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let mut canvas = Canvas::default();
    let situation = situation(view);
    head(&mut canvas, layout, view, situation);
    readings(&mut canvas, layout, view);
    let steepness = view
        .sample
        .gradient_tenths
        .map(|tenths| Steepness::of(i32::from(tenths)));
    canvas.shape(Shape::Bar {
        rect: layout.climb.band,
        radius: layout.scaled.unit(2),
        color: steepness.map_or(GRID, color),
    });

    cells(&mut canvas, layout, view, situation);
    finish(&mut canvas, layout, view);
    canvas
}

/// The road's two readings under power and heart rate.
#[inline(never)]
fn cells(canvas: &mut Canvas, layout: &Layout, view: &View, situation: Situation) {
    let odometer = view.totals.distance_mm();
    let top = layout.climb.cells.top;
    let mut values: [Text<12>; 2] = core::array::from_fn(|_| Text::new());
    let labels: [(&[u8], u16); 2] = match situation {
        Situation::Ahead(pin) => {
            let climb = pin.summary();
            kilometres(&mut values[0], climb.length_mm);
            cell(
                canvas,
                layout,
                0,
                top,
                b"LENGTH KM",
                values[0].as_bytes(),
                WHITE,
            );
            let mut average = Text::<12>::new();
            grade(&mut average, climb.grade_tenths());
            let mut gain = Text::<12>::new();
            metres(&mut gain, climb.gain_dm as i32);
            let at = &layout.scaled;
            stacked(canvas, layout, b"AVG", &average, top + at.unit(16));
            let value = top + layout.ride.cells.value;
            stacked(canvas, layout, b"GAIN M", &gain, value);
            return;
        }
        Situation::OnMap(pin) => {
            tenths_km(&mut values[0], pin.top_mm.saturating_sub(odometer));
            let left = pin.altitude_dm(pin.top_mm) - pin.altitude_dm(odometer);
            metres(&mut values[1], left.max(0));
            [(b"TO TOP KM", WHITE), (b"UP TO GO M", GRADE)]
        }
        Situation::OnClimb { length_mm, gain_dm } => {
            kilometres(&mut values[0], length_mm);
            metres(&mut values[1], gain_dm as i32);
            [(b"SO FAR KM", WHITE), (b"GAIN M", GRADE)]
        }
        Situation::Clear => {
            match view.totals.altitude_dm() {
                Some(dm) => metres(&mut values[0], dm),
                None => whole(&mut values[0], None::<u8>),
            }
            metres(&mut values[1], view.totals.ascent_dm() as i32);
            [(b"ALTITUDE M", GRADE), (b"ASCENT M", GRADE)]
        }
    };
    for (column, ((caption, color), value)) in labels.iter().zip(&values).enumerate() {
        cell(
            canvas,
            layout,
            column,
            top,
            caption,
            value.as_bytes(),
            *color,
        );
    }
}

/// The progress bar under the chart, FOOT to TOP, and the rider's disc.
#[inline(never)]
fn finish(canvas: &mut Canvas, layout: &Layout, view: &View) {
    let (at, regions) = (&layout.scaled, &layout.climb);
    let frame = view.climb();
    if let Some(done) = frame.done {
        let (chart, track) = (regions.chart, regions.bar);
        let radius = at.unit(3);
        canvas.shape(Shape::Bar {
            rect: track,
            radius,
            color: GRID,
        });
        let filled = chart.width() * usize::from(done) / 1000;
        if filled >= track.height() {
            canvas.shape(Shape::Bar {
                rect: Rect {
                    right: chart.left + filled,
                    ..track
                },
                radius,
                color: WHITE,
            });
        }
        let width = chart.width() / 3;
        let base = regions.labels.bottom;
        canvas.caption(at, b"FOOT", width, chart.left, base, Align::Left);
        let mut share = Text::<12>::new();
        let _ = write!(share, "{}% UP", (done + 5) / 10);
        canvas.fit(
            &[at.face(FaceRole::Caption)],
            share.as_bytes(),
            width,
            (chart.left + chart.right) / 2,
            base,
            Align::Center,
            WHITE,
        );
        canvas.caption(at, b"TOP", width, chart.right, base, Align::Right);
    }
    rider(canvas, layout, frame);
}

/// The title on the left and the category, or NO MAP, on the right.
#[inline(never)]
fn head(canvas: &mut Canvas, layout: &Layout, view: &View, situation: Situation) {
    let (at, head) = (&layout.scaled, layout.climb.head);
    let odometer = view.totals.distance_mm();
    let mut title = Text::<24>::new();
    let (category, color) = match situation {
        Situation::Ahead(pin) => {
            let mut km = Text::<12>::new();
            tenths_km(&mut km, pin.foot_mm.saturating_sub(odometer));
            let _ = write!(title, "CLIMB IN {} KM", km.as_str());
            category(pin.summary().gain_dm, pin.summary().grade_tenths())
        }
        Situation::OnMap(pin) => {
            let _ = title.push_str("CLIMBING");
            category(pin.summary().gain_dm, pin.summary().grade_tenths())
        }
        Situation::OnClimb { length_mm, gain_dm } => {
            let _ = title.push_str("CLIMBING");
            let grade = (length_mm > 0).then(|| (gain_dm as u64 * 100_000 / length_mm) as i32);
            category(gain_dm, grade)
        }
        Situation::Clear => {
            let _ = title.push_str("NO CLIMB");
            (None, SECONDARY)
        }
    };
    let title_color = match situation {
        Situation::Clear => SECONDARY,
        _ => ACCENT,
    };
    let caption = [at.face(FaceRole::Caption)];
    let title = canvas.fit(
        &caption,
        title.as_bytes(),
        head.width(),
        head.left,
        head.bottom,
        Align::Left,
        title_color,
    );
    let right: Option<(&[u8], u16)> = match category {
        Some(name) => Some((name, color)),
        // No terrain pack (or no memory for it): only the barometer speaks.
        None if !view.climb_lent().forecast.map => Some((b"NO MAP", SECONDARY)),
        None => None,
    };
    if let Some((text, color)) = right {
        canvas.fit(
            &caption,
            text,
            head.right.saturating_sub(title.right() + at.unit(8)),
            head.right,
            head.bottom,
            Align::Right,
            color,
        );
    }
}

/// A climb's category name in the colour of its average steepness; none
/// for a climb too small to have one.
fn category(gain_dm: u32, grade_tenths: Option<i32>) -> (Option<&'static [u8]>, u16) {
    let category = Category::of(gain_dm);
    let color = color(Steepness::of(grade_tenths.unwrap_or(0)));
    match category {
        Category::Uncategorised => (None, color),
        category => (Some(category.name()), color),
    }
}

/// The grade, centred down its box, and power and heart rate stacked
/// beside it, each with its zone on a chip.
#[inline(never)]
fn readings(canvas: &mut Canvas, layout: &Layout, view: &View) {
    let (at, regions) = (&layout.scaled, &layout.climb);
    let mut now = Text::<12>::new();
    grade(&mut now, view.sample.gradient_tenths.map(i32::from));
    let box_ = regions.grade;
    let faces = [at.face(FaceRole::Large), at.face(FaceRole::Value)];
    let line = Line::<24>::fit(&faces, now.as_bytes(), box_.width());
    let cap = line.face().cap as usize;
    let base = (box_.top + box_.bottom + cap) / 2;
    canvas.run(crate::screens::canvas::on_baseline(
        line,
        box_.left,
        base,
        Align::Left,
        GRADE,
    ));
    let power = view.power_3s;
    let heart = view.sample.heart_bpm;
    let rows = [
        (
            &b"POWER 3S"[..],
            power,
            power.map_or(0, |w| view.profile.power_zone(w)),
        ),
        (
            &b"HEART"[..],
            heart,
            heart.map_or(0, |bpm| view.profile.heart_zone(bpm)),
        ),
    ];
    let caption_face = at.face(FaceRole::Caption);
    for (index, (caption, value, zone)) in rows.into_iter().enumerate() {
        let (above, values, chip) = (
            regions.captions[index],
            regions.values[index],
            regions.chips[index],
        );
        canvas.caption(
            at,
            caption,
            above.width(),
            above.right,
            above.bottom,
            Align::Right,
        );
        canvas.shape(Shape::Bar {
            rect: chip,
            radius: at.unit(4),
            color: zone_color(zone),
        });
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
        canvas.fit(
            &[caption_face],
            name.as_bytes(),
            chip.width(),
            chip.right - chip.width() / 2,
            values.bottom,
            Align::Center,
            DIM_UNDER,
        );
        let mut text = Text::<12>::new();
        whole(&mut text, value);
        canvas.fit(
            &[at.face(FaceRole::Body), caption_face],
            text.as_bytes(),
            values.width(),
            values.right,
            values.bottom,
            Align::Right,
            WHITE,
        );
    }
}

/// A caption and its value on one line of the right column: two of them
/// stack where one cell would be.
fn stacked(canvas: &mut Canvas, layout: &Layout, caption: &[u8], value: &Text<12>, base: usize) {
    let at = &layout.scaled;
    let (middle, right) = layout.ride.cells.columns[1];
    let run = canvas.fit(
        &[at.face(FaceRole::Body)],
        value.as_bytes(),
        right - middle,
        right,
        base,
        Align::Right,
        GRADE,
    );
    canvas.caption(
        at,
        caption,
        run.left.saturating_sub(middle + at.unit(4)),
        middle,
        base,
        Align::Left,
    );
}

/// The rider's disc on the chart.
fn rider(canvas: &mut Canvas, layout: &Layout, frame: &Frame) {
    if frame.rider == EMPTY {
        return;
    }
    let chart = layout.climb.chart;
    let column = usize::from(frame.rider);
    let top = frame.tops[column];
    if top != EMPTY {
        canvas.shape(Shape::Disc {
            cx: 4 * (chart.left + column) as i32 + 2,
            cy: 4 * (chart.top + usize::from(top)) as i32 + 2,
            radius: layout.scaled.unit(RIDER_RADIUS) as i32,
            color: WHITE,
        });
    }
}

/// A column's colour: its steepness, at 25% when dimmed.
fn shade(frame: &Frame, column: usize) -> u16 {
    let full = color(frame.bands[column]);
    if frame.dim[column] {
        mix(full, DIM_UNDER, 4)
    } else {
        full
    }
}

/// One chart column: its altitude, steepness and whether it is dimmed.
type Column = (i32, Steepness, bool);

/// Scale `columns` (from `first` on) into the frame's rows.
fn fill(frame: &mut Frame, layout: &Layout, first: usize, columns: &[Column]) {
    let (Some(low), Some(high)) = (
        columns.iter().map(|c| c.0).min(),
        columns.iter().map(|c| c.0).max(),
    ) else {
        return;
    };
    let range = high - low;
    let span = range.max(MIN_SPAN_DM);
    // A range smaller than the minimum span sits in the lower part, as a road.
    let floor = low - (span - range) / 4;
    let height = (frame.bottom - layout.climb.chart.top) as i32;
    // Leave room above the highest point for the rider's disc.
    let rows = height - layout.scaled.unit(4) as i32;
    for (index, &(z, steepness, dim)) in columns.iter().enumerate() {
        let filled = 1 + (z - floor) * rows / span;
        frame.tops[first + index] = (height - filled) as u8;
        frame.bands[first + index] = steepness;
        frame.dim[first + index] = dim;
    }
}

/// Picks one of the profile's `len` samples, held at the front of
/// `columns`, for each of `count` chart columns, newest last, and moves the
/// picks to the front in place. Returns the first column with a sample.
fn spread_profile(columns: &mut [Column; PROFILE_LEN], len: usize, count: usize) -> Option<usize> {
    let missing = PROFILE_LEN - len;
    let sample = |column: usize| column * (PROFILE_LEN - 1) / (count - 1);
    let first = (0..count).find(|&column| sample(column) >= missing)?;
    for column in first..count {
        let (into, pick) = (column - first, sample(column) - missing);
        debug_assert!(
            pick >= into,
            "column {into} reads sample {pick} after overwriting it"
        );
        columns[into] = columns[pick];
    }
    Some(first)
}

/// Precompute every chart column for the road the page shows.
pub(super) fn frame(view: &View) -> Frame {
    let layout = view.layout;
    let chart = layout.climb.chart;
    let count = chart.width();
    let mut frame = Frame {
        bottom: chart.bottom,
        ..Frame::EMPTY
    };
    let lent = view.climb_lent();
    let odometer = view.totals.distance_mm();
    let mut columns = [(0, Steepness::Easy, false); PROFILE_LEN];
    match situation(view) {
        Situation::Ahead(pin) | Situation::OnMap(pin) => {
            let on = odometer >= pin.foot_mm;
            frame.bottom = layout.climb.barred.bottom;
            frame.done = Some(if on {
                pin.done_permille(odometer) as u16
            } else {
                0
            });
            // Ahead from the rider, on the climb from its foot; past the top
            // a little road for context, as far as the map reaches.
            let start = if on { pin.foot_mm } else { odometer };
            let after = ((pin.top_mm - pin.foot_mm) / 10).max(AFTER_MIN_MM);
            let end = (pin.top_mm + after)
                .min(pin.span_mm().1)
                .max(start + STEP_MM);
            let last = count as u64 - 1;
            for (column, slot) in columns[..count].iter_mut().enumerate() {
                let at = start + column as u64 * (end - start) / last;
                let outside = at < pin.foot_mm || at > pin.top_mm;
                *slot = (
                    pin.altitude_dm(at),
                    pin.steepness(at),
                    outside || (on && at < odometer),
                );
            }
            let rider = odometer.saturating_sub(start) * last / (end - start);
            frame.rider = rider.min(last) as u8;
            fill(&mut frame, layout, 0, &columns[..count]);
        }
        Situation::OnClimb { length_mm, .. } => {
            let mut samples = [ProfileSample::default(); PROFILE_LEN];
            let len = lent.climbs.profile(&mut samples);
            let climbed = (length_mm / STEP_MM) as usize;
            let mut band = Steepness::of(samples.first().map_or(0, |s| i32::from(s.grade_tenths)));
            for (index, (sample, slot)) in samples[..len].iter().zip(&mut columns).enumerate() {
                band = band.after(i32::from(sample.grade_tenths));
                *slot = (
                    i32::from(sample.altitude_dm),
                    band,
                    index + climbed < len.saturating_sub(1),
                );
            }
            if let Some(first) = spread_profile(&mut columns, len, count) {
                frame.rider = (count - 1) as u8;
                fill(&mut frame, layout, first, &columns[..count - first]);
            }
        }
        Situation::Clear => {
            let trace = lent.trace;
            let len = trace.len();
            if len < 2 {
                return frame;
            }
            // A column's grade spans the road 100 m each side.
            let reach = (100_000 / trace.step_mm()).max(1) as usize;
            let mut band = None;
            let last = count - 1;
            for (column, slot) in columns[..count].iter_mut().enumerate() {
                let at = column * (len - 1);
                let (index, into) = (at / last, (at % last) as i32);
                let next = (index + 1).min(len - 1);
                let (a, b) = (trace.altitude_dm(index), trace.altitude_dm(next));
                let (from, to) = (index.saturating_sub(reach), (index + reach).min(len - 1));
                let rise = i64::from(trace.altitude_dm(to) - trace.altitude_dm(from));
                let run = (to - from) as i64 * trace.step_mm() as i64;
                let tenths = if run == 0 {
                    0
                } else {
                    (rise * 100_000 / run) as i32
                };
                let steepness =
                    band.map_or_else(|| Steepness::of(tenths), |b: Steepness| b.after(tenths));
                band = Some(steepness);
                *slot = (a + (b - a) * into / last as i32, steepness, false);
            }
            frame.rider = last as u8;
            fill(&mut frame, layout, 0, &columns[..count]);
        }
    }
    frame
}

pub(super) fn chart_parts(frame: &Frame, regions: &Regions, parts: &mut Parts) {
    parts.add(regions.chart.top..frame.bottom + 1, frame);
}

pub(super) fn paint_chart(frame: &Frame, regions: &Regions, row: &mut Row) {
    let chart = regions.chart;
    let y = row.y();
    if y == frame.bottom {
        row.span(chart.left..chart.right, GRID);
    }
    if !(chart.top..frame.bottom).contains(&y) {
        return;
    }
    let line = y - chart.top;
    // Runs of one colour fill as spans.
    let mut column = 0;
    while column < chart.width() {
        let filled = |c: usize| frame.tops[c] != EMPTY && line >= usize::from(frame.tops[c]);
        if !filled(column) {
            column += 1;
            continue;
        }
        let shade_of = shade(frame, column);
        let start = column;
        while column < chart.width() && filled(column) && shade(frame, column) == shade_of {
            column += 1;
        }
        row.span(chart.left + start..chart.left + column, shade_of);
    }
}

/// The per-pixel original of `paint_chart`.
#[cfg(test)]
pub(super) fn chart_pixel(frame: &Frame, regions: &Regions, x: usize, y: usize) -> u16 {
    let chart = regions.chart;
    if !(chart.left..chart.right).contains(&x) {
        return BLACK;
    }
    if y == frame.bottom {
        return GRID;
    }
    if !(chart.top..frame.bottom).contains(&y) {
        return BLACK;
    }
    let column = x - chart.left;
    let top = frame.tops[column];
    if top != EMPTY && y - chart.top >= usize::from(top) {
        shade(frame, column)
    } else {
        BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::climb_ahead::{self, tests::ahead};
    use crate::screens::scaled::BASE_PANEL;
    use crate::screens::workout::Page;
    use std::boxed::Box;

    const REGIONS: Regions = crate::screens::BASE.climb;

    /// What a climb page view borrows; tests leak it.
    struct Ride {
        climbs: Climbs,
        trace: Trace,
        forecast: Forecast,
        pinned: Option<Pinned>,
        odometer_mm: u64,
    }

    impl Ride {
        fn new() -> Self {
            Self {
                climbs: Climbs::default(),
                trace: Trace::default(),
                forecast: Forecast::NONE,
                pinned: None,
                odometer_mm: 0,
            }
        }

        /// Ride `metres` at 5 m/s along `altitude_dm(m)` on the barometer.
        fn ridden(mut self, metres: u32, altitude_dm: impl Fn(u32) -> i32) -> Self {
            for m in (5..=metres).step_by(5) {
                let z = altitude_dm(m);
                self.climbs.add(1000, 5000, Some(z));
                self.odometer_mm += 5000;
                self.trace.add(self.odometer_mm, Some(z));
            }
            self
        }

        /// The map sees `road(metres)` ahead over `km`, from here.
        fn mapped(mut self, generation: u32, km: usize, road: impl Fn(f64) -> f64) -> Self {
            self.forecast.map = true;
            self.forecast.update(&ahead(generation, km, road));
            climb_ahead::follow(&mut self.pinned, Some(&self.forecast), self.odometer_mm);
            self
        }

        /// Move on `metres` without a new forecast.
        fn on(mut self, metres: u64) -> Self {
            self.odometer_mm += metres * 1000;
            climb_ahead::follow(&mut self.pinned, None, self.odometer_mm);
            self
        }

        fn view(self) -> View<'static> {
            let ride: &'static Ride = Box::leak(Box::new(self));
            let mut view = View::new(Page::Ride, 0, None);
            view.radar_paired = true;
            view.ride_page = crate::screens::ride::RidePage::Climb;
            let sample = view.sample;
            view.totals
                .add(1000, ride.odometer_mm, &sample, Some(95_000));
            view.show_climb(Lent {
                climbs: &ride.climbs,
                climbing: ride.climbs.climbing(),
                trace: &ride.trace,
                forecast: &ride.forecast,
                pinned: ride.pinned.as_ref(),
            });
            view
        }
    }

    /// The per-pixel reference models only the 240 × 320 ride page.
    fn has_reference(layout: &crate::screens::Layout) -> bool {
        layout.scaled.panel == BASE_PANEL
    }

    /// Every painted row equals the per-pixel reference.
    fn assert_paints_reference(view: &mut View) {
        let scene = view.prepare();
        for y in 0..320 {
            let mut pixels = [0x5555; crate::screens::chrome::WIDTH];
            firmware_shell::shell::Picture::paint(&scene, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, scene.reference_pixel(x, y).0, "{x},{y}");
            }
        }
    }

    /// 1 km flat, then 1 km at 7%, 1 km at 10%, then 400 m down at 5%.
    fn hill(m: u32) -> i32 {
        match m {
            ..1000 => 0,
            1000..2000 => (m as i32 - 1000) * 7 / 10,
            2000..3000 => 700 + (m as i32 - 2000),
            _ => 1700 - (m as i32 - 3000) / 2,
        }
    }

    #[test]
    fn grade_power_and_heart_rate_show_on_every_road() {
        let mut view = Ride::new().ridden(1000, |_| 0).view();
        view.sample.gradient_tenths = Some(105);
        view.power_3s = Some(260);
        view.sample.heart_bpm = Some(148);
        view.prepare();
        let canvas = view.canvas();
        for text in [
            "NO CLIMB",
            "10.5%",
            "POWER 3S",
            "260",
            "HEART",
            "148",
            "ALTITUDE M",
            "ASCENT M",
        ] {
            assert!(canvas.shows(text), "{text}");
        }
        assert!(!canvas.shows("GRADE"), "the grade needs no caption");
        assert_eq!(canvas.find("10.5%").map(|run| run.color), Some(GRADE));
        let zone = view.profile.power_zone(260);
        let name = std::format!("Z{zone}");
        assert!(canvas.shows(&name), "{name}");
        // Its chip takes the zone's colour.
        assert!(canvas.shapes.iter().any(|shape| matches!(
            shape,
            Shape::Bar { color, .. } if *color == zone_color(zone)
        )));
        // The band under the readings takes the grade's steepness colour.
        assert!(canvas.shapes.iter().any(|shape| matches!(
            shape,
            Shape::Bar { color, .. } if *color == ZONES[5]
        )));
        // The grade sits in the middle of its box.
        let grade = canvas.find("10.5%").unwrap();
        let middle = (REGIONS.grade.top + REGIONS.grade.bottom) / 2;
        let centre =
            grade.top + grade.line.face().top as usize + grade.line.face().cap as usize / 2;
        assert!(centre.abs_diff(middle) <= 1, "{centre} vs {middle}");
    }

    #[test]
    fn with_no_climb_the_whole_ride_is_the_profile() {
        let mut view = Ride::new().ridden(3400, hill).view();
        view.prepare();
        let frame = view.climb();
        assert!(view.canvas().shows("NO CLIMB") && view.canvas().shows("NO MAP"));
        // The whole ride spans the chart, the rider at its end.
        assert!(frame.tops.iter().all(|&top| top != EMPTY));
        assert_eq!(usize::from(frame.rider), PROFILE_LEN - 1);
        assert_eq!(frame.done, None);
        assert!(frame.dim.iter().all(|&d| !d));
        let height = REGIONS.chart.height();
        assert_eq!(
            usize::from(frame.tops[10]),
            height - 1,
            "the start is lowest"
        );
        assert!(frame.bands[10..50].iter().all(|&b| b == Steepness::Easy));
        assert!(frame.bands[125..165].iter().all(|&b| b == Steepness::Hard));
        assert!(frame.bands[185..].iter().all(|&b| b == Steepness::Descent));
        assert_paints_reference(&mut view);
    }

    #[test]
    fn on_a_climb_without_a_map_the_climb_so_far_is_in_colour() {
        let mut view = Ride::new().ridden(2600, hill).view();
        view.prepare();
        let canvas = view.canvas();
        for text in ["CLIMBING", "SO FAR KM", "GAIN M"] {
            assert!(canvas.shows(text), "{text}");
        }
        let frame = view.climb();
        assert_eq!(usize::from(frame.rider), PROFILE_LEN - 1);
        assert_eq!(frame.done, None, "no top, no bar");
        // The flat before the climb is dim; the climb itself is not.
        let first = PROFILE_LEN - 130;
        assert!(frame.dim[first..first + 40].iter().all(|&d| d));
        assert!(frame.dim[PROFILE_LEN - 60..].iter().all(|&d| !d));
        assert_paints_reference(&mut view);
    }

    #[test]
    fn a_climb_ahead_shows_its_foot_category_and_the_road_to_its_top() {
        // 300 m flat, then 1 km at 8% (80 m), then flat.
        let road = |m: f64| (m - 300.0).clamp(0.0, 1000.0) * 0.08;
        let mut view = Ride::new().mapped(1, 5, road).view();
        view.sample.gradient_tenths = Some(10);
        view.prepare();
        let canvas = view.canvas();
        for text in [
            "CLIMB IN 0.3 KM",
            "CAT 4",
            "LENGTH KM",
            "AVG",
            "8.0%",
            "GAIN M",
            "80",
            "0% UP",
        ] {
            assert!(canvas.shows(text), "{text}");
        }
        // Average and gain stack in the right column.
        let (average, gain) = (canvas.find("8.0%").unwrap(), canvas.find("80").unwrap());
        assert!(gain.top > average.top + 10, "stacked");
        let frame = view.climb();
        assert_eq!(frame.rider, 0);
        assert_eq!(frame.done, Some(0));
        assert_eq!(frame.bottom, REGIONS.barred.bottom);
        // The road to the foot is dim, the climb in full colour.
        let foot = 300 * (PROFILE_LEN - 1) / 1500;
        assert!(frame.dim[..foot - 2].iter().all(|&d| d));
        assert!(frame.dim[foot + 2..foot + 100].iter().all(|&d| !d));
        assert!(
            frame.bands[foot + 10..foot + 100]
                .iter()
                .all(|&b| b == Steepness::Steep)
        );
        assert_paints_reference(&mut view);
    }

    #[test]
    fn on_the_map_climb_the_rider_moves_along_the_whole_climb() {
        let road = |m: f64| (m - 300.0).clamp(0.0, 1000.0) * 0.08;
        let ride = Ride::new().mapped(1, 5, road).on(900);
        // 600 m up, 400 m to go: the map sees the rest from here.
        let ride = ride.mapped(2, 5, |m: f64| m.min(400.0) * 0.08);
        let mut view = ride.view();
        let scene = view.prepare();
        let canvas = scene.canvas();
        for text in [
            "CLIMBING",
            "CAT 4",
            "TO TOP KM",
            "0.4",
            "UP TO GO M",
            "32",
            "60% UP",
        ] {
            assert!(canvas.shows(text), "{text}");
        }
        let frame = scene.climb();
        // The chart starts at the foot: the rider is 600 m into 1.2 km of it.
        let rider = usize::from(frame.rider);
        assert!((95..=105).contains(&rider), "{rider}");
        assert!(frame.dim[..rider - 1].iter().all(|&d| d), "climbed");
        assert!(
            frame.dim[rider + 1..rider + 60].iter().all(|&d| !d),
            "ahead"
        );
        assert_eq!(frame.done, Some(600));
        // The rider's disc sits on the profile at the rider's column.
        let x = REGIONS.chart.left + rider;
        assert_eq!(
            scene.pixel(x, REGIONS.chart.top + usize::from(frame.tops[rider])),
            WHITE
        );
        // The bar is filled to the share climbed.
        let fill = canvas.shapes.iter().find_map(|shape| match shape {
            Shape::Bar { rect, color, .. } if *color == WHITE => Some(*rect),
            _ => None,
        });
        let fill = fill.expect("the climbed part of the bar");
        assert_eq!(fill.left, REGIONS.chart.left);
        assert_eq!(
            fill.right - REGIONS.chart.left,
            REGIONS.chart.width() * 600 / 1000
        );
        assert!(canvas.shows("FOOT") && canvas.shows("TOP"));
        assert_paints_reference(&mut view);
    }

    #[test]
    fn extreme_climbs_never_overlap_or_clip() {
        type Case = (&'static str, fn() -> Ride);
        let cases: [Case; 8] = [
            ("nothing ridden", Ride::new),
            ("flat", || Ride::new().ridden(4000, |_| 5000)),
            ("climbing 25%", || {
                Ride::new().ridden(4000, |m| m as i32 * 25 / 10)
            }),
            ("climb under way", || Ride::new().ridden(1300, hill)),
            ("far HC", || {
                Ride::new().mapped(1, 10, |m| (m - 450.0).max(0.0) * 0.25)
            }),
            ("steep open", || Ride::new().mapped(1, 10, |m| m * 0.25)),
            ("short", || {
                Ride::new().mapped(1, 10, |m| (m - 300.0).clamp(0.0, 900.0) * 0.1)
            }),
            ("on HC", || {
                Ride::new()
                    .mapped(1, 10, |m| (m - 100.0).max(0.0) * 0.25)
                    .on(5000)
            }),
        ];
        for (what, ride) in cases {
            for (tenths, watts, bpm) in [
                (None, None, None),
                (Some(-125), Some(1999), Some(220)),
                (Some(255), Some(85), Some(60)),
                (Some(999), Some(999), Some(199)),
            ] {
                for layout in crate::screens::panels::ALL {
                    for radar in [false, true] {
                        let mut view = ride().view();
                        view.layout = layout;
                        view.radar_paired = radar;
                        view.sample.gradient_tenths = tenths;
                        view.power_3s = watts;
                        view.sample.heart_bpm = bpm;
                        view.prepare().check_layout();
                        let panel = layout.scaled.panel;
                        let what = std::format!("{what} {tenths:?} on {panel:?}");
                        let canvas = view.canvas();
                        assert!(canvas.shows("POWER 3S") && canvas.shows("HEART"), "{what}");
                        let frame = view.climb();
                        let columns = layout.climb.chart.width();
                        assert!(frame.tops[columns..].iter().all(|&top| top == EMPTY));
                        if frame.rider != EMPTY {
                            assert!(usize::from(frame.rider) < columns, "{what}");
                        }
                        let want = ["POWER 3S", "HEART"];
                        canvas.assert_laid_out_at(&layout.scaled, layout.ride.body, &want, &what);
                        if has_reference(layout) {
                            assert_paints_reference(&mut view);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn an_unsure_forecast_pins_nothing() {
        let mut ride = Ride::new();
        ride.forecast.map = true;
        let mut unsure = ahead(1, 10, |m| (m - 300.0).max(0.0) * 0.08);
        unsure.confidence = 20;
        ride.forecast.update(&unsure);
        climb_ahead::follow(&mut ride.pinned, Some(&ride.forecast), 0);
        let mut view = ride.view();
        view.prepare();
        assert!(view.canvas().shows("NO CLIMB"));
        assert!(!view.canvas().shows("NO MAP"));
    }

    #[test]
    fn the_chart_is_bounded_by_each_panels_own_layout() {
        assert_eq!(
            columns(&[(240, 320)]),
            200,
            "the C606's frame keeps its columns"
        );
        for (width, height) in [(180, 320), (240, 300)] {
            let own = chart_columns(&Scaled::of(width, height)).width();
            assert_eq!(
                columns(&[(240, 320), (width, height)]),
                own.max(200),
                "{width} × {height}"
            );
        }
    }
}
