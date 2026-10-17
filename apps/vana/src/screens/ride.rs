//! The ride screen: pages that MOVE or a swipe steps through.
//!
//! The shared chrome draws the status bar (ride state, clock, page dots) and
//! toasts. This frame adds the radar strip on the left while a radar is
//! paired. A page owns [`BODY`] and the values it prepares once per frame;
//! without a radar the frame centres the body, so pages never move their own
//! coordinates. Keep the least important readings at the bottom: toasts
//! cover `chrome::TOAST` for a moment.
//!
//! To add a page: add a variant to `RidePage` and `RidePage::ALL`, then a
//! module beside `main` with `prepare` and `paint`, dispatched below.
//!
//! The climb page also follows the road: when a climb is confirmed the ride
//! screen switches to it, and when the climb ends it returns to the page the
//! rider was on, unless they changed page during the climb (see
//! `Navigation::follow_climb`).
use super::chrome::{self, CONTENT, Rect};
use super::workout::{Text, View};
use crate::ride::laps::Lap;
use crate::sensors::radar::{self, close};
use core::fmt::Write;

mod climb;
mod lap;
mod main;
mod stats;
mod zones;

pub(super) use zones::{Marks, marks};

pub use climb::Frame as ClimbFrame;

use chrome::{ACCENT, AMBER, BLACK, POWER, Row};
pub(super) use chrome::{GREEN, RED, WHITE};
#[cfg(test)]
use firmware_shell::rendering::text::text;

/// The lap page still draws in the 5x7 font with these colours; it moves to
/// the anti-aliased faces in a later pass.
pub(super) const CYAN: u16 = ACCENT;
pub(super) const DIM: u16 = chrome::SECONDARY;
pub(super) const GRID: u16 = chrome::TERTIARY;

/// The radar strip and its divider.
const RADAR_WIDTH: usize = 28;
/// What a page draws into, beside the radar strip.
pub(super) const BODY: Rect = Rect {
    left: CONTENT.left + RADAR_WIDTH,
    top: CONTENT.top,
    right: CONTENT.right,
    bottom: CONTENT.bottom,
};
/// The body's edges by name, for pages placed from them.
pub(super) const CONTENT_LEFT: usize = BODY.left;
pub(super) const CONTENT_TOP: usize = BODY.top;
pub(super) const CONTENT_RIGHT: usize = BODY.right;
/// The 5x7 pages end above the toast band.
pub(super) const CONTENT_BOTTOM: usize = 285;
pub(super) const BODY_BOTTOM: usize = CONTENT_BOTTOM;
/// Without a radar the body moves left by this much, to the centre.
const CENTRE_SHIFT: usize = RADAR_WIDTH / 2;

/// A ride page, in MOVE order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RidePage {
    /// Live speed, power, heart rate, cadence, distance, time and grade.
    #[default]
    Main,
    /// Ride totals: distance, moving time, averages, maxima, NP, work, ascent.
    Stats,
    /// The open lap against the last finished one.
    Lap,
    /// Grade, the climb under way or the last one, and the elevation profile.
    Climb,
}

impl RidePage {
    pub const ALL: [Self; 4] = [Self::Main, Self::Stats, Self::Lap, Self::Climb];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The page after this one; none after the last.
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    /// The page after this one, wrapping to the first.
    pub fn following(self) -> Self {
        self.next().unwrap_or(Self::ALL[0])
    }

    /// The page before this one, wrapping to the last.
    pub fn preceding(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Formatted readings a 5x7 page prepares once per frame.
pub type Values = [Text<12>; 12];

/// Text the main page lays out once per frame in the anti-aliased faces,
/// and the zones its gauges show.
pub(super) type Labels = heapless::Vec<chrome::Label<12>, 14>;

#[derive(Default)]
pub struct Smooth {
    pub(super) labels: Labels,
    pub(super) zones: [u8; 2],
}

/// Prepare the shown page once per frame: the main page's labels, or the
/// 5x7 pages' values and the climb page's chart.
pub(super) fn prepare(view: &mut View) {
    if view.ride_page == RidePage::Main {
        view.smooth = main::prepare(view);
        return;
    }
    let mut values = core::array::from_fn(|_| Text::new());
    match view.ride_page {
        RidePage::Main => {}
        RidePage::Stats => stats::prepare(view, &mut values),
        RidePage::Lap => lap::prepare(view, &mut values),
        RidePage::Climb => {
            climb::prepare(view, &mut values);
            view.climb = climb::frame(view);
        }
    }
    view.values = values;
}

/// Paint a content row: the radar strip while a radar is paired, and the
/// page in the body, centred without a radar.
pub(super) fn paint(view: &View, row: &mut Row) {
    row.fill(BLACK);
    if view.radar_paired {
        paint_radar(view.radar, &mut row.within(0..RADAR_WIDTH - 2));
        paint_page(view, &mut row.within(BODY.left..BODY.right));
        return;
    }
    // Paint the page where it sits beside a radar, then move it to the centre.
    let mut page = [BLACK; chrome::WIDTH];
    paint_page(
        view,
        &mut Row::new(row.y(), &mut page).within(BODY.left..BODY.right),
    );
    let clip = row.clip();
    let start = (BODY.left - CENTRE_SHIFT).max(clip.start);
    let end = (BODY.right - CENTRE_SHIFT).min(clip.end);
    if start < end {
        row.pixels_mut()[start..end]
            .copy_from_slice(&page[start + CENTRE_SHIFT..end + CENTRE_SHIFT]);
    }
}

/// The 5x7 pages end above the toast band; the main page uses all of it.
fn paint_page(view: &View, row: &mut Row) {
    match view.ride_page {
        RidePage::Main => main::paint(view, row),
        _ if row.y() >= BODY_BOTTOM => {}
        RidePage::Stats => stats::paint(view, row),
        RidePage::Lap => lap::paint(view, row),
        RidePage::Climb => climb::paint(view, row),
    }
}

/// Where the radar track runs: the rider at the top, 200 m at the bottom.
/// It ends above the toast band, so a toast never hides a target.
const TRACK_TOP: usize = BODY.top + 22;
const TRACK_BOTTOM: usize = chrome::TOAST.top - 1 - TARGET_RADIUS;
/// Targets are discs this many rows above and below their range.
const TARGET_RADIUS: usize = 5;
const ARROW_TOP: usize = BODY.top + 8;
const RANGE_MM: u32 = 200_000;

fn target_y(target: radar::Target) -> i32 {
    let span = (TRACK_BOTTOM - TRACK_TOP) as u32;
    (TRACK_TOP as u32 + target.range_mm.min(RANGE_MM) * span / RANGE_MM) as i32
}

fn track_color(status: radar::Status) -> u16 {
    match status {
        radar::Status::Ready => ACCENT,
        radar::Status::Error | radar::Status::ReservedThreat => RED,
        _ => AMBER,
    }
}

/// The radar strip: the arrow, the track, then targets, the first on top.
fn paint_radar(radar: radar::Snapshot, row: &mut Row) {
    let y = row.y();
    if (ARROW_TOP..ARROW_TOP + 11).contains(&y) {
        let half = (y - ARROW_TOP) / 2;
        row.span(12 - half..13 + half, WHITE);
    }
    if (TRACK_TOP..=TRACK_BOTTOM).contains(&y) {
        row.span(11..14, track_color(radar.status));
    }
    for target in radar.targets.iter().rev().flatten() {
        let dy = y as i32 - target_y(*target);
        if dy * dy > (TARGET_RADIUS * TARGET_RADIUS) as i32 {
            continue;
        }
        let mut half = 0;
        while (half + 1) * (half + 1) + dy * dy <= (TARGET_RADIUS * TARGET_RADIUS) as i32 {
            half += 1;
        }
        let color = if close(*target) { RED } else { POWER };
        row.span((12 - half) as usize..(13 + half) as usize, color);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    let x = if view.radar_paired {
        if x < RADAR_WIDTH - 2 {
            return reference_radar_pixel(view.radar, x, y);
        }
        x
    } else {
        x + CENTRE_SHIFT
    };
    if !(BODY.left..BODY.right).contains(&x) {
        return BLACK;
    }
    match view.ride_page {
        RidePage::Main => main::reference_pixel(view, x, y),
        _ if y >= BODY_BOTTOM => BLACK,
        RidePage::Stats => stats::reference_pixel(view, x, y),
        RidePage::Lap => lap::reference_pixel(view, x, y),
        RidePage::Climb => climb::reference_pixel(view, x, y),
    }
}

#[cfg(test)]
fn reference_radar_pixel(radar: radar::Snapshot, x: usize, y: usize) -> u16 {
    for target in radar.targets.into_iter().flatten() {
        let dx = x as i32 - 12;
        let dy = y as i32 - target_y(target);
        if dx * dx + dy * dy <= (TARGET_RADIUS * TARGET_RADIUS) as i32 {
            return if close(target) { RED } else { POWER };
        }
    }
    if (11..=13).contains(&x) && (TRACK_TOP..=TRACK_BOTTOM).contains(&y) {
        return track_color(radar.status);
    }
    let top = ARROW_TOP;
    if (top..top + 11).contains(&y) && (x as i32 - 12).abs() <= (y - top) as i32 / 2 {
        return WHITE;
    }
    BLACK
}

/// A label and the value under it, in a 5x7 page's cell.
pub(super) struct Cell {
    pub(super) left: usize,
    pub(super) top: usize,
    pub(super) label: &'static [u8],
    pub(super) label_scale: usize,
    pub(super) value_top: usize,
    pub(super) value_scale: usize,
}

impl Cell {
    /// The cell's label in DIM over `value` in `color`.
    pub(super) fn paint(&self, row: &mut Row, value: &Text<12>, color: u16) {
        row.text(
            self.left,
            self.value_top,
            self.value_scale,
            value.as_bytes(),
            color,
        );
        row.text(self.left, self.top, self.label_scale, self.label, DIM);
    }

    /// The cell's label in DIM and `value` in WHITE, or none of its pixels.
    #[cfg(test)]
    pub(super) fn pixel(&self, value: &Text<12>, x: usize, y: usize) -> Option<u16> {
        if text(x, y, self.left, self.top, self.label_scale, self.label) {
            return Some(DIM);
        }
        text(
            x,
            y,
            self.left,
            self.value_top,
            self.value_scale,
            value.as_bytes(),
        )
        .then_some(WHITE)
    }
}

/// A finished lap's toast: number, time and average power.
pub fn lap_toast(lap: Lap) -> Text<19> {
    let mut toast = Text::new();
    let mut time = Text::<12>::new();
    lap_time(&mut time, lap.time_ms / 1000);
    let time = time.as_str();
    let written = match lap.avg_power() {
        Some(watts) => write!(toast, "LAP {} {time} {watts}W", lap.number),
        None => write!(toast, "LAP {} {time} --W", lap.number),
    };
    // Past lap 99 and an hour the power no longer fits.
    if written.is_err() {
        toast = Text::new();
        let _ = write!(toast, "LAP {} {time}", lap.number);
    }
    toast
}

/// km/h with one truncated decimal, as the live reading always showed, or "--".
pub(super) fn speed(value: &mut Text<12>, mm_s: Option<u32>) {
    let _ = match mm_s {
        Some(v) => {
            let k = u64::from(v) * 36 / 1000;
            write!(value, "{}.{}", k / 10, k % 10)
        }
        None => value.push_str("--"),
    };
}

/// H:MM:SS, the way a workout clock reads.
pub(super) fn elapsed(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = write!(value, "{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
}

/// HH:MM:SS, as the 5x7 pages show it.
pub(super) fn clock(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = write!(value, "{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
}

/// Kilometres with two rounded decimals, one from 100 km.
pub(super) fn kilometres(value: &mut Text<12>, mm: u64) {
    let hundredths = (mm + 5_000) / 10_000;
    let _ = if hundredths < 10_000 {
        write!(value, "{}.{:02}", hundredths / 100, hundredths % 100)
    } else {
        write!(value, "{}.{}", hundredths / 100, hundredths % 100 / 10)
    };
}

/// Lap time: MM:SS, or H:MM:SS from an hour.
pub(super) fn lap_time(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = if t < 3600 {
        write!(value, "{:02}:{:02}", t / 60, t % 60)
    } else {
        write!(value, "{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    };
}

/// A grade in percent with one decimal, or "--%".
pub(super) fn grade(value: &mut Text<12>, tenths: Option<i32>) {
    let _ = match tenths {
        Some(v) => write!(
            value,
            "{}{}.{}%",
            if v < 0 { "-" } else { "" },
            v.unsigned_abs() / 10,
            v.unsigned_abs() % 10
        ),
        None => value.push_str("--%"),
    };
}

/// A whole reading, or "--" without one.
pub(super) fn whole(value: &mut Text<12>, reading: Option<impl core::fmt::Display>) {
    let _ = match reading {
        Some(v) => write!(value, "{v}"),
        None => value.push_str("--"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_step_in_order_and_wrap_both_ways() {
        assert_eq!(RidePage::default(), RidePage::Main);
        assert_eq!(RidePage::Main.next(), Some(RidePage::Stats));
        assert_eq!(RidePage::Stats.next(), Some(RidePage::Lap));
        assert_eq!(RidePage::Lap.next(), Some(RidePage::Climb));
        assert_eq!(RidePage::Climb.next(), None);
        assert_eq!(RidePage::Climb.following(), RidePage::Main);
        assert_eq!(RidePage::Main.preceding(), RidePage::Climb);
        assert_eq!(RidePage::Stats.preceding(), RidePage::Main);
    }

    /// Radar alerts are safety-critical: even the farthest target is drawn
    /// whole above the band a toast may cover.
    #[test]
    fn radar_targets_stay_clear_of_toasts() {
        for range_mm in [0, 190_000, 200_000, 400_000] {
            let target = radar::Target {
                threat: radar::Threat::Approach,
                side: radar::Side::Behind,
                range_mm,
                closing_speed_cm_s: 0,
            };
            let bottom = target_y(target) + TARGET_RADIUS as i32;
            assert!(bottom < chrome::TOAST.top as i32, "{range_mm} mm");
            let mut snapshot = radar::Snapshot::default();
            snapshot.targets[0] = Some(target);
            for y in chrome::TOAST.top..chrome::HEIGHT {
                for x in 0..RADAR_WIDTH {
                    assert_eq!(reference_radar_pixel(snapshot, x, y), BLACK, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn formats_distance_speed_and_readings() {
        let format = |write: &dyn Fn(&mut Text<12>)| {
            let mut value = Text::new();
            write(&mut value);
            value.as_bytes().to_vec()
        };
        assert_eq!(format(&|v| kilometres(v, 4_999_800)), b"5.00");
        assert_eq!(format(&|v| kilometres(v, 0)), b"0.00");
        assert_eq!(format(&|v| kilometres(v, 99_994_999)), b"99.99");
        assert_eq!(format(&|v| kilometres(v, 123_456_000)), b"123.4");
        assert_eq!(format(&|v| speed(v, Some(8334))), b"30.0");
        assert_eq!(format(&|v| speed(v, Some(8333))), b"29.9");
        assert_eq!(format(&|v| speed(v, None)), b"--");
        assert_eq!(format(&|v| elapsed(v, 3725)), b"1:02:05");
        assert_eq!(format(&|v| lap_time(v, 312)), b"05:12");
        assert_eq!(format(&|v| lap_time(v, 3725)), b"1:02:05");
        assert_eq!(format(&|v| whole(v, Some(42))), b"42");
        assert_eq!(format(&|v| whole(v, None::<u16>)), b"--");
        assert_eq!(format(&|v| grade(v, Some(-35))), b"-3.5%");
        assert_eq!(format(&|v| grade(v, Some(-5))), b"-0.5%");
        assert_eq!(format(&|v| grade(v, Some(120))), b"12.0%");
        assert_eq!(format(&|v| grade(v, None)), b"--%");
    }
}
