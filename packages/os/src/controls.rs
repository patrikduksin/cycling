//! Touch test and brightness UI on the existing 80x106 canvas.
use crate::{
    coin::PIXELS,
    companion::{Button, Status},
    input::Point,
    ui::{cross, number, rect, text, theme},
};

pub struct Controls {
    pub brightness: u8,
    pub point: Option<Point>,
    dragging: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            brightness: 50,
            point: None,
            dragging: false,
        }
    }
}

impl Controls {
    pub fn button(&mut self, button: Button, code: u16) {
        if code == 1 {
            match button {
                Button::BottomLeft => self.brightness = self.brightness.saturating_sub(5).max(5),
                Button::BottomRight => self.brightness = self.brightness.saturating_add(5).min(100),
                Button::TopLeft => {}
            }
        }
    }

    pub fn update(&mut self, point: Option<Point>) {
        if self.point.is_none() {
            self.dragging = point.is_some_and(|p| (231..=294).contains(&p.y));
        }
        if let Some(p) = point {
            if self.dragging {
                self.brightness = (5 + (u32::from(p.x.clamp(24, 216)) - 24) * 95 / 192) as u8;
            }
        } else {
            self.dragging = false;
        }
        self.point = point;
    }

    pub fn render(&self, pixels: &mut [u16; PIXELS], available: bool, status: &Status) {
        pixels.fill(theme::BACKGROUND);
        text(pixels, 5, 5, b"BATTERY", theme::TEXT);
        if let Some((percent, mv)) = status.battery {
            number(pixels, 53, 5, percent as u32, theme::ACCENT);
            text(pixels, 69, 5, b"%", theme::ACCENT);
            let voltage = [
                b'0' + (mv / 1000) as u8,
                b'.',
                b'0' + ((mv / 100) % 10) as u8,
                b'0' + ((mv / 10) % 10) as u8,
                b'V',
            ];
            text(pixels, 5, 15, &voltage, theme::MUTED);
        } else {
            text(pixels, 53, 5, b"--", theme::MUTED);
        }
        text(
            pixels,
            5,
            25,
            match status.power {
                Some(0) => b"CHARGING",
                Some(1) => b"ON BATTERY",
                _ => b"POWER UNKNOWN",
            },
            theme::MUTED,
        );
        for (i, label) in [b"TOP".as_slice(), b"LEFT", b"RIGHT"].iter().enumerate() {
            let x = 3 + i * 26;
            let color = if status.last_button.is_some_and(|b| b.index() == i) {
                theme::ACCENT
            } else {
                theme::MUTED
            };
            rect(pixels, x, 37, 24, 23, theme::SURFACE);
            text(pixels, x + 2, 40, label, color);
            number(pixels, x + 2, 51, status.button_counts[i].min(999), color);
        }
        text(
            pixels,
            5,
            65,
            if available {
                b"TAP OR DRAG"
            } else {
                b"TOUCH OFFLINE"
            },
            theme::MUTED,
        );
        if let Some(p) = self.point {
            cross(
                pixels,
                (p.x / 3) as i32,
                (p.y.saturating_sub(1) / 3) as i32,
                theme::ACCENT,
            );
        }
        rect(pixels, 0, 72, 80, 34, theme::SURFACE);
        text(pixels, 5, 75, b"BRIGHTNESS", theme::TEXT);
        let digits = [
            b'0' + self.brightness / 100,
            b'0' + (self.brightness / 10) % 10,
            b'0' + self.brightness % 10,
            b'%',
        ];
        text(
            pixels,
            59,
            75,
            &digits[usize::from(self.brightness < 100)..],
            theme::ACCENT,
        );
        rect(pixels, 8, 87, 65, 2, 0x4a69);
        let knob = 8 + (usize::from(self.brightness) - 5) * 64 / 95;
        rect(pixels, 8, 87, knob - 8 + 1, 2, theme::ACCENT);
        rect(pixels, knob - 2, 83, 5, 10, theme::TEXT);
        text(pixels, 5, 98, b"LEFT - RIGHT +", theme::MUTED);
    }
}

/// Show connection progress without displaying network identifiers.
pub fn wifi_label(pixels: &mut [u16; PIXELS], label: &[u8]) {
    rect(pixels, 0, 63, 80, 8, theme::BACKGROUND);
    text(pixels, 5, 65, label, theme::ACCENT);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slider_captures_only_drags_started_inside_and_clamps() {
        let mut ui = Controls::default();
        ui.update(Some(Point { x: 0, y: 100 }));
        ui.update(Some(Point { x: 239, y: 260 }));
        assert_eq!(ui.brightness, 50);
        ui.update(None);
        ui.update(Some(Point { x: 0, y: 260 }));
        assert_eq!(ui.brightness, 5);
        ui.update(Some(Point { x: 239, y: 310 }));
        assert_eq!(ui.brightness, 100);
        ui.update(None);
        assert!(ui.point.is_none());
    }
}
