//! Choosing what to ride, and saving what an FTP test measured.
//!
//! The picker lists FREE RIDE, the built-in workouts, the FTP tests and the
//! uploaded workouts as cards, four at a time; the list scrolls to keep the
//! focused card in view. MOVE focuses the next card, ACT chooses the
//! focused one and opens Ready, BACK returns Home, and a tap chooses the
//! card under the finger. The chosen workout carries a green dot.
//!
//! The FTP sheet follows a test: the new FTP as the hero, the old one under
//! it, ACT (its corner lit) saves and BACK keeps the old one.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, Align, BODY, CAPTION, CARD, CONTENT, GREEN, HERO, POWER, RAISED, Rect, SECONDARY,
    TITLE, WHITE,
};
use super::scroll::{Column, Scroll};
use super::workout::{Text, View};
use crate::workout::model::Name;
use core::fmt::Write;

/// Cards in view; held MOVE pages by this many.
pub const VISIBLE: usize = 4;
const MARGIN: usize = 10;
const INSET: usize = 16;
const RADIUS: usize = 12;
const RING: usize = 3;
const TITLE_BASE: usize = CONTENT.top + 38;
const ROW_HEIGHT: usize = 52;
/// The cards, 8 px apart.
const COLUMN: Column = Column {
    left: MARGIN,
    right: CONTENT.right - MARGIN,
    top: TITLE_BASE + 12,
    height: ROW_HEIGHT,
    pitch: ROW_HEIGHT + 8,
};
const _: () = assert!(COLUMN.bottom(VISIBLE) + RING <= CONTENT.bottom);

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
    /// The cards in view, from the window's first row.
    rows: heapless::Vec<Choice, VISIBLE>,
    scroll: Scroll,
    /// Rows pushed so far.
    pushed: usize,
    /// The row the next ride will use.
    pub chosen: usize,
}

impl Default for Picker {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Picker {
    pub const EMPTY: Self = Self {
        rows: heapless::Vec::new(),
        scroll: Scroll::new(0, 0, VISIBLE),
        pushed: 0,
        chosen: 0,
    };

    /// A list of `count` rows focused on `cursor`; `push` them in order.
    pub fn new(count: usize, cursor: usize, chosen: usize) -> Self {
        Self {
            scroll: Scroll::new(count, cursor, VISIBLE),
            chosen,
            ..Self::EMPTY
        }
    }

    /// The next row, kept when it is in view.
    pub fn push(&mut self, choice: Choice) {
        if self.pushed >= self.scroll.first() {
            let _ = self.rows.push(choice);
        }
        self.pushed += 1;
    }

    /// The list's window: which rows show, and which one is focused.
    pub fn scroll(&self) -> Scroll {
        self.scroll
    }

    /// The row under a touch at (`x`, `y`).
    pub fn row_at(&self, x: usize, y: usize) -> Option<usize> {
        self.scroll.row_at(COLUMN, x, y)
    }
}

pub(super) fn prepare(view: &View) -> Canvas {
    let picker = &view.picker();
    let mut canvas = Canvas::default();
    let width = CONTENT.right - 2 * MARGIN;
    canvas.fit(
        &[&TITLE, &BODY],
        b"WORKOUTS",
        width - 60,
        MARGIN,
        TITLE_BASE,
        Align::Left,
        WHITE,
    );
    let mut position = Text::<12>::new();
    let scroll = picker.scroll;
    let _ = write!(position, "{}/{}", scroll.cursor + 1, scroll.count);
    canvas.caption(
        position.as_bytes(),
        60,
        CONTENT.right - MARGIN,
        TITLE_BASE,
        Align::Right,
    );
    for ((slot, row), choice) in scroll.visible().zip(picker.rows.iter()) {
        let rect = COLUMN.rect(slot);
        let focused = row == scroll.cursor;
        if focused {
            canvas.shape(Shape::Panel {
                rect: Rect {
                    left: rect.left - RING,
                    top: rect.top - RING,
                    right: rect.right + RING,
                    bottom: rect.bottom + RING,
                },
                radius: RADIUS + RING,
                color: ACCENT,
            });
        }
        canvas.shape(Shape::Panel {
            rect,
            radius: RADIUS,
            color: if focused { RAISED } else { CARD },
        });
        let chosen = row == picker.chosen;
        let text_right = rect.right - INSET - if chosen { 16 } else { 0 };
        if chosen {
            canvas.shape(Shape::Disc {
                cx: 4 * (rect.right - INSET - 4) as i32,
                cy: 4 * (rect.top + ROW_HEIGHT / 2) as i32,
                radius: 4 * 4,
                color: GREEN,
            });
        }
        let left = rect.left + INSET;
        canvas.fit(
            &[&BODY, &CAPTION],
            choice.name.as_bytes(),
            text_right - left,
            left,
            rect.top + 24,
            Align::Left,
            if focused { WHITE } else { super::chrome::MUTED },
        );
        canvas.caption(
            choice.detail.as_bytes(),
            text_right - left,
            left,
            rect.top + 42,
            Align::Left,
        );
    }
    canvas
}

/// The FTP sheet over the whole screen.
pub fn ftp_sheet(new: u16, old: u16) -> Canvas {
    use super::chrome::{HEIGHT, WIDTH as SCREEN};
    let mut canvas = Canvas::default();
    let centre = SCREEN / 2;
    let width = SCREEN - 20;
    canvas.fit(
        &[&BODY],
        b"NEW FTP",
        width,
        centre,
        92,
        Align::Center,
        SECONDARY,
    );
    let mut value = Text::<12>::new();
    let _ = write!(value, "{new}");
    let hero = canvas.fit(
        &[&HERO],
        value.as_bytes(),
        width - 40,
        centre,
        172,
        Align::Center,
        POWER,
    );
    canvas.fit(
        &[&CAPTION],
        b"W",
        30,
        hero.right() + 6,
        172,
        Align::Left,
        SECONDARY,
    );
    let mut was = Text::<24>::new();
    let _ = write!(was, "NOW {old} W");
    canvas.fit(
        &[&BODY, &CAPTION],
        was.as_bytes(),
        width,
        centre,
        206,
        Align::Center,
        SECONDARY,
    );
    canvas.fit(
        &[&TITLE, &BODY],
        b"SAVE?",
        width,
        centre,
        262,
        Align::Center,
        WHITE,
    );
    // BACK keeps the old FTP, by its button; ACT saves, its corner lit.
    canvas.caption(b"KEEP", 60, 8, 20, Align::Left);
    for rect in [
        Rect {
            left: SCREEN - 70,
            top: HEIGHT - 4,
            right: SCREEN,
            bottom: HEIGHT,
        },
        Rect {
            left: SCREEN - 4,
            top: HEIGHT - 50,
            right: SCREEN,
            bottom: HEIGHT,
        },
    ] {
        canvas.shape(Shape::Bar {
            rect,
            radius: 0,
            color: ACCENT,
        });
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    fn picker(rows: usize, cursor: usize) -> View<'static> {
        let mut view = View::new(Page::Workouts, 0, None);
        let mut picker = Picker::new(rows, cursor, 2);
        for i in 0..rows {
            let mut detail = Text::new();
            let _ = write!(detail, "{} MIN  {} STEPS", 100 + i, 96);
            let name = if i % 2 == 0 {
                Name::new("SWEET SPOT 2X20 WIDE")
            } else {
                Name::new("VO2")
            };
            picker.push(Choice { name, detail });
        }
        view.show_picker(picker);
        view.prepare();
        view
    }

    #[test]
    fn the_list_scrolls_to_the_focused_row_and_taps_find_rows() {
        let view = picker(12, 0);
        assert!(view.canvas().shows("1/12"));
        assert_eq!(view.picker().row_at(120, COLUMN.top + 10), Some(0));
        let view = picker(12, 6);
        assert_eq!(view.picker().scroll().first(), 3);
        assert_eq!(view.picker().row_at(120, COLUMN.rect(3).top + 10), Some(6));
        assert!(view.canvas().shows("7/12"));
        let view = picker(12, 11);
        assert_eq!(view.picker().scroll().first(), 8);
        let short = picker(2, 1);
        // The focused card is raised and ringed.
        let top = COLUMN.rect(1).top;
        assert_eq!(short.pixel(100, top + 30), RAISED);
        assert_eq!(short.pixel(100, top - 2), ACCENT);
    }

    #[test]
    fn rows_and_the_sheet_lay_out_whole() {
        let screen = Rect {
            left: 0,
            top: CONTENT.top,
            right: 240,
            bottom: 320,
        };
        for cursor in 0..12 {
            picker(12, cursor).canvas().assert_laid_out(
                screen,
                &["WORKOUTS"],
                &std::format!("{cursor}"),
            );
        }
        for (new, old) in [(247, 215), (600, 50), (50, 600)] {
            let sheet = ftp_sheet(new, old);
            let full = Rect { top: 0, ..screen };
            sheet.assert_laid_out(full, &["NEW FTP", "SAVE?", "KEEP"], "sheet");
            assert!(sheet.shows(&std::format!("{new}")));
        }
    }
}
