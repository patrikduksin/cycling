//! The ride screen: pages that BottomLeft steps through inside a shared frame.
//!
//! The frame draws the radar strip (x < `CONTENT_LEFT`) and, below the
//! content, one status row for the ride state and short toasts. There are no
//! button labels: the buttons keep one meaning system-wide (see
//! docs/ride-recording.md). A page owns the content rectangle,
//! `CONTENT_LEFT..CONTENT_RIGHT` by `CONTENT_TOP..CONTENT_BOTTOM`, and the
//! values it prepares once per frame; new pages place everything from these
//! constants so the chrome can change without rewriting them.
//!
//! To add a page: add a variant to `RidePage` and `RidePage::ALL`, then a
//! module beside `main` with `prepare` and `paint`, dispatched below.
//!
//! The climb page also follows the road: when a climb is confirmed the ride
//! screen switches to it, and when the climb ends it returns to the page the
//! rider was on, unless they changed page during the climb (see
//! `Navigation::follow_climb`).
use super::workout::{Text, View};
use crate::ride::laps::Lap;
use crate::ride::log::Status;
use crate::sensors::radar::{self, close};
use core::fmt::Write;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

mod climb;
mod lap;
mod main;
mod stats;
mod zones;

pub(super) use zones::{Marks, marks};

pub use climb::Frame as ClimbFrame;

pub(super) const WHITE: u16 = 0xffff;
pub(super) const CYAN: u16 = 0x07ff;
pub(super) const DIM: u16 = 0x8410;
pub(super) const GRID: u16 = 0x39e7;
const YELLOW: u16 = 0xffe0;
pub(super) const RED: u16 = 0xf800;
pub(super) const GREEN: u16 = 0x07e0;

/// The content rectangle a page draws in.
pub(super) const CONTENT_LEFT: usize = 28;
pub(super) const CONTENT_TOP: usize = 30;
pub(super) const CONTENT_RIGHT: usize = 240;
/// The content ends at the footer divider; the status row is below it.
pub(super) const CONTENT_BOTTOM: usize = 285;
pub(super) const BODY_BOTTOM: usize = CONTENT_BOTTOM;
/// The status row's text line, full width under the content.
const STATUS_TEXT_TOP: usize = 296;

/// A ride page, in BottomLeft order.
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

    /// "1/2" style position, for the header.
    pub(super) fn indicator(self) -> [u8; 3] {
        [
            b'1' + self.index() as u8,
            b'/',
            b'0' + Self::ALL.len() as u8,
        ]
    }
}

/// Formatted readings a page prepares once per frame and paints per row.
pub type Values = [Text<12>; 12];

/// Prepare the shown page's values, and the climb page's chart, once per frame.
pub(super) fn prepare(view: &mut View) {
    let mut values = core::array::from_fn(|_| Text::new());
    match view.ride_page {
        RidePage::Main => main::prepare(view, &mut values),
        RidePage::Stats => stats::prepare(view, &mut values),
        RidePage::Lap => lap::prepare(view, &mut values),
        RidePage::Climb => {
            climb::prepare(view, &mut values);
            view.climb = climb::frame(view);
        }
    }
    view.values = values;
}

/// Everything below the header, except the footer divider the view draws.
/// The row arrives cleared.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    if y < CONTENT_BOTTOM {
        let mut content = row.within(CONTENT_LEFT..CONTENT_RIGHT);
        match view.ride_page {
            RidePage::Main => main::paint(view, &mut content),
            RidePage::Stats => stats::paint(view, &mut content),
            RidePage::Lap => lap::paint(view, &mut content),
            RidePage::Climb => climb::paint(view, &mut content),
        }
        row.set(27, GRID);
    } else {
        paint_status(view, row);
    }
    if (38..279).contains(&y) {
        paint_radar(view.radar, &mut row.within(0..26));
    }
}

/// The radar strip, which owns its columns: an arrow, the range track and
/// targets on top.
fn paint_radar(radar: radar::Snapshot, row: &mut Row) {
    let y = row.y();
    row.fill(0);
    if y < 49 {
        let half = (y - 38) / 2;
        row.span(12 - half..13 + half, WHITE);
    }
    if (58..=272).contains(&y) {
        let color = match radar.status {
            radar::Status::Ready => CYAN,
            radar::Status::Error | radar::Status::ReservedThreat => RED,
            _ => DIM,
        };
        row.span(11..14, color);
    }
    // The first target wins where dots overlap, so paint it last.
    for target in radar.targets.iter().rev().flatten() {
        let cy = 58 + (target.range_mm.min(200000) * 214 / 200000) as i32;
        let dy = y as i32 - cy;
        if dy * dy > 25 {
            continue;
        }
        let mut half = 0;
        while (half + 1) * (half + 1) + dy * dy <= 25 {
            half += 1;
        }
        let color = if close(*target) { RED } else { YELLOW };
        row.span((12 - half) as usize..(13 + half) as usize, color);
    }
}

/// The status row under the content: the end-ride confirmation, a lap
/// toast, or the recorder state. No button labels.
fn paint_status(view: &View, row: &mut Row) {
    if row.y() <= CONTENT_BOTTOM {
        return;
    }
    // An armed end ride outranks a lap toast: the next press saves.
    if !view.stop_confirm && !view.toast.as_bytes().is_empty() {
        row.fill(CYAN);
        row.text(6, STATUS_TEXT_TOP, 2, view.toast.as_bytes(), 0);
        return;
    }
    // End-ride confirmation, storage warning or recorder state, prepared once.
    let color = if view.state_warns { YELLOW } else { CYAN };
    row.text(6, STATUS_TEXT_TOP, 2, view.state_text.as_bytes(), color);
    // Under the state: the honest note that auto-pause has no wheel speed.
    if view.state == Status::Recording && view.sample.speed_mm_s.is_none() {
        row.text(6, 287, 1, b"NO SPEED: AUTO PAUSE OFF", DIM);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    if x < 26 && (38..279).contains(&y) {
        return reference_radar_pixel(view.radar, x, y);
    }
    if x == 27 && y < 285 {
        return GRID;
    }
    if x >= CONTENT_LEFT && y < CONTENT_BOTTOM {
        return match view.ride_page {
            RidePage::Main => main::reference_pixel(view, x, y),
            RidePage::Stats => stats::reference_pixel(view, x, y),
            RidePage::Lap => lap::reference_pixel(view, x, y),
            RidePage::Climb => climb::reference_pixel(view, x, y),
        };
    }
    reference_status_pixel(view, x, y)
}

#[cfg(test)]
fn reference_radar_pixel(radar: radar::Snapshot, x: usize, y: usize) -> u16 {
    for target in radar.targets.into_iter().flatten() {
        let cy = 58 + (target.range_mm.min(200000) * 214 / 200000) as i32;
        let dx = x as i32 - 12;
        let dy = y as i32 - cy;
        if dx * dx + dy * dy <= 25 {
            return if close(target) { RED } else { YELLOW };
        }
    }
    if (11..=13).contains(&x) && (58..=272).contains(&y) {
        return match radar.status {
            radar::Status::Ready => CYAN,
            radar::Status::Error | radar::Status::ReservedThreat => RED,
            _ => DIM,
        };
    }
    if y < 49 && (x as i32 - 12).abs() <= (y as i32 - 38) / 2 {
        return WHITE;
    }
    0
}

/// The finished lap the status row announces: number, time and average power.
pub(super) fn toast(lap: Option<Lap>) -> Text<19> {
    let mut toast = Text::new();
    let Some(lap) = lap else {
        return toast;
    };
    let mut time = Text::<12>::new();
    lap_time(&mut time, lap.time_ms / 1000);
    let time = core::str::from_utf8(time.as_bytes()).unwrap_or("");
    let written = match lap.avg_power() {
        Some(watts) => write!(toast, "LAP {} {time} {watts}W", lap.number),
        None => write!(toast, "LAP {} {time} --W", lap.number),
    };
    // Past lap 99 and an hour the power no longer fits the row.
    if written.is_err() {
        toast = Text::new();
        let _ = write!(toast, "LAP {} {time}", lap.number);
    }
    toast
}

#[cfg(test)]
fn reference_status_pixel(view: &View, x: usize, y: usize) -> u16 {
    if y <= CONTENT_BOTTOM {
        return 0;
    }
    // An armed end ride outranks a lap toast: the next press saves.
    if !view.stop_confirm && !view.toast.as_bytes().is_empty() {
        return if text(x, y, 6, STATUS_TEXT_TOP, 2, view.toast.as_bytes()) {
            0
        } else {
            CYAN
        };
    }
    // End-ride confirmation, storage warning or recorder state, prepared once.
    if text(x, y, 6, STATUS_TEXT_TOP, 2, view.state_text.as_bytes()) {
        return if view.state_warns { YELLOW } else { CYAN };
    }
    // Under the state: the honest note that auto-pause has no wheel speed.
    if view.state == Status::Recording
        && view.sample.speed_mm_s.is_none()
        && text(x, y, 6, 287, 1, b"NO SPEED: AUTO PAUSE OFF")
    {
        return DIM;
    }
    0
}

/// A label and the value under it, in a page's cell.
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

/// HH:MM:SS.
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
    fn pages_step_in_order_and_end() {
        assert_eq!(RidePage::default(), RidePage::Main);
        assert_eq!(RidePage::Main.next(), Some(RidePage::Stats));
        assert_eq!(RidePage::Stats.next(), Some(RidePage::Lap));
        assert_eq!(RidePage::Lap.next(), Some(RidePage::Climb));
        assert_eq!(RidePage::Climb.next(), None);
        assert_eq!(&RidePage::Main.indicator(), b"1/4");
        assert_eq!(&RidePage::Stats.indicator(), b"2/4");
        assert_eq!(&RidePage::Lap.indicator(), b"3/4");
        assert_eq!(&RidePage::Climb.indicator(), b"4/4");
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
        assert_eq!(format(&|v| clock(v, 3725)), b"01:02:05");
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
