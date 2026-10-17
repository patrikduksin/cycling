//! VANA workout rendering.
use super::ride::{self, Marks, RidePage, Values};
use crate::ride::log::{Sample, Status};
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::sensors::radar;
use core::fmt::Write;
use firmware_shell::rendering::text::text;

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
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    pub ride_page: RidePage,
    /// What the ride page prepared for this frame.
    pub(super) values: Values,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
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
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            ride_page: RidePage::Main,
            values: core::array::from_fn(|_| Text::new()),
            marks: Marks::default(),
            page,
            cursor,
            editing: false,
            time,
            lines: core::array::from_fn(|_| Text::new()),
        }
    }
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        const CYAN: u16 = 0x07ff;
        const WHITE: u16 = 0xffff;
        const DIM: u16 = 0x8410;
        if self.page == Page::Boot {
            let frame = self.now_ms / 180;
            let glitch = frame % 9 == 1 || frame % 9 == 2;
            let shift = if glitch && (y / 9 + frame as usize).is_multiple_of(3) {
                8
            } else {
                0
            };
            if text(x + shift, y, 24, 108, 8, b"VANA") {
                return WHITE;
            }
            if glitch
                && (text(x + shift + 4, y, 24, 106, 8, b"VANA")
                    || text(x + shift, y + 3, 28, 108, 8, b"VANA"))
            {
                return if y.is_multiple_of(2) { CYAN } else { 0xf81f };
            }
            if (y == 98 || y == 174) && (24..216).contains(&x) {
                return CYAN;
            }
            if text(x, y, 96, 195, 2, b"LABS") {
                return CYAN;
            }
            if text(x, y, 8, 269, 1, self.lines[0].as_bytes()) {
                return WHITE;
            }
            if y == 300 && x < ((self.now_ms % 6500) * 240 / 6500) as usize {
                return CYAN;
            }
            return 0;
        }
        if y < 29 {
            if text(x, y, 128, 7, 2, self.time.as_bytes()) {
                return WHITE;
            }
            if text(x, y, 8, 7, 2, if self.gps_fix { b"GPS" } else { b"GPS?" }) {
                return if self.gps_fix { CYAN } else { DIM };
            }
            if self.page == Page::Ride && text(x, y, 72, 7, 2, &self.ride_page.indicator()) {
                return DIM;
            }
            return 0;
        }
        if y == 29 || y == 285 {
            return 0x39e7;
        }
        if self.page == Page::Ride {
            return ride::pixel(self, x, y);
        }
        if self.page == Page::Settings {
            return super::settings::pixel(self, x, y);
        }
        if self.page == Page::Home {
            if text(x, y, 24, 50, 2, b"READY TO RIDE") {
                return DIM;
            }
            for (i, label) in HOME.iter().enumerate() {
                let top = 82 + i * 68;
                if (12..228).contains(&x) && (top..top + 58).contains(&y) {
                    let selected = self.cursor == i;
                    if text(x, y, 22, top + 18, 3, label) {
                        return if selected { 0 } else { WHITE };
                    }
                    if selected {
                        return CYAN;
                    }
                    if x == 12 || x == 227 || y == top || y == top + 57 {
                        return DIM;
                    }
                }
            }
        } else {
            for (i, line) in self.lines.iter().enumerate() {
                if text(
                    x,
                    y,
                    10,
                    36 + i * 27,
                    if i == 0
                        || (matches!(self.page, Page::Sensors | Page::Preflight)
                            && (2..=5).contains(&i))
                    {
                        1
                    } else {
                        2
                    },
                    line.as_bytes(),
                ) {
                    return if i == 0 { CYAN } else { WHITE };
                }
            }
        }
        let (left, right): (&[u8], &[u8]) = match self.page {
            Page::Home => (b"NEXT", b"SELECT"),
            Page::Sensors => (b"BACK", b"SCAN"),
            _ => (b"", b""),
        };
        if text(x, y, 8, 300, 2, left) || text(x, y, 156, 300, 2, right) {
            return CYAN;
        }
        0
    }
    pub fn prepare(&mut self) {
        if self.page == Page::Ride {
            self.values = ride::prepare(self);
            self.marks = ride::marks(self);
        }
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
        assert!(shows(&view, 72, 7, 2, b"1/2", 0x8410));
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
        assert!(shows(&view, 30, 263, 2, b"RECORDING", CYAN));
        assert!(shows(&view, 8, 300, 2, b"PAUSE", WHITE));
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
        assert!(shows(&view, 72, 7, 2, b"2/2", 0x8410));
        assert!(shows(&view, 34, 36, 1, b"DISTANCE KM", 0x8410));
        assert!(shows(&view, 34, 45, 3, b"5.00", WHITE));
        assert!(shows(&view, 141, 48, 2, b"00:10:00", WHITE));
        assert!(shows(&view, 34, 79, 3, b"29.9", WHITE));
        assert!(shows(&view, 141, 113, 3, b"200", WHITE));
        assert!(shows(&view, 34, 147, 3, b"200", WHITE));
        assert!(shows(&view, 141, 147, 3, b"120", WHITE));
        assert!(shows(&view, 141, 181, 3, b"140", WHITE));
        assert!(shows(&view, 34, 215, 3, b"90", WHITE));
        assert!(shows(&view, 141, 206, 1, b"ASCENT M", 0x8410));
        assert!(shows(&view, 141, 215, 3, b"0", WHITE));
        // Ten minutes at 200 W and 140 bpm: all Z4 power, all Z1 heart rate.
        assert!(shows(&view, 34, 240, 1, b"PWR", 0x8410));
        assert!(shows(&view, 34, 249, 1, b"HR", 0x8410));
        for x in [58, 150, 237] {
            assert_eq!(view.pixel(x, 243), 0xffe0);
            assert_eq!(view.pixel(x, 252), 0x8410);
        }
        assert_eq!(view.pixel(238, 243), 0);
        // The shared frame stays: status and ride controls.
        assert!(shows(&view, 30, 263, 2, b"PAUSED", CYAN));
        assert!(shows(&view, 8, 300, 2, b"RESUME", WHITE));
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
