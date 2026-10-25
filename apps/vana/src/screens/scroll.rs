//! Lists longer than the screen: a column of equal cards seen through a
//! window of slots that keeps the focused card in view.
//!
//! MOVE steps the focus on (held, a page on) and wraps past the last card to
//! the first. A vertical swipe pages the focus on or back without wrapping.
//! The window follows from the focus alone, with the focus on its last slot
//! once the list has scrolled, so a list keeps no scroll state of its own.
use super::chrome::Rect;

/// A column of equal cards: where each slot sits and which one a touch hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub left: usize,
    pub right: usize,
    /// The first card's top edge.
    pub top: usize,
    pub height: usize,
    /// From one card's top to the next one's.
    pub pitch: usize,
}

impl Column {
    /// The bounds of the card in `slot`.
    pub const fn rect(self, slot: usize) -> Rect {
        let top = self.top + slot * self.pitch;
        Rect {
            left: self.left,
            top,
            right: self.right,
            bottom: top + self.height,
        }
    }

    /// The slot of the first `slots` under a touch at (`x`, `y`); the gaps
    /// between cards belong to none.
    pub fn slot_at(self, slots: usize, x: usize, y: usize) -> Option<usize> {
        (0..slots).find(|&slot| self.rect(slot).contains(x, y))
    }

    /// The bottom edge of `slots` cards.
    pub const fn bottom(self, slots: usize) -> usize {
        self.top + slots.saturating_sub(1) * self.pitch + self.height
    }
}

/// `count` rows seen through `shown` slots, keeping `cursor` in view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scroll {
    pub count: usize,
    pub cursor: usize,
    pub shown: usize,
}

impl Scroll {
    /// A list of `count` rows focused on `cursor`, clamped into the list.
    pub const fn new(count: usize, cursor: usize, shown: usize) -> Self {
        let last = count.saturating_sub(1);
        Self {
            count,
            cursor: if cursor < last { cursor } else { last },
            shown,
        }
    }

    /// The first row in view.
    pub const fn first(self) -> usize {
        let first = self.cursor.saturating_sub(self.shown.saturating_sub(1));
        let last_first = self.count.saturating_sub(self.shown);
        if first < last_first {
            first
        } else {
            last_first
        }
    }

    /// Rows in view, at most `shown`.
    pub fn in_view(self) -> usize {
        self.shown.min(self.count - self.first())
    }

    /// Each row in view as (slot, row).
    pub fn visible(self) -> impl Iterator<Item = (usize, usize)> {
        (self.first()..self.first() + self.in_view()).enumerate()
    }

    /// The row under a touch at (`x`, `y`) on `column`.
    pub fn row_at(self, column: Column, x: usize, y: usize) -> Option<usize> {
        column
            .slot_at(self.in_view(), x, y)
            .map(|slot| self.first() + slot)
    }

    /// The row MOVE focuses: the next one, or held the one a page on; past
    /// the last it wraps to the first.
    pub fn next(self, held: bool) -> usize {
        let step = if held { self.shown.max(1) } else { 1 };
        if self.cursor + 1 >= self.count {
            0
        } else {
            (self.cursor + step).min(self.count - 1)
        }
    }

    /// The row a page on, stopping at the last.
    pub fn page_on(self) -> usize {
        (self.cursor + self.shown.max(1)).min(self.count.saturating_sub(1))
    }

    /// The row a page back, stopping at the first.
    pub fn page_back(self) -> usize {
        self.cursor.saturating_sub(self.shown.max(1))
    }

    /// Where a scroll bar's thumb sits in a track of `top..bottom`: as long as
    /// the window is against the whole list, and none while everything shows.
    pub fn thumb(self, top: usize, bottom: usize) -> Option<(usize, usize)> {
        if self.count <= self.shown {
            return None;
        }
        let track = bottom - top;
        let start = top + track * self.first() / self.count;
        let end = top + track * (self.first() + self.in_view()) / self.count;
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLUMN: Column = Column {
        left: 10,
        right: 230,
        top: 50,
        height: 52,
        pitch: 60,
    };

    #[test]
    fn the_window_keeps_the_focus_on_its_last_slot_once_scrolled() {
        let first = |count, cursor| Scroll::new(count, cursor, 4).first();
        assert_eq!([0, 1, 2, 3].map(|cursor| first(12, cursor)), [0; 4]);
        assert_eq!(first(12, 4), 1);
        assert_eq!(first(12, 11), 8);
        assert_eq!(first(2, 1), 0, "a short list never scrolls");
        assert_eq!(first(0, 0), 0);
        assert_eq!(Scroll::new(12, 40, 4).cursor, 11, "clamped");
        assert_eq!(Scroll::new(0, 3, 4).cursor, 0);
    }

    #[test]
    fn only_rows_in_view_are_visited() {
        let rows = |count, cursor| {
            Scroll::new(count, cursor, 4)
                .visible()
                .collect::<std::vec::Vec<_>>()
        };
        assert_eq!(rows(12, 6), [(0, 3), (1, 4), (2, 5), (3, 6)]);
        assert_eq!(rows(2, 0), [(0, 0), (1, 1)]);
        assert!(rows(0, 0).is_empty());
    }

    #[test]
    fn taps_find_rows_in_view_and_miss_gaps_and_empty_slots() {
        let scroll = Scroll::new(12, 6, 4);
        assert_eq!(scroll.row_at(COLUMN, 120, 50 + 10), Some(3));
        assert_eq!(scroll.row_at(COLUMN, 120, 50 + 3 * 60 + 10), Some(6));
        assert_eq!(scroll.row_at(COLUMN, 120, 50 + 52 + 2), None, "gap");
        assert_eq!(scroll.row_at(COLUMN, 5, 50 + 10), None, "margin");
        let short = Scroll::new(2, 1, 4);
        assert_eq!(short.row_at(COLUMN, 120, 50 + 2 * 60 + 10), None);
        assert_eq!(COLUMN.bottom(4), 50 + 3 * 60 + 52);
    }

    #[test]
    fn move_steps_or_pages_on_and_wraps_while_swipes_stop_at_the_ends() {
        let at = |cursor| Scroll::new(10, cursor, 4);
        assert_eq!(at(0).next(false), 1);
        assert_eq!(at(0).next(true), 4);
        assert_eq!(at(8).next(true), 9, "a page stops on the last row");
        assert_eq!(at(9).next(false), 0, "wraps");
        assert_eq!(at(9).next(true), 0);
        assert_eq!(Scroll::new(0, 0, 4).next(true), 0);
        assert_eq!(at(3).page_on(), 7);
        assert_eq!(at(8).page_on(), 9);
        assert_eq!(at(9).page_on(), 9);
        assert_eq!(at(7).page_back(), 3);
        assert_eq!(at(2).page_back(), 0);
        assert_eq!(Scroll::new(0, 0, 4).page_on(), 0);
    }

    #[test]
    fn the_thumb_shows_the_window_against_the_list() {
        assert_eq!(Scroll::new(4, 3, 4).thumb(0, 100), None);
        assert_eq!(Scroll::new(5, 0, 4).thumb(0, 100), Some((0, 80)));
        assert_eq!(Scroll::new(5, 4, 4).thumb(0, 100), Some((20, 100)));
    }
}
