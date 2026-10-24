//! Row painting: a screen fills one display row at a time with a few span
//! writes instead of answering a hit test for every pixel. Later paints cover
//! earlier ones, so paint backgrounds first. Coordinates stay those of the
//! whole row; writes outside its clip range are dropped.
use super::text::glyph;
use super::work;
use core::ops::Range;

pub struct Row<'a> {
    y: usize,
    /// Pixels before this column are outside the clip; the slice ends at the other edge.
    start: usize,
    pixels: &'a mut [u16],
}

impl<'a> Row<'a> {
    pub fn new(y: usize, pixels: &'a mut [u16]) -> Self {
        Self {
            y,
            start: 0,
            pixels,
        }
    }
    pub fn y(&self) -> usize {
        self.y
    }
    pub fn width(&self) -> usize {
        self.pixels.len()
    }
    pub fn pixels(&self) -> &[u16] {
        self.pixels
    }
    /// Columns this row may write.
    pub fn clip(&self) -> Range<usize> {
        self.start..self.pixels.len()
    }
    /// The whole row for paints that read what lies beneath, such as blended
    /// glyph runs. Indices are row columns; such paints must respect `clip()`.
    pub fn pixels_mut(&mut self) -> &mut [u16] {
        self.pixels
    }
    /// The same row clipped further to `x`, for regions that own their pixels.
    pub fn within(&mut self, x: Range<usize>) -> Row<'_> {
        let end = x.end.min(self.pixels.len());
        Row {
            y: self.y,
            start: self.start.max(x.start),
            pixels: &mut self.pixels[..end],
        }
    }
    pub fn fill(&mut self, color: u16) {
        self.span(0..usize::MAX, color);
    }
    pub fn span(&mut self, x: Range<usize>, color: u16) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        if start < end {
            self.pixels[start..end].fill(color);
            work::add(|work| {
                work.spans += 1;
                work.span_pixels += (end - start) as u64;
            });
        }
    }
    pub fn set(&mut self, x: usize, color: u16) {
        if x >= self.start
            && let Some(pixel) = self.pixels.get_mut(x)
        {
            *pixel = color;
        }
    }
    /// Paint `left + i` for every set bit `i` of a bitmap row.
    pub fn bits(&mut self, left: usize, bits: u32, color: u16) {
        self.bits_scaled(left, bits, 1, color);
    }
    /// `bits`, with every bit covering `scale` pixels.
    pub fn bits_scaled(&mut self, left: usize, mut bits: u32, scale: usize, color: u16) {
        while bits != 0 {
            let x = left + bits.trailing_zeros() as usize * scale;
            self.span(x..x + scale, color);
            bits &= bits - 1;
        }
    }
    /// Paint exactly the pixels of this row that `text::text` lights for the label.
    pub fn text(&mut self, left: usize, top: usize, scale: usize, label: &[u8], color: u16) {
        if self.y < top || self.y >= top + 7 * scale {
            return;
        }
        let line = (self.y - top) / scale;
        let advance = 6 * scale;
        for (i, &character) in label.iter().enumerate() {
            let start = left + i * advance;
            if start >= self.pixels.len() {
                break;
            }
            // Glyph rows put the leftmost of five dots in bit four.
            let bits = glyph(character)[line].reverse_bits() >> 3;
            self.bits_scaled(start, u32::from(bits), scale, color);
        }
    }
    /// Per-pixel fallback for shapes without a cheaper row form; `None` keeps the pixel.
    pub fn each(&mut self, x: Range<usize>, mut pixel: impl FnMut(usize) -> Option<u16>) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        work::add(|work| work.evaluations += end.saturating_sub(start) as u64);
        for x in start..end {
            if let Some(color) = pixel(x) {
                self.pixels[x] = color;
            }
        }
    }
    /// `each`, given what the row already holds at each pixel, for edges
    /// that blend over it.
    pub fn each_over(&mut self, x: Range<usize>, mut pixel: impl FnMut(usize, u16) -> Option<u16>) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        work::add(|work| work.evaluations += end.saturating_sub(start) as u64);
        for x in start..end {
            if let Some(color) = pixel(x, self.pixels[x]) {
                self.pixels[x] = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Row;
    use crate::rendering::text::text;

    #[test]
    fn text_paints_exactly_the_hit_tested_dots() {
        let mut label = [0u8; 96];
        for (i, byte) in label.iter_mut().enumerate() {
            *byte = 32 + i as u8;
        }
        for scale in 1..=4 {
            for chunk in label.chunks(9) {
                for left in [0, 3, 17] {
                    for y in 0..7 * scale + 4 {
                        let mut pixels = [0u16; 240];
                        Row::new(y, &mut pixels).text(left, 2, scale, chunk, 7);
                        for (x, &pixel) in pixels.iter().enumerate() {
                            assert_eq!(pixel == 7, text(x, y, left, 2, scale, chunk));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn writes_outside_the_clip_are_dropped() {
        let mut pixels = [0u16; 8];
        let mut row = Row::new(0, &mut pixels);
        row.span(6..20, 1);
        row.set(8, 2);
        row.bits(5, 0b1001, 3);
        row.text(7, 0, 2, b"W", 4);
        row.each(7..30, |_| Some(5));
        let mut middle = row.within(2..4);
        assert_eq!(middle.clip(), 2..4);
        middle.fill(6);
        middle.within(3..9).set(3, 7);
        middle.set(1, 8);
        assert_eq!(pixels, [0, 0, 6, 7, 0, 3, 1, 5]);
    }
}
