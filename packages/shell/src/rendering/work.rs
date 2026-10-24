//! Painter work counted for the host render benchmark (`sim-bench`, #167).
//! Unlike host time, the counts do not depend on the CPU. Counting needs the
//! `work-counters` feature and is off until `count` turns it on; without the
//! feature `add` compiles to nothing.

/// Work done by the painters while counting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Solid runs filled with one colour.
    pub spans: u64,
    /// Pixels those runs covered.
    pub span_pixels: u64,
    /// Pixels a shape worked out one at a time: coverage evaluations at
    /// anti-aliased edges and the per-pixel fallback.
    pub evaluations: u64,
    /// Label rows painted: a label crossing a display row.
    pub label_rows: u64,
    /// Columns those label rows walked to find their glyph pixels.
    pub glyph_columns: u64,
    /// Glyph pixels with ink: blended over what lay beneath.
    pub glyph_pixels: u64,
    /// Colour mixes worked out for partly covered glyph pixels.
    pub mixes: u64,
}

#[cfg(feature = "work-counters")]
mod counting {
    use super::Work;
    use core::cell::Cell;

    std::thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static WORK: Cell<Option<Work>> = const { Cell::new(None) };
    }

    pub fn add(change: impl FnOnce(&mut Work)) {
        if !COUNTING.with(Cell::get) {
            return;
        }
        WORK.with(|work| {
            if let Some(mut counted) = work.get() {
                change(&mut counted);
                work.set(Some(counted));
            }
        });
    }

    pub fn count(paint: impl FnOnce()) -> Work {
        WORK.with(|work| work.set(Some(Work::default())));
        COUNTING.with(|counting| counting.set(true));
        paint();
        COUNTING.with(|counting| counting.set(false));
        WORK.with(|work| work.take()).unwrap_or_default()
    }
}

/// Count the work `paint` does on this thread.
#[cfg(feature = "work-counters")]
pub fn count(paint: impl FnOnce()) -> Work {
    counting::count(paint)
}

#[inline(always)]
pub(crate) fn add(_change: impl FnOnce(&mut Work)) {
    #[cfg(feature = "work-counters")]
    counting::add(_change);
}

#[cfg(all(test, feature = "work-counters"))]
mod tests {
    use super::count;
    use crate::rendering::row::Row;

    #[test]
    fn counting_sees_spans_and_evaluations_only_while_on() {
        let mut pixels = [0u16; 16];
        Row::new(0, &mut pixels).span(2..6, 1);
        let work = count(|| {
            let mut row = Row::new(0, &mut pixels);
            row.span(2..6, 1);
            row.span(20..30, 1);
            row.each(0..3, |_| None);
        });
        assert_eq!((work.spans, work.span_pixels, work.evaluations), (1, 4, 3));
        assert_eq!(count(|| {}).spans, 0);
    }
}
