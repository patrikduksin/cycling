//! Choosing what to ride.
//!
//! The picker lists FREE RIDE, the built-in workouts, the FTP tests and the
//! uploaded workouts. Its screen is a tree (`compose::builtin::workouts`);
//! here is what a frame keeps of the list: the cards in view.
use super::scroll::Layout;
use super::workout::Text;
use crate::compose::data::Entry;
use crate::compose::list::MOST_IN_VIEW;
use crate::workout::model::Name;
use core::ops::Range;

/// Held MOVE pages by this many cards.
pub const VISIBLE: usize = 4;

/// One card: a workout's name and what it asks.
#[derive(Clone)]
pub struct Choice {
    pub name: Name,
    pub detail: Text<24>,
}

/// The picker as shown: only the cards in view are kept, so a frame's view
/// stays small however long the list is.
#[derive(Clone)]
pub struct Picker {
    /// The cards in view, from `shown.start`.
    rows: heapless::Vec<Choice, MOST_IN_VIEW>,
    shown: Range<usize>,
    count: usize,
    pub cursor: usize,
    /// The row the next ride will use.
    pub chosen: usize,
    /// The card under a resting finger.
    pub pressed: Option<usize>,
    /// How far the page is scrolled.
    pub offset: i32,
    /// Rows pushed so far.
    pushed: usize,
}

impl Default for Picker {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Picker {
    pub const EMPTY: Self = Self {
        rows: heapless::Vec::new(),
        shown: 0..0,
        count: 0,
        cursor: 0,
        chosen: 0,
        pressed: None,
        offset: 0,
        pushed: 0,
    };

    /// A list of `count` rows on `list` focused on `cursor` and scrolled
    /// to `offset`; `push` them in order.
    pub fn new(list: &Layout, count: usize, cursor: usize, chosen: usize, offset: i32) -> Self {
        Self {
            shown: list.visible(count, offset),
            count,
            cursor,
            chosen,
            offset,
            ..Self::EMPTY
        }
    }

    /// The next row, kept when it is in view.
    pub fn push(&mut self, choice: Choice) {
        if self.shown.contains(&self.pushed) {
            let _ = self.rows.push(choice);
        }
        self.pushed += 1;
    }

    pub const fn count(&self) -> usize {
        self.count
    }

    /// Card `index` as the list draws it, while it is in view.
    pub fn entry(&self, index: usize) -> Option<Entry<'_>> {
        let choice = self.rows.get(index.checked_sub(self.shown.start)?)?;
        Some(Entry {
            title: choice.name.as_str(),
            detail: choice.detail.as_str(),
            note: "",
            note_color: 0,
            note_first: false,
            chosen: index == self.chosen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::Layout as Screens;
    use crate::screens::chrome::{ACCENT, Rect};
    use crate::screens::chrome::{PRESSED, RAISED};
    use crate::screens::workout::{Page, View};
    use core::fmt::Write;

    /// Workouts on `layout` composed, with `rows` cards focused on `cursor`
    /// and the page scrolled to `offset`, or to show the focus.
    fn picker_on(
        layout: &'static Screens,
        rows: usize,
        cursor: usize,
        offset: Option<i32>,
        name: impl Fn(usize) -> Name,
        detail: impl Fn(usize) -> Text<24>,
    ) -> (View<'static>, Layout) {
        let mut view = View::new(Page::Workouts, 0, None);
        view.layout = layout;
        view.compose_main();
        let list = *view.layout().expect("Workouts scrolls a list");
        let offset = offset.unwrap_or_else(|| list.reveal(cursor, 0));
        view.scroll = offset;
        let mut picker = Picker::new(&list, rows, cursor, 2, offset);
        for i in 0..rows {
            picker.push(Choice {
                name: name(i),
                detail: detail(i),
            });
        }
        view.show_picker(picker);
        view.prepare();
        (view, list)
    }

    fn wide(i: usize) -> Name {
        Name::new(if i.is_multiple_of(2) {
            "SWEET SPOT 2X20 WIDE"
        } else {
            "VO2"
        })
    }

    fn steps(i: usize) -> Text<24> {
        let mut detail = Text::new();
        let _ = write!(detail, "{} MIN  96 STEPS", 100 + i);
        detail
    }

    fn picker(rows: usize, cursor: usize) -> (View<'static>, Layout) {
        picker_on(&crate::screens::BASE, rows, cursor, None, wide, steps)
    }

    #[test]
    fn the_heading_names_the_list_and_the_place_of_its_focus() {
        let (view, list) = picker(12, 0);
        assert!(view.canvas().shows("1/12") && view.canvas().shows("WORKOUTS"));
        let (view, _) = picker(12, 6);
        assert!(view.canvas().shows("7/12"));
        // The heading stays put over the page however far it scrolls.
        let title = view.canvas().find("WORKOUTS").unwrap();
        assert!(
            title.bottom() <= list.top + list.column.top,
            "above the cards"
        );
    }

    #[test]
    fn the_focused_card_is_raised_and_ringed_and_a_pressed_one_darkens() {
        let (mut view, list) = picker(2, 1);
        let top = list.top + list.column.rect(1).top;
        assert_eq!(view.pixel(100, top + 30), RAISED);
        assert_eq!(view.pixel(100, top - 2), ACCENT);
        let mut pressed = Picker::new(&list, 2, 1, 2, 0);
        pressed.pressed = Some(0);
        for i in 0..2 {
            pressed.push(Choice {
                name: wide(i),
                detail: steps(i),
            });
        }
        view.show_picker(pressed);
        let first = list.top + list.column.rect(0).top;
        assert_eq!(view.pixel(100, first + 30), PRESSED);
    }

    #[test]
    fn rows_lay_out_whole_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            for cursor in 0..12 {
                let (mut view, _) = picker_on(layout, 12, cursor, None, wide, steps);
                view.prepare().check_layout();
                let what = std::format!("{cursor} on {:?}", layout.scaled.panel);
                let want: &[&str] = match cursor {
                    0 => &["WORKOUTS", "SWEET SPOT 2X20 WIDE", "100 MIN  96 STEPS"],
                    _ => &["WORKOUTS"],
                };
                let page = Rect {
                    top: 0,
                    bottom: 100_000,
                    ..layout.scaled.screen()
                };
                view.canvas().assert_laid_out(page, want, &what);
            }
        }
    }

    #[test]
    fn every_scroll_keeps_the_words_of_every_card_in_view_on_every_panel() {
        let count = 12;
        for layout in crate::screens::panels::ALL {
            let (_, list) = picker_on(layout, count, 1, Some(0), wide, steps);
            for offset in -40..=list.reveal(count - 1, 0) {
                let vo2 = |_| Name::new("VO2");
                let (mut view, _) = picker_on(layout, count, 1, Some(offset), vo2, steps);
                view.prepare().check_layout();
                let what = std::format!("{offset} on {:?}", layout.scaled.panel);
                let whole = list
                    .visible(count, offset)
                    .filter(|&index| list.whole(index, offset));
                for index in whole {
                    let text = steps(index);
                    assert!(
                        view.canvas().shows(text.as_str()),
                        "{} at {what}",
                        text.as_str()
                    );
                }
            }
        }
    }
}
