//! The climb page's road and its profile chart. What the page shows
//! depends on the road (`Situation`):
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
//! `frame` precomputes every chart column once per frame in the profile
//! tile's box; `paint_chart` only looks them up.
use super::GRID;
use crate::ride::climb::{Category, Climbs, PROFILE_LEN, ProfileSample, STEP_MM, Steepness};
use crate::ride::climb_ahead::{Forecast, Pinned};
use crate::ride::elevation::Trace;
#[cfg(test)]
use crate::screens::chrome::BLACK;
use crate::screens::chrome::{BLACK as DIM_UNDER, Rect, Row, ZONES, mix};
use crate::screens::scaled::Scaled;
use crate::screens::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;
use firmware_shell::shell::Parts;

/// The profile's columns: one per profile sample at scale 1.
pub const fn chart_width(at: &Scaled) -> usize {
    at.unit(PROFILE_LEN)
}

/// The most columns the chart has: the whole panel, which a tree may give
/// it.
const COLUMNS: usize =
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.width);
const _: () = assert!(COLUMNS < NO_RIDER as usize, "a column's index is a u8");
const _: () = assert!(COLUMNS >= PROFILE_LEN);
/// The fewest rows the chart keeps over the progress bar.
pub const MIN_CHART_UNITS: usize = 64;
/// A flat road spans at least 20 m of chart height, so it stays flat.
pub(super) const MIN_SPAN_DM: i32 = 200;
const NO_RIDER: u8 = u8::MAX;
/// The rider's disc: 6 px across, in quarter pixels.
pub const RIDER_RADIUS: usize = 12;
/// The map's road past the top: a tenth of the climb, at least 200 m.
const AFTER_MIN_MM: u64 = 200_000;

/// Steepness colours, from the zone ramp: blue down, then green, yellow,
/// orange, red and purple.
pub const fn color(steepness: Steepness) -> u16 {
    ZONES[match steepness {
        Steepness::Descent => 1,
        Steepness::Easy => 2,
        Steepness::Moderate => 3,
        Steepness::Steep => 4,
        Steepness::Hard => 5,
        Steepness::Severe => 6,
    }]
}

/// Under a chart with a progress bar: where the chart ends, the bar, and
/// the baseline of FOOT, the share and TOP under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Barred {
    pub chart_bottom: usize,
    pub bar: Rect,
    pub labels_base: usize,
}

/// Rows from the chart to the bar, the bar's own, from the bar to the
/// labels' capitals, and under the labels' baseline.
const OVER_BAR_UNITS: usize = 5;
const BAR_UNITS: usize = 6;
const UNDER_BAR_UNITS: usize = 4;
const UNDER_LABELS_UNITS: usize = 3;

/// The rows the bar and its labels take under the chart.
pub const fn barred_units(at: &Scaled) -> usize {
    at.unit(OVER_BAR_UNITS + BAR_UNITS + UNDER_BAR_UNITS + UNDER_LABELS_UNITS)
        + at.face(FaceRole::Caption).cap as usize
}

/// The bar and its labels at the foot of `chart`, the profile's box.
pub const fn barred(chart: Rect, at: &Scaled) -> Barred {
    let labels_base = chart.bottom - at.unit(UNDER_LABELS_UNITS);
    let caption_cap = at.face(FaceRole::Caption).cap as usize;
    let bar_bottom = labels_base - caption_cap - at.unit(UNDER_BAR_UNITS);
    let bar_top = bar_bottom - at.unit(BAR_UNITS);
    Barred {
        chart_bottom: bar_top - at.unit(OVER_BAR_UNITS),
        bar: Rect {
            top: bar_top,
            bottom: bar_bottom,
            ..chart
        },
        labels_base,
    }
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
pub enum Situation<'a> {
    Ahead(&'a Pinned),
    OnMap(&'a Pinned),
    /// The barometer's climb under way: its length and gain so far.
    OnClimb {
        length_mm: u64,
        gain_dm: u32,
    },
    Clear,
}

/// What the climb page's tiles draw from in one frame.
#[derive(Clone, Copy)]
pub struct Road<'a> {
    pub situation: Situation<'a>,
    pub odometer_mm: u64,
    pub altitude_dm: Option<i32>,
    pub ascent_dm: u32,
    pub terrain: bool,
    pub frame: &'a Frame,
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

/// The road `view` shows, on the climb page.
pub fn road<'v>(view: &'v View) -> Option<Road<'v>> {
    let frame = view.climb_frame()?;
    Some(Road {
        situation: situation(view),
        odometer_mm: view.totals.distance_mm(),
        altitude_dm: view.totals.altitude_dm(),
        ascent_dm: view.totals.ascent_dm(),
        terrain: view.climb_lent().forecast.map,
        frame,
    })
}

/// What the climb page precomputes for one frame.
#[derive(Clone, Hash)]
pub struct Frame {
    /// The profile's box: the chart's columns from its left, its rows from
    /// its top.
    pub chart: Rect,
    /// The row under the fill, which a grid line draws.
    pub bottom: usize,
    bars: [Bar; COLUMNS],
    /// The rider's column, or `NO_RIDER`.
    rider: u8,
    /// The share of the climb climbed, 0..=1000, for the progress bar.
    pub done: Option<u16>,
}

impl Frame {
    pub const EMPTY: Self = Self {
        chart: Rect::EMPTY,
        bottom: 0,
        bars: [Bar::EMPTY; COLUMNS],
        rider: NO_RIDER,
        done: None,
    };

    /// Where the rider's disc sits, in quarter pixels: the centre of the
    /// top of the rider's column; none before the rider is on the chart.
    pub fn rider(&self) -> Option<(i32, i32)> {
        let column = usize::from(self.rider);
        let top = self.bars.get(column)?.top()?;
        Some((
            4 * (self.chart.left + column) as i32 + 2,
            4 * (self.chart.top + top) as i32 + 2,
        ))
    }
}

/// One chart column in 16 bits, so a whole panel's width keeps the view
/// small: its top row, its steepness, and whether it is drawn at 25% (road
/// outside the climb, or already climbed).
#[derive(Clone, Copy, Hash)]
struct Bar(u16);

const TOP_BITS: u32 = 9;
const NO_TOP: u16 = (1 << TOP_BITS) - 1;
const DIM_BIT: u16 = 1 << (TOP_BITS + 3);
const _: () = assert!(
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.height)
        < NO_TOP as usize
);

impl Bar {
    /// A column with no profile sample yet.
    const EMPTY: Self = Self(NO_TOP);

    fn new(top: usize, steepness: Steepness, dim: bool) -> Self {
        let band = (steepness as u16) << TOP_BITS;
        Self(top as u16 | band | if dim { DIM_BIT } else { 0 })
    }

    fn top(self) -> Option<usize> {
        let top = self.0 & NO_TOP;
        (top != NO_TOP).then_some(usize::from(top))
    }

    fn steepness(self) -> Steepness {
        Steepness::ALL[usize::from(self.0 >> TOP_BITS & 0b111)]
    }

    fn dim(self) -> bool {
        self.0 & DIM_BIT != 0
    }
}

pub fn metres(value: &mut Text<12>, dm: i32) {
    super::whole(value, Some((dm + 5).div_euclid(10)));
}

/// Kilometres with one decimal: `0.4`, `12.3`.
pub fn tenths_km(value: &mut Text<12>, mm: u64) {
    let tenths = (mm + 50_000) / 100_000;
    let _ = write!(value, "{}.{}", tenths / 10, tenths % 10);
}

/// A climb's category name in the colour of its average steepness; none
/// for a climb too small to have one.
pub fn category(gain_dm: u32, grade_tenths: Option<i32>) -> (Option<&'static [u8]>, u16) {
    let category = Category::of(gain_dm);
    let color = color(Steepness::of(grade_tenths.unwrap_or(0)));
    match category {
        Category::Uncategorised => (None, color),
        category => (Some(category.name()), color),
    }
}

/// A column's colour: its steepness, at 25% when dimmed.
fn shade(frame: &Frame, column: usize) -> u16 {
    let bar = frame.bars[column];
    let full = color(bar.steepness());
    if bar.dim() {
        mix(full, DIM_UNDER, 4)
    } else {
        full
    }
}

/// One chart column: its altitude, steepness and whether it is dimmed.
type Column = (i32, Steepness, bool);

/// Scale `columns` (from `first` on) into the frame's rows.
fn fill(frame: &mut Frame, at: &Scaled, first: usize, columns: &[Column]) {
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
    let height = (frame.bottom - frame.chart.top) as i32;
    // Leave room above the highest point for the rider's disc.
    let rows = height - at.unit(4) as i32;
    for (index, &(z, steepness, dim)) in columns.iter().enumerate() {
        let filled = 1 + (z - floor) * rows / span;
        frame.bars[first + index] = Bar::new((height - filled) as usize, steepness, dim);
    }
}

/// Picks one of the profile's `len` samples, held at the front of
/// `columns`, for each of `count` chart columns, newest last, and moves the
/// picks to the front in place. Returns the first column with a sample.
fn spread_profile(columns: &mut [Column; COLUMNS], len: usize, count: usize) -> Option<usize> {
    let missing = PROFILE_LEN - len;
    let sample = |column: usize| column * (PROFILE_LEN - 1) / (count - 1);
    let first = (0..count).find(|&column| sample(column) >= missing)?;
    let moves = |column: usize| (column - first, sample(column) - missing);
    // Fewer columns than samples read ahead of what they write, more read
    // behind it: copy in the order that reads each sample before it moves.
    if count <= PROFILE_LEN {
        for (into, pick) in (first..count).map(moves) {
            columns[into] = columns[pick];
        }
    } else {
        for (into, pick) in (first..count).rev().map(moves) {
            columns[into] = columns[pick];
        }
    }
    Some(first)
}

/// Precompute every chart column for the road the page shows, in `chart`,
/// the profile tile's box.
pub(super) fn frame(view: &View, chart: Rect) -> Frame {
    let at = &view.device.scaled;
    let count = chart.width();
    let mut frame = Frame {
        chart,
        bottom: chart.bottom,
        ..Frame::EMPTY
    };
    let lent = view.climb_lent();
    let odometer = view.totals.distance_mm();
    let mut columns = [(0, Steepness::Easy, false); COLUMNS];
    match situation(view) {
        Situation::Ahead(pin) | Situation::OnMap(pin) => {
            let on = odometer >= pin.foot_mm;
            frame.bottom = barred(chart, at).chart_bottom;
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
            fill(&mut frame, at, 0, &columns[..count]);
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
                fill(&mut frame, at, first, &columns[..count - first]);
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
            fill(&mut frame, at, 0, &columns[..count]);
        }
    }
    frame
}

pub(super) fn chart_parts(frame: &Frame, parts: &mut Parts) {
    parts.add(frame.chart.top..frame.bottom + 1, frame);
}

pub(super) fn paint_chart(frame: &Frame, row: &mut Row) {
    let chart = frame.chart;
    let y = row.y();
    if y == frame.bottom && chart.width() > 0 {
        row.span(chart.left..chart.right, GRID);
    }
    if !(chart.top..frame.bottom).contains(&y) {
        return;
    }
    let line = y - chart.top;
    // Runs of one colour fill as spans.
    let mut column = 0;
    while column < chart.width() {
        let filled = |c: usize| frame.bars[c].top().is_some_and(|top| line >= top);
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
pub(super) fn chart_pixel(frame: &Frame, x: usize, y: usize) -> u16 {
    let chart = frame.chart;
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
    if frame.bars[column]
        .top()
        .is_some_and(|top| y - chart.top >= top)
    {
        shade(frame, column)
    } else {
        BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::ScreenId;
    use crate::compose::registry::{CLIMB_PROFILE, STATUS};
    use crate::ride::climb_ahead::{self, tests::ahead};
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::{GRADE, WHITE, zone_color};
    use crate::screens::scaled::BASE_PANEL;
    use crate::screens::workout::Page;
    use std::boxed::Box;
    use std::vec::Vec;

    impl Frame {
        /// Each column's top row, `EMPTY` where it has no sample.
        fn tops(&self) -> Vec<u16> {
            let top = |bar: &Bar| bar.top().map_or(EMPTY, |top| top as u16);
            self.bars.iter().map(top).collect()
        }

        fn bands(&self) -> Vec<Steepness> {
            self.bars.iter().map(|bar| bar.steepness()).collect()
        }

        fn dim(&self) -> Vec<bool> {
            self.bars.iter().map(|bar| bar.dim()).collect()
        }
    }

    const EMPTY: u16 = NO_TOP;

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
            view.compose_main();
            view
        }
    }

    /// The chart as the page prepared it.
    fn chart(view: &View) -> Frame {
        view.climb_frame().expect("the climb page's chart").clone()
    }

    /// The per-pixel reference models only the 240 × 320 ride page.
    fn has_reference(device: &crate::screens::Device) -> bool {
        device.scaled.panel == BASE_PANEL
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
        // The grade has a line of its own over power and heart rate, which
        // share theirs, power on the left.
        let grade = canvas.find("10.5%").unwrap();
        let (power, heart) = (
            canvas.find("POWER 3S").unwrap(),
            canvas.find("HEART").unwrap(),
        );
        assert!(grade.bottom() <= power.top && power.top == heart.top);
        assert!(power.left < heart.left);
    }

    #[test]
    fn with_no_climb_the_whole_ride_is_the_profile() {
        let mut view = Ride::new().ridden(3400, hill).view();
        view.prepare();
        let frame = chart(&view);
        assert!(view.canvas().shows("NO CLIMB") && view.canvas().shows("NO MAP"));
        // The whole ride spans the chart, the rider at its end.
        assert!(
            frame.tops()[..frame.chart.width()]
                .iter()
                .all(|&top| top != EMPTY)
        );
        assert_eq!(usize::from(frame.rider), PROFILE_LEN - 1);
        assert_eq!(frame.done, None);
        assert!(frame.dim().iter().all(|&d| !d));
        let height = frame.chart.height();
        assert_eq!(
            usize::from(frame.tops()[10]),
            height - 1,
            "the start is lowest"
        );
        assert!(frame.bands()[10..50].iter().all(|&b| b == Steepness::Easy));
        assert!(
            frame.bands()[125..165]
                .iter()
                .all(|&b| b == Steepness::Hard)
        );
        assert!(
            frame.bands()[185..frame.chart.width()]
                .iter()
                .all(|&b| b == Steepness::Descent)
        );
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
        let frame = chart(&view);
        assert_eq!(usize::from(frame.rider), PROFILE_LEN - 1);
        assert_eq!(frame.done, None, "no top, no bar");
        // The flat before the climb is dim; the climb itself is not.
        let first = PROFILE_LEN - 130;
        assert!(frame.dim()[first..first + 40].iter().all(|&d| d));
        assert!(frame.dim()[PROFILE_LEN - 60..].iter().all(|&d| !d));
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
        let frame = chart(&view);
        assert_eq!(frame.rider, 0);
        assert_eq!(frame.done, Some(0));
        let at = &view.device.scaled;
        assert_eq!(frame.bottom, barred(frame.chart, at).chart_bottom);
        // The road to the foot is dim, the climb in full colour.
        let foot = 300 * (PROFILE_LEN - 1) / 1500;
        assert!(frame.dim()[..foot - 2].iter().all(|&d| d));
        assert!(frame.dim()[foot + 2..foot + 100].iter().all(|&d| !d));
        assert!(
            frame.bands()[foot + 10..foot + 100]
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
        view.prepare();
        let frame = chart(&view);
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
        // The chart starts at the foot: the rider is 600 m into 1.2 km of it.
        let rider = usize::from(frame.rider);
        assert!((95..=105).contains(&rider), "{rider}");
        assert!(frame.dim()[..rider - 1].iter().all(|&d| d), "climbed");
        assert!(
            frame.dim()[rider + 1..rider + 60].iter().all(|&d| !d),
            "ahead"
        );
        assert_eq!(frame.done, Some(600));
        // The rider's disc sits on the profile at the rider's column.
        let x = frame.chart.left + rider;
        assert_eq!(
            scene.pixel(x, frame.chart.top + usize::from(frame.tops()[rider])),
            WHITE
        );
        // The bar is filled to the share climbed.
        let fill = canvas.shapes.iter().find_map(|shape| match shape {
            Shape::Bar { rect, color, .. } if *color == WHITE => Some(*rect),
            _ => None,
        });
        let fill = fill.expect("the climbed part of the bar");
        assert_eq!(fill.left, frame.chart.left);
        assert_eq!(
            fill.right - frame.chart.left,
            frame.chart.width() * 600 / 1000
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
                for device in crate::screens::panels::ALL {
                    for radar in [false, true] {
                        let mut view = ride().view();
                        view.device = device;
                        view.radar_paired = radar;
                        view.sample.gradient_tenths = tenths;
                        view.power_3s = watts;
                        view.sample.heart_bpm = bpm;
                        view.compose_main();
                        view.prepare().check_layout();
                        let panel = device.scaled.panel;
                        let what = std::format!("{what} {tenths:?} on {panel:?}");
                        let canvas = view.canvas();
                        assert!(canvas.shows("POWER 3S") && canvas.shows("HEART"), "{what}");
                        let frame = chart(&view);
                        let columns = frame.chart.width();
                        assert_eq!(columns, chart_width(&device.scaled), "{what}");
                        assert!(frame.tops()[columns..].iter().all(|&top| top == EMPTY));
                        if frame.rider != NO_RIDER {
                            assert!(usize::from(frame.rider) < columns, "{what}");
                        }
                        let want = ["POWER 3S", "HEART"];
                        let page = device.scaled.screen();
                        canvas.assert_laid_out_at(&device.scaled, page, &want, &what);
                        if has_reference(device) {
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

    /// A tree may grow the profile over the whole panel: on every panel,
    /// in every situation, the chart fills that box and paints every row
    /// as its per-pixel reference does.
    #[test]
    fn the_profile_at_the_largest_box_accept_allows_paints_on_every_panel() {
        type Case = (&'static str, fn() -> Ride);
        let cases: [Case; 6] = [
            ("nothing ridden", Ride::new),
            ("two samples", || Ride::new().ridden(10, |m| m as i32 * 2)),
            ("climbing 25%", || {
                Ride::new().ridden(4000, |m| m as i32 * 25 / 10)
            }),
            ("climb under way", || Ride::new().ridden(1300, hill)),
            ("far HC", || {
                Ride::new().mapped(1, 10, |m| (m - 450.0).max(0.0) * 0.25)
            }),
            ("on HC", || {
                Ride::new()
                    .mapped(1, 10, |m| (m - 100.0).max(0.0) * 0.25)
                    .on(5000)
            }),
        ];
        for (what, ride) in cases {
            for device in crate::screens::panels::ALL {
                let rect = crate::screens::ride::largest_box(device, CLIMB_PROFILE);
                let mut view = ride().view();
                view.device = device;
                let frame = frame(&view, rect);
                let what = std::format!("{what} on {:?}", device.scaled.panel);
                let rows = frame.bottom - rect.top;
                let columns = &frame.tops()[..rect.width()];
                assert!(
                    columns
                        .iter()
                        .all(|&top| top == EMPTY || usize::from(top) < rows)
                );
                if frame.rider != NO_RIDER {
                    assert!(usize::from(frame.rider) < rect.width(), "{what}");
                }
                for y in rect.top..rect.bottom {
                    let mut pixels = [BLACK; crate::screens::chrome::WIDTH];
                    paint_chart(&frame, &mut Row::new(y, &mut pixels));
                    for (x, &pixel) in pixels.iter().enumerate() {
                        assert_eq!(pixel, chart_pixel(&frame, x, y), "{what}: {x},{y}");
                    }
                }
            }
        }
    }

    /// Wider than the profile has samples, the chart repeats samples in
    /// order, the newest last.
    #[test]
    fn a_profile_wider_than_its_samples_spreads_them_in_order() {
        let len = 150;
        let mut columns = [(0, Steepness::Easy, false); COLUMNS];
        for (index, column) in columns[..len].iter_mut().enumerate() {
            column.0 = index as i32;
        }
        let first = spread_profile(&mut columns, len, COLUMNS).unwrap();
        let spread: Vec<i32> = columns[..COLUMNS - first].iter().map(|c| c.0).collect();
        assert_eq!(spread.first(), Some(&0));
        assert_eq!(spread.last(), Some(&(len as i32 - 1)));
        assert!(
            spread
                .windows(2)
                .all(|pair| pair[1] - pair[0] <= 1 && pair[1] >= pair[0])
        );
    }

    /// The page's tiles on `device`, with and without a radar.
    fn laid(device: &'static crate::screens::Device, radar: bool) -> crate::compose::builtin::Laid {
        let flags = crate::screens::ride::main_flags(false, radar, false, false);
        let choices = crate::compose::builtin::Choices::default();
        crate::compose::builtin::Laid::new(ScreenId::RideClimb, device, choices, flags, None)
            .unwrap()
    }

    /// The balance rule: between the status bar and the profile, the rows
    /// the words leave are even gaps, above the first and below the last as
    /// well as between them, on every panel, with and without a radar.
    #[test]
    fn the_words_gaps_are_even_on_every_panel() {
        for device in crate::screens::panels::ALL {
            for radar in [false, true] {
                let laid = laid(device, radar);
                let bar = laid.box_of(STATUS).unwrap().bottom as u16;
                let profile = laid.box_of(CLIMB_PROFILE).unwrap().top as u16;
                // Tiles side by side share a row; the margins beside the
                // words run their whole height.
                let mut rows: Vec<(u16, u16)> = Vec::new();
                let words = laid
                    .leaves()
                    .iter()
                    .map(|placed| placed.rect)
                    .filter(|rect| {
                        rect.top >= bar
                            && rect.bottom <= profile
                            && rect.height() > 0
                            && !(rect.top == bar && rect.bottom == profile)
                    });
                for rect in words {
                    match rows
                        .iter_mut()
                        .find(|(top, bottom)| rect.top < *bottom && *top < rect.bottom)
                    {
                        Some(row) => *row = (row.0.min(rect.top), row.1.max(rect.bottom)),
                        None => rows.push((rect.top, rect.bottom)),
                    }
                }
                rows.sort();
                let mut edges = std::vec![i32::from(bar)];
                for (top, bottom) in &rows {
                    edges.extend([i32::from(*top), i32::from(*bottom)]);
                }
                edges.push(i32::from(profile));
                let gaps: Vec<i32> = edges.chunks(2).map(|pair| pair[1] - pair[0]).collect();
                let (least, most) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
                assert!(
                    rows.len() == 5 && most - least <= 1 && *least > 0,
                    "radar {radar} on {:?}: gaps {gaps:?}",
                    device.scaled.panel
                );
            }
        }
    }

    /// The profile keeps one column a sample, centred under the words, and
    /// reaches the panel's foot.
    #[test]
    fn the_profile_is_centred_and_reaches_the_foot() {
        for device in crate::screens::panels::ALL {
            for radar in [false, true] {
                let laid = laid(device, radar);
                let profile = laid.box_of(CLIMB_PROFILE).unwrap();
                let at = &device.scaled;
                let left = if radar {
                    at.unit(crate::screens::ride::RADAR_WIDTH)
                } else {
                    0
                };
                assert_eq!(profile.width(), chart_width(at));
                let (before, after) = (profile.left - left, at.panel.width - profile.right);
                assert!(before.abs_diff(after) <= 1, "{before} | {after}");
                assert_eq!(profile.bottom, at.panel.height);
            }
        }
    }

    #[test]
    fn the_page_reads_grade_power_and_heart_rate() {
        let readings = crate::screens::ride::RidePage::Climb
            .readings(crate::screens::ride::slots::Slots::default())
            .unwrap();
        assert_eq!(readings.to_vec(), ["power", "hr", "grade"]);
    }
}
