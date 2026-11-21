//! The ride's elevation profile on the summary: a filled area of altitude
//! over distance, each column in the steepness colour of the road there
//! (docs/ui.md, Climb steepness).
//!
//! A `Chart` holds one top row and one steepness band per column, worked
//! out from the ride's `Trace` when it changes and kept beside it in
//! `Ride`, so painting a row only looks them up. The summary's regions say
//! where it lies: the fill ends above the region's bottom row, the
//! baseline.
use super::GRID;
use super::climb::MIN_SPAN_DM;
use super::climb::color;
use crate::ride::climb::Steepness;
use crate::ride::elevation::Trace;
use crate::screens::chrome::{Rect, Row};

/// The most columns a chart has on any of `panels`.
const fn columns(panels: &[(usize, usize)]) -> usize {
    crate::screens::scaled::largest!(panels, |at| at.panel.width)
}

/// The most columns a chart has on any panel this build meets.
pub const COLUMNS: usize = columns(crate::screens::limits::PANELS);
/// A column's grade spans the road 100 m each side.
const REACH_MM: u64 = 100_000;

/// The chart's columns, and the altitudes the captions name.
#[derive(Clone, Hash)]
pub struct Chart {
    /// Where it was charted, once it was: the fill ends above its bottom
    /// row, the baseline.
    rect: Option<Rect>,
    /// The lowest and highest altitude in decimetres, none without a trace.
    range_dm: Option<(i32, i32)>,
    tops: [u16; COLUMNS],
    bands: [Steepness; COLUMNS],
}

impl Default for Chart {
    fn default() -> Self {
        Self {
            rect: None,
            range_dm: None,
            tops: [0; COLUMNS],
            bands: [Steepness::Easy; COLUMNS],
        }
    }
}

impl Chart {
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// The lowest and highest altitude drawn, in whole metres.
    pub fn range_m(&self) -> Option<(i32, i32)> {
        self.range_dm
            .map(|(low, high)| ((low + 5).div_euclid(10), (high + 5).div_euclid(10)))
    }

    /// Chart `trace` across `rect`; a ride without altitude leaves it empty.
    pub fn refresh(&mut self, trace: &Trace, rect: Rect) {
        self.rect = Some(rect);
        let len = trace.len();
        self.range_dm = trace.range_dm().filter(|_| len >= 2);
        let Some((low, high)) = self.range_dm else {
            return;
        };
        let (columns, height) = (rect.width(), rect.height() as i32);
        let range = high - low;
        let span = range.max(MIN_SPAN_DM);
        // A flat road sits in the lower part, as on the climb page.
        let floor = low - (span - range) / 4;
        let rows = height - 1;
        let reach = (REACH_MM / trace.step_mm()).max(1) as usize;
        let mut band = None;
        for column in 0..columns {
            let at = column * (len - 1);
            let (index, into) = (at / (columns - 1), (at % (columns - 1)) as i32);
            let next = (index + 1).min(len - 1);
            let (a, b) = (trace.altitude_dm(index), trace.altitude_dm(next));
            let altitude = a + (b - a) * into / (columns as i32 - 1);
            let filled = 1 + (altitude - floor) * rows / span;
            self.tops[column] = (height - filled.clamp(1, height)) as u16;
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

/// The fill and its baseline, on the rows the chart was charted across.
pub fn paint(chart: &Chart, row: &mut Row) {
    let y = row.y();
    let Some(rect) = chart.rect.filter(|_| chart.range_dm.is_some()) else {
        return;
    };
    if y == rect.bottom {
        row.span(rect.left..rect.right, GRID);
    }
    if !(rect.top..rect.bottom).contains(&y) {
        return;
    }
    let line = y - rect.top;
    // Neighbouring filled columns of one colour fill as one span.
    let mut run: Option<(usize, u16)> = None;
    for column in 0..rect.width() {
        let shade = (line >= usize::from(chart.tops[column])).then(|| color(chart.bands[column]));
        if let Some((start, open)) = run
            && shade != Some(open)
        {
            row.span(rect.left + start..rect.left + column, open);
            run = None;
        }
        if run.is_none()
            && let Some(shade) = shade
        {
            run = Some((column, shade));
        }
    }
    if let Some((start, open)) = run {
        row.span(rect.left + start..rect.right, open);
    }
}

/// The per-pixel original of `paint`.
#[cfg(test)]
pub fn pixel(chart: &Chart, x: usize, y: usize) -> Option<u16> {
    let rect = chart.rect.filter(|_| chart.range_dm.is_some())?;
    if !(rect.left..rect.right).contains(&x) {
        return None;
    }
    if y == rect.bottom {
        return Some(GRID);
    }
    let column = x - rect.left;
    ((rect.top..rect.bottom).contains(&y) && y - rect.top >= usize::from(chart.tops[column]))
        .then(|| color(chart.bands[column]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        super::super::summary::chart_on(&crate::screens::C606)
    }

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
        let rect = rect();
        chart.refresh(&Trace::default(), rect);
        assert!(chart.range_m().is_none());
        assert_eq!(pixel(&chart, rect.left + 5, rect.bottom), None);
    }

    #[test]
    fn climbs_fill_to_the_height_and_colour_by_steepness() {
        // 1 km flat, 1 km at 8%, 1 km down at 5%.
        let trace = trace(3000, |m| match m {
            ..1000 => 6000,
            1000..2000 => 6000 + (m as i32 - 1000) * 8 / 10,
            _ => 6800 - (m as i32 - 2000) / 2,
        });
        for device in crate::screens::panels::ALL {
            let rect = super::super::summary::chart_on(device);
            let mut chart = Chart::default();
            chart.refresh(&trace, rect);
            assert_eq!(chart.range_m(), Some((600, 680)));
            let columns = rect.width();
            let peak = columns * 2 / 3;
            assert!(chart.tops[peak - 1] <= 4);
            let low = usize::from(chart.tops[0]);
            assert!(low > rect.height() / 2, "the start is low");
            let at = |from: usize, to: usize| from * columns / 208..to * columns / 208;
            assert!(
                chart.bands[at(10, 50)]
                    .iter()
                    .all(|&b| b == Steepness::Easy)
            );
            assert!(
                chart.bands[at(90, 130)]
                    .iter()
                    .all(|&b| b == Steepness::Steep)
            );
            assert!(
                chart.bands[at(160, 208)]
                    .iter()
                    .all(|&b| b == Steepness::Descent)
            );
        }
    }

    #[test]
    fn the_view_holds_only_a_borrow_of_the_chart() {
        assert_eq!(
            core::mem::size_of::<Option<&Chart>>(),
            core::mem::size_of::<usize>()
        );
    }

    /// A tree may grow the profile over the whole panel: a steady climb
    /// fills from the baseline at its start to the top at its end, on every
    /// panel.
    #[test]
    fn the_chart_at_the_largest_box_accept_allows_paints_on_every_panel() {
        use crate::compose::registry::PROFILE;
        let trace = trace(1000, |m| 6000 + m as i32);
        for device in crate::screens::panels::ALL {
            let rect = super::super::summary::chart_rect(crate::screens::ride::largest_box(
                device, PROFILE,
            ));
            let mut chart = Chart::default();
            chart.refresh(&trace, rect);
            let (first, last) = (rect.left, rect.right - 1);
            let at = |x, y| pixel(&chart, x, y).is_some();
            let what = std::format!("{:?}", device.scaled.panel);
            assert!(
                !at(first, rect.bottom - 2) && at(first, rect.bottom - 1),
                "{what}"
            );
            assert!(
                at(last, rect.top) && (rect.top..rect.bottom).all(|y| at(last, y)),
                "{what}"
            );
            for y in rect.top..=rect.bottom {
                let mut pixels = [0u16; crate::screens::chrome::WIDTH];
                paint(&chart, &mut Row::new(y, &mut pixels));
                for (x, &got) in pixels.iter().enumerate() {
                    assert_eq!(got, pixel(&chart, x, y).unwrap_or(0), "{what} at {x},{y}");
                }
            }
        }
    }
}
