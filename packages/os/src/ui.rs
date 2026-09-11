//! Portable application state and drawing primitives for the C606 display.

use crate::{
    coin::{HEIGHT, PIXELS, WIDTH},
    companion::{Button, Status},
    controls::Controls,
    input::Point,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Screen {
    Menu,
    Controls,
}

impl Screen {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Controls => "controls",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    screen: Screen,
    focus: u8,
    brightness: u8,
}

pub struct App {
    pub screen: Screen,
    pub focus: u8,
    pub pressed: Option<u8>,
    pub controls: Controls,
    point: Option<Point>,
    origin: Option<u8>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Menu,
            focus: 0,
            pressed: None,
            controls: Controls::default(),
            point: None,
            origin: None,
        }
    }
}

impl App {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            screen: self.screen,
            focus: self.focus,
            brightness: self.controls.brightness,
        }
    }

    pub fn restore(&mut self, snapshot: Snapshot) {
        self.cancel();
        self.screen = snapshot.screen;
        self.focus = snapshot.focus;
        self.controls.brightness = snapshot.brightness;
    }

    pub fn point(&self) -> Option<Point> {
        match self.screen {
            Screen::Menu => self.point,
            Screen::Controls => self.controls.point,
        }
    }

    pub fn pointer(&mut self, point: Point) {
        match self.screen {
            Screen::Menu => {
                if self.point.is_none() {
                    self.origin = menu_item(point);
                }
                self.point = Some(point);
                self.pressed = self
                    .origin
                    .filter(|&item| menu_item(point) == Some(item) && enabled(item));
                if let Some(item) = menu_item(point) {
                    self.focus = item;
                }
            }
            Screen::Controls => self.controls.update(Some(point)),
        }
    }

    /// Complete a physical or injected pointer gesture and activate its target.
    pub fn release(&mut self) {
        match self.screen {
            Screen::Menu => {
                self.point = None;
                self.origin = None;
                if let Some(item) = self.pressed.take() {
                    self.activate(item);
                }
            }
            Screen::Controls => self.controls.update(None),
        }
    }

    /// Abandon a gesture without activating it.
    pub fn cancel(&mut self) {
        self.point = None;
        self.origin = None;
        self.pressed = None;
        self.controls.update(None);
    }

    pub fn button(&mut self, button: Button, code: u16) {
        if code != 1 {
            return;
        }
        self.cancel();
        match self.screen {
            Screen::Menu => match button {
                Button::BottomLeft => self.focus = self.focus.saturating_sub(1),
                Button::BottomRight => self.focus = (self.focus + 1).min(1),
                Button::TopLeft => self.activate(self.focus),
            },
            Screen::Controls => match button {
                Button::TopLeft => self.navigate(Screen::Menu),
                _ => self.controls.button(button, code),
            },
        }
    }

    pub fn render(
        &self,
        pixels: &mut [u16; PIXELS],
        available: bool,
        status: &Status,
        wifi: &[u8],
    ) {
        match self.screen {
            Screen::Menu => self.render_menu(pixels),
            Screen::Controls => {
                self.controls.render(pixels, available, status);
                crate::controls::wifi_label(pixels, wifi);
            }
        }
    }

    fn navigate(&mut self, screen: Screen) {
        self.cancel();
        self.screen = screen;
        self.focus = 0;
    }

    fn activate(&mut self, item: u8) {
        if item == 0 {
            self.navigate(Screen::Controls);
        }
    }

    fn render_menu(&self, pixels: &mut [u16; PIXELS]) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 5, 6, b"CYCLING", theme::TEXT);
        text(pixels, 5, 15, b"MENU", theme::MUTED);
        item(
            pixels,
            25,
            b"CONTROLS",
            true,
            self.focus == 0,
            self.pressed == Some(0),
        );
        item(pixels, 50, b"DEVICE  LATER", false, self.focus == 1, false);
        text(pixels, 5, 91, b"TOP SELECT", theme::MUTED);
        text(pixels, 5, 99, b"LEFT RIGHT", theme::MUTED);
    }
}

fn menu_item(point: Point) -> Option<u8> {
    if !(9..=230).contains(&point.x) {
        return None;
    }
    match point.y {
        76..=135 => Some(0),
        151..=210 => Some(1),
        _ => None,
    }
}

fn enabled(item: u8) -> bool {
    item == 0
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
    let color = if enabled {
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
            b'Y' => [5, 5, 2, 2, 2],
            b'.' => [0, 0, 0, 0, 2],
            b'-' => [0, 0, 7, 0, 0],
            b'+' => [0, 2, 7, 2, 0],
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

    #[test]
    fn release_activates_but_cancel_does_not() {
        let mut app = App::default();
        app.pointer(Point { x: 20, y: 100 });
        assert_eq!(app.pressed, Some(0));
        app.cancel();
        assert_eq!(app.screen, Screen::Menu);
        app.pointer(Point { x: 20, y: 100 });
        app.release();
        assert_eq!(app.screen, Screen::Controls);
    }

    #[test]
    fn disabled_item_can_focus_but_not_press_or_activate() {
        let mut app = App::default();
        app.pointer(Point { x: 20, y: 180 });
        assert_eq!(app.focus, 1);
        assert_eq!(app.pressed, None);
        app.release();
        assert_eq!(app.screen, Screen::Menu);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Menu);
    }

    #[test]
    fn menu_hit_test_requires_visible_bounds_and_original_target() {
        let mut app = App::default();
        app.pointer(Point { x: 0, y: 100 });
        app.pointer(Point { x: 80, y: 100 });
        assert_eq!(app.pressed, None);
        app.release();
        assert_eq!(app.screen, Screen::Menu);

        app.pointer(Point { x: 80, y: 100 });
        app.pointer(Point { x: 80, y: 180 });
        assert_eq!(app.pressed, None);
        app.pointer(Point { x: 80, y: 100 });
        assert_eq!(app.pressed, Some(0));
    }

    #[test]
    fn short_click_buttons_navigate_and_disabled_item_does_not_activate() {
        let mut app = App::default();
        app.button(Button::BottomRight, 1);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Menu);
        app.button(Button::BottomLeft, 1);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Controls);
        app.button(Button::TopLeft, 1);
        assert_eq!(app.screen, Screen::Menu);
    }

    #[test]
    fn snapshot_restores_navigation_and_cancels_gesture() {
        let mut app = App {
            focus: 1,
            ..App::default()
        };
        let snapshot = app.snapshot();
        app.focus = 0;
        app.button(Button::TopLeft, 1);
        app.pointer(Point { x: 24, y: 260 });
        app.restore(snapshot);
        assert_eq!(app.screen, Screen::Menu);
        assert_eq!(app.focus, 1);
        assert_eq!(app.point(), None);
        assert_eq!(app.pressed, None);
    }
}
