//! The lists that scroll under the finger (Rides, Workouts and a Settings
//! section): their length, focus and scroll, what the buttons do to them
//! and how a finger drives them.
//!
//! Focus is the buttons' first responder; the finger scrolls and never moves
//! it. When the focus has scrolled out of view, a press first brings it
//! back (`scroll::Layout::next` and `act`).
use super::Runtime;
use super::navigation::To;
use crate::compose::builtin::ScreenId;
use crate::compose::input::{Cursor, ListAct};
use crate::screens::kinetic::Kinetic;
use crate::screens::scroll::{Layout, Press};
use crate::screens::settings::Kind;
use crate::screens::touch::{List, Pan};
use crate::screens::workout::Page;
use device_api::ant::Ant;

impl Runtime {
    /// The list the page shows, if any.
    pub(super) fn page_list(&self) -> Option<List> {
        let navigation = &self.navigation;
        match navigation.page() {
            Page::Workouts if self.ftp_sheet().is_none() => Some(List::Workouts),
            Page::Settings
                if !(navigation.editing() && navigation.setting().kind() == Kind::Number) =>
            {
                Some(List::Settings)
            }
            _ => None,
        }
    }

    /// The page of cards the list's built-in scrolls.
    pub(super) fn list_layout(&self, list: List) -> Layout {
        match list {
            List::Workouts => self.composed_page(ScreenId::Workouts),
            List::Settings => self.composed_page(ScreenId::Settings),
        }
    }

    /// What the list's owner does with a press or a tap on its items.
    pub(super) fn list_input(
        &mut self,
        list: List,
        act: ListAct,
        held: bool,
        now: u64,
        ant: &mut impl Ant,
    ) {
        match act {
            // Held, MOVE pages by the cards a list shows at rest.
            ListAct::Step => match list {
                List::Workouts => {
                    self.list_move(list, crate::screens::workouts::VISIBLE, held, now)
                }
                List::Settings => self.settings_move(now),
            },
            ListAct::Enter => self.list_enter(list, now),
            ListAct::Act if self.list_act(list, now) => {
                self.list_chosen(list, self.list_cursor(list), now, ant);
            }
            ListAct::Act => {}
            ListAct::Tap(index) => self.list_chosen(list, usize::from(index), now, ant),
        }
    }

    fn list_chosen(&mut self, list: List, index: usize, now: u64, ant: &mut impl Ant) {
        match list {
            List::Workouts => {
                self.choose_workout(index);
                self.navigation.go(To::Page(Page::Home));
                self.ride_press(device_api::input::Role::Act, false, now, now);
            }
            List::Settings => {
                self.set_list_cursor(List::Settings, index);
                self.act_on_setting(now, ant);
            }
        }
    }

    pub(super) fn list_count(&self, list: List) -> usize {
        match list {
            List::Workouts => self.training.rows(),
            List::Settings => {
                let navigation = &self.navigation;
                navigation
                    .sections()
                    .rows(navigation.settings_field())
                    .0
                    .len()
            }
        }
    }

    /// The focused card's index in the list.
    pub(super) fn list_cursor(&self, list: List) -> usize {
        match list {
            List::Workouts => self.training.picker_cursor,
            List::Settings => {
                let (field, sections) =
                    (self.navigation.settings_field(), self.navigation.sections());
                field % sections.fields() - sections.rows(field).1
            }
        }
    }

    fn set_list_cursor(&mut self, list: List, index: usize) {
        match list {
            List::Workouts => self.training.picker_cursor = index,
            List::Settings => {
                let navigation = &self.navigation;
                let first = navigation.sections().rows(navigation.settings_field()).1;
                self.navigation.go(To::Field(first + index));
            }
        }
    }

    pub(super) fn kinetic(&self, list: List) -> &Kinetic {
        match list {
            List::Workouts => &self.training.picker_scroll,
            List::Settings => self.navigation.settings_scroll(),
        }
    }

    pub(super) fn kinetic_mut(&mut self, list: List) -> &mut Kinetic {
        match list {
            List::Workouts => &mut self.training.picker_scroll,
            List::Settings => self.navigation.settings_scroll_mut(),
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
        let layout = self.list_layout(list);
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
                    self.navigation.touches_mut().claim_pan();
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
        let layout = self.list_layout(list);
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

    /// Where the list shown keeps its focus, for MOVE to step it or leave
    /// it: on its last item once that shows whole. A screen without a list
    /// has none to step.
    pub(super) fn list_cursor_at(&self) -> Cursor {
        let Some(list) = self.page_list() else {
            return Cursor::Last;
        };
        let (count, cursor) = (self.list_count(list), self.list_cursor(list));
        let last = match list {
            List::Settings => {
                let navigation = &self.navigation;
                navigation.settings_field() + 1 == navigation.sections().fields()
            }
            List::Workouts => cursor + 1 >= count,
        };
        let layout = self.list_layout(list);
        if count == 0 || (last && layout.whole(cursor, self.list_rest(list))) {
            Cursor::Last
        } else {
            Cursor::Within
        }
    }

    /// The ring came to the list: its first item takes the focus and the
    /// list shows from its top.
    fn list_enter(&mut self, list: List, now: u64) {
        match list {
            List::Settings => self.navigation.go(To::Field(0)),
            List::Workouts => self.set_list_cursor(list, 0),
        }
        self.kinetic_mut(list).reveal(0.0, now);
    }

    /// ACT on a list: true when the focused card shows whole, for the caller
    /// to act on; otherwise the list scrolls it into view first.
    pub(super) fn list_act(&mut self, list: List, now: u64) -> bool {
        let layout = self.list_layout(list);
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
        let layout = self.list_layout(List::Settings);
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
        let sections = navigation.sections();
        let section = sections.section(navigation.settings_field()).0;
        navigation.go(To::Field(
            (navigation.settings_field() + 1) % sections.fields(),
        ));
        if sections.section(navigation.settings_field()).0 == section {
            let target = layout.reveal(cursor + 1, rest);
            navigation.settings_scroll_mut().reveal(target as f32, now);
        }
    }

    /// A list opened afresh shows its focused card.
    pub(super) fn show_focus(&mut self, list: List, now: u64) {
        let layout = self.list_layout(list);
        let content = layout.content(self.list_count(list));
        let target = layout.reveal(self.list_cursor(list), 0);
        let scroll = self.kinetic_mut(list);
        *scroll = Kinetic::default();
        scroll.fit(layout.viewport(), content, now);
        scroll.place(target as f32);
    }
}
