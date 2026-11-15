//! Synchronous display submission and geometry.
use crate::observation::{Availability, Error};
use core::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgb565,
}

/// Rows a submission sends come in bands of this many.
pub const BAND_ROWS: usize = 4;
/// The most bands `Rows` holds.
pub const MAX_BANDS: usize = 128;

/// A set of whole bands of `BAND_ROWS` rows, the first at the top of the
/// panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rows(u128);

impl Rows {
    pub const NONE: Self = Self(0);

    /// Every row of a panel `height` rows tall.
    pub const fn all(height: usize) -> Self {
        let bands = height.div_ceil(BAND_ROWS);
        if bands >= MAX_BANDS {
            Self(u128::MAX)
        } else {
            Self((1 << bands) - 1)
        }
    }

    /// Band `index`, rows `index * BAND_ROWS` on.
    pub const fn band(index: usize) -> Self {
        if index < MAX_BANDS {
            Self(1 << index)
        } else {
            Self::NONE
        }
    }

    /// Every band that `rows` touches.
    pub const fn covering(rows: Range<usize>) -> Self {
        if rows.start >= rows.end {
            return Self::NONE;
        }
        let first = rows.start / BAND_ROWS;
        let last = (rows.end - 1) / BAND_ROWS;
        Self(Self::all((last + 1) * BAND_ROWS).0 & !Self::all(first * BAND_ROWS).0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, y: usize) -> bool {
        y / BAND_ROWS < MAX_BANDS && self.0 >> (y / BAND_ROWS) & 1 == 1
    }

    /// The runs of consecutive rows, top to bottom, cut at `height`.
    pub fn runs(self, height: usize) -> impl Iterator<Item = Range<usize>> {
        let mut bands = self.0 & Self::all(height).0;
        core::iter::from_fn(move || {
            if bands == 0 {
                return None;
            }
            let first = bands.trailing_zeros() as usize;
            let after = first + (bands >> first).trailing_ones() as usize;
            bands &= !Self::all(after * BAND_ROWS).0;
            Some(first * BAND_ROWS..(after * BAND_ROWS).min(height))
        })
    }

    /// How many rows of a panel `height` rows tall the set covers.
    pub fn count(self, height: usize) -> usize {
        self.runs(height).map(|run| run.len()).sum()
    }
}

impl core::ops::BitOr for Rows {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOrAssign for Rows {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Submission is synchronous. Return, including error, means all DMA access has
/// ended and the device owns its buffers again. No cancellable future is exposed.
pub trait Display {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn geometry(&self) -> Geometry;
    fn format(&self) -> Format {
        Format::Rgb565
    }
    /// One frame: calls `row(y, pixels)` for each row of `rows` on the panel,
    /// top to bottom, with `geometry().width` pixels to fill completely;
    /// their prior contents are unspecified. Rows are requested in order so
    /// a device can transfer earlier rows while later ones render. The other
    /// rows keep what the panel showed.
    fn submit(&mut self, rows: Rows, row: impl FnMut(usize, &mut [u16])) -> Result<(), Error>;
    /// Called with the frame the shell believes the panel shows, every row
    /// in turn. A display that keeps its pixels, such as a simulator, can
    /// check that they match; a panel has nothing to compare.
    fn check(&mut self, _row: impl FnMut(usize, &mut [u16])) {}
}

#[cfg(test)]
mod tests {
    use super::{BAND_ROWS, Rows};
    use std::vec::Vec;

    fn runs(rows: Rows, height: usize) -> Vec<(usize, usize)> {
        rows.runs(height).map(|run| (run.start, run.end)).collect()
    }

    #[test]
    fn covering_a_range_takes_every_band_it_touches() {
        assert_eq!(runs(Rows::covering(5..9), 320), [(4, 12)]);
        assert_eq!(runs(Rows::covering(4..8), 320), [(4, 8)]);
        assert_eq!(runs(Rows::covering(0..1), 320), [(0, 4)]);
        assert_eq!(runs(Rows::covering(3..3), 320), []);
        assert_eq!(runs(Rows::covering(316..400), 320), [(316, 320)]);
        assert!(Rows::covering(7..9).contains(11) && !Rows::covering(7..9).contains(12));
    }

    #[test]
    fn runs_join_neighbouring_bands_and_stop_at_the_panel() {
        let rows = Rows::covering(0..4) | Rows::band(1) | Rows::band(3) | Rows::all(512);
        assert_eq!(runs(rows, 23), [(0, 23)]);
        let rows = Rows::band(0) | Rows::band(1) | Rows::band(3) | Rows::band(79);
        assert_eq!(runs(rows, 320), [(0, 8), (12, 16), (316, 320)]);
        assert_eq!(rows.count(320), 16);
        assert_eq!(runs(Rows::all(320), 320), [(0, 320)]);
        assert_eq!(Rows::all(23).count(23), 23);
        assert_eq!(runs(Rows::all(128 * BAND_ROWS), 1000), [(0, 512)]);
    }
}
