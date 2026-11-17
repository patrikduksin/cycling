//! How each pixel was last written, so a host check can tell a palette
//! colour mapped through a row's look from one written as given.
//! Recording needs the `paint-record` feature, or the shell's own tests, and a
//! `start` on the painting thread; without them nothing is kept and `of`
//! answers `None`.
//!
//! A record belongs to the pixel buffer a `Row::new` was given, found by its
//! address, so reborrowed rows share it without carrying anything.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// No row wrote the pixel.
    #[default]
    Unwritten,
    /// An entry point wrote a palette colour through the row's look.
    Mapped,
    /// Written on a row without a look, such as `Row::plain`, by an entry
    /// point or `span_shown`: a colour that reads the same in every look.
    Fixed,
    /// A colour worked out per pixel or from colours already shown: `each`,
    /// `each_over`, `worked_out`, the blended pixels a `pixels_mut` paint
    /// reports, and under a look `span_shown`.
    Computed,
}

#[cfg(any(test, feature = "paint-record"))]
mod recording {
    use super::Kind;
    use core::cell::{Cell, RefCell};
    use core::ops::Range;
    use std::vec::Vec;

    const KEPT: usize = 64;

    std::thread_local! {
        static STARTED: Cell<bool> = const { Cell::new(false) };
        static RECORDS: RefCell<Vec<(usize, Vec<Kind>)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn start() {
        STARTED.set(true);
    }

    pub fn open(pixels: &[u16]) {
        if !STARTED.get() {
            return;
        }
        RECORDS.with_borrow_mut(|records| {
            let key = pixels.as_ptr() as usize;
            let at = match records.iter().position(|(kept, _)| *kept == key) {
                Some(at) => at,
                None => {
                    if records.len() == KEPT {
                        records.remove(0);
                    }
                    records.push((key, Vec::new()));
                    records.len() - 1
                }
            };
            let kinds = &mut records[at].1;
            kinds.clear();
            kinds.resize(pixels.len(), Kind::Unwritten);
        });
    }

    fn with(buffer: *const u16, change: impl FnOnce(&mut [Kind])) -> bool {
        if !STARTED.get() {
            return true;
        }
        RECORDS.with_borrow_mut(|records| {
            let key = buffer as usize;
            let found = records.iter_mut().rev().find(|(kept, _)| *kept == key);
            found.map(|(_, kinds)| change(kinds)).is_some()
        })
    }

    fn write(buffer: *const u16, change: impl FnOnce(&mut [Kind])) {
        let opened = with(buffer, change);
        debug_assert!(opened, "a row wrote a buffer that was never opened");
    }

    pub fn mark(buffer: *const u16, x: Range<usize>, kind: Kind) {
        write(buffer, |kinds| kinds[x].fill(kind));
    }

    pub fn mark_each(buffer: *const u16, x: Range<usize>, kind: impl Fn(usize) -> Option<Kind>) {
        write(buffer, |kinds| {
            for x in x {
                if let Some(kind) = kind(x) {
                    kinds[x] = kind;
                }
            }
        });
    }

    pub fn of(pixels: &[u16]) -> Option<Vec<Kind>> {
        let mut found = None;
        with(pixels.as_ptr(), |kinds| {
            found = Some(kinds[..pixels.len()].to_vec())
        });
        found
    }
}

/// Record the rows this thread paints from now on.
#[inline(always)]
pub fn start() {
    #[cfg(any(test, feature = "paint-record"))]
    recording::start();
}

/// Start a fresh record of `pixels`, every pixel unwritten. `Row::new` opens
/// its buffer; a display that hands a buffer to a painter opens it first, so
/// a row painted without `Row::new` shows no older record.
#[inline(always)]
pub fn open(_pixels: &[u16]) {
    #[cfg(any(test, feature = "paint-record"))]
    recording::open(_pixels);
}

#[inline(always)]
pub(crate) fn mark(_buffer: *const u16, _x: core::ops::Range<usize>, _kind: Kind) {
    #[cfg(any(test, feature = "paint-record"))]
    recording::mark(_buffer, _x, _kind);
}

#[inline(always)]
pub(crate) fn mark_each(
    _buffer: *const u16,
    _x: core::ops::Range<usize>,
    _kind: impl Fn(usize) -> Option<Kind>,
) {
    #[cfg(any(test, feature = "paint-record"))]
    recording::mark_each(_buffer, _x, _kind);
}

/// How each pixel of a buffer rows were painted into was last written, or
/// `None` without the `paint-record` feature, a `start` or a row over it.
#[cfg(any(test, feature = "std"))]
pub fn of(_pixels: &[u16]) -> Option<std::vec::Vec<Kind>> {
    #[cfg(any(test, feature = "paint-record"))]
    return recording::of(_pixels);
    #[cfg(not(any(test, feature = "paint-record")))]
    None
}
