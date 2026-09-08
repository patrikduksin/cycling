//! A tiny software renderer. One 80×106 RGB565 canvas, no heap or frame history.

pub const WIDTH: usize = 80;
pub const HEIGHT: usize = 106;
pub const PIXELS: usize = WIDTH * HEIGHT;
pub const PERIOD: u32 = 96;
const LOGO: &[u8; 512] = include_bytes!("../assets/rust-logo.mask");
const BACKGROUND: u16 = rgb(10, 15, 24);

const fn rgb(r: u16, g: u16, b: u16) -> u16 {
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

fn logo(x: usize, y: usize) -> bool {
    x < 64 && y < 64 && LOGO[y * 8 + x / 8] & (0x80 >> (x % 8)) != 0
}

/// Render a complete frame; integer frame numbers make captures reproducible.
pub fn render(frame: u32, pixels: &mut [u16; PIXELS]) {
    pixels.fill(BACKGROUND);
    let angle = (frame % PERIOD) as f32 * core::f32::consts::TAU / PERIOD as f32;
    let cosine = libm::cosf(angle);
    let sine = libm::sinf(angle);
    let face_width = cosine.abs().max(0.015);
    let thickness = sine * 3.0;
    let center_y = 49.0 + libm::sinf(angle * 2.0) * 1.5;
    const RADIUS: f32 = 27.0;

    for y in 0..HEIGHT {
        let dy = y as f32 - center_y;
        for x in 0..WIDTH {
            let dx = x as f32 - 39.5;
            let mut color = BACKGROUND;
            // A soft, stepped shadow anchors the coin without a second buffer.
            let shadow = dx * dx / 625.0 + ((y as f32 - 83.0) * (y as f32 - 83.0)) / 12.0;
            if shadow < 1.0 {
                color = if shadow < 0.55 {
                    rgb(3, 7, 12)
                } else {
                    rgb(7, 11, 18)
                };
            }
            if dy.abs() <= RADIUS {
                let half = libm::sqrtf(RADIUS * RADIUS - dy * dy) * face_width;
                let left = (-half).min(-half + thickness);
                let right = half.max(half + thickness);
                if dx >= left && dx <= right {
                    // The exposed edge has alternating milled ridges.
                    color = if y % 3 == 0 {
                        rgb(178, 77, 24)
                    } else {
                        rgb(112, 45, 19)
                    };
                }
                if dx.abs() <= half {
                    let surface_x = dx / face_width;
                    let distance = surface_x * surface_x + dy * dy;
                    let shine = ((-surface_x * 0.6 - dy * 0.8) / RADIUS + 1.0) * 0.5;
                    let shade = (shine * 5.0) as u16;
                    color = rgb(205 + shade * 9, 94 + shade * 16, 34 + shade * 10);
                    if distance > (RADIUS - 1.7) * (RADIUS - 1.7) {
                        color = if dy < 0.0 {
                            rgb(255, 204, 111)
                        } else {
                            rgb(156, 60, 22)
                        };
                    } else if distance > (RADIUS - 3.2) * (RADIUS - 3.2) {
                        color = rgb(172, 70, 24);
                    }
                    // Back face mirrors naturally as the coin turns through 180°.
                    let local_x = if cosine >= 0.0 { surface_x } else { -surface_x };
                    let u = ((local_x + 23.0) * 64.0 / 46.0) as i32;
                    let v = ((dy + 23.0) * 64.0 / 46.0) as i32;
                    if u >= 0 && v >= 0 && logo(u as usize, v as usize) {
                        color = rgb(39, 26, 24);
                    }
                }
            }
            pixels[y * WIDTH + x] = color;
        }
    }
    text(pixels, "CYCLING", 20, 10, rgb(136, 154, 170));
    text(pixels, "RUST", 29, 94, rgb(237, 150, 78));
    pixels[99 * WIDTH + 39] = rgb(60, 80, 91);
    pixels[99 * WIDTH + 40] = rgb(60, 80, 91);
}

fn text(pixels: &mut [u16; PIXELS], s: &str, x: usize, y: usize, color: u16) {
    for (i, c) in s.bytes().enumerate() {
        let rows = match c {
            b'C' => [7, 4, 4, 4, 7],
            b'Y' => [5, 5, 2, 2, 2],
            b'L' => [4, 4, 4, 4, 7],
            b'I' => [7, 2, 2, 2, 7],
            b'N' => [5, 7, 7, 7, 5],
            b'G' => [7, 4, 5, 5, 7],
            b'R' => [6, 5, 6, 5, 5],
            b'U' => [5, 5, 5, 5, 7],
            b'S' => [7, 4, 7, 1, 7],
            b'T' => [7, 2, 2, 2, 2],
            _ => [0; 5],
        };
        for (row, bits) in rows.into_iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    pixels[(y + row) * WIDTH + x + i * 6 + col] = color;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_turn_returns_to_the_same_frame() {
        let mut start = [0; PIXELS];
        let mut end = [0; PIXELS];
        render(0, &mut start);
        render(PERIOD, &mut end);
        assert_eq!(start, end);
    }

    #[test]
    fn edge_on_silhouette_is_narrower_than_the_face() {
        let mut face = [0; PIXELS];
        let mut edge = [0; PIXELS];
        render(0, &mut face);
        render(PERIOD / 4, &mut edge);
        let count = |frame: &[u16; PIXELS]| {
            frame[49 * WIDTH..50 * WIDTH]
                .iter()
                .filter(|&&p| p != BACKGROUND)
                .count()
        };
        assert!(count(&face) > 45);
        assert!(count(&edge) < 10);
    }

    #[test]
    fn every_angle_renders_without_leaving_the_canvas() {
        let mut frame = [0; PIXELS];
        for angle in 0..PERIOD {
            render(angle, &mut frame);
            assert_eq!(frame[0], BACKGROUND);
            assert_eq!(frame[PIXELS - 1], BACKGROUND);
        }
    }
}
