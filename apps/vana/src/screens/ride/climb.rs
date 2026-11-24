//! The climb page's road: which stretch it is, the readings it gives, and
//! the profile chart precomputed per frame for `paint_chart`.
use crate::compose::data::{Data, Metric};
use crate::ride::climb::{Climbs, PROFILE_LEN, ProfileSample, STEP_MM, Steepness, Summary};
use crate::ride::climb_ahead::Pinned;
use crate::screens::chrome::{Rect, Row, ZONES, mix};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::{Text, View};
use core::fmt::Write;

/// The most columns the chart has: the whole panel, which a tree may give
/// it.
const COLUMNS: usize =
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.width);
const _: () = assert!(COLUMNS < NO_RIDER as usize, "a column's index is a u8");
/// A flat road spans at least 20 m of chart height, so it stays flat.
pub(super) const MIN_SPAN_DM: i32 = 200;
const NO_RIDER: u8 = u8::MAX;
const FLAT_WINDOW_MM: u64 = 2_000_000;
/// The road behind the rider: its colour's share over the background, in
/// `mix`'s coverage of 15.
const BEHIND_COVERAGE: u8 = 8;

pub const fn color(steepness: Steepness) -> u16 {
    ZONES[zone(steepness)]
}

const fn zone(steepness: Steepness) -> usize {
    match steepness {
        Steepness::Descent => 1,
        Steepness::Easy => 2,
        Steepness::Moderate => 3,
        Steepness::Steep => 4,
        Steepness::Hard => 5,
        Steepness::Severe => 6,
    }
}

/// What the climb page draws from, lent by the ride and the look-ahead.
#[derive(Clone, Copy)]
pub struct Lent<'a> {
    pub climbs: &'a Climbs,
    pub climbing: Option<Summary>,
    /// The map's climb near or under way, on the ride's odometer.
    pub pinned: Option<&'a Pinned>,
}

impl Lent<'static> {
    /// Nothing ridden and no map.
    pub const NONE: Self = Self {
        climbs: &Climbs::NONE,
        climbing: None,
        pinned: None,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stretch {
    Flat,
    Climb,
    Course,
}

#[derive(Clone, Copy)]
enum Situation<'a> {
    Flat,
    Climb(Summary),
    Course(&'a Pinned),
}

/// What the climb page's tiles draw from in one frame.
#[derive(Clone, Copy)]
pub struct Road<'a> {
    pub stretch: Stretch,
    pub frame: &'a Frame,
}

fn situation<'a>(view: &View<'a>) -> Option<Situation<'a>> {
    let lent = view.extra.climb_lent()?;
    Some(match (lent.pinned, lent.climbing) {
        (Some(pin), _) => Situation::Course(pin),
        (None, Some(climb)) => Situation::Climb(climb),
        (None, None) => Situation::Flat,
    })
}

/// The road `view` shows, on the climb page.
pub fn road<'v>(view: &'v View) -> Option<Road<'v>> {
    let stretch = match situation(view)? {
        Situation::Flat => Stretch::Flat,
        Situation::Climb(_) => Stretch::Climb,
        Situation::Course(_) => Stretch::Course,
    };
    Some(Road {
        stretch,
        frame: view.climb_frame()?,
    })
}

pub(crate) fn reading(view: &View, metric: Metric) -> Option<i32> {
    let metres = |dm: i32| (dm + 5).div_euclid(10);
    let odometer = view.totals.distance_mm();
    let int = |mm: u64| i32::try_from(mm / 1000).unwrap_or(i32::MAX);
    match (metric, situation(view)?) {
        (Metric::ClimbDistance, Situation::Climb(climb)) => Some(int(climb.length_mm)),
        (Metric::ClimbGain, Situation::Climb(climb)) => {
            Some(metres(i32::try_from(climb.gain_dm).unwrap_or(i32::MAX)))
        }
        (Metric::ToTop, Situation::Course(pin)) => Some(int(pin.top_mm.saturating_sub(odometer))),
        (Metric::UpToGo, Situation::Course(pin)) => {
            let left = pin.altitude_dm(pin.top_mm) - pin.altitude_dm(odometer.max(pin.foot_mm));
            Some(metres(left.max(0)))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sums {
    pub length_mm: u64,
    pub average_tenths: Option<i32>,
    pub steepest_tenths: Option<i32>,
}

impl Sums {
    pub fn write_length(&self, out: &mut Text<12>) {
        let hundredths = (self.length_mm + 5_000) / 10_000;
        let tenths = (self.length_mm + 50_000) / 100_000;
        let _ = match (hundredths, tenths) {
            (..100, _) => write!(out, "0.{hundredths:02}"),
            (_, ..1000) if !tenths.is_multiple_of(10) => {
                write!(out, "{}.{}", tenths / 10, tenths % 10)
            }
            _ => write!(out, "{}", (self.length_mm + 500_000) / 1_000_000),
        };
        let _ = out.push_str(" km");
    }

    pub fn write_grade(tenths: Option<i32>, out: &mut Text<12>) {
        match tenths {
            Some(tenths) if tenths.abs() >= 100 => {
                let percent = (tenths.clamp(-999, 999) + 5 * tenths.signum()) / 10;
                let _ = write!(out, "{percent}%");
            }
            tenths => super::grade(out, tenths),
        }
    }
}

/// What the climb page precomputes for one frame.
#[derive(Clone)]
pub struct Frame {
    pub chart: Rect,
    bars: [Bar; COLUMNS],
    /// The rider's column, or `NO_RIDER`.
    rider: u8,
    palette: Palette,
    outline: u16,
    outline_rows: u8,
    pub sums: Option<Sums>,
}

/// Each steepness's colour, in `Steepness` order, ahead of the rider and
/// behind.
#[derive(Clone, Copy, Hash)]
struct Palette {
    ahead: [u16; 6],
    behind: [u16; 6],
}

/// The chart's rows hash what they paint: the rider's marker and the
/// summary are the canvas's.
impl core::hash::Hash for Frame {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.chart.hash(state);
        self.bars.hash(state);
        self.palette.hash(state);
        self.outline.hash(state);
        self.outline_rows.hash(state);
    }
}

impl Frame {
    pub const EMPTY: Self = Self {
        chart: Rect::EMPTY,
        bars: [Bar::EMPTY; COLUMNS],
        rider: NO_RIDER,
        palette: Palette {
            ahead: [0; 6],
            behind: [0; 6],
        },
        outline: 0,
        outline_rows: 0,
        sums: None,
    };

    /// Where the rider's marker sits, in quarter pixels: the centre of the
    /// top of the rider's column; none before the rider is on the chart.
    pub fn rider(&self) -> Option<(i32, i32)> {
        let column = usize::from(self.rider);
        let top = self.bars.get(column)?.top()?;
        Some((
            4 * (self.chart.left + column) as i32 + 2,
            4 * (self.chart.top + top) as i32 + 2,
        ))
    }

    fn color(&self, column: usize, line: usize) -> Option<u16> {
        let bar = self.bars[column];
        let top = bar.top().filter(|&top| line >= top)?;
        if line < self.outline_end(column, top) {
            return Some(self.outline);
        }
        let colors = if bar.behind() {
            &self.palette.behind
        } else {
            &self.palette.ahead
        };
        Some(colors[bar.steepness() as usize])
    }

    /// The outline runs down a column to a lower neighbour's top, so it
    /// never breaks where the road steps.
    fn outline_end(&self, column: usize, top: usize) -> usize {
        let neighbour = |at: Option<usize>| {
            at.and_then(|at| self.bars.get(at))
                .and_then(|bar| bar.top())
                .unwrap_or(top)
        };
        (top + usize::from(self.outline_rows))
            .max(neighbour(column.checked_sub(1)))
            .max(neighbour(Some(column + 1)))
    }
}

#[derive(Clone, Copy, Hash)]
struct Bar(u16);

const TOP_BITS: u32 = 9;
const NO_TOP: u16 = (1 << TOP_BITS) - 1;
const BEHIND_BIT: u16 = 1 << (TOP_BITS + 3);
const _: () = assert!(
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.height)
        < NO_TOP as usize
);

impl Bar {
    const EMPTY: Self = Self(NO_TOP);

    fn new(top: usize, steepness: Steepness, behind: bool) -> Self {
        let band = (steepness as u16) << TOP_BITS;
        Self(top as u16 | band | if behind { BEHIND_BIT } else { 0 })
    }

    fn top(self) -> Option<usize> {
        let top = self.0 & NO_TOP;
        (top != NO_TOP).then_some(usize::from(top))
    }

    fn steepness(self) -> Steepness {
        Steepness::ALL[usize::from(self.0 >> TOP_BITS & 0b111)]
    }

    fn behind(self) -> bool {
        self.0 & BEHIND_BIT != 0
    }
}

fn palette(theme: &Theme) -> Palette {
    let ahead = Steepness::ALL.map(|steepness| theme.zone[zone(steepness)]);
    Palette {
        ahead,
        behind: ahead.map(|color| mix(color, theme.bg, BEHIND_COVERAGE)),
    }
}

#[derive(Clone, Copy)]
struct Column {
    altitude_dm: i32,
    steepness: Steepness,
    behind: bool,
}

impl Column {
    const FLAT: Self = Self {
        altitude_dm: 0,
        steepness: Steepness::Easy,
        behind: false,
    };
}

/// The grade of `rise_dm` over `steps` profile steps.
fn grade_tenths(rise_dm: i32, steps: i32) -> i32 {
    (i64::from(rise_dm) * 100_000 / (i64::from(steps) * STEP_MM as i64)) as i32
}

fn fill(frame: &mut Frame, at: &Scaled, columns: &[Column]) {
    let (Some(low), Some(high)) = (
        columns.iter().map(|c| c.altitude_dm).min(),
        columns.iter().map(|c| c.altitude_dm).max(),
    ) else {
        return;
    };
    let range = high - low;
    let span = range.max(MIN_SPAN_DM);
    // A range smaller than the minimum span sits in the lower part, as a road.
    let floor = low - (span - range) / 4;
    let height = frame.chart.height() as i32;
    let headroom = (at.unit(usize::from(LAYOUT.space.marker)) / 2
        + at.unit(usize::from(LAYOUT.space.stroke))) as i32;
    let rows = (height - headroom).max(1);
    for (bar, column) in frame.bars.iter_mut().zip(columns) {
        let filled = 1 + (column.altitude_dm - floor) * (rows - 1) / span;
        *bar = Bar::new((height - filled) as usize, column.steepness, column.behind);
    }
}

fn spread(samples: &[ProfileSample], columns: &mut [Column]) {
    let (Some(first), Some(last)) = (samples.first(), columns.len().checked_sub(1)) else {
        return;
    };
    let reach = samples.len() - 1;
    let mut band = Steepness::of(i32::from(first.grade_tenths));
    for (column, slot) in columns.iter_mut().enumerate() {
        let along = column * reach;
        let (index, into) = (along / last.max(1), (along % last.max(1)) as i32);
        let a = i32::from(samples[index].altitude_dm);
        let b = i32::from(samples[(index + 1).min(reach)].altitude_dm);
        band = band.after(i32::from(samples[index].grade_tenths));
        *slot = Column {
            altitude_dm: a + (b - a) * into / last.max(1) as i32,
            steepness: band,
            behind: true,
        };
    }
}

fn sums(samples: &[ProfileSample]) -> Option<Sums> {
    let (first, last) = (samples.first()?, samples.last()?);
    let steps = samples.len() as i32 - 1;
    let rise = i32::from(last.altitude_dm) - i32::from(first.altitude_dm);
    Some(Sums {
        length_mm: steps as u64 * STEP_MM,
        average_tenths: (steps > 0).then(|| grade_tenths(rise, steps)),
        steepest_tenths: samples
            .iter()
            .map(|s| i32::from(s.grade_tenths))
            .max()
            .filter(|_| steps > 0),
    })
}

pub(super) fn frame(view: &View, chart: Rect) -> Frame {
    let at = &view.device.scaled;
    let theme = view.theme();
    let mut frame = Frame {
        chart,
        palette: palette(theme),
        outline: theme.text,
        outline_rows: at.unit(usize::from(LAYOUT.space.stroke)) as u8,
        ..Frame::EMPTY
    };
    let count = chart.width().min(COLUMNS);
    let Some(situation) = situation(view) else {
        return frame;
    };
    if count < 2 {
        return frame;
    }
    let lent = view.climb_lent();
    let odometer = view.totals.distance_mm();
    let mut columns = [Column::FLAT; COLUMNS];
    let columns = &mut columns[..count];
    let last = count - 1;
    if let Situation::Course(pin) = situation {
        let start = odometer.min(pin.foot_mm);
        let end = pin.top_mm.max(start + STEP_MM);
        for (column, slot) in columns.iter_mut().enumerate() {
            let at = start + column as u64 * (end - start) / last as u64;
            *slot = Column {
                altitude_dm: pin.altitude_dm(at),
                steepness: pin.steepness(at),
                behind: at < odometer,
            };
        }
        let rider = (odometer - start) * last as u64 / (end - start);
        frame.rider = rider.min(last as u64) as u8;
        let climb = pin.summary();
        frame.sums = Some(Sums {
            length_mm: climb.length_mm,
            average_tenths: climb.grade_tenths(),
            steepest_tenths: climb.steepest_tenths,
        });
        fill(&mut frame, at, columns);
        return frame;
    }
    let mut samples = [ProfileSample::default(); PROFILE_LEN];
    // Only samples the latched odometer has reached, so the summary's
    // words change on the second as the other numbers do.
    let unseen = lent.climbs.newest_mm().map_or(0, |newest| {
        newest.saturating_sub(odometer).div_ceil(STEP_MM) as usize
    });
    let len = lent.climbs.profile(&mut samples).saturating_sub(unseen);
    let window = match situation {
        Situation::Climb(climb) => climb.length_mm,
        _ => FLAT_WINDOW_MM,
    };
    let kept = ((window / STEP_MM) as usize + 1).min(len);
    let samples = &samples[len - kept..len];
    frame.sums = match situation {
        // The whole climb, which may reach past the samples drawn.
        Situation::Climb(climb) => Some(Sums {
            length_mm: climb.length_mm,
            average_tenths: climb.grade_tenths(),
            steepest_tenths: climb.steepest_tenths,
        }),
        _ => sums(samples),
    };
    if samples.len() < 2 {
        return frame;
    }
    spread(samples, columns);
    frame.rider = last as u8;
    fill(&mut frame, at, columns);
    frame
}

pub(super) fn chart_parts(frame: &Frame, parts: &mut firmware_shell::shell::Parts) {
    parts.add(frame.chart.top..frame.chart.bottom, frame);
}

pub(super) fn paint_chart(frame: &Frame, row: &mut Row) {
    let chart = frame.chart;
    let y = row.y();
    if !(chart.top..chart.bottom).contains(&y) {
        return;
    }
    let line = y - chart.top;
    let width = chart.width().min(COLUMNS);
    let mut column = 0;
    while column < width {
        let Some(color) = frame.color(column, line) else {
            column += 1;
            continue;
        };
        let start = column;
        column += 1;
        while column < width && frame.color(column, line) == Some(color) {
            column += 1;
        }
        row.span(chart.left + start..chart.left + column, color);
    }
}

#[cfg(test)]
pub(super) fn chart_pixel(frame: &Frame, x: usize, y: usize) -> Option<u16> {
    let chart = frame.chart;
    if !(chart.left..chart.left + chart.width().min(COLUMNS)).contains(&x)
        || !(chart.top..chart.bottom).contains(&y)
    {
        return None;
    }
    frame.color(x - chart.left, y - chart.top)
}

#[cfg(test)]
mod tests;
