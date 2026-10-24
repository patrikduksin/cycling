//! The ride's elevation profile on the summary: a filled area of altitude
//! over distance, each column in the steepness colour of the road there
//! (docs/ui.md, Climb steepness).
//!
//! A `Chart` holds one top row and one steepness band per column, worked
//! out from the ride's `Trace` when it changes and kept beside it in
//! `Ride`, so painting a row only looks them up.
use super::GRID;
use super::climb::MIN_SPAN_DM;
use super::climb::color;
use crate::ride::climb::Steepness;
use crate::ride::elevation::Trace;
use crate::screens::chrome::Row;

/// The chart spans the summary's text margins.
pub const LEFT: usize = 16;
pub const RIGHT: usize = crate::screens::chrome::WIDTH - 16;
pub const COLUMNS: usize = RIGHT - LEFT;
/// The fill ends above this baseline row.
pub const BOTTOM: usize = 272;
pub const HEIGHT: usize = 128;
pub const TOP: usize = BOTTOM - HEIGHT;
/// A column's grade spans the road 100 m each side.
const REACH_MM: u64 = 100_000;

/// The chart's columns, and the altitudes the captions name.
#[derive(Clone)]
pub struct Chart {
    /// The lowest and highest altitude in decimetres, none without a trace.
    range_dm: Option<(i32, i32)>,
    /// Rows below `TOP` where each column's fill starts.
    tops: [u8; COLUMNS],
    bands: [Steepness; COLUMNS],
}

impl Default for Chart {
    fn default() -> Self {
        Self {
            range_dm: None,
            tops: [HEIGHT as u8; COLUMNS],
            bands: [Steepness::Easy; COLUMNS],
        }
    }
}

impl Chart {
    /// The lowest and highest altitude drawn, in whole metres.
    pub fn range_m(&self) -> Option<(i32, i32)> {
        self.range_dm
            .map(|(low, high)| ((low + 5).div_euclid(10), (high + 5).div_euclid(10)))
    }

    /// Chart `trace`; a ride without altitude leaves it empty.
    pub fn refresh(&mut self, trace: &Trace) {
        let len = trace.len();
        self.range_dm = trace.range_dm().filter(|_| len >= 2);
        let Some((low, high)) = self.range_dm else {
            return;
        };
        let range = high - low;
        let span = range.max(MIN_SPAN_DM);
        // A flat road sits in the lower part, as on the climb page.
        let floor = low - (span - range) / 4;
        let rows = HEIGHT as i32 - 1;
        let reach = (REACH_MM / trace.step_mm()).max(1) as usize;
        let mut band = None;
        for column in 0..COLUMNS {
            let at = column * (len - 1);
            let (index, into) = (at / (COLUMNS - 1), (at % (COLUMNS - 1)) as i32);
            let next = (index + 1).min(len - 1);
            let (a, b) = (trace.altitude_dm(index), trace.altitude_dm(next));
            let altitude = a + (b - a) * into / (COLUMNS as i32 - 1);
            let height = 1 + (altitude - floor) * rows / span;
            self.tops[column] = (HEIGHT as i32 - height.clamp(1, HEIGHT as i32)) as u8;
            let (from, to) = (index.saturating_sub(reach), (index + reach).min(len - 1));
            let rise = i64::from(trace.altitude_dm(to) - trace.altitude_dm(from));
            let run = (to - from) as u64 * trace.step_mm();
            let tenths = if run == 0 {
                0
            } else {
                (rise * 100_000 / run as i64) as i32
            };
            let steepness = band.unwrap_or_else(|| Steepness::of(tenths)).after(tenths);
            band = Some(steepness);
            self.bands[column] = steepness;
        }
    }
}

/// The fill and its baseline, on the chart's rows.
pub fn paint(chart: &Chart, row: &mut Row) {
    let y = row.y();
    if chart.range_dm.is_none() {
        return;
    }
    if y == BOTTOM {
        row.span(LEFT..RIGHT, GRID);
    }
    if !(TOP..BOTTOM).contains(&y) {
        return;
    }
    let line = y - TOP;
    // Neighbouring filled columns of one colour fill as one span.
    let mut run: Option<(usize, u16)> = None;
    for column in 0..COLUMNS {
        let shade = (line >= usize::from(chart.tops[column])).then(|| color(chart.bands[column]));
        if let Some((start, open)) = run
            && shade != Some(open)
        {
            row.span(LEFT + start..LEFT + column, open);
            run = None;
        }
        if run.is_none()
            && let Some(shade) = shade
        {
            run = Some((column, shade));
        }
    }
    if let Some((start, open)) = run {
        row.span(LEFT + start..RIGHT, open);
    }
}

/// The per-pixel original of `paint`.
#[cfg(test)]
pub fn pixel(chart: &Chart, x: usize, y: usize) -> Option<u16> {
    if chart.range_dm.is_none() || !(LEFT..RIGHT).contains(&x) {
        return None;
    }
    if y == BOTTOM {
        return Some(GRID);
    }
    let column = x - LEFT;
    ((TOP..BOTTOM).contains(&y) && y - TOP >= usize::from(chart.tops[column]))
        .then(|| color(chart.bands[column]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(metres: u64, altitude_dm: impl Fn(u64) -> i32) -> Trace {
        let mut trace = Trace::default();
        for m in (0..=metres).step_by(5) {
            trace.add(m * 1000, Some(altitude_dm(m)));
        }
        trace
    }

    #[test]
    fn a_ride_without_altitude_draws_nothing() {
        let mut chart = Chart::default();
        chart.refresh(&Trace::default());
        assert!(chart.range_m().is_none());
        assert_eq!(pixel(&chart, LEFT + 5, BOTTOM), None);
    }

    #[test]
    fn climbs_fill_to_the_height_and_colour_by_steepness() {
        // 1 km flat, 1 km at 8%, 1 km down at 5%.
        let trace = trace(3000, |m| match m {
            ..1000 => 6000,
            1000..2000 => 6000 + (m as i32 - 1000) * 8 / 10,
            _ => 6800 - (m as i32 - 2000) / 2,
        });
        let mut chart = Chart::default();
        chart.refresh(&trace);
        assert_eq!(chart.range_m(), Some((600, 680)));
        let peak = COLUMNS * 2 / 3;
        assert!(chart.tops[peak - 1] <= 4);
        assert!(usize::from(chart.tops[0]) > HEIGHT / 2, "the start is low");
        assert!(chart.bands[10..50].iter().all(|&b| b == Steepness::Easy));
        assert!(chart.bands[90..130].iter().all(|&b| b == Steepness::Steep));
        assert!(chart.bands[160..].iter().all(|&b| b == Steepness::Descent));
    }

    #[test]
    fn the_view_holds_only_a_borrow_of_the_chart() {
        assert_eq!(
            core::mem::size_of::<Option<&Chart>>(),
            core::mem::size_of::<usize>()
        );
    }
}
