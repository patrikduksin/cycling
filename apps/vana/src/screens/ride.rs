//! The ride screen: pages that MOVE or a swipe steps through.
//!
//! The shared chrome draws the status bar (ride state, clock, page dots) and
//! toasts. This frame adds the radar strip on the left while a radar is
//! paired. A page owns [`BODY`] and lays out its text and shapes once per
//! frame; without a radar the frame centres the body, so pages never move
//! their own coordinates. Keep the least important readings at the bottom:
//! toasts cover `chrome::TOAST` for a moment.
//!
//! To add a page: add a variant to `RidePage` and `RidePage::ALL`, then a
//! module beside `main` whose `prepare` returns its canvas, dispatched below.
//!
//! The climb page also follows the road: when a climb is confirmed the ride
//! screen switches to it, and when the climb ends it returns to the page the
//! rider was on, unless they changed page during the climb (see
//! `Navigation::follow_climb`).
use super::canvas::Canvas;
use super::chrome::{self, Align, CONTENT, Rect};
use super::workout::{Text, View};
use crate::ride::laps::Lap;
use crate::sensors::radar::{self, close};
use core::fmt::Write;

pub mod back;
mod climb;
pub mod elevation;
pub(super) mod end;
mod lap;
mod main;
pub(super) mod paused;
mod stats;
pub mod summary;
mod workout;
mod zones;

pub(super) use zones::{Marks, marks};

pub use back::{Arrow, Back, Light};
pub use climb::Frame as ClimbFrame;
pub use elevation::Chart as ElevationChart;
pub use lap::{LapBar, LapBars};
pub use workout::{Chart as WorkoutChart, EXTEND_ROWS, Training, chart as workout_chart};

use chrome::{ACCENT, AMBER, BLACK, BODY as BODY_FACE, POWER, Row, VALUE};
pub(super) use chrome::{RED, WHITE};

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
/// Text margins inside the body, as on the main page.
pub(super) const LEFT: usize = BODY.left + 10;
pub(super) const RIGHT: usize = BODY.right - 8;
/// Where a page's right column of cells starts.
pub(super) const MIDDLE: usize = (LEFT + RIGHT) / 2 + 4;
/// Clear columns between a left cell's ink and the right column.
const COLUMN_GAP: usize = 8;
/// A caption over its value: one band of cells. The next caption clears the
/// value above it by 7 rows.
pub(super) const CELL_PITCH: usize = 48;
const _: () = assert!(
    CELL_PITCH + 12 - chrome::CAPTION.cap as usize > 41 + 6,
    "a caption needs 6 clear rows over the value above it"
);
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
    /// The workout step on now; first, and only while a workout is chosen.
    Workout,
    /// The way back to the start: straight, or along the ride's own track.
    Back,
}

impl RidePage {
    /// The pages of a free ride.
    pub const ALL: [Self; 5] = [Self::Main, Self::Stats, Self::Lap, Self::Climb, Self::Back];
    /// The pages while a workout is chosen or running.
    pub const TRAINING: [Self; 6] = [
        Self::Workout,
        Self::Main,
        Self::Stats,
        Self::Lap,
        Self::Climb,
        Self::Back,
    ];

    pub const fn pages(workout: bool) -> &'static [Self] {
        if workout { &Self::TRAINING } else { &Self::ALL }
    }

    /// A number unique to the page, for touch ownership.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Where the page sits in `pages(workout)`, for the page dots; a page
    /// that is not there counts as the first.
    pub fn position(self, workout: bool) -> usize {
        Self::pages(workout)
            .iter()
            .position(|page| *page == self)
            .unwrap_or(0)
    }

    /// The page after this one in `pages(workout)`, wrapping.
    pub fn following_in(self, workout: bool) -> Self {
        let pages = Self::pages(workout);
        pages[(self.position(workout) + 1) % pages.len()]
    }

    /// The page before this one in `pages(workout)`, wrapping.
    pub fn preceding_in(self, workout: bool) -> Self {
        let pages = Self::pages(workout);
        pages[(self.position(workout) + pages.len() - 1) % pages.len()]
    }

    /// The free-ride page after this one; none after the last.
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.position(false) + 1).copied()
    }

    /// The page after this one in `pages(workout)`; none after the last.
    pub fn next_in(self, workout: bool) -> Option<Self> {
        Self::pages(workout)
            .get(self.position(workout) + 1)
            .copied()
    }

    /// The free-ride page after this one, wrapping to the first.
    pub fn following(self) -> Self {
        self.following_in(false)
    }

    /// The free-ride page before this one, wrapping to the last.
    pub fn preceding(self) -> Self {
        self.preceding_in(false)
    }
}

/// Text the main page lays out once per frame in the anti-aliased faces,
/// and the zones its gauges show.
pub(super) type Labels = heapless::Vec<chrome::Label<12>, 14>;

#[derive(Default)]
pub struct Smooth {
    pub(super) labels: Labels,
    pub(super) zones: [u8; 2],
    /// A workout target under a gauge: which gauge and its columns.
    pub(super) band: Option<(usize, core::ops::Range<usize>)>,
}

/// Prepare the shown page once per frame: the main page's labels, or the
/// other pages' canvas and the climb page's chart.
pub(super) fn prepare(view: &mut View) {
    match view.ride_page {
        RidePage::Main => smooth(view),
        RidePage::Stats => super::workout::fill(view, stats::prepare),
        RidePage::Lap => super::workout::fill(view, lap::prepare),
        RidePage::Climb => {
            climb_frame(view);
            super::workout::fill(view, climb::prepare);
        }
        // The app lays out the chart from its player: the workout stays there.
        RidePage::Workout => super::workout::fill(view, workout::prepare),
        RidePage::Back => super::workout::fill(view, back::prepare),
    }
}

/// The main page's gauges, prepared in their own frame (#167).
#[inline(never)]
fn smooth(view: &mut View) {
    let smooth = main::prepare(view);
    view.smooth = smooth;
}

/// The climb chart, prepared in its own frame (#167).
#[inline(never)]
fn climb_frame(view: &mut View) {
    let climb = climb::frame(view);
    view.climb = climb;
}

/// A caption over its value in `column` (0 or 1) of the band whose caption
/// top is `top`. The value steps down to `BODY` before it would leave its
/// column.
pub(super) fn cell(
    canvas: &mut Canvas,
    column: usize,
    top: usize,
    caption: &[u8],
    value: &[u8],
    color: u16,
) {
    let (left, right) = if column == 0 {
        (LEFT, MIDDLE - COLUMN_GAP)
    } else {
        (MIDDLE, RIGHT)
    };
    let width = right - left;
    canvas.caption(caption, width, left, top + 12, Align::Left);
    canvas.fit(
        &[&VALUE, &BODY_FACE],
        value,
        width,
        left,
        top + 41,
        Align::Left,
        color,
    );
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

fn paint_page(view: &View, row: &mut Row) {
    match view.ride_page {
        RidePage::Main => main::paint(view, row),
        RidePage::Stats | RidePage::Lap => view.canvas.paint(row),
        RidePage::Workout => {
            workout::paint_chart(&view.chart, row);
            view.canvas.paint(row);
        }
        RidePage::Climb => {
            climb::paint_chart(&view.climb, row);
            view.canvas.paint(row);
        }
        RidePage::Back => {
            view.canvas.paint(row);
            back::paint(view, row);
        }
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
        RidePage::Stats | RidePage::Lap => view.canvas.pixel(x, y, BLACK),
        RidePage::Workout => view
            .canvas
            .pixel(x, y, workout::chart_pixel(&view.chart, x, y)),
        RidePage::Climb => view
            .canvas
            .pixel(x, y, climb::chart_pixel(&view.climb, x, y)),
        RidePage::Back => {
            back::arrow_pixel(&back::frame(view), x, y, view.canvas.pixel(x, y, BLACK))
        }
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
pub(crate) fn elapsed(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = write!(value, "{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
}

/// Kilometres with two rounded decimals, one from 100 km.
pub(crate) fn kilometres(value: &mut Text<12>, mm: u64) {
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
        assert_eq!(RidePage::Climb.next(), Some(RidePage::Back));
        assert_eq!(RidePage::Back.next(), None);
        assert_eq!(RidePage::Back.following(), RidePage::Main);
        assert_eq!(RidePage::Main.preceding(), RidePage::Back);
        assert_eq!(RidePage::Stats.preceding(), RidePage::Main);
        // A workout puts its page first.
        assert_eq!(RidePage::Workout.following_in(true), RidePage::Main);
        assert_eq!(RidePage::Climb.following_in(true), RidePage::Back);
        assert_eq!(RidePage::Back.following_in(true), RidePage::Workout);
        assert_eq!(RidePage::Workout.preceding_in(true), RidePage::Back);
        assert_eq!(RidePage::Main.position(true), 1);
        assert_eq!(RidePage::Workout.position(false), 0);
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
