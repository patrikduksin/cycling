//! VANA workout rendering.
use crate::ride::metrics::{heart_zone, power_zone};
use core::fmt::Write;
use firmware_shell::rendering::text::text;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Boot,
    Home,
    Sensors,
    Preflight,
    Ride,
}

pub struct View {
    pub now_ms: u64,
    pub sample: crate::ride::log::Sample,
    pub active_secs: u64,
    pub state: crate::ride::log::Status,
    pub radar: crate::sensors::radar::Snapshot,
    pub gps_fix: bool,
    pub stop_confirm: bool,
    values: [Text<12>; 7],
    pub page: Page,
    pub cursor: bool,
    pub time: Text<16>,
    pub lines: [Text<38>; 9],
}
impl View {
    pub fn new(page: Page, cursor: bool, utc: Option<u64>) -> Self {
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
            sample: crate::ride::log::Sample::default(),
            active_secs: 0,
            state: crate::ride::log::Status::Ready,
            radar: crate::sensors::radar::Snapshot::default(),
            gps_fix: false,
            stop_confirm: false,
            values: core::array::from_fn(|_| Text::new()),
            page,
            cursor,
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
            return 0;
        }
        if y == 29 || y == 285 {
            return 0x39e7;
        }
        if self.page == Page::Ride {
            return self.ride_pixel(x, y);
        }
        if self.page == Page::Home {
            if text(x, y, 24, 65, 2, b"READY TO RIDE") {
                return DIM;
            }
            for (i, label) in [b"TRAIN".as_slice(), b"SENSORS"].iter().enumerate() {
                let top = 110 + i * 86;
                if (12..228).contains(&x) && (top..top + 68).contains(&y) {
                    let selected = usize::from(self.cursor) == i;
                    if text(x, y, 22, top + 23, 3, label) {
                        return if selected { 0 } else { WHITE };
                    }
                    if selected {
                        return CYAN;
                    }
                    if x == 12 || x == 227 || y == top || y == top + 67 {
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
        let mut speed = Text::<12>::new();
        let mut power = Text::<12>::new();
        let mut heart = Text::<12>::new();
        let mut pz = Text::<12>::new();
        let mut hz = Text::<12>::new();
        let mut timer = Text::<12>::new();
        let mut grade = Text::<12>::new();
        if let Some(v) = self.sample.speed_mm_s {
            let k = v * 36 / 1000;
            let _ = write!(speed, "{}.{}", k / 10, k % 10);
        } else {
            let _ = speed.push_str("--");
        }
        if let Some(v) = self.sample.power_watts {
            let _ = write!(power, "{}", v);
            let _ = write!(pz, "Z{}", power_zone(v));
        } else {
            let _ = power.push_str("--");
            let _ = pz.push_str("Z-");
        }
        if let Some(v) = self.sample.heart_bpm {
            let _ = write!(heart, "{}", v);
            let _ = write!(hz, "Z{}", heart_zone(v));
        } else {
            let _ = heart.push_str("--");
            let _ = hz.push_str("Z-");
        }
        let t = self.active_secs;
        let _ = write!(timer, "{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
        if let Some(v) = self.sample.gradient_tenths {
            let _ = write!(
                grade,
                "{}{}.{}%",
                if v < 0 { "-" } else { "" },
                v.unsigned_abs() / 10,
                v.unsigned_abs() % 10
            );
        } else {
            let _ = grade.push_str("--%");
        }
        self.values = [speed, power, heart, pz, hz, timer, grade];
    }
    fn ride_pixel(&self, x: usize, y: usize) -> u16 {
        const WHITE: u16 = 0xffff;
        const CYAN: u16 = 0x07ff;
        const DIM: u16 = 0x8410;
        if x < 26 && (38..279).contains(&y) {
            let status = self.radar.status;
            for target in self.radar.targets.into_iter().flatten() {
                let cy = 58 + (target.range_mm.min(200000) * 214 / 200000) as i32;
                let dx = x as i32 - 12;
                let dy = y as i32 - cy;
                if dx * dx + dy * dy <= 25 {
                    return if crate::sensors::radar::close(target) {
                        0xf800
                    } else {
                        0xffe0
                    };
                }
            }
            if (11..=13).contains(&x) && (58..=272).contains(&y) {
                return match status {
                    crate::sensors::radar::Status::Ready => CYAN,
                    crate::sensors::radar::Status::Error
                    | crate::sensors::radar::Status::ReservedThreat => 0xf800,
                    _ => DIM,
                };
            }
            if y < 49 && (x as i32 - 12).abs() <= (y as i32 - 38) / 2 {
                return WHITE;
            }
            return 0;
        }
        if (x == 27 && y < 285)
            || (x >= 28 && (y == 115 || y == 202))
            || (x == 135 && (116..202).contains(&y))
        {
            return 0x39e7;
        }
        let [speed, power, heart, pz, hz, timer, grade] = &self.values;
        if y < 115 {
            if text(x, y, 38, 38, 2, b"SPEED") || text(x, y, 178, 91, 1, b"KM/H") {
                return DIM;
            }
            return if text(x, y, 38, 64, 6, speed.as_bytes()) {
                WHITE
            } else {
                0
            };
        }
        if y < 202 {
            let (left, label, value, zone) = if x < 135 {
                (38, b"WATTS".as_slice(), power, pz)
            } else {
                (146, b"BPM".as_slice(), heart, hz)
            };
            if text(x, y, left, 123, 2, label) {
                return DIM;
            }
            if text(
                x,
                y,
                if x < 135 { 36 } else { 145 },
                148,
                4,
                value.as_bytes(),
            ) {
                return WHITE;
            }
            return if text(x, y, left, 184, 2, zone.as_bytes()) {
                CYAN
            } else {
                0
            };
        }
        if y < 257 {
            if x < 150 {
                if text(x, y, 38, 211, 1, b"ACTIVE TIME") {
                    return DIM;
                }
                return if text(x, y, 33, 232, 2, timer.as_bytes()) {
                    WHITE
                } else {
                    0
                };
            }
            if text(x, y, 164, 211, 1, b"GRADE") {
                return DIM;
            }
            return if text(x, y, 159, 232, 2, grade.as_bytes()) {
                WHITE
            } else {
                0
            };
        }
        let state: &[u8] = if self.stop_confirm {
            b"STOP TO SAVE"
        } else {
            match self.state {
                crate::ride::log::Status::Recording => b"RECORDING",
                crate::ride::log::Status::Paused => b"PAUSED",
                crate::ride::log::Status::Saved => b"SAVED",
                crate::ride::log::Status::Error => b"STORAGE ERROR",
                crate::ride::log::Status::Full => b"STORAGE FULL",
                _ => b"READY",
            }
        };
        if text(x, y, 30, 263, 2, state) {
            return if self.stop_confirm { 0xffe0 } else { CYAN };
        }
        let left: &[u8] = match self.state {
            crate::ride::log::Status::Recording => b"PAUSE",
            crate::ride::log::Status::Paused => b"RESUME",
            _ => b"START",
        };
        if text(x, y, 8, 300, 2, left) || text(x, y, 180, 300, 2, b"STOP") {
            return WHITE;
        }
        0
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
