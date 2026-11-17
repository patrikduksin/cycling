//! Row painting: a screen fills one display row at a time with a few span
//! writes instead of answering a hit test for every pixel. Later paints cover
//! earlier ones, so paint backgrounds first. Coordinates stay those of the
//! whole row; writes outside its clip range are dropped.
//!
//! A row may carry a look, a map from palette colours to the colours shown.
//! Palette colours are bare `u16`s and shown ones are [`Shown`], so a colour
//! mapped twice or never fails to compile. The entry points (`fill`, `span`,
//! `set`, `bits`, `bits_scaled`, `text`) take palette colours and map them
//! once a call; `span_shown`, `each`, `each_over` and `pixels_mut` write
//! colours already shown.
use super::record::{self, Kind};
use super::text::glyph;
use super::work;
use core::ops::Range;

/// A colour as the display shows it: a palette colour through a look, or
/// one worked out from such colours. A palette colour does not pass for one:
///
/// ```compile_fail
/// # use firmware_shell::rendering::row::Row;
/// let mut pixels = [0u16; 4];
/// Row::new(0, &mut pixels).span_shown(0..1, 0xffff);
/// ```
///
/// nor one for a palette colour:
///
/// ```compile_fail
/// # use firmware_shell::rendering::row::Row;
/// let mut pixels = [0u16; 4];
/// let mut row = Row::new(0, &mut pixels);
/// let white = row.ink(0xffff);
/// row.span(0..1, white);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Shown(pub u16);

impl core::fmt::LowerHex for Shown {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::LowerHex::fmt(&self.0, f)
    }
}

/// A map from palette colours to the colours a row shows; `PLAIN` shows
/// them as they are.
#[derive(Clone, Copy, Default, Hash)]
pub struct Look(Option<fn(u16) -> u16>);

impl Look {
    pub const PLAIN: Self = Self(None);

    pub const fn new(map: fn(u16) -> u16) -> Self {
        Self(Some(map))
    }

    /// The colour shown for palette colour `color`.
    #[inline]
    pub fn ink(self, color: u16) -> Shown {
        Shown(match self.0 {
            Some(map) => map(color),
            None => color,
        })
    }
}

fn shown_mut(pixels: &mut [u16]) -> &mut [Shown] {
    // SAFETY: `Shown` is `repr(transparent)` over `u16`, so the two slices
    // share layout, and every `u16` is a valid `Shown`.
    unsafe { &mut *(core::ptr::from_mut(pixels) as *mut [Shown]) }
}

pub struct Row<'a> {
    y: usize,
    /// Columns before this one are outside the clip; the slice ends at the other edge.
    start: usize,
    /// The column `pixels[0]` holds: paints placed at `x` land at `x - origin`.
    origin: usize,
    pixels: &'a mut [u16],
    look: Look,
}

impl<'a> Row<'a> {
    pub fn new(y: usize, pixels: &'a mut [u16]) -> Self {
        record::open(pixels);
        Self {
            y,
            start: 0,
            origin: 0,
            pixels,
            look: Look::PLAIN,
        }
    }
    pub fn y(&self) -> usize {
        self.y
    }
    /// Columns this row may write.
    pub fn clip(&self) -> Range<usize> {
        self.start..self.pixels.len() + self.origin
    }
    pub fn shown(&self, x: usize) -> Option<Shown> {
        let i = x.checked_sub(self.origin)?;
        self.pixels.get(i).map(|&pixel| Shown(pixel))
    }
    fn indices(&self, x: Range<usize>) -> Range<usize> {
        let start = x.start.max(self.start) - self.origin;
        let end = x.end.saturating_sub(self.origin).min(self.pixels.len());
        start.min(end)..end
    }
    /// Paint the columns of `x` inside `clip()`, for paints that read what
    /// lies beneath, such as blended glyph runs. `paint` gets them from
    /// `x.start.max(clip().start)` on, and takes its palette colours through
    /// `ink`. `coverage(x)` is how much of the paint's colour column `x` took: 15
    /// for exactly that colour, less for a blend over what lay beneath, and
    /// 0 for none, which keeps the pixel's record.
    pub fn pixels_mut(
        &mut self,
        x: Range<usize>,
        coverage: impl Fn(usize) -> u8,
        paint: impl FnOnce(&mut [Shown]),
    ) {
        let x = self.indices(x);
        paint(shown_mut(&mut self.pixels[x.clone()]));
        let (ink, origin) = (self.palette_kind(), self.origin);
        record::mark_each(self.pixels.as_ptr(), x, |i| match coverage(i + origin) {
            0 => None,
            15.. => Some(ink),
            _ => Some(Kind::Computed),
        });
    }
    /// `pixels_mut` for a paint that works out every column of `x` from
    /// shown colours, such as a map raster from its colour table.
    pub fn worked_out(&mut self, x: Range<usize>, paint: impl FnOnce(&mut [Shown])) {
        let x = self.indices(x);
        paint(shown_mut(&mut self.pixels[x.clone()]));
        record::mark(self.pixels.as_ptr(), x, Kind::Computed);
    }
    /// The row's look, for paints that work out colours from palette ones.
    pub fn look(&self) -> Look {
        self.look
    }
    /// The colour this row shows for palette colour `color`.
    #[inline]
    pub fn ink(&self, color: u16) -> Shown {
        self.look.ink(color)
    }
    fn reborrow(&mut self, y: usize, start: usize, end: usize, origin: usize) -> Row<'_> {
        Row {
            y,
            start,
            origin,
            pixels: &mut self.pixels[..end],
            look: self.look,
        }
    }
    /// The same row clipped further to `x`, for regions that own their pixels.
    pub fn within(&mut self, x: Range<usize>) -> Row<'_> {
        let end = x.end.saturating_sub(self.origin).min(self.pixels.len());
        self.reborrow(self.y, self.start.max(x.start), end, self.origin)
    }
    /// The same pixels as row `y` of content that scrolls: paints placed for
    /// `y` land on this display row.
    pub fn at(&mut self, y: usize) -> Row<'_> {
        self.reborrow(y, self.start, self.pixels.len(), self.origin)
    }
    /// The same pixels for content placed `shift` columns right of where it
    /// shows: paints placed at `x` land at `x - shift`.
    pub fn shifted(&mut self, shift: usize) -> Row<'_> {
        let len = self.pixels.len();
        self.reborrow(self.y, self.start + shift, len, self.origin + shift)
    }
    /// The same row, showing palette colours through `look`.
    pub fn looked(&mut self, look: Look) -> Row<'_> {
        let mut row = self.at(self.y);
        row.look = look;
        row
    }
    /// The same row without a look, for colours that read the same in every
    /// look.
    pub fn plain(&mut self) -> Row<'_> {
        self.looked(Look::PLAIN)
    }
    fn palette_kind(&self) -> Kind {
        match self.look.0 {
            Some(_) => Kind::Mapped,
            None => Kind::Fixed,
        }
    }
    pub fn fill(&mut self, color: u16) {
        self.span(0..usize::MAX, color);
    }
    pub fn span(&mut self, x: Range<usize>, color: u16) {
        self.write_span(x, self.ink(color), self.palette_kind());
    }
    /// `span` for a colour already shown, so a paint that also blends the
    /// colour maps it once.
    pub fn span_shown(&mut self, x: Range<usize>, color: Shown) {
        let kind = match self.look.0 {
            Some(_) => Kind::Computed,
            None => Kind::Fixed,
        };
        self.write_span(x, color, kind);
    }
    fn write_span(&mut self, x: Range<usize>, color: Shown, kind: Kind) {
        let x = self.indices(x);
        if !x.is_empty() {
            work::add(|work| {
                work.spans += 1;
                work.span_pixels += x.len() as u64;
            });
            fill(&mut self.pixels[x.clone()], color.0);
            record::mark(self.pixels.as_ptr(), x, kind);
        }
    }
    pub fn set(&mut self, x: usize, color: u16) {
        let color = self.ink(color);
        if x >= self.start
            && let Some(pixel) = self.pixels.get_mut(x - self.origin)
        {
            *pixel = color.0;
            let i = x - self.origin;
            record::mark(self.pixels.as_ptr(), i..i + 1, self.palette_kind());
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
    fn bits_shown(&mut self, left: usize, mut bits: u32, scale: usize, color: Shown) {
        let kind = self.palette_kind();
        while bits != 0 {
            let x = left + bits.trailing_zeros() as usize * scale;
            self.write_span(x..x + scale, color, kind);
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
        let end = self.clip().end;
        for (i, &character) in label.iter().enumerate() {
            let start = left + i * advance;
            if start >= end {
                break;
            }
            // Glyph rows put the leftmost of five dots in bit four.
            let bits = glyph(character)[line].reverse_bits() >> 3;
            self.bits_shown(start, u32::from(bits), scale, color);
        }
    }
    /// Per-pixel fallback for shapes without a cheaper row form; `None` keeps
    /// the pixel.
    pub fn each(&mut self, x: Range<usize>, mut pixel: impl FnMut(usize) -> Option<Shown>) {
        self.each_over(x, |x, _| pixel(x));
    }
    /// `each`, given what the row already holds at each pixel, for edges
    /// that blend over it.
    pub fn each_over(
        &mut self,
        x: Range<usize>,
        mut pixel: impl FnMut(usize, Shown) -> Option<Shown>,
    ) {
        let x = self.indices(x);
        work::add(|work| work.evaluations += x.len() as u64);
        let (buffer, origin) = (self.pixels.as_ptr(), self.origin);
        for (i, out) in shown_mut(&mut self.pixels[x.clone()])
            .iter_mut()
            .enumerate()
        {
            if let Some(color) = pixel(x.start + i + origin, *out) {
                *out = color;
                record::mark(buffer, x.start + i..x.start + i + 1, Kind::Computed);
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
    use super::{Look, Row, Shown};
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
        row.each(7..30, |_| Some(Shown(5)));
        row.each(30..40, |_| Some(Shown(9)));
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
        let mut row = Row::new(0, pixels);
        let mut row = row.looked(Look::new(plus_one));
        row.plain().span(13..14, 120);
        let ink = row.ink(100);
        let coverage = |x: usize| [15, 7, 0][x - 11];
        row.pixels_mut(11..14, coverage, |blended| {
            blended[..2].copy_from_slice(&[ink, Shown(ink.0 - 1)]);
        });
        row.span(0..2, 10);
        row.set(2, 20);
        row.bits(3, 0b1, 30);
        row.within(4..5).fill(40);
        row.at(7).set(5, 50);
        row.each(6..7, |_| Some(Shown(60)));
        row.each_over(7..8, |_, under| Some(Shown(under.0 + 1)));
        row.plain().span(8..9, 70);
        row.span_shown(9..10, Shown(80));
        row.shifted(3).set(13, 90);
    }

    #[test]
    fn a_look_maps_palette_colours_once_and_leaves_worked_out_ones() {
        let mut pixels = [0u16; 14];
        paint_every_write(&mut pixels);
        assert_eq!(
            pixels,
            [11, 11, 21, 31, 41, 51, 60, 1, 70, 80, 91, 101, 100, 120]
        );
        let mut pixels = [0u16; 24];
        Row::new(1, &mut pixels)
            .looked(Look::new(plus_one))
            .text(0, 0, 1, b"WM", 50);
        let lit: std::vec::Vec<_> = pixels.into_iter().filter(|&p| p != 0).collect();
        assert!(lit.len() > 2 && lit.iter().all(|&p| p == 51), "{lit:?}");
    }

    #[test]
    fn a_shifted_row_lands_paints_left_and_keeps_its_clip() {
        let mut pixels = [0u16; 10];
        let mut row = Row::new(0, &mut pixels);
        let mut body = row.within(2..8);
        let mut shifted = body.shifted(3);
        assert_eq!(shifted.clip(), 5..11);
        shifted.span(0..20, 1);
        shifted.each_over(9..10, |x, under| Some(Shown(under.0 + x as u16)));
        shifted.within(6..8).set(7, 2);
        assert_eq!(shifted.shown(9), Some(Shown(10)));
        assert_eq!(pixels, [0, 0, 1, 1, 2, 1, 10, 1, 0, 0]);
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
