//! Row painting: a screen fills one display row at a time with a few span
//! writes instead of answering a hit test for every pixel. Later paints cover
//! earlier ones, so paint backgrounds first. Coordinates stay those of the
//! whole row; writes outside its clip range are dropped.
//!
//! A row may carry a look, a map from palette colours to the colours shown.
//! The entry points (`fill`, `span`, `set`, `bits`, `bits_scaled`, `text`)
//! take palette colours and map them once a call. Colours already worked
//! out, such as blends and colours read back from the row, are written as
//! given: `each` and `each_over` write their results unmapped, and a span of
//! one goes through `computed`.
use super::record::{self, Kind};
use super::text::glyph;
use super::work;
use core::ops::Range;

/// A map from palette colours to the colours a row shows; `PLAIN` shows
/// them as they are.
#[derive(Clone, Copy, Default)]
pub struct Look(Option<fn(u16) -> u16>);

impl Look {
    pub const PLAIN: Self = Self(None);

    pub const fn new(map: fn(u16) -> u16) -> Self {
        Self(Some(map))
    }

    /// The colour shown for palette colour `color`.
    #[inline]
    pub fn ink(self, color: u16) -> u16 {
        match self.0 {
            Some(map) => map(color),
            None => color,
        }
    }
}

pub struct Row<'a> {
    y: usize,
    /// Pixels before this column are outside the clip; the slice ends at the other edge.
    start: usize,
    pixels: &'a mut [u16],
    look: Look,
    /// How a write without a look is recorded: `Fixed`, or `Computed` for
    /// a `computed` row.
    plain_kind: Kind,
}

impl<'a> Row<'a> {
    pub fn new(y: usize, pixels: &'a mut [u16]) -> Self {
        record::open(pixels);
        Self {
            y,
            start: 0,
            pixels,
            look: Look::PLAIN,
            plain_kind: Kind::Fixed,
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
    /// Columns `x` of the row, from `x.start`, for paints that read what lies
    /// beneath, such as blended glyph runs. Such paints must respect `clip()`,
    /// take their palette colours through `ink`, and report what they wrote
    /// through `wrote`.
    pub fn pixels_mut(&mut self, x: Range<usize>) -> &mut [u16] {
        &mut self.pixels[x]
    }
    /// Record a paint through `pixels_mut` over columns `x`: `coverage(x)`
    /// is how much of the paint's colour column `x` took, 15 for exactly
    /// that colour, less for a blend over what lay beneath, and 0 for none,
    /// which keeps the pixel's record.
    pub fn wrote(&self, x: Range<usize>, coverage: impl Fn(usize) -> u8) {
        let (full, blended) = (self.written(), self.derived());
        let x = x.start.max(self.start)..x.end.min(self.pixels.len());
        record::mark_each(self.pixels.as_ptr(), x, |x| match coverage(x) {
            0 => None,
            15.. => Some(full),
            _ => Some(blended),
        });
    }
    /// The row's look, for paints that work out colours from palette ones.
    pub fn look(&self) -> Look {
        self.look
    }
    /// The colour this row shows for palette colour `color`.
    #[inline]
    pub fn ink(&self, color: u16) -> u16 {
        self.look.ink(color)
    }
    fn reborrow(&mut self, y: usize, start: usize, end: usize) -> Row<'_> {
        Row {
            y,
            start,
            pixels: &mut self.pixels[..end],
            look: self.look,
            plain_kind: self.plain_kind,
        }
    }
    /// The same row clipped further to `x`, for regions that own their pixels.
    pub fn within(&mut self, x: Range<usize>) -> Row<'_> {
        let end = x.end.min(self.pixels.len());
        self.reborrow(self.y, self.start.max(x.start), end)
    }
    /// The same pixels as row `y` of content that scrolls: paints placed for
    /// `y` land on this display row.
    pub fn at(&mut self, y: usize) -> Row<'_> {
        self.reborrow(y, self.start, self.pixels.len())
    }
    /// The same row, showing palette colours through `look`.
    pub fn looked(&mut self, look: Look) -> Row<'_> {
        let mut row = self.reborrow(self.y, self.start, self.pixels.len());
        row.look = look;
        row
    }
    /// The same row without a look, for colours that read the same in every
    /// look.
    pub fn plain(&mut self) -> Row<'_> {
        self.unmapped(Kind::Fixed)
    }
    /// The same row without a look, for colours already worked out from the
    /// colours it shows.
    pub fn computed(&mut self) -> Row<'_> {
        self.unmapped(self.derived())
    }
    fn unmapped(&mut self, kind: Kind) -> Row<'_> {
        let mut row = self.looked(Look::PLAIN);
        row.plain_kind = kind;
        row
    }
    fn written(&self) -> Kind {
        match self.look.0 {
            Some(_) => Kind::Mapped,
            None => self.plain_kind,
        }
    }
    fn derived(&self) -> Kind {
        if self.look.0.is_some() {
            Kind::Computed
        } else {
            self.written()
        }
    }
    /// Copy columns `x` from `from`'s columns `shift` further right, which
    /// already hold shown colours.
    pub fn copy(&mut self, x: Range<usize>, from: &Row, shift: usize) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        if start < end {
            self.pixels[start..end].copy_from_slice(&from.pixels[start + shift..end + shift]);
            record::copy(
                (from.pixels.as_ptr(), start + shift),
                (self.pixels.as_ptr(), start..end),
            );
        }
    }
    pub fn fill(&mut self, color: u16) {
        self.span(0..usize::MAX, color);
    }
    pub fn span(&mut self, x: Range<usize>, color: u16) {
        self.span_shown(x, self.ink(color));
    }
    /// `span` for a colour `ink` already gave, so a paint that also blends
    /// the colour maps it once.
    pub fn span_shown(&mut self, x: Range<usize>, color: u16) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        if start < end {
            fill(&mut self.pixels[start..end], color);
            record::mark(self.pixels.as_ptr(), start..end, self.written());
            work::add(|work| {
                work.spans += 1;
                work.span_pixels += (end - start) as u64;
            });
        }
    }
    pub fn set(&mut self, x: usize, color: u16) {
        let color = self.ink(color);
        if x >= self.start
            && let Some(pixel) = self.pixels.get_mut(x)
        {
            *pixel = color;
            record::mark(self.pixels.as_ptr(), x..x + 1, self.written());
        }
    }
    /// Paint `left + i` for every set bit `i` of a bitmap row.
    pub fn bits(&mut self, left: usize, bits: u32, color: u16) {
        self.bits_scaled(left, bits, 1, color);
    }
    /// `bits`, with every bit covering `scale` pixels.
    pub fn bits_scaled(&mut self, left: usize, bits: u32, scale: usize, color: u16) {
        self.bits_shown(left, bits, scale, self.ink(color));
    }
    fn bits_shown(&mut self, left: usize, mut bits: u32, scale: usize, color: u16) {
        while bits != 0 {
            let x = left + bits.trailing_zeros() as usize * scale;
            self.span_shown(x..x + scale, color);
            bits &= bits - 1;
        }
    }
    /// Paint exactly the pixels of this row that `text::text` lights for the label.
    pub fn text(&mut self, left: usize, top: usize, scale: usize, label: &[u8], color: u16) {
        if self.y < top || self.y >= top + 7 * scale {
            return;
        }
        let color = self.ink(color);
        let line = (self.y - top) / scale;
        let advance = 6 * scale;
        for (i, &character) in label.iter().enumerate() {
            let start = left + i * advance;
            if start >= self.pixels.len() {
                break;
            }
            // Glyph rows put the leftmost of five dots in bit four.
            let bits = glyph(character)[line].reverse_bits() >> 3;
            self.bits_shown(start, u32::from(bits), scale, color);
        }
    }
    /// Per-pixel fallback for shapes without a cheaper row form; `None` keeps
    /// the pixel. Colours are written as given, so palette ones go through
    /// `ink` first.
    pub fn each(&mut self, x: Range<usize>, mut pixel: impl FnMut(usize) -> Option<u16>) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        work::add(|work| work.evaluations += end.saturating_sub(start) as u64);
        let buffer = self.pixels.as_ptr();
        for (x, out) in self.pixels.iter_mut().enumerate().take(end).skip(start) {
            if let Some(color) = pixel(x) {
                *out = color;
                record::mark(buffer, x..x + 1, Kind::Computed);
            }
        }
    }
    /// `each`, given what the row already holds at each pixel, for edges
    /// that blend over it.
    pub fn each_over(&mut self, x: Range<usize>, mut pixel: impl FnMut(usize, u16) -> Option<u16>) {
        let start = x.start.max(self.start);
        let end = x.end.min(self.pixels.len());
        work::add(|work| work.evaluations += end.saturating_sub(start) as u64);
        let buffer = self.pixels.as_ptr();
        for (x, out) in self.pixels.iter_mut().enumerate().take(end).skip(start) {
            if let Some(color) = pixel(x, *out) {
                *out = color;
                record::mark(buffer, x..x + 1, Kind::Computed);
            }
        }
    }
}

/// `pixels.fill(color)` two pixels a store: at the firmware's opt-level
/// "s" a `u16` fill stays one store per pixel, and spans are most of a
/// frame's writes.
fn fill(pixels: &mut [u16], color: u16) {
    // SAFETY: every bit pattern is a valid u32 and u16, and `align_to_mut`
    // keeps the middle slice inside `pixels` with u32 alignment.
    let (head, pairs, tail) = unsafe { pixels.align_to_mut::<u32>() };
    head.fill(color);
    let pair = u32::from(color) * 0x0001_0001;
    // Four stores a turn, so the loop's own count and branch cost less per pixel.
    let (quads, pairs) = pairs.as_chunks_mut::<4>();
    for [a, b, c, d] in quads {
        (*a, *b, *c, *d) = (pair, pair, pair, pair);
    }
    pairs.fill(pair);
    tail.fill(color);
}

#[cfg(test)]
mod tests {
    use super::{Look, Row};
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
    fn spans_fill_every_pixel_at_any_alignment() {
        for start in 0..4 {
            for end in start..24 {
                let mut pixels = [0u16; 24];
                Row::new(0, &mut pixels).span(start..end, 0xabcd);
                for (x, &pixel) in pixels.iter().enumerate() {
                    let inside = (start..end).contains(&x);
                    assert_eq!(
                        pixel,
                        if inside { 0xabcd } else { 0 },
                        "{start}..{end} at {x}"
                    );
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

    fn plus_one(color: u16) -> u16 {
        color + 1
    }

    fn paint_every_write(pixels: &mut [u16; 14]) {
        let mut source = [0u16; 12];
        let mut beside = Row::new(0, &mut source);
        beside.looked(Look::new(plus_one)).span(11..12, 89);
        let mut row = Row::new(0, pixels);
        let mut row = row.looked(Look::new(plus_one));
        row.plain().span(13..14, 120);
        let ink = row.ink(100);
        let blended = &mut row.pixels_mut(11..14)[..2];
        blended.copy_from_slice(&[ink, ink - 1]);
        row.wrote(11..14, |x| [15, 7, 0][x - 11]);
        row.span(0..2, 10);
        row.set(2, 20);
        row.bits(3, 0b1, 30);
        row.within(4..5).fill(40);
        row.at(7).set(5, 50);
        row.each(6..7, |_| Some(60));
        row.each_over(7..8, |_, under| Some(under + 1));
        row.plain().span(8..9, 70);
        row.computed().span(9..10, 80);
        row.copy(10..11, &beside, 1);
    }

    #[test]
    fn a_look_maps_palette_colours_once_and_leaves_worked_out_ones() {
        let mut pixels = [0u16; 14];
        paint_every_write(&mut pixels);
        assert_eq!(
            pixels,
            [11, 11, 21, 31, 41, 51, 60, 1, 70, 80, 90, 101, 100, 120]
        );
        let mut pixels = [0u16; 24];
        Row::new(1, &mut pixels)
            .looked(Look::new(plus_one))
            .text(0, 0, 1, b"WM", 50);
        let lit: std::vec::Vec<_> = pixels.into_iter().filter(|&p| p != 0).collect();
        assert!(lit.len() > 2 && lit.iter().all(|&p| p == 51), "{lit:?}");
    }

    #[test]
    fn rows_record_how_each_pixel_was_last_written() {
        use crate::rendering::record::{Kind::*, of, start};
        start();
        let mut pixels = [0u16; 14];
        let mut row = Row::new(0, &mut pixels);
        row.span(0..1, 1);
        row.each(1..2, |_| None);
        assert_eq!(of(&pixels).unwrap()[..3], [Fixed, Unwritten, Unwritten]);
        paint_every_write(&mut pixels);
        let mapped = [Mapped; 6];
        let worked = [Computed, Computed, Fixed, Computed, Mapped];
        let glyph = [Mapped, Computed, Fixed];
        assert_eq!(
            of(&pixels).unwrap(),
            [&mapped[..], &worked[..], &glyph[..]].concat()
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "never opened")]
    fn a_row_over_a_forgotten_record_is_caught() {
        use crate::rendering::record::{open, start};
        start();
        let mut pixels = [0u16; 4];
        let mut row = Row::new(0, &mut pixels);
        let others = std::vec![[0u16; 4]; 64];
        others.iter().for_each(|other| open(other));
        row.span(0..1, 1);
    }
}
