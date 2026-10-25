//! Card lists that scroll under the finger: where each card sits on the
//! page, which rows of the page the screen shows, and which card a touch or
//! a focus change means.
//!
//! A [`Layout`] is a page of equal cards seen through the viewport, the
//! screen rows `top..bottom`, scrolled by an offset that `Kinetic` drives:
//! page row `p` shows on screen row `top + p - offset`. Screens prepare the
//! page's runs and shapes at canvas row `PAGE + p`, clear of everything that
//! stays put (a title, a status line, a pinned band), which keeps its screen
//! rows; `canvas_row` says which canvas row each screen row paints.
//!
//! Focus is the buttons' first responder and scrolling never moves it, so the
//! focused card may scroll out of view. A button press then first brings it
//! back: MOVE focuses the first whole card in view, ACT scrolls the focused
//! card into view (`MOVE` and `ACT` in [`Layout::press`]).
use super::chrome::Rect;
use core::ops::Range;

/// Canvas row of page row 0: below every screen row, with room above it for
/// the rubber band's stretch past the top.
pub const PAGE: usize = 1024;

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
        let slot = y.checked_sub(self.top)? / self.pitch;
        (slot < slots && self.rect(slot).contains(x, y)).then_some(slot)
    }

    /// The bottom edge of `slots` cards.
    pub const fn bottom(self, slots: usize) -> usize {
        self.top + slots.saturating_sub(1) * self.pitch + self.height
    }
}

/// A band pinned at the viewport's top once the page has scrolled past
/// `after`, as iOS folds a large title into its navigation bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub after: i32,
    pub height: usize,
}

/// What a button press does to a list's focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Focus this card; scroll to `offset` if it is some.
    Focus(usize, Option<i32>),
    /// Act on the focused card: it shows whole.
    Act,
    /// Scroll to bring the focused card into view; nothing else.
    Reveal(i32),
}

/// A page of cards seen through the screen rows `top..bottom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub top: usize,
    pub bottom: usize,
    /// The cards in page rows: 0 is the page's top.
    pub column: Column,
    /// Room under the last card with the page scrolled to its end.
    pub end: usize,
    /// A card brought into view keeps this far from the viewport's edges.
    pub margin: usize,
    pub pin: Option<Pin>,
}

impl Layout {
    pub const fn viewport(&self) -> usize {
        self.bottom - self.top
    }

    /// The page's height with `count` cards.
    pub const fn content(&self, count: usize) -> usize {
        if count == 0 {
            return self.viewport();
        }
        self.column.bottom(count) + self.end
    }

    /// Card `index`'s top and bottom in page rows.
    pub const fn span(&self, index: usize) -> (i32, i32) {
        let rect = self.column.rect(index);
        (rect.top as i32, rect.bottom as i32)
    }

    /// Card `index` on the canvas.
    pub const fn rect(&self, index: usize) -> Rect {
        let rect = self.column.rect(index);
        Rect {
            top: PAGE + rect.top,
            bottom: PAGE + rect.bottom,
            ..rect
        }
    }

    /// Whether the band pins with the page scrolled to `offset`.
    pub fn pinned(&self, offset: i32) -> bool {
        self.pin.is_some_and(|pin| offset > pin.after)
    }

    /// Viewport rows the pinned band covers at `offset`.
    fn inset(&self, offset: i32) -> i32 {
        match self.pin {
            Some(pin) if self.pinned(offset) => pin.height as i32,
            _ => 0,
        }
    }

    /// Whether card `index` shows whole at `offset`.
    pub fn whole(&self, index: usize, offset: i32) -> bool {
        let (top, bottom) = self.span(index);
        top >= offset + self.inset(offset) && bottom <= offset + self.viewport() as i32
    }

    /// The offset that shows card `index` whole, moving as little as it can
    /// from `offset`. The first card shows the page from its top.
    pub fn reveal(&self, index: usize, offset: i32) -> i32 {
        if index == 0 {
            return 0;
        }
        let (top, bottom) = self.span(index);
        let margin = self.margin as i32;
        let band = self.pin.map_or(0, |pin| pin.height as i32);
        if top - margin - self.inset(offset) < offset {
            top - margin - band
        } else if bottom + margin > offset + self.viewport() as i32 {
            bottom + margin - self.viewport() as i32
        } else {
            offset
        }
    }

    /// The cards any of whose rows show at `offset`.
    pub fn visible(&self, count: usize, offset: i32) -> Range<usize> {
        let pitch = self.column.pitch as i32;
        let (first_top, first_bottom) = self.span(0);
        let bottom = offset + self.viewport() as i32;
        // Card i shows while its bottom is below the top and its top above
        // the bottom.
        let first = ((offset - first_bottom).div_euclid(pitch) + 1).max(0) as usize;
        let end = ((bottom - first_top - 1).div_euclid(pitch) + 1).clamp(0, count as i32) as usize;
        first.min(end)..end
    }

    /// The card under a touch at screen (`x`, `y`) at `offset`.
    pub fn card_at(&self, count: usize, offset: i32, x: usize, y: usize) -> Option<usize> {
        if y < self.top + self.inset(offset) as usize || y >= self.bottom {
            return None;
        }
        let row = usize::try_from(y as i32 - self.top as i32 + offset).ok()?;
        self.column.slot_at(count, x, row)
    }

    /// Whether screen row `y` lies on the page rather than the fixed parts.
    pub fn on_page(&self, y: usize) -> bool {
        (self.top..self.bottom).contains(&y)
    }

    /// The canvas row screen row `y` shows at `offset`: its own row outside
    /// the viewport and under the pinned band, else the page's.
    pub fn canvas_row(&self, y: usize, offset: i32) -> Option<usize> {
        if !self.on_page(y) || y < self.top + self.inset(offset) as usize {
            return Some(y);
        }
        usize::try_from(PAGE as i32 + y as i32 - self.top as i32 + offset).ok()
    }

    /// The canvas row that shows on screen row `y` of the viewport at
    /// `offset`: for marks that stay put over the page, such as a scroll bar.
    pub fn held(&self, y: usize, offset: i32) -> usize {
        (PAGE as i32 + y as i32 - self.top as i32 + offset).max(0) as usize
    }

    /// Where a scroll bar's thumb sits in the viewport's rows at `offset`,
    /// none while the whole page shows.
    pub fn thumb(&self, count: usize, offset: i32) -> Option<(usize, usize)> {
        let content = self.content(count) as i32;
        let viewport = self.viewport() as i32;
        if content <= viewport {
            return None;
        }
        let at =
            |page: i32| (self.top as i32 + page.clamp(0, content) * viewport / content) as usize;
        Some((at(offset), at(offset + viewport).max(at(offset) + 4)))
    }

    /// The first card that shows whole at `offset`.
    pub fn first_whole(&self, count: usize, offset: i32) -> Option<usize> {
        self.visible(count, offset)
            .find(|&index| self.whole(index, offset))
    }

    /// What MOVE does with the focus on `cursor` and the page resting at
    /// `rest`: a focus out of view comes back to the first whole card;
    /// otherwise the next card (held, `page` on; past the last, the first)
    /// is focused and scrolled into view.
    pub fn next(&self, count: usize, cursor: usize, rest: i32, page: usize, held: bool) -> Press {
        if count == 0 {
            return Press::Focus(0, None);
        }
        if !self.whole(cursor, rest)
            && let Some(first) = self.first_whole(count, rest)
        {
            return Press::Focus(first, None);
        }
        let step = if held { page.max(1) } else { 1 };
        let next = if cursor + 1 >= count {
            0
        } else {
            (cursor + step).min(count - 1)
        };
        Press::Focus(next, Some(self.reveal(next, rest)))
    }

    /// What ACT does: act on a focused card that shows whole, else scroll
    /// it into view first.
    pub fn act(&self, cursor: usize, rest: i32) -> Press {
        if self.whole(cursor, rest) {
            Press::Act
        } else {
            Press::Reveal(self.reveal(cursor, rest))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twelve-card pages: cards 52 tall, 60 apart, after a 100-row head,
    /// seen through 296 rows, with a 24-row band pinned past 20.
    const LAYOUT: Layout = Layout {
        top: 24,
        bottom: 320,
        column: Column {
            left: 10,
            right: 230,
            top: 100,
            height: 52,
            pitch: 60,
        },
        end: 10,
        margin: 6,
        pin: Some(Pin {
            after: 20,
            height: 24,
        }),
    };

    #[test]
    fn the_page_is_its_cards_and_a_margin_and_shows_what_reaches_the_viewport() {
        assert_eq!(LAYOUT.content(12), 100 + 11 * 60 + 52 + 10);
        assert_eq!(LAYOUT.content(0), 296);
        assert_eq!(LAYOUT.visible(12, 0), 0..4, "the fourth reaches row 280");
        assert_eq!(LAYOUT.visible(12, 151), 0..6);
        assert_eq!(LAYOUT.visible(12, 152), 1..6, "card 0 ends at row 152");
        assert_eq!(LAYOUT.visible(12, -50), 0..3);
        assert_eq!(LAYOUT.visible(2, 0), 0..2);
        assert_eq!(LAYOUT.visible(0, 0), 0..0);
        assert_eq!(LAYOUT.visible(12, 5000), 12..12);
    }

    #[test]
    fn screen_rows_paint_the_page_shifted_the_fixed_rows_and_the_band_in_place() {
        assert_eq!(LAYOUT.canvas_row(10, 300), Some(10), "the status bar");
        assert_eq!(LAYOUT.canvas_row(24, 0), Some(PAGE));
        assert_eq!(LAYOUT.canvas_row(100, 0), Some(PAGE + 76));
        assert_eq!(LAYOUT.canvas_row(100, -30), Some(PAGE + 46), "stretched");
        assert_eq!(LAYOUT.canvas_row(30, 300), Some(30), "the pinned band");
        assert_eq!(LAYOUT.canvas_row(48, 300), Some(PAGE + 324));
        assert_eq!(LAYOUT.canvas_row(30, 10), Some(PAGE + 16), "not pinned yet");
    }

    #[test]
    fn taps_find_cards_where_they_show_and_miss_gaps_bands_and_margins() {
        let card = |offset: i32, index: usize| {
            let (top, _) = LAYOUT.span(index);
            (top - offset + 24 + 10) as usize
        };
        assert_eq!(LAYOUT.card_at(12, 0, 120, card(0, 0)), Some(0));
        assert_eq!(LAYOUT.card_at(12, 300, 120, card(300, 6)), Some(6));
        assert_eq!(LAYOUT.card_at(12, 0, 120, card(0, 0) + 45), None, "gap");
        assert_eq!(LAYOUT.card_at(12, 0, 4, card(0, 0)), None, "margin");
        assert_eq!(LAYOUT.card_at(2, 0, 120, card(0, 2)), None, "no card");
        // At 320 card 4's top rows, from screen row 44, lie under the band.
        assert_eq!(LAYOUT.card_at(12, 320, 120, 46), None);
        assert_eq!(LAYOUT.card_at(12, 320, 120, 50), Some(4));
        assert_eq!(LAYOUT.card_at(12, 0, 120, 10), None, "the status bar");
    }

    #[test]
    fn reveals_move_as_little_as_they_can_and_clear_the_band() {
        assert_eq!(LAYOUT.reveal(1, 0), 0, "already whole");
        let low = LAYOUT.reveal(11, 0);
        assert_eq!(low, 100 + 11 * 60 + 52 + 6 - 296);
        assert!(LAYOUT.whole(11, low));
        assert_eq!(LAYOUT.reveal(10, low), low);
        let up = LAYOUT.reveal(4, low);
        assert_eq!(up, 100 + 4 * 60 - 6 - 24);
        assert!(LAYOUT.whole(4, up) && !LAYOUT.whole(3, up));
        assert_eq!(LAYOUT.reveal(0, low), 0, "the first card brings the head");
    }

    #[test]
    fn a_focus_out_of_view_comes_back_before_move_or_act_does_anything_else() {
        let low = LAYOUT.reveal(11, 0);
        let first = LAYOUT.first_whole(12, low).unwrap();
        // Scrolled away from the focus on card 0: MOVE brings it to the
        // first whole card, without scrolling; ACT scrolls back to it.
        assert_eq!(LAYOUT.next(12, 0, low, 4, false), Press::Focus(first, None));
        assert_eq!(LAYOUT.act(0, low), Press::Reveal(0));
        // In view, MOVE steps (held, pages) and scrolls the next card in.
        assert_eq!(LAYOUT.next(12, 1, 0, 4, false), Press::Focus(2, Some(0)));
        assert_eq!(
            LAYOUT.next(12, 1, 0, 4, true),
            Press::Focus(5, Some(LAYOUT.reveal(5, 0)))
        );
        assert_eq!(
            LAYOUT.next(12, 11, low, 4, true),
            Press::Focus(0, Some(0)),
            "wraps"
        );
        assert_eq!(
            LAYOUT.next(12, 10, low, 4, true),
            Press::Focus(11, Some(low))
        );
        assert_eq!(LAYOUT.act(2, 0), Press::Act);
        assert_eq!(LAYOUT.next(0, 0, 0, 4, false), Press::Focus(0, None));
    }

    #[test]
    fn the_thumb_shows_the_viewport_against_the_page() {
        let short = Layout {
            pin: None,
            ..LAYOUT
        };
        assert_eq!(short.thumb(2, 0), None);
        let (start, end) = LAYOUT.thumb(12, 0).unwrap();
        assert_eq!(start, 24);
        assert!(end > start && end < 320);
        let low = LAYOUT.content(12) as i32 - 296;
        assert_eq!(LAYOUT.thumb(12, low).unwrap().1, 320);
    }
}
