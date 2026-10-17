//! VANA workout rendering.
use super::ride::{self, ClimbFrame, Marks, RidePage, Values};
use crate::ride::climb::Climbs;
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Status};
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::sensors::radar;
use core::fmt::Write;
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

#[cfg(test)]
mod reference;

/// Below this much recording time left, the ride state row warns the rider.
pub const LOW_STORAGE_S: u32 = 60 * 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Boot,
    Home,
    Sensors,
    Preflight,
    Ride,
    Settings,
}

/// Home choices, in BottomLeft order.
pub const HOME: [&[u8]; 3] = [b"TRAIN", b"SENSORS", b"SETTINGS"];

pub struct View {
    pub now_ms: u64,
    pub sample: Sample,
    pub active_secs: u64,
    pub state: Status,
    pub radar: radar::Snapshot,
    pub gps_fix: bool,
    pub stop_confirm: bool,
    pub totals: Totals,
    /// The open lap and the last finished one.
    pub lap: Lap,
    pub previous_lap: Option<Lap>,
    /// A just-finished lap the status row announces.
    pub lap_toast: Option<Lap>,
    pub ride_page: RidePage,
    /// Climbs and the elevation profile; only the climb page reads them.
    pub climbs: Climbs,
    /// What the ride page prepared for this frame.
    pub(super) values: Values,
    /// The status-row lap announcement prepared for this frame, if any.
    pub(super) toast: Text<19>,
    /// Recording time left in the ride journal, when the recorder can record.
    pub storage_left_s: Option<u32>,
    /// The ride state row, prepared once per frame.
    pub(super) state_text: Text<19>,
    pub(super) state_warns: bool,
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
    /// The climb page's chart, prepared with `values`.
    pub(super) climb: ClimbFrame,
    pub page: Page,
    /// The highlighted Home choice, or the selected Settings field.
    pub cursor: usize,
    /// Settings: the selected field is being changed.
    pub editing: bool,
    pub time: Text<16>,
    pub lines: [Text<38>; 9],
}
impl View {
    pub fn new(page: Page, cursor: usize, utc: Option<u64>) -> Self {
        let mut time = Text::new();
        // Santiago summer time for this workout build. UTC remains unchanged in recordings.
        if let Some(utc) = utc {
            let minutes = ((utc + 86400 - 3 * 3600) % 86400) / 60;
            let hour = minutes / 60;
            let _ = write!(
                time,
                "{:02}:{:02} {}",
                (hour + 11) % 12 + 1,
                minutes % 60,
                if hour < 12 { "AM" } else { "PM" }
            );
        } else {
            let _ = time.push_str("--:-- --");
        }
        Self {
            now_ms: 0,
            sample: Sample::default(),
            active_secs: 0,
            state: Status::Ready,
            radar: radar::Snapshot::default(),
            gps_fix: false,
            stop_confirm: false,
            totals: Totals::default(),
            lap: Laps::default().current(),
            previous_lap: None,
            lap_toast: None,
            ride_page: RidePage::Main,
            storage_left_s: None,
            climbs: Climbs::default(),
            values: core::array::from_fn(|_| Text::new()),
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            marks: Marks::default(),
            toast: Text::new(),
            state_text: Text::new(),
            state_warns: false,
            climb: ClimbFrame::default(),
            page,
            cursor,
            editing: false,
            time,
            lines: core::array::from_fn(|_| Text::new()),
        }
    }
    /// One pixel of the painted canvas; tests and previews only, as it paints a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [0u16; 240];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }
    /// Paint one row of the 240 x 320 canvas; later paints cover earlier ones.
    pub fn paint(&self, row: &mut Row) {
        const CYAN: u16 = 0x07ff;
        const WHITE: u16 = 0xffff;
        const DIM: u16 = 0x8410;
        let y = row.y();
        row.fill(0);
        if self.page == Page::Boot {
            let frame = self.now_ms / 180;
            let glitch = frame % 9 == 1 || frame % 9 == 2;
            let shift = if glitch && (y / 9 + frame as usize).is_multiple_of(3) {
                8
            } else {
                0
            };
            if y == 300 {
                row.span(0..((self.now_ms % 6500) * 240 / 6500) as usize, CYAN);
            }
            row.text(8, 269, 1, self.lines[0].as_bytes(), WHITE);
            row.text(96, 195, 2, b"LABS", CYAN);
            if y == 98 || y == 174 {
                row.span(24..216, CYAN);
            }
            // The type is sampled `shift` pixels to the right; move it left instead.
            if glitch {
                let color = if y.is_multiple_of(2) { CYAN } else { 0xf81f };
                row.text(28 - shift, 105, 8, b"VANA", color);
                row.text(20 - shift, 106, 8, b"VANA", color);
            }
            row.text(24 - shift, 108, 8, b"VANA", WHITE);
            return;
        }
        if y < 29 {
            if self.page == Page::Ride {
                row.text(72, 7, 2, &self.ride_page.indicator(), DIM);
            }
            let (gps, color): (&[u8], u16) = if self.gps_fix {
                (b"GPS", CYAN)
            } else {
                (b"GPS?", DIM)
            };
            row.text(8, 7, 2, gps, color);
            row.text(128, 7, 2, self.time.as_bytes(), WHITE);
            return;
        }
        if y == 29 || y == 285 {
            row.fill(0x39e7);
            return;
        }
        if self.page == Page::Ride {
            return ride::paint(self, row);
        }
        if self.page == Page::Settings {
            return super::settings::paint(self, row);
        }
        let (left, right): (&[u8], &[u8]) = match self.page {
            Page::Home => (b"NEXT", b"SELECT"),
            Page::Sensors => (b"BACK", b"SCAN"),
            _ => (b"", b""),
        };
        row.text(156, 300, 2, right, CYAN);
        row.text(8, 300, 2, left, CYAN);
        if self.page == Page::Home {
            for (i, label) in HOME.iter().enumerate().rev() {
                let top = 82 + i * 68;
                if !(top..top + 58).contains(&y) {
                    continue;
                }
                let selected = self.cursor == i;
                let mut tile = row.within(12..228);
                tile.fill(0);
                if selected {
                    tile.fill(CYAN);
                } else if y == top || y == top + 57 {
                    tile.fill(DIM);
                } else {
                    tile.set(12, DIM);
                    tile.set(227, DIM);
                }
                tile.text(22, top + 18, 3, label, if selected { 0 } else { WHITE });
            }
            row.text(24, 50, 2, b"READY TO RIDE", DIM);
        } else {
            for (i, line) in self.lines.iter().enumerate().rev() {
                let scale = if i == 0
                    || (matches!(self.page, Page::Sensors | Page::Preflight)
                        && (2..=5).contains(&i))
                {
                    1
                } else {
                    2
                };
                let color = if i == 0 { CYAN } else { WHITE };
                row.text(10, 36 + i * 27, scale, line.as_bytes(), color);
            }
        }
    }
    pub fn prepare(&mut self) {
        if self.page == Page::Ride {
            ride::prepare(self);
            self.toast = ride::toast(self.lap_toast);
            self.prepare_state();
            self.marks = ride::marks(self);
        }
    }

    fn prepare_state(&mut self) {
        let mut state = Text::new();
        let low = self.storage_left_s.filter(|left| *left < LOW_STORAGE_S);
        self.state_warns = self.stop_confirm || low.is_some();
        let label = match self.state {
            Status::Recording => "REC",
            Status::Paused => "PAUSED",
            _ => "STORAGE",
        };
        let _ = match low {
            _ if self.stop_confirm => state.push_str("PRESS AGAIN TO SAVE"),
            Some(left) => write!(state, "{label} {}M LEFT", left / 60),
            None => state.push_str(match self.state {
                Status::Recording => "RECORDING",
                Status::Paused => "PAUSED",
                Status::Saved => "SAVED",
                Status::Recovered => "RECOVERED",
                Status::Error => "STORAGE ERROR",
                Status::Full => "STORAGE FULL",
                _ => "READY",
            }),
        };
        self.state_text = state;
    }
}

#[derive(Clone)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Text<N> {
    pub fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn push_str(&mut self, value: &str) -> core::fmt::Result {
        self.write_str(value)
    }
}
impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        if self.len + value.len() > N {
            return Err(core::fmt::Error);
        }
        for byte in value.bytes() {
            self.bytes[self.len] = byte.to_ascii_uppercase();
            self.len += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: u16 = 0xffff;
    const CYAN: u16 = 0x07ff;

    /// Every pixel in the label's box has `color` exactly where the label draws.
    fn shows(view: &View, left: usize, top: usize, scale: usize, label: &[u8], color: u16) -> bool {
        (top..top + 7 * scale).all(|y| {
            (left..left + label.len() * 6 * scale)
                .all(|x| (view.pixel(x, y) == color) == text(x, y, left, top, scale, label))
        })
    }

    #[test]
    fn clock_shows_twelve_hour_local_time() {
        let clock = |utc| View::new(Page::Home, 0, utc).time.as_bytes().to_vec();
        assert_eq!(clock(Some(0)), b"09:00 PM");
        assert_eq!(clock(Some(15 * 3600)), b"12:00 PM");
        assert_eq!(clock(Some(3 * 3600)), b"12:00 AM");
        assert_eq!(
            clock(Some(5 * 86400 + 3 * 3600 + 59 * 60 + 59)),
            b"12:59 AM"
        );
        assert_eq!(clock(None), b"--:-- --");
    }

    #[test]
    fn ride_page_shows_prepared_metrics_and_zones() {
        let mut view = View::new(Page::Ride, 0, None);
        view.sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(250),
            cadence_tenths: Some(904),
            gradient_tenths: Some(-35),
            ..Sample::default()
        };
        view.totals.add(1000, 12_345, &view.sample, None);
        view.active_secs = 3725;
        view.state = Status::Recording;
        view.prepare();
        assert!(shows(&view, 72, 7, 2, b"1/4", 0x8410));
        assert!(shows(&view, 38, 54, 6, b"29.9", WHITE));
        assert!(shows(&view, 38, 124, 4, b"250", WHITE));
        // 250 W is Z5 at the default FTP: black on an orange chip, and the
        // gauge lit to its place in 227..259 W.
        const ORANGE: u16 = 0xfc60;
        assert!(shows(&view, 38, 157, 2, b"Z5", 0));
        assert_eq!(view.pixel(36, 164), ORANGE);
        assert_eq!(view.pixel(74, 164), ORANGE);
        assert_eq!(view.pixel(113, 164), ORANGE);
        assert_eq!(view.pixel(114, 164), 0x39e7);
        assert_eq!(view.pixel(70, 164), 0, "a gap between chip and gauge");
        assert!(shows(&view, 146, 124, 4, b"--", WHITE));
        // No heart rate: a dim Z- with no chip or gauge.
        assert!(shows(&view, 146, 157, 2, b"Z-", 0x8410));
        assert_eq!(view.pixel(143, 155), 0);
        assert_eq!(view.pixel(190, 164), 0);
        // The same watts sit in a lower zone for a stronger rider.
        view.profile = crate::ride::zones::TrainingProfile::new(300, 185).unwrap();
        view.prepare();
        assert!(shows(&view, 38, 157, 2, b"Z3", 0));
        assert_eq!(view.pixel(36, 164), 0x07e0);
        assert!(shows(&view, 38, 192, 3, b"90", WHITE));
        assert!(shows(&view, 146, 192, 3, b"0.01", WHITE));
        assert!(shows(&view, 38, 234, 2, b"01:02:05", WHITE));
        assert!(shows(&view, 150, 234, 2, b"-3.5%", WHITE));
        assert!(shows(&view, 6, 296, 2, b"RECORDING", CYAN));
    }

    #[test]
    fn stats_page_shows_ride_totals() {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        let sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(200),
            heart_bpm: Some(140),
            cadence_tenths: Some(900),
            ..Sample::default()
        };
        for _ in 0..600 {
            view.totals.add(1000, 8333, &sample, None);
            view.zone_time.add(1000, &sample, &view.profile);
        }
        view.state = Status::Paused;
        view.prepare();
        assert!(shows(&view, 72, 7, 2, b"2/4", 0x8410));
        assert!(shows(&view, 34, 36, 1, b"DISTANCE KM", 0x8410));
        assert!(shows(&view, 34, 46, 3, b"5.00", WHITE));
        assert!(shows(&view, 141, 49, 2, b"00:10:00", WHITE));
        assert!(shows(&view, 34, 83, 3, b"29.9", WHITE));
        assert!(shows(&view, 141, 120, 3, b"200", WHITE));
        assert!(shows(&view, 34, 157, 3, b"200", WHITE));
        assert!(shows(&view, 141, 157, 3, b"120", WHITE));
        assert!(shows(&view, 141, 194, 3, b"140", WHITE));
        assert!(shows(&view, 34, 231, 3, b"90", WHITE));
        assert!(shows(&view, 141, 221, 1, b"ASCENT M", 0x8410));
        assert!(shows(&view, 141, 231, 3, b"0", WHITE));
        // Ten minutes at 200 W and 140 bpm: all Z4 power, all Z1 heart rate.
        assert!(shows(&view, 34, 261, 1, b"PWR", 0x8410));
        assert!(shows(&view, 34, 273, 1, b"HR", 0x8410));
        for x in [60, 150, 235] {
            assert_eq!(view.pixel(x, 264), 0xffe0);
            assert_eq!(view.pixel(x, 276), 0x8410);
        }
        assert_eq!(view.pixel(238, 264), 0);
        // The shared frame stays: the status row.
        assert!(shows(&view, 6, 296, 2, b"PAUSED", CYAN));
    }

    /// A first lap of five minutes at 200 W and 30 km/h, then `seconds` of
    /// the second at `watts` and `speed_mm_s`.
    fn two_laps(view: &mut View, seconds: u64, watts: u16, speed_mm_s: u32) {
        let mut laps = Laps::default();
        let steady = |watts, speed_mm_s| Sample {
            speed_mm_s: Some(speed_mm_s),
            power_watts: Some(watts),
            heart_bpm: Some(150),
            ..Sample::default()
        };
        for second in 0..300 {
            laps.add(1000, 8334, &steady(200, 8334), second * 1000);
        }
        view.lap_toast = laps.lap(300_000);
        for second in 0..seconds {
            let sample = steady(watts, speed_mm_s);
            laps.add(
                1000,
                u64::from(speed_mm_s),
                &sample,
                300_000 + second * 1000,
            );
        }
        view.lap = laps.current();
        view.previous_lap = laps.previous();
    }

    #[test]
    fn lap_page_compares_the_open_lap_with_the_last_one() {
        const GREEN: u16 = 0x07e0;
        const RED: u16 = 0xf800;
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Lap;
        view.state = Status::Recording;
        two_laps(&mut view, 125, 250, 9000);
        view.lap_toast = None;
        view.prepare();
        assert!(shows(&view, 72, 7, 2, b"3/4", 0x8410));
        assert!(shows(&view, 38, 38, 1, b"LAP TIME", 0x8410));
        assert!(shows(&view, 146, 34, 2, b"LAP 2", CYAN));
        assert!(shows(&view, 38, 54, 6, b"02:05", WHITE));
        assert!(shows(&view, 38, 122, 3, b"1.13", WHITE));
        assert!(shows(&view, 146, 122, 3, b"250", WHITE));
        assert!(shows(&view, 38, 170, 3, b"32.4", WHITE));
        assert!(shows(&view, 146, 170, 3, b"150", WHITE));
        assert!(shows(&view, 38, 207, 1, b"PREV LAP 1", 0x8410));
        assert!(shows(&view, 38, 219, 2, b"05:00", WHITE));
        assert!(shows(&view, 146, 219, 2, b"200", WHITE));
        // Faster and harder: green, and signed for colour-blind riders.
        assert!(shows(&view, 38, 254, 3, b"+2.4", GREEN));
        assert!(shows(&view, 146, 254, 3, b"+50", GREEN));

        // Slower and easier: red with a minus; level is white and unsigned.
        two_laps(&mut view, 60, 150, 7667);
        view.prepare();
        assert!(shows(&view, 38, 254, 3, b"-2.4", RED));
        assert!(shows(&view, 146, 254, 3, b"-50", RED));
        two_laps(&mut view, 60, 200, 8334);
        view.prepare();
        assert!(shows(&view, 38, 254, 3, b"0.0", WHITE));
        assert!(shows(&view, 146, 254, 3, b"0", WHITE));
    }

    #[test]
    fn lap_page_before_the_first_lap_has_no_comparison() {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Lap;
        view.prepare();
        assert!(shows(&view, 146, 34, 2, b"LAP 1", CYAN));
        assert!(shows(&view, 38, 54, 6, b"00:00", WHITE));
        assert!(shows(&view, 38, 207, 1, b"PREV LAP", 0x8410));
        assert!(shows(&view, 38, 254, 3, b"--", WHITE));
        assert!(shows(&view, 146, 254, 3, b"--", WHITE));
    }

    #[test]
    fn status_row_shows_state_and_toasts_without_button_labels() {
        let mut view = View::new(Page::Ride, 0, None);
        view.state = Status::Recording;
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"RECORDING", CYAN));
        // No labels: everything under the status text is blank.
        assert!((0..240).all(|x| (311..320).all(|y| view.pixel(x, y) == 0)));
        two_laps(&mut view, 10, 250, 9000);
        view.prepare();
        // The toast is a cyan banner with dark text over the status row.
        assert!(shows(&view, 6, 296, 2, b"LAP 1 05:00 200W", 0));
        assert_eq!(view.pixel(3, 290), CYAN);

        let mut view = View::new(Page::Ride, 0, None);
        view.state = Status::Paused;
        view.stop_confirm = true;
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"PRESS AGAIN TO SAVE", 0xffe0));

        // An armed end ride outranks a lap toast still showing.
        two_laps(&mut view, 10, 250, 9000);
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"PRESS AGAIN TO SAVE", 0xffe0));
    }

    #[test]
    fn ride_page_warns_before_storage_runs_out_and_names_a_recovered_ride() {
        let mut view = View::new(Page::Ride, 0, None);
        view.state = Status::Recording;
        view.storage_left_s = Some(42 * 60 + 59);
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"REC 42M LEFT", 0xffe0));

        view.storage_left_s = Some(LOW_STORAGE_S);
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"RECORDING", CYAN));

        view.state = Status::Ready;
        view.storage_left_s = Some(59);
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"STORAGE 0M LEFT", 0xffe0));

        view.state = Status::Recovered;
        view.storage_left_s = Some(4 * 3600);
        view.prepare();
        assert!(shows(&view, 6, 296, 2, b"RECOVERED", CYAN));
    }

    #[test]
    fn home_highlights_the_selected_choice() {
        let mut view = View::new(Page::Home, 1, None);
        view.prepare();
        assert_eq!(view.pixel(13, 151), CYAN, "Sensors is filled");
        assert_eq!(view.pixel(13, 83), 0, "Train is outlined only");
        assert_eq!(view.pixel(12, 83), 0x8410);
        assert_eq!(view.pixel(13, 219), 0, "Settings is outlined only");
        assert!(shows(&view, 22, 236, 3, b"SETTINGS", WHITE));
    }

    #[test]
    fn text_is_uppercase_and_bounded() {
        let mut line = Text::<4>::new();
        assert!(line.push_str("ok").is_ok());
        assert!(line.push_str("abc").is_err());
        assert_eq!(line.as_bytes(), b"OK");
    }
}
