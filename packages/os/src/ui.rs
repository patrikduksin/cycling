//! Portable application state and drawing primitives for the C606 display.

use crate::{
    coin::{HEIGHT, PIXELS, WIDTH},
    companion::{Button, Status},
    controls::Controls,
    input::Point,
    metrics,
};

pub mod theme {
    pub const BACKGROUND: u16 = 0x0863;
    pub const SURFACE: u16 = 0x18e5;
    pub const TEXT: u16 = 0xffff;
    pub const MUTED: u16 = 0x8c71;
    pub const ACCENT: u16 = 0x07ff;
    pub const PRESSED: u16 = 0x259b;
    pub const DISABLED: u16 = 0x4228;
}

pub const TAP_SLOP: u16 = 18;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Screen {
    Home,
    Settings,
    Device,
    Diagnostics,
    Controls,
}

impl Screen {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Settings => "settings",
            Self::Device => "device",
            Self::Diagnostics => "diagnostics",
            Self::Controls => "controls",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    screen: Screen,
    focus: u8,
    brightness: u8,
    dim_timeout_secs: u16,
    dim_brightness: u8,
    timezone_minutes: i16,
}

pub struct App {
    pub screen: Screen,
    pub focus: u8,
    pub pressed: Option<u8>,
    pub controls: Controls,
    pub dim_timeout_secs: u16,
    pub dim_brightness: u8,
    pub timezone_minutes: i16,
    point: Option<Point>,
    origin: Option<(u8, Point)>,
    settings_dragging: bool,
    suppress_pointer: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Home,
            focus: 0,
            pressed: None,
            controls: Controls::default(),
            dim_timeout_secs: 30,
            dim_brightness: 10,
            timezone_minutes: 0,
            point: None,
            origin: None,
            settings_dragging: false,
            suppress_pointer: false,
        }
    }
}

impl App {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            screen: self.screen,
            focus: self.focus,
            brightness: self.controls.brightness,
            dim_timeout_secs: self.dim_timeout_secs,
            dim_brightness: self.dim_brightness,
            timezone_minutes: self.timezone_minutes,
        }
    }

    pub fn restore(&mut self, snapshot: Snapshot) {
        self.cancel();
        self.screen = snapshot.screen;
        self.focus = snapshot.focus;
        self.controls.brightness = snapshot.brightness;
        self.dim_timeout_secs = snapshot.dim_timeout_secs;
        self.dim_brightness = snapshot.dim_brightness;
        self.timezone_minutes = snapshot.timezone_minutes;
    }

    pub fn point(&self) -> Option<Point> {
        match self.screen {
            Screen::Controls => self.controls.point,
            _ => self.point,
        }
    }

    pub fn pointer(&mut self, point: Point) {
        if self.suppress_pointer {
            return;
        }
        match self.screen {
            Screen::Home => self.home_pointer(point),
            Screen::Settings => self.settings_pointer(point),
            Screen::Device => self.device_pointer(point),
            Screen::Diagnostics => self.point = Some(point),
            Screen::Controls => self.controls.update(Some(point)),
        }
    }

    /// Complete a physical or injected pointer gesture and activate its target.
    pub fn release(&mut self) {
        if core::mem::take(&mut self.suppress_pointer) {
            return;
        }
        match self.screen {
            Screen::Home => {
                self.point = None;
                self.origin = None;
                if let Some(item) = self.pressed.take() {
                    self.activate_home(item);
                }
            }
            Screen::Settings => {
                self.point = None;
                self.settings_dragging = false;
                match self.pressed.take() {
                    Some(1) => self.dim_timeout_secs = next_timeout(self.dim_timeout_secs),
                    Some(2) => self.dim_brightness = next_dim_level(self.dim_brightness),
                    Some(3) => self.timezone_minutes = next_timezone(self.timezone_minutes),
                    _ => {}
                }
            }
            Screen::Device => {
                self.point = None;
                if self.pressed.take().is_some() {
                    self.navigate(Screen::Diagnostics);
                }
            }
            Screen::Diagnostics => self.point = None,
            Screen::Controls => self.controls.update(None),
        }
    }

    /// Abandon a gesture without activating it.
    pub fn cancel(&mut self) {
        self.point = None;
        self.origin = None;
        self.pressed = None;
        self.settings_dragging = false;
        self.suppress_pointer = false;
        self.controls.update(None);
    }

    pub fn button(&mut self, button: Button, code: u16) {
        if code != 1 {
            return;
        }
        let pointer_was_held = self.suppress_pointer || self.point().is_some();
        self.cancel();
        match self.screen {
            Screen::Home => match button {
                Button::BottomLeft => self.focus = self.focus.saturating_sub(1),
                Button::BottomRight => self.focus = (self.focus + 1).min(3),
                Button::TopLeft => self.activate_home(self.focus),
            },
            Screen::Settings => match button {
                Button::TopLeft => self.navigate(Screen::Home),
                Button::BottomLeft => self.controls.button(button, code),
                Button::BottomRight => self.controls.button(button, code),
            },
            Screen::Device => match button {
                Button::TopLeft => self.navigate(Screen::Home),
                Button::BottomRight => self.navigate(Screen::Diagnostics),
                _ => {}
            },
            Screen::Diagnostics => {
                if button == Button::TopLeft {
                    self.navigate(Screen::Device);
                }
            }
            Screen::Controls => match button {
                Button::TopLeft => self.navigate(Screen::Home),
                _ if self.screen == Screen::Controls => self.controls.button(button, code),
                _ => {}
            },
        }
        self.suppress_pointer = pointer_was_held;
    }

    pub fn pointer_suppressed(&self) -> bool {
        self.suppress_pointer
    }

    pub fn render(
        &self,
        pixels: &mut [u16; PIXELS],
        available: bool,
        status: &Status,
        wifi: &[u8],
        metrics: &metrics::Snapshot,
        clock: &crate::network_time::Snapshot,
    ) {
        match self.screen {
            Screen::Home => self.render_home(pixels, status, wifi),
            Screen::Settings => self.render_settings(pixels),
            Screen::Device => self.render_device(pixels, status, wifi, clock),
            Screen::Diagnostics => self.render_diagnostics(pixels, wifi, metrics),
            Screen::Controls => {
                self.controls.render(pixels, available, status);
                crate::controls::wifi_label(pixels, wifi);
            }
        }
    }

    fn home_pointer(&mut self, point: Point) {
        if self.point.is_none() {
            self.origin = home_item(point).map(|item| (item, point));
        }
        self.point = Some(point);
        self.pressed = self
            .origin
            .filter(|&(item, start)| {
                home_item(point) == Some(item)
                    && point.x.abs_diff(start.x) <= TAP_SLOP
                    && point.y.abs_diff(start.y) <= TAP_SLOP
                    && home_enabled(item)
            })
            .map(|(item, _)| item);
        if self.origin.is_some() && self.pressed.is_none() {
            self.origin = None;
        }
        if let Some(item) = home_item(point) {
            self.focus = item;
        }
    }

    fn settings_pointer(&mut self, point: Point) {
        if self.point.is_none() {
            self.settings_dragging = (130..=210).contains(&point.y);
            if !self.settings_dragging {
                self.origin = settings_item(point).map(|item| (item, point));
            }
        }
        self.point = Some(point);
        self.pressed = if self.settings_dragging {
            Some(0)
        } else {
            self.origin
                .filter(|&(item, start)| {
                    settings_item(point) == Some(item)
                        && point.x.abs_diff(start.x) <= TAP_SLOP
                        && point.y.abs_diff(start.y) <= TAP_SLOP
                })
                .map(|(item, _)| item)
        };
        if !self.settings_dragging && self.origin.is_some() && self.pressed.is_none() {
            self.origin = None;
        }
        if self.settings_dragging {
            self.controls.brightness = brightness_at(point.x);
        }
    }

    fn device_pointer(&mut self, point: Point) {
        if self.point.is_none() {
            self.origin = diagnostics_item(point).then_some((0, point));
        }
        self.point = Some(point);
        self.pressed = self
            .origin
            .filter(|&(_, start)| {
                diagnostics_item(point)
                    && point.x.abs_diff(start.x) <= TAP_SLOP
                    && point.y.abs_diff(start.y) <= TAP_SLOP
            })
            .map(|_| 0);
        if self.origin.is_some() && self.pressed.is_none() {
            self.origin = None;
        }
    }

    fn navigate(&mut self, screen: Screen) {
        self.cancel();
        self.screen = screen;
        self.focus = 0;
    }

    fn activate_home(&mut self, item: u8) {
        let screen = match item {
            0 => Some(Screen::Settings),
            1 => Some(Screen::Device),
            2 => Some(Screen::Controls),
            _ => None,
        };
        if let Some(screen) = screen {
            self.navigate(screen);
        }
    }

    fn render_home(&self, pixels: &mut [u16; PIXELS], status: &Status, wifi: &[u8]) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 5, 4, b"CYCLING", theme::TEXT);
        text(pixels, 5, 13, b"BAT", theme::MUTED);
        match status.battery {
            Some((percent, _)) => {
                number(pixels, 20, 13, u32::from(percent), theme::ACCENT);
                text(pixels, 32, 13, b"%", theme::ACCENT);
            }
            None => text(pixels, 20, 13, b"--", theme::MUTED),
        }
        text(pixels, 40, 13, short_wifi(wifi), theme::MUTED);
        item(
            pixels,
            23,
            b"SETTINGS",
            true,
            self.focus == 0,
            self.pressed == Some(0),
        );
        item(
            pixels,
            42,
            b"DEVICE",
            true,
            self.focus == 1,
            self.pressed == Some(1),
        );
        item(
            pixels,
            61,
            b"CONTROLS",
            true,
            self.focus == 2,
            self.pressed == Some(2),
        );
        item(pixels, 80, b"RIDES  LATER", false, self.focus == 3, false);
        text(pixels, 5, 100, b"TOP SELECT", theme::MUTED);
    }

    fn render_settings(&self, pixels: &mut [u16; PIXELS]) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 5, 5, b"SETTINGS", theme::TEXT);
        text(pixels, 5, 17, b"BRIGHTNESS", theme::MUTED);
        rect(
            pixels,
            3,
            27,
            74,
            39,
            if self.pressed == Some(0) {
                theme::PRESSED
            } else {
                theme::SURFACE
            },
        );
        let digits = [
            b'0' + self.controls.brightness / 100,
            b'0' + (self.controls.brightness / 10) % 10,
            b'0' + self.controls.brightness % 10,
            b'%',
        ];
        text(
            pixels,
            31,
            35,
            &digits[usize::from(self.controls.brightness < 100)..],
            theme::TEXT,
        );
        rect(pixels, 8, 52, 65, 2, 0x4a69);
        let knob = 8 + (usize::from(self.controls.brightness) - 5) * 64 / 95;
        rect(pixels, 8, 52, knob - 7, 2, theme::ACCENT);
        rect(pixels, knob - 2, 48, 5, 10, theme::TEXT);
        text(pixels, 5, 61, b"DRAG OR BUTTONS", theme::MUTED);
        text(pixels, 5, 72, b"DIM AFTER", theme::MUTED);
        if self.dim_timeout_secs == 0 {
            text(pixels, 49, 72, b"OFF", theme::ACCENT);
        } else {
            number(
                pixels,
                49,
                72,
                u32::from(self.dim_timeout_secs),
                theme::ACCENT,
            );
            text(pixels, 61, 72, b"S", theme::ACCENT);
        }
        text(pixels, 5, 83, b"DIM LEVEL", theme::MUTED);
        number(
            pixels,
            49,
            83,
            u32::from(self.dim_brightness),
            theme::ACCENT,
        );
        text(pixels, 61, 83, b"%", theme::ACCENT);
        text(pixels, 5, 92, b"TIMEZONE", theme::MUTED);
        timezone(pixels, 41, 92, self.timezone_minutes, theme::ACCENT);
        text(pixels, 5, 98, b"TOP BACK", theme::MUTED);
    }

    fn render_device(
        &self,
        pixels: &mut [u16; PIXELS],
        status: &Status,
        wifi: &[u8],
        clock: &crate::network_time::Snapshot,
    ) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 5, 3, b"DEVICE", theme::TEXT);
        text(pixels, 5, 13, b"BATTERY", theme::MUTED);
        if let Some((percent, millivolts)) = status.battery {
            number(pixels, 45, 13, u32::from(percent), theme::ACCENT);
            text(pixels, 57, 13, b"%", theme::ACCENT);
            let voltage = [
                b'0' + (millivolts / 1000) as u8,
                b'.',
                b'0' + ((millivolts / 100) % 10) as u8,
                b'0' + ((millivolts / 10) % 10) as u8,
                b'V',
            ];
            text(pixels, 45, 22, &voltage, theme::ACCENT);
        } else {
            text(pixels, 45, 13, b"--", theme::MUTED);
            text(pixels, 45, 22, b"--.--V", theme::MUTED);
        }
        text(pixels, 5, 32, b"POWER", theme::MUTED);
        text(
            pixels,
            33,
            32,
            power_label(status.power),
            if status.power.is_some() {
                theme::ACCENT
            } else {
                theme::MUTED
            },
        );
        text(pixels, 5, 42, b"WIFI", theme::MUTED);
        text(pixels, 5, 51, wifi, theme::ACCENT);
        text(pixels, 5, 61, b"FW", theme::MUTED);
        text(
            pixels,
            20,
            61,
            env!("CARGO_PKG_VERSION").as_bytes(),
            theme::TEXT,
        );
        text(pixels, 5, 71, b"TIME", theme::MUTED);
        match clock.local_minutes {
            Some(minutes) => {
                let time = [
                    b'0' + (minutes / 60 / 10) as u8,
                    b'0' + (minutes / 60 % 10) as u8,
                    b':',
                    b'0' + (minutes % 60 / 10) as u8,
                    b'0' + (minutes % 10) as u8,
                ];
                text(pixels, 25, 71, &time, theme::TEXT);
            }
            None => text(pixels, 25, 71, b"--:--", theme::MUTED),
        }
        text(pixels, 5, 80, time_status(clock.status), theme::ACCENT);
        item(
            pixels,
            89,
            b"DIAGNOSTICS",
            true,
            true,
            self.pressed == Some(0),
        );
    }

    fn render_diagnostics(&self, pixels: &mut [u16; PIXELS], wifi: &[u8], m: &metrics::Snapshot) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 3, 3, b"DIAGNOSTICS", theme::TEXT);
        metric_row(pixels, 3, 13, b"UP", m.uptime_ms / 1_000, b"S");
        metric_pair(
            pixels,
            3,
            22,
            b"F MS",
            m.frame_ms as u64,
            b"MAX",
            m.max_frame_ms as u64,
        );
        metric_row(pixels, 3, 31, b"HEAP KIB", (m.heap_free / 1024) as u64, b"");
        metric_row(
            pixels,
            3,
            40,
            b"MIN SAMP KIB",
            (m.heap_min_sampled / 1024) as u64,
            b"",
        );
        metric_pair(
            pixels,
            3,
            49,
            b"PS KIB",
            (m.psram_free / 1024) as u64,
            b"OF",
            (m.psram_capacity / 1024) as u64,
        );
        text(pixels, 3, 58, wifi, theme::ACCENT);
        metric_pair(
            pixels,
            3,
            67,
            b"OK",
            m.companion_valid as u64,
            b"CRC",
            m.companion_bad_crc as u64,
        );
        metric_pair(
            pixels,
            3,
            76,
            b"UART",
            m.uart_errors as u64,
            b"TCH",
            m.touch_errors as u64,
        );
        text(
            pixels,
            3,
            85,
            match (m.harness, m.recording) {
                (true, true) => b"HAR ON  REC ON",
                (true, false) => b"HAR ON  REC OFF",
                (false, _) => b"HAR OFF REC OFF",
            },
            theme::MUTED,
        );
        text(pixels, 3, 98, b"TOP BACK", theme::MUTED);
    }
}

fn diagnostics_item(point: Point) -> bool {
    (9..=230).contains(&point.x) && (267..=319).contains(&point.y)
}

fn settings_item(point: Point) -> Option<u8> {
    if !(9..=230).contains(&point.x) {
        return None;
    }
    match point.y {
        216..=249 => Some(1),
        250..=280 => Some(2),
        281..=319 => Some(3),
        _ => None,
    }
}

fn next_timezone(value: i16) -> i16 {
    let value = value.clamp(-720, 840).div_euclid(30) * 30;
    if value >= 840 {
        -720
    } else {
        (value + 30).min(840)
    }
}

fn timezone(pixels: &mut [u16; PIXELS], x: usize, y: usize, minutes: i16, color: u16) {
    if minutes == 0 {
        text(pixels, x, y, b"UTC", color);
        return;
    }
    let hours = minutes.unsigned_abs() / 60;
    let remainder = minutes.unsigned_abs() % 60;
    let value = [
        b'U',
        b'T',
        b'C',
        if minutes < 0 { b'-' } else { b'+' },
        b'0' + (hours / 10) as u8,
        b'0' + (hours % 10) as u8,
        b':',
        b'0' + (remainder / 10) as u8,
        b'0' + (remainder % 10) as u8,
    ];
    text(pixels, x, y, &value, color);
}

fn time_status(status: crate::network_time::Status) -> &'static [u8] {
    use crate::network_time::Status;
    match status {
        Status::Unavailable => b"NO TIME",
        Status::Syncing => b"SYNCING",
        Status::Fresh => b"SYNCED",
        Status::Offline => b"OFFLINE TIME",
        Status::Stale => b"STALE TIME",
    }
}

fn next_timeout(value: u16) -> u16 {
    match value {
        0 => 15,
        1..=15 => 30,
        16..=30 => 60,
        31..=60 => 120,
        _ => 0,
    }
}

fn next_dim_level(value: u8) -> u8 {
    match value {
        0..=5 => 10,
        6..=10 => 20,
        11..=20 => 30,
        _ => 5,
    }
}

fn metric_row(
    pixels: &mut [u16; PIXELS],
    x: usize,
    y: usize,
    label: &[u8],
    value: u64,
    suffix: &[u8],
) -> usize {
    text(pixels, x, y, label, theme::MUTED);
    let value_x = x + (label.len() + 1) * 4;
    let (digits, length) = metrics::compact(value);
    text(pixels, value_x, y, &digits[..length], theme::TEXT);
    text(pixels, value_x + length * 4, y, suffix, theme::MUTED);
    value_x + (length + suffix.len()) * 4
}

fn metric_pair(
    pixels: &mut [u16; PIXELS],
    x: usize,
    y: usize,
    first: &[u8],
    a: u64,
    second: &[u8],
    b: u64,
) {
    let next = metric_row(pixels, x, y, first, a, b"");
    metric_row(pixels, next + 4, y, second, b, b"");
}

fn home_item(point: Point) -> Option<u8> {
    if !(9..=230).contains(&point.x) {
        return None;
    }
    match point.y {
        70..=123 => Some(0),
        127..=180 => Some(1),
        184..=237 => Some(2),
        241..=294 => Some(3),
        _ => None,
    }
}

fn home_enabled(item: u8) -> bool {
    item < 3
}

fn brightness_at(x: u16) -> u8 {
    (5 + (u32::from(x.clamp(24, 216)) - 24) * 95 / 192) as u8
}

fn short_wifi(wifi: &[u8]) -> &[u8] {
    match wifi {
        b"WIFI TEST OK" | b"WIFI CONNECTED" => b"WIFI OK",
        b"WIFI NOT SET UP" => b"WIFI --",
        _ => b"WIFI ...",
    }
}

fn power_label(power: Option<u8>) -> &'static [u8] {
    match power {
        Some(0) => b"CHARGING",
        Some(1) => b"BATTERY",
        _ => b"UNKNOWN",
    }
}

pub fn item(
    pixels: &mut [u16; PIXELS],
    y: usize,
    label: &[u8],
    enabled: bool,
    focused: bool,
    pressed: bool,
) {
    let surface = if pressed {
        theme::PRESSED
    } else if focused {
        theme::SURFACE
    } else {
        theme::BACKGROUND
    };
    let color = if pressed {
        theme::BACKGROUND
    } else if enabled {
        if focused { theme::ACCENT } else { theme::TEXT }
    } else {
        theme::DISABLED
    };
    rect(pixels, 3, y, 74, 20, surface);
    if focused {
        rect(pixels, 3, y, 2, 20, color);
    }
    text(pixels, 8, y + 7, label, color);
}

pub fn number(p: &mut [u16; PIXELS], x: usize, y: usize, n: u32, color: u16) {
    let n = n.min(999);
    let digits = [
        b'0' + (n / 100) as u8,
        b'0' + ((n / 10) % 10) as u8,
        b'0' + (n % 10) as u8,
    ];
    let start = if n < 10 {
        2
    } else if n < 100 {
        1
    } else {
        0
    };
    text(p, x, y, &digits[start..], color);
}

pub fn rect(p: &mut [u16; PIXELS], x: usize, y: usize, w: usize, h: usize, color: u16) {
    for row in y..(y + h).min(HEIGHT) {
        for col in x..(x + w).min(WIDTH) {
            p[row * WIDTH + col] = color;
        }
    }
}

pub fn cross(p: &mut [u16; PIXELS], x: i32, y: i32, color: u16) {
    for d in -3..=3 {
        for (xx, yy) in [(x + d, y), (x, y + d)] {
            if xx >= 0 && yy >= 0 && xx < WIDTH as i32 && yy < HEIGHT as i32 {
                p[yy as usize * WIDTH + xx as usize] = color;
            }
        }
    }
}

pub fn text(p: &mut [u16; PIXELS], x: usize, y: usize, s: &[u8], color: u16) {
    for (i, &ch) in s.iter().enumerate() {
        let rows = match ch {
            b'A' => [2, 5, 7, 5, 5],
            b'B' => [6, 5, 6, 5, 6],
            b'C' => [3, 4, 4, 4, 3],
            b'D' => [6, 5, 5, 5, 6],
            b'E' => [7, 4, 6, 4, 7],
            b'F' => [7, 4, 6, 4, 4],
            b'G' => [3, 4, 5, 5, 3],
            b'H' => [5, 5, 7, 5, 5],
            b'I' => [7, 2, 2, 2, 7],
            b'K' => [5, 5, 6, 5, 5],
            b'L' => [4, 4, 4, 4, 7],
            b'M' => [5, 7, 7, 5, 5],
            b'N' => [5, 7, 7, 7, 5],
            b'O' => [2, 5, 5, 5, 2],
            b'P' => [6, 5, 6, 4, 4],
            b'R' => [6, 5, 6, 5, 5],
            b'S' => [3, 4, 2, 1, 6],
            b'T' => [7, 2, 2, 2, 2],
            b'U' => [5, 5, 5, 5, 7],
            b'V' => [5, 5, 5, 5, 2],
            b'W' => [5, 5, 7, 7, 5],
            b'X' => [5, 5, 2, 5, 5],
            b'Y' => [5, 5, 2, 2, 2],
            b'.' => [0, 0, 0, 0, 2],
            b'-' => [0, 0, 7, 0, 0],
            b'+' => [0, 2, 7, 2, 0],
            b':' => [0, 2, 0, 2, 0],
            b'0' => [7, 5, 5, 5, 7],
            b'1' => [2, 6, 2, 2, 7],
            b'2' => [6, 1, 7, 4, 7],
            b'3' => [6, 1, 3, 1, 6],
            b'4' => [5, 5, 7, 1, 1],
            b'5' => [7, 4, 6, 1, 6],
            b'6' => [3, 4, 7, 5, 7],
            b'7' => [7, 1, 2, 2, 2],
            b'8' => [7, 5, 7, 5, 7],
            b'9' => [7, 5, 7, 1, 6],
            b'%' => [5, 1, 2, 4, 5],
            _ => [0; 5],
        };
        for (dy, row) in rows.iter().enumerate() {
            for dx in 0..3 {
                if row & (4 >> dx) != 0 {
                    rect(p, x + i * 4 + dx, y + dy, 1, 1, color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(app: &mut App, point: Point) {
        app.pointer(point);
        app.release();
    }

    #[test]
    fn home_routes_to_all_available_screens_and_keeps_disabled_item() {
        for (point, screen) in [
            (Point { x: 80, y: 100 }, Screen::Settings),
            (Point { x: 80, y: 150 }, Screen::Device),
            (Point { x: 80, y: 210 }, Screen::Controls),
        ] {
            let mut app = App::default();
            tap(&mut app, point);
            assert_eq!(app.screen, screen);
            app.button(Button::TopLeft, 1);
            assert_eq!(app.screen, Screen::Home);
        }
        let mut app = App::default();
        tap(&mut app, Point { x: 80, y: 260 });
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.focus, 3);
    }

    #[test]
    fn menu_hit_test_requires_bounds_original_target_and_tap_slop() {
        let mut app = App::default();
        app.pointer(Point { x: 0, y: 100 });
        app.pointer(Point { x: 80, y: 100 });
        assert_eq!(app.pressed, None);
        app.release();
        assert_eq!(app.screen, Screen::Home);

        app.pointer(Point { x: 80, y: 100 });
        app.pointer(Point { x: 80, y: 119 });
        app.pointer(Point { x: 80, y: 100 });
        assert_eq!(app.pressed, None);
        app.release();
        assert_eq!(app.screen, Screen::Home);

        app.pointer(Point { x: 9, y: 70 });
        assert_eq!(app.pressed, Some(0));
        app.cancel();
        app.pointer(Point { x: 8, y: 70 });
        assert_eq!(app.pressed, None);
    }

    #[test]
    fn settings_brightness_supports_drag_buttons_and_cancel() {
        let mut app = App::default();
        tap(&mut app, Point { x: 80, y: 100 });
        app.pointer(Point { x: 24, y: 170 });
        assert_eq!(app.controls.brightness, 5);
        assert_eq!(app.pressed, Some(0));
        app.pointer(Point { x: 216, y: 300 });
        assert_eq!(app.controls.brightness, 100);
        app.cancel();
        assert_eq!(app.point(), None);
        assert_eq!(app.pressed, None);
        app.button(Button::BottomLeft, 1);
        assert_eq!(app.controls.brightness, 95);
        app.button(Button::BottomRight, 1);
        assert_eq!(app.controls.brightness, 100);
        tap(&mut app, Point { x: 80, y: 235 });
        assert_eq!(app.dim_timeout_secs, 60);
        tap(&mut app, Point { x: 80, y: 275 });
        assert_eq!(app.dim_brightness, 20);
        tap(&mut app, Point { x: 80, y: 300 });
        assert_eq!(app.timezone_minutes, 30);
        app.pointer(Point { x: 80, y: 235 });
        app.cancel();
        app.release();
        assert_eq!(app.dim_timeout_secs, 60);
        app.pointer(Point { x: 80, y: 235 });
        app.pointer(Point { x: 0, y: 235 });
        app.pointer(Point { x: 80, y: 235 });
        app.release();
        assert_eq!(app.dim_timeout_secs, 60);
    }

    #[test]
    fn held_pointer_is_suppressed_across_repeated_button_navigation() {
        let mut app = App::default();
        app.pointer(Point { x: 80, y: 100 });
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Settings);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Home);
        assert!(app.pointer_suppressed());
        app.pointer(Point { x: 80, y: 210 });
        assert_eq!(app.point(), None);
        app.release();
        tap(&mut app, Point { x: 80, y: 210 });
        assert_eq!(app.screen, Screen::Controls);
    }

    #[test]
    fn button_focus_select_and_unknown_codes_are_bounded() {
        let mut app = App::default();
        for _ in 0..8 {
            app.button(Button::BottomRight, 1);
        }
        assert_eq!(app.focus, 3);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Home);
        app.button(Button::BottomLeft, 1);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Controls);
        app.button(Button::TopLeft, 2);
        assert_eq!(app.screen, Screen::Controls);
    }

    #[test]
    fn snapshot_restores_screen_focus_brightness_and_cancels_gesture() {
        let mut app = App {
            focus: 1,
            ..App::default()
        };
        let snapshot = app.snapshot();
        app.button(Button::TopLeft, 1);
        app.pointer(Point { x: 10, y: 10 });
        app.controls.brightness = 90;
        app.restore(snapshot);
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.focus, 1);
        assert_eq!(app.controls.brightness, 50);
        assert_eq!(app.dim_timeout_secs, 30);
        assert_eq!(app.dim_brightness, 10);
        assert_eq!(app.timezone_minutes, 0);
        assert_eq!(app.point(), None);
        assert_eq!(app.pressed, None);
    }

    #[test]
    fn missing_device_values_render_explicit_placeholders() {
        assert_eq!(power_label(None), b"UNKNOWN");
        assert_eq!(power_label(Some(2)), b"UNKNOWN");
        assert_eq!(short_wifi(b"WIFI NOT SET UP"), b"WIFI --");
        let mut app = App::default();
        tap(&mut app, Point { x: 80, y: 150 });
        let mut pixels = [0; PIXELS];
        app.render(
            &mut pixels,
            true,
            &Status::default(),
            b"WIFI NOT SET UP",
            &metrics::Snapshot::default(),
            &crate::network_time::Snapshot::default(),
        );
        assert_ne!(pixels, [theme::BACKGROUND; PIXELS]);
    }

    #[test]
    fn timezone_cycle_is_bounded_and_clock_states_render() {
        assert_eq!(next_timezone(825), 840);
        assert_eq!(next_timezone(840), -720);
        assert_eq!(next_timezone(-719), -690);
        let app = App {
            screen: Screen::Device,
            timezone_minutes: 330,
            ..App::default()
        };
        let mut pixels = [0; PIXELS];
        let clock = crate::network_time::Snapshot {
            unix_seconds: Some(1_700_000_000),
            millis: 0,
            local_minutes: Some(5 * 60 + 30),
            age_ms: Some(2_000),
            status: crate::network_time::Status::Fresh,
        };
        app.render(
            &mut pixels,
            true,
            &Status::default(),
            b"WIFI TEST OK",
            &metrics::Snapshot::default(),
            &clock,
        );
        assert_ne!(pixels, [theme::BACKGROUND; PIXELS]);
        assert_eq!(
            time_status(crate::network_time::Status::Offline),
            b"OFFLINE TIME"
        );
        assert_eq!(
            time_status(crate::network_time::Status::Stale),
            b"STALE TIME"
        );
    }

    #[test]
    fn diagnostics_is_reachable_by_touch_and_button_and_returns_to_device() {
        let mut app = App::default();
        tap(&mut app, Point { x: 80, y: 150 });
        tap(&mut app, Point { x: 80, y: 270 });
        assert_eq!(app.screen, Screen::Diagnostics);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Device);
        app.button(Button::BottomRight, 1);
        assert_eq!(app.screen, Screen::Diagnostics);

        let mut pixels = [0; PIXELS];
        let metrics = metrics::Snapshot {
            uptime_ms: 3_723_000,
            frame_ms: 20,
            max_frame_ms: 44,
            heap_free: 116 * 1024,
            heap_min_sampled: 115 * 1024,
            psram_capacity: 2 * 1024 * 1024,
            psram_free: 2 * 1024 * 1024,
            companion_valid: u32::MAX,
            companion_bad_crc: 2,
            uart_errors: 1,
            touch_errors: 0,
            harness: true,
            recording: false,
            selected_brightness: 50,
            effective_brightness: 50,
            dimmed: false,
            idle_ms: 0,
            dim_timeout_secs: 30,
            dim_brightness: 10,
        };
        app.render(
            &mut pixels,
            true,
            &Status::default(),
            b"WIFI TEST OK",
            &metrics,
            &crate::network_time::Snapshot::default(),
        );
        assert_ne!(pixels, [theme::BACKGROUND; PIXELS]);
    }
}
