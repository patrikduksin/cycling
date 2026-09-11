//! Touch test and brightness UI on the existing 80x106 canvas.
use crate::{
    coin::{HEIGHT, PIXELS, WIDTH},
    input::Point,
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

    pub fn render(&self, pixels: &mut [u16; PIXELS], available: bool) {
        pixels.fill(0x0863);
        text(pixels, 5, 5, b"TOUCH TEST", 0xffff);
        text(
            pixels,
            5,
            15,
            if available {
                b"TAP OR DRAG"
            } else {
                b"TOUCH OFFLINE"
            },
            0x8c71,
        );
        for x in [5, 74] {
            for y in [26, 65] {
                cross(pixels, x, y, 0x4a69);
            }
        }
        if let Some(p) = self.point {
            cross(
                pixels,
                (p.x / 3) as i32,
                (p.y.saturating_sub(1) / 3) as i32,
                0x07ff,
            );
        }
        rect(pixels, 0, 72, 80, 34, 0x18e5);
        text(pixels, 5, 75, b"BRIGHTNESS", 0xffff);
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
            0x07ff,
        );
        rect(pixels, 8, 87, 65, 2, 0x4a69);
        let knob = 8 + (usize::from(self.brightness) - 5) * 64 / 95;
        rect(pixels, 8, 87, knob - 8 + 1, 2, 0x07ff);
        rect(pixels, knob - 2, 83, 5, 10, 0xffff);
        text(pixels, 5, 98, b"DIM", 0x8c71);
        text(pixels, 51, 98, b"BRIGHT", 0x8c71);
    }
}

fn rect(p: &mut [u16; PIXELS], x: usize, y: usize, w: usize, h: usize, color: u16) {
    for row in y..(y + h).min(HEIGHT) {
        for col in x..(x + w).min(WIDTH) {
            p[row * WIDTH + col] = color;
        }
    }
}
fn cross(p: &mut [u16; PIXELS], x: i32, y: i32, color: u16) {
    for d in -3..=3 {
        for (xx, yy) in [(x + d, y), (x, y + d)] {
            if xx >= 0 && yy >= 0 && xx < WIDTH as i32 && yy < HEIGHT as i32 {
                p[yy as usize * WIDTH + xx as usize] = color;
            }
        }
    }
}
fn text(p: &mut [u16; PIXELS], x: usize, y: usize, s: &[u8], color: u16) {
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
            b'L' => [4, 4, 4, 4, 7],
            b'M' => [5, 7, 7, 5, 5],
            b'N' => [5, 7, 7, 7, 5],
            b'O' => [2, 5, 5, 5, 2],
            b'P' => [6, 5, 6, 4, 4],
            b'R' => [6, 5, 6, 5, 5],
            b'S' => [3, 4, 2, 1, 6],
            b'T' => [7, 2, 2, 2, 2],
            b'U' => [5, 5, 5, 5, 7],
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
