//! The lists that scroll under the finger (Rides, Workouts and a Settings
//! section): their length, focus and scroll, what the buttons do to them
//! and how a finger drives them.
//!
//! Focus is the buttons' first responder; the finger scrolls and never moves
//! it. When the focus has scrolled out of view, a press first brings it
//! back (`scroll::Layout::next` and `act`).
use super::Runtime;
use crate::screens::kinetic::Kinetic;
use crate::screens::scroll::{Layout, Press};
use crate::screens::settings::{self, Kind, Row};
use crate::screens::touch::{List, Pan};
use crate::screens::workout::Page;
use crate::screens::{history, workouts};

impl Runtime {
    /// The list the page shows, if any.
    pub(super) fn page_list(&self) -> Option<List> {
        let navigation = &self.navigation;
        match navigation.page {
            Page::History => Some(List::Rides),
            Page::Workouts if self.training.ftp_offer.is_none() => Some(List::Workouts),
            Page::Settings
                if !(navigation.settings_editing
                    && Row::at(navigation.settings_field).kind() == Kind::Number) =>
            {
                Some(List::Settings)
            }
            _ => None,
        }
    }

    pub(super) fn list_layout(list: List) -> &'static Layout {
        match list {
            List::Rides => &history::LAYOUT,
            List::Workouts => &workouts::LAYOUT,
            List::Settings => &settings::LAYOUT,
        }
    }

    pub(super) fn list_count(&self, list: List) -> usize {
        match list {
            List::Rides => self.history_count(),
            List::Workouts => self.training.rows(),
            List::Settings => settings::rows(self.navigation.settings_field).0.len(),
        }
    }

    /// The focused card's index in the list.
    pub(super) fn list_cursor(&self, list: List) -> usize {
        match list {
            List::Rides => self.history.cursor,
            List::Workouts => self.training.picker_cursor,
            List::Settings => {
                let field = self.navigation.settings_field;
                field % settings::ROWS - settings::rows(field).1
            }
        }
    }

    fn set_list_cursor(&mut self, list: List, index: usize) {
        match list {
            List::Rides => self.history.cursor = index,
            List::Workouts => self.training.picker_cursor = index,
            List::Settings => {
                let first = settings::rows(self.navigation.settings_field).1;
                self.navigation.settings_field = first + index;
            }
        }
    }

    pub(super) fn kinetic(&self, list: List) -> &Kinetic {
        match list {
            List::Rides => &self.history.scroll,
            List::Workouts => &self.training.picker_scroll,
            List::Settings => &self.navigation.settings_scroll,
        }
    }

    pub(super) fn kinetic_mut(&mut self, list: List) -> &mut Kinetic {
        match list {
            List::Rides => &mut self.history.scroll,
            List::Workouts => &mut self.training.picker_scroll,
            List::Settings => &mut self.navigation.settings_scroll,
        }
    }

    /// Where the list rests once its motion ends, in whole pixels.
    fn list_rest(&self, list: List) -> i32 {
        libm::roundf(self.kinetic(list).rest()) as i32
    }

    /// Before each frame of a page with a list: its size and its motion.
    /// True while it moves, so frames keep coming.
    pub(super) fn list_frame(&mut self, now: u64) -> bool {
        let Some(list) = self.page_list() else {
            return false;
        };
        let layout = Self::list_layout(list);
        let content = layout.content(self.list_count(list));
        let scroll = self.kinetic_mut(list);
        scroll.fit(layout.viewport(), content, now);
        scroll.advance(now);
        scroll.moving()
    }

    /// The list's scroll as drawn at `now`.
    pub(super) fn list_offset(&self, list: List, now: u64) -> i32 {
        self.kinetic(list).offset(now)
    }

    /// A finger's part in the list's scroll. A finger that stopped moving
    /// content takes the contact from the card under it.
    pub(super) fn list_pan(&mut self, list: List, pan: Pan, now: u64) {
        let scroll = self.kinetic_mut(list);
        match pan {
            Pan::Down(y) => {
                if scroll.press(y, now) {
                    self.navigation.touches.claim_pan();
                }
            }
            Pan::Move(y) => scroll.drag(y, now),
            Pan::Up => {
                scroll.release(now);
            }
            Pan::Cancel => scroll.cancel(now),
        }
        self.display.redraw();
    }

    /// MOVE on a list: a focus out of view comes back to the first card in
    /// view; otherwise the next card (held, `page` on; past the last, the
    /// first) is focused and scrolled into view.
    pub(super) fn list_move(&mut self, list: List, page: usize, held: bool, now: u64) {
        let layout = Self::list_layout(list);
        let (count, cursor, rest) = (
            self.list_count(list),
            self.list_cursor(list),
            self.list_rest(list),
        );
        if let Press::Focus(index, scroll) = layout.next(count, cursor, rest, page, held) {
            self.set_list_cursor(list, index);
            if let Some(offset) = scroll {
                self.kinetic_mut(list).reveal(offset as f32, now);
            }
        }
    }

    /// ACT on a list: true when the focused card shows whole, for the caller
    /// to act on; otherwise the list scrolls it into view first.
    pub(super) fn list_act(&mut self, list: List, now: u64) -> bool {
        let layout = Self::list_layout(list);
        match layout.act(self.list_cursor(list), self.list_rest(list)) {
            Press::Act => true,
            Press::Reveal(offset) => {
                self.kinetic_mut(list).reveal(offset as f32, now);
                false
            }
            Press::Focus(..) => false,
        }
    }

    /// Settings' MOVE: within the section as on any list, and past its last
    /// row into the next section, shown from its top.
    pub(super) fn settings_move(&mut self, now: u64) {
        let layout = Self::list_layout(List::Settings);
        let (count, cursor, rest) = (
            self.list_count(List::Settings),
            self.list_cursor(List::Settings),
            self.list_rest(List::Settings),
        );
        if !layout.whole(cursor, rest)
            && let Some(first) = layout.first_whole(count, rest)
        {
            self.set_list_cursor(List::Settings, first);
            return;
        }
        let navigation = &mut self.navigation;
        let section = settings::section(navigation.settings_field).0;
        navigation.settings_field = (navigation.settings_field + 1) % settings::ROWS;
        if settings::section(navigation.settings_field).0 == section {
            let target = layout.reveal(cursor + 1, rest);
            navigation.settings_scroll.reveal(target as f32, now);
        } else {
            navigation.settings_scroll = Kinetic::default();
        }
    }

    /// A list opened afresh shows its focused card.
    pub(super) fn show_focus(&mut self, list: List, now: u64) {
        let layout = Self::list_layout(list);
        let content = layout.content(self.list_count(list));
        let target = layout.reveal(self.list_cursor(list), 0);
        let scroll = self.kinetic_mut(list);
        *scroll = Kinetic::default();
        scroll.fit(layout.viewport(), content, now);
        scroll.place(target as f32);
    }
}
