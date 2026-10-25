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
use super::{
    BODY, CELL_PITCH, COLUMN_GAP, GRID, LEFT, MIDDLE, RIGHT, cell, grade, kilometres, whole,
};
use crate::ride::climb::{Category, Climbs, PROFILE_LEN, ProfileSample, STEP_MM, Steepness};
use crate::ride::climb_ahead::{Forecast, Pinned};
use crate::ride::elevation::Trace;
use crate::screens::canvas::{Canvas, Shape};
#[cfg(test)]
use crate::screens::chrome::BLACK;
use crate::screens::chrome::{
    ACCENT, Align, BLACK as DIM_UNDER, BODY as BODY_FACE, CAPTION, GRADE, HEART, HERO, LARGE,
    POWER, Rect, Row, SECONDARY, VALUE, WHITE, ZONES, blend, zone_color,
};
use crate::screens::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, Line};

/// The title and the category share this baseline.
const HEAD: usize = 40;
/// The grade's box, between the title and the steepness band.
const BOX_TOP: usize = HEAD + 4;
const BAND_TOP: usize = HEAD + 4 + HERO.cap as usize + 6;
const BAND_HEIGHT: usize = 5;
/// Power and heart rate, then the climb's two readings: two bands of cells.
const READINGS: usize = BAND_TOP + BAND_HEIGHT + 4;
const CELLS: usize = READINGS + CELL_PITCH;
/// One chart column per profile sample, ending at the body's right margin.
const CHART_RIGHT: usize = BODY.right - 6;
const CHART_LEFT: usize = CHART_RIGHT - PROFILE_LEN;
const _: () = assert!(CHART_LEFT >= BODY.left, "the profile needs 200 columns");
const CHART_TOP: usize = CELLS + CELL_PITCH - 4;
/// The baseline row without a progress bar; the fill ends above it. The
/// chart runs into the toast band, where a toast covers it for a moment.
const CHART_BOTTOM: usize = BODY.bottom - 8;
/// With a progress bar, which sits under the chart.
const BAR_BOTTOM: usize = BODY.bottom - 6;
const BAR_HEIGHT: usize = 6;
const BARRED_BOTTOM: usize = BAR_BOTTOM - BAR_HEIGHT - 6;
const _: () = assert!(CHART_BOTTOM - CHART_TOP >= 64 && CHART_BOTTOM - CHART_TOP < EMPTY as usize);
/// A flat road spans at least 20 m of chart height, so it stays flat.
pub(super) const MIN_SPAN_DM: i32 = 200;
/// A column with no profile sample yet.
const EMPTY: u8 = u8::MAX;
/// The rider's disc: 6 px across, in quarter pixels.
const RIDER_RADIUS: i32 = 12;
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
    match (lent.pinned, lent.climbs.climbing()) {
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
#[derive(Clone)]
pub struct Frame {
    /// The chart's baseline row: `CHART_BOTTOM`, or `BARRED_BOTTOM` over a
    /// progress bar.
    bottom: usize,
    /// Rows below `CHART_TOP` where each column's fill starts, or `EMPTY`.
    tops: [u8; PROFILE_LEN],
    bands: [Steepness; PROFILE_LEN],
    /// Columns drawn at 25%: road outside the climb, or already climbed.
    dim: [bool; PROFILE_LEN],
    /// The rider's column, or `EMPTY`.
    rider: u8,
    /// The share of the climb climbed, 0..=1000, for the progress bar.
    done: Option<u16>,
}

impl Frame {
    pub const EMPTY: Self = Self {
        bottom: CHART_BOTTOM,
        tops: [EMPTY; PROFILE_LEN],
        bands: [Steepness::Easy; PROFILE_LEN],
        dim: [false; PROFILE_LEN],
        rider: EMPTY,
        done: None,
    };
}

impl Default for Frame {
    fn default() -> Self {
        Self::EMPTY
    }
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
    let mut canvas = Canvas::default();
    let situation = situation(view);
    let odometer = view.totals.distance_mm();
    head(&mut canvas, view, situation);
    readings(&mut canvas, view);
    let steepness = view
        .sample
        .gradient_tenths
        .map(|tenths| Steepness::of(i32::from(tenths)));
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: LEFT,
            top: BAND_TOP,
            right: RIGHT,
            bottom: BAND_TOP + BAND_HEIGHT,
        },
        radius: 2,
        color: steepness.map_or(GRID, color),
    });

    let mut values: [Text<12>; 2] = core::array::from_fn(|_| Text::new());
    let labels: [(&[u8], u16); 2] = match situation {
        Situation::Ahead(pin) => {
            let climb = pin.summary();
            kilometres(&mut values[0], climb.length_mm);
            cell(
                &mut canvas,
                0,
                CELLS,
                b"LENGTH KM",
                values[0].as_bytes(),
                WHITE,
            );
            let mut average = Text::<12>::new();
            grade(&mut average, climb.grade_tenths());
            let mut gain = Text::<12>::new();
            metres(&mut gain, climb.gain_dm as i32);
            stacked(&mut canvas, b"AVG", &average, CELLS + 16);
            stacked(&mut canvas, b"GAIN M", &gain, CELLS + 41);
            return finish(canvas, view);
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
            &mut canvas,
            column,
            CELLS,
            caption,
            value.as_bytes(),
            *color,
        );
    }
    finish(canvas, view)
}

/// The progress bar and the rider's disc over the chart.
fn finish(mut canvas: Canvas, view: &View) -> Canvas {
    let frame = view.climb();
    if let Some(done) = frame.done {
        let mut share = Text::<12>::new();
        let _ = write!(share, "{}% UP", (done + 5) / 10);
        let label = canvas.fit(
            &[&CAPTION],
            share.as_bytes(),
            RIGHT - LEFT,
            RIGHT,
            BAR_BOTTOM,
            Align::Right,
            WHITE,
        );
        let right = label.left - 6;
        let track = Rect {
            left: LEFT,
            top: BAR_BOTTOM - BAR_HEIGHT,
            right,
            bottom: BAR_BOTTOM,
        };
        canvas.shape(Shape::Bar {
            rect: track,
            radius: 3,
            color: GRID,
        });
        let filled = (right - LEFT) * usize::from(done) / 1000;
        if filled >= BAR_HEIGHT {
            canvas.shape(Shape::Bar {
                rect: Rect {
                    right: LEFT + filled,
                    ..track
                },
                radius: 3,
                color: WHITE,
            });
        }
    }
    rider(&mut canvas, frame);
    canvas
}

/// The title on the left and the category, or NO MAP, on the right.
fn head(canvas: &mut Canvas, view: &View, situation: Situation) {
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
    let title = canvas.fit(
        &[&CAPTION],
        title.as_bytes(),
        RIGHT - LEFT,
        LEFT,
        HEAD,
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
            &[&CAPTION],
            text,
            RIGHT.saturating_sub(title.right() + 8),
            RIGHT,
            HEAD,
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

/// The grade, centred in its box, then power and heart rate in cells, each
/// with its zone on the caption's line.
fn readings(canvas: &mut Canvas, view: &View) {
    let mut now = Text::<12>::new();
    grade(&mut now, view.sample.gradient_tenths.map(i32::from));
    let faces: [&'static Face; 3] = [&HERO, &LARGE, &VALUE];
    let line = Line::<24>::fit(&faces, now.as_bytes(), RIGHT - LEFT);
    let cap = line.face().cap as usize;
    let base = (BOX_TOP + BAND_TOP - 6 + cap) / 2;
    canvas.run(crate::screens::canvas::on_baseline(
        line,
        (LEFT + RIGHT) / 2,
        base,
        Align::Center,
        GRADE,
    ));
    let power = view.power_3s;
    let heart = view.sample.heart_bpm;
    let rows = [
        (
            &b"POWER"[..],
            power,
            POWER,
            power.map_or(0, |w| view.profile.power_zone(w)),
        ),
        (
            &b"HEART"[..],
            heart,
            HEART,
            heart.map_or(0, |bpm| view.profile.heart_zone(bpm)),
        ),
    ];
    for (column, (caption, value, color, zone)) in rows.into_iter().enumerate() {
        let (left, right) = if column == 0 {
            (LEFT, MIDDLE - COLUMN_GAP)
        } else {
            (MIDDLE, RIGHT)
        };
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
        let zone = canvas.fit(
            &[&CAPTION],
            name.as_bytes(),
            right - left,
            right,
            READINGS + 12,
            Align::Right,
            zone_color(zone),
        );
        canvas.caption(
            caption,
            zone.left.saturating_sub(left + 4),
            left,
            READINGS + 12,
            Align::Left,
        );
        let mut text = Text::<12>::new();
        whole(&mut text, value);
        canvas.fit(
            &[&VALUE, &BODY_FACE],
            text.as_bytes(),
            right - left,
            left,
            READINGS + 41,
            Align::Left,
            color,
        );
    }
}

/// A caption and its value on one line of the right column: two of them
/// stack where one cell would be.
fn stacked(canvas: &mut Canvas, caption: &[u8], value: &Text<12>, base: usize) {
    let width = RIGHT - MIDDLE;
    let run = canvas.fit(
        &[&BODY_FACE],
        value.as_bytes(),
        width,
        RIGHT,
        base,
        Align::Right,
        GRADE,
    );
    canvas.caption(
        caption,
        run.left.saturating_sub(MIDDLE + 4),
        MIDDLE,
        base,
        Align::Left,
    );
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
            cy: 4 * (CHART_TOP + usize::from(top)) as i32 + 2,
            radius: RIDER_RADIUS,
            color: WHITE,
        });
    }
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

/// One chart column: its altitude, steepness and whether it is dimmed.
type Column = (i32, Steepness, bool);

/// Scale `columns` (from `first` on) into the frame's rows.
fn fill(frame: &mut Frame, first: usize, columns: &[Column]) {
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
    let height = (frame.bottom - CHART_TOP) as i32;
    // Leave room above the highest point for the rider's disc.
    let rows = height - 4;
    for (index, &(z, steepness, dim)) in columns.iter().enumerate() {
        let filled = 1 + (z - floor) * rows / span;
        frame.tops[first + index] = (height - filled) as u8;
        frame.bands[first + index] = steepness;
        frame.dim[first + index] = dim;
    }
}

/// Precompute every chart column for the road the page shows.
pub(super) fn frame(view: &View) -> Frame {
    let mut frame = Frame::default();
    let lent = view.climb_lent();
    let odometer = view.totals.distance_mm();
    let mut columns = [(0, Steepness::Easy, false); PROFILE_LEN];
    match situation(view) {
        Situation::Ahead(pin) | Situation::OnMap(pin) => {
            let on = odometer >= pin.foot_mm;
            frame.bottom = BARRED_BOTTOM;
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
            let last = PROFILE_LEN as u64 - 1;
            for (column, slot) in columns.iter_mut().enumerate() {
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
            fill(&mut frame, 0, &columns);
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
            if len > 0 {
                frame.rider = (PROFILE_LEN - 1) as u8;
                fill(&mut frame, PROFILE_LEN - len, &columns[..len]);
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
            let last = PROFILE_LEN - 1;
            for (column, slot) in columns.iter_mut().enumerate() {
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
            fill(&mut frame, 0, &columns);
        }
    }
    frame
}

/// The profile fill and its baseline.
pub(super) fn paint_chart(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if y == frame.bottom {
        row.span(CHART_LEFT..CHART_RIGHT, GRID);
    }
    if !(CHART_TOP..frame.bottom).contains(&y) {
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
    if y == frame.bottom {
        return GRID;
    }
    if !(CHART_TOP..frame.bottom).contains(&y) {
        return BLACK;
    }
    let column = x - CHART_LEFT;
    let top = frame.tops[column];
    if top != EMPTY && y - CHART_TOP >= usize::from(top) {
        shade(frame, column)
    } else {
        BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::climb_ahead::{self, tests::ahead};
    use crate::screens::workout::Page;
    use std::boxed::Box;

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
                trace: &ride.trace,
                forecast: &ride.forecast,
                pinned: ride.pinned.as_ref(),
            });
            view
        }
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
            "POWER",
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
        assert_eq!(
            canvas.find(&name).map(|run| run.color),
            Some(zone_color(zone))
        );
        // The band under the readings takes the grade's steepness colour.
        assert!(canvas.shapes.iter().any(|shape| matches!(
            shape,
            Shape::Bar { color, .. } if *color == ZONES[5]
        )));
        // The grade sits in the middle of its box.
        let grade = canvas.find("10.5%").unwrap();
        let middle = (BOX_TOP + BAND_TOP - 6) / 2;
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
        let height = CHART_BOTTOM - CHART_TOP;
        assert_eq!(
            usize::from(frame.tops[10]),
            height - 1,
            "the start is lowest"
        );
        assert!(frame.bands[10..50].iter().all(|&b| b == Steepness::Easy));
        assert!(frame.bands[125..165].iter().all(|&b| b == Steepness::Hard));
        assert!(frame.bands[185..].iter().all(|&b| b == Steepness::Descent));
        assert_paints_reference(&view);
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
        assert_paints_reference(&view);
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
        assert_eq!(frame.bottom, BARRED_BOTTOM);
        // The road to the foot is dim, the climb in full colour.
        let foot = 300 * (PROFILE_LEN - 1) / 1500;
        assert!(frame.dim[..foot - 2].iter().all(|&d| d));
        assert!(frame.dim[foot + 2..foot + 100].iter().all(|&d| !d));
        assert!(
            frame.bands[foot + 10..foot + 100]
                .iter()
                .all(|&b| b == Steepness::Steep)
        );
        assert_paints_reference(&view);
    }

    #[test]
    fn on_the_map_climb_the_rider_moves_along_the_whole_climb() {
        let road = |m: f64| (m - 300.0).clamp(0.0, 1000.0) * 0.08;
        let ride = Ride::new().mapped(1, 5, road).on(900);
        // 600 m up, 400 m to go: the map sees the rest from here.
        let ride = ride.mapped(2, 5, |m: f64| m.min(400.0) * 0.08);
        let mut view = ride.view();
        view.prepare();
        let canvas = view.canvas();
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
        let frame = view.climb();
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
        let x = CHART_LEFT + rider;
        assert_eq!(
            view.pixel(x, CHART_TOP + usize::from(frame.tops[rider])),
            WHITE
        );
        // The bar is filled to the share climbed.
        let fill = canvas.shapes.iter().find_map(|shape| match shape {
            Shape::Bar { rect, color, .. } if *color == WHITE => Some(*rect),
            _ => None,
        });
        let fill = fill.expect("the climbed part of the bar");
        let track = canvas.find("60% UP").unwrap().left - 6 - LEFT;
        assert_eq!(fill.right - LEFT, track * 600 / 1000);
        assert_paints_reference(&view);
    }

    /// Every road, with the widest values, lays out whole.
    #[test]
    fn extreme_climbs_never_overlap_or_clip() {
        type Case = (&'static str, fn() -> Ride);
        let cases: [Case; 7] = [
            ("nothing ridden", Ride::new),
            ("flat", || Ride::new().ridden(4000, |_| 5000)),
            ("climbing 25%", || {
                Ride::new().ridden(4000, |m| m as i32 * 25 / 10)
            }),
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
                let mut view = ride().view();
                view.sample.gradient_tenths = tenths;
                view.power_3s = watts;
                view.sample.heart_bpm = bpm;
                view.prepare();
                view.canvas().assert_laid_out(
                    BODY,
                    &["POWER", "HEART"],
                    &std::format!("{what} {tenths:?}"),
                );
                assert_paints_reference(&view);
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
}
