//! RGB8 compositing surface with RGB565 blitting, firmware text and PNG output.

use firmware_shell::rendering::text::text;

pub fn rgb565_to_rgb8(p: u16) -> [u8; 3] {
    let r = ((p >> 11) & 0x1f) as u8;
    let g = ((p >> 5) & 0x3f) as u8;
    let b = (p & 0x1f) as u8;
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, background: [u8; 3]) -> Self {
        Self {
            width,
            height,
            rgb: background.repeat(width * height),
        }
    }

    pub fn set(&mut self, x: usize, y: usize, color: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = (y * self.width + x) * 3;
            self.rgb[i..i + 3].copy_from_slice(&color);
        }
    }

    /// Nearest-neighbour scaled blit; pixels outside the canvas are clipped.
    pub fn blit565(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        pixels: &[u16],
        scale: usize,
    ) {
        for row in 0..height {
            for col in 0..width {
                let Some(&p) = pixels.get(row * width + col) else {
                    return;
                };
                let color = rgb565_to_rgb8(p);
                self.fill_rect(x + col * scale, y + row * scale, scale, scale, color);
            }
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
        let x_end = (x + w).min(self.width);
        for row in y..(y + h).min(self.height) {
            for col in x..x_end {
                let i = (row * self.width + col) * 3;
                self.rgb[i..i + 3].copy_from_slice(&color);
            }
        }
    }

    /// Draws with the firmware font, which only knows uppercase, digits and `: / + % , . - >`.
    pub fn text(&mut self, x: usize, y: usize, scale: usize, color: [u8; 3], label: &str) -> usize {
        let bytes = label.to_ascii_uppercase().into_bytes();
        let width = bytes.len() * 6 * scale;
        for dy in 0..7 * scale {
            for dx in 0..width {
                if text(dx, dy, 0, 0, scale, &bytes) {
                    self.set(x + dx, y + dy, color);
                }
            }
        }
        width
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&self.rgb))
            .expect("encoding into a Vec cannot fail for a consistent canvas");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_extremes_expand_fully() {
        assert_eq!(rgb565_to_rgb8(0x0000), [0, 0, 0]);
        assert_eq!(rgb565_to_rgb8(0xffff), [255, 255, 255]);
        assert_eq!(rgb565_to_rgb8(0xf800), [255, 0, 0]);
        assert_eq!(rgb565_to_rgb8(0x07e0), [0, 255, 0]);
        assert_eq!(rgb565_to_rgb8(0x001f), [0, 0, 255]);
    }

    #[test]
    fn png_round_trips_size_and_pixels() {
        let mut canvas = Canvas::new(8, 6, [1, 2, 3]);
        canvas.blit565(2, 1, 2, 1, &[0xf800, 0x07e0], 2);
        let data = canvas.png();
        let mut reader = png::Decoder::new(std::io::Cursor::new(data))
            .read_info()
            .unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((info.width, info.height), (8, 6));
        let at = |x: usize, y: usize| buffer[(y * 8 + x) * 3..][..3].to_vec();
        assert_eq!(at(0, 0), [1, 2, 3]);
        assert_eq!(at(2, 1), [255, 0, 0]);
        assert_eq!(at(5, 2), [0, 255, 0]);
    }

    #[test]
    fn text_draws_pixels_and_reports_width() {
        let mut canvas = Canvas::new(40, 10, [0, 0, 0]);
        let width = canvas.text(1, 1, 1, [255, 255, 255], "ab 12");
        assert_eq!(width, 30);
        assert!(canvas.rgb.contains(&255));
    }
}
