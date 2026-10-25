//! Choosing what to ride, and saving what an FTP test measured.
//!
//! The picker lists FREE RIDE, the built-in workouts, the FTP tests and the
//! uploaded workouts as cards on a page that scrolls under the finger
//! (`scroll::Layout`); once the title has scrolled away, WORKOUTS and the
//! focus's place pin at the top. MOVE focuses the next card, ACT chooses the
//! focused one and opens Ready, BACK returns Home, and a tap chooses the
//! card under the finger. The chosen workout carries a green dot.
//!
//! The FTP sheet follows a test: the new FTP as the hero, the old one under
//! it, ACT (its corner lit) saves and BACK keeps the old one.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, Align, BODY, CAPTION, CARD, CONTENT, GREEN, HERO, POWER, PRESSED, RAISED, Rect,
    SECONDARY, TITLE, WHITE,
};
use super::scroll::{Column, Layout, PAGE, Pin};
use super::workout::{Text, View};
use crate::workout::model::Name;
use core::fmt::Write;
use core::ops::Range;

/// Held MOVE pages by this many cards.
pub const VISIBLE: usize = 4;
/// The most cards any of whose rows show at once.
const SHOWN: usize = 6;
const MARGIN: usize = 10;
const INSET: usize = 16;
const RADIUS: usize = 12;
const RING: usize = 3;
const TITLE_BASE: usize = CONTENT.top + 38;
const ROW_HEIGHT: usize = 52;
/// The page under the status bar: the title, then the cards 8 px apart.
pub const LAYOUT: Layout = Layout {
    top: CONTENT.top,
    bottom: CONTENT.bottom,
    column: Column {
        left: MARGIN,
        right: CONTENT.right - MARGIN,
        top: TITLE_BASE + 12 - CONTENT.top,
        height: ROW_HEIGHT,
        pitch: ROW_HEIGHT + 8,
    },
    end: 10,
    margin: 6,
    pin: Some(Pin {
        after: 20,
        height: PIN,
    }),
};
const _: () = assert!(LAYOUT.viewport() / LAYOUT.column.pitch + 2 <= SHOWN);
const PIN: usize = 24;
const PIN_BASE: usize = CONTENT.top + 18;
/// Shifts a row of the page as it lies at rest to its canvas row.
const AT: usize = PAGE - CONTENT.top;

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
    rows: heapless::Vec<Choice, SHOWN>,
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

    /// A list of `count` rows focused on `cursor` and scrolled to `offset`;
    /// `push` them in order.
    pub fn new(count: usize, cursor: usize, chosen: usize, offset: i32) -> Self {
        Self {
            shown: LAYOUT.visible(count, offset),
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
}

pub(super) fn prepare(view: &View) -> Canvas {
    let picker = &view.picker();
    let mut canvas = Canvas::default();
    let width = CONTENT.right - 2 * MARGIN;
    let mut position = Text::<12>::new();
    let _ = write!(position, "{}/{}", picker.cursor + 1, picker.count);
    // The title scrolls with the page; once it is gone, a band pins it.
    let (title_base, faces): (usize, &[&'static firmware_shell::rendering::font::Face]) =
        if LAYOUT.pinned(picker.offset) {
            canvas.shape(Shape::Bar {
                rect: Rect {
                    left: 0,
                    top: CONTENT.top + PIN - 1,
                    right: CONTENT.right,
                    bottom: CONTENT.top + PIN,
                },
                radius: 0,
                color: CARD,
            });
            (PIN_BASE, &[&BODY])
        } else {
            (AT + TITLE_BASE, &[&TITLE, &BODY])
        };
    canvas.fit(
        faces,
        b"WORKOUTS",
        width - 60,
        MARGIN,
        title_base,
        Align::Left,
        WHITE,
    );
    canvas.caption(
        position.as_bytes(),
        60,
        CONTENT.right - MARGIN,
        title_base,
        Align::Right,
    );
    for (row, choice) in picker.shown.clone().zip(picker.rows.iter()) {
        let rect = LAYOUT.rect(row);
        let focused = row == picker.cursor;
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
            color: if picker.pressed == Some(row) {
                PRESSED
            } else if focused {
                RAISED
            } else {
                CARD
            },
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
        let offset = LAYOUT.reveal(cursor, 0);
        view.scroll = offset;
        let mut picker = Picker::new(rows, cursor, 2, offset);
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
    fn the_page_shows_the_cards_in_view_and_pins_its_title() {
        let view = picker(12, 0);
        assert!(view.canvas().shows("1/12") && view.canvas().shows("WORKOUTS"));
        assert_eq!(view.picker().shown, 0..5);
        let view = picker(12, 6);
        assert!(view.canvas().shows("7/12"));
        let offset = LAYOUT.reveal(6, 0);
        assert!(LAYOUT.pinned(offset));
        assert_eq!(view.picker().shown, LAYOUT.visible(12, offset));
        // The pinned title paints on its own rows, over the page.
        let title = view.canvas().find("WORKOUTS").unwrap();
        assert!(title.bottom() <= CONTENT.top + PIN);
        let short = picker(2, 1);
        // The focused card is raised and ringed where it shows.
        let top = LAYOUT.top + LAYOUT.column.rect(1).top;
        assert_eq!(short.pixel(100, top + 30), RAISED);
        assert_eq!(short.pixel(100, top - 2), ACCENT);
        // A card under a resting finger is pressed.
        let mut pressed = Picker::new(2, 1, 2, 0);
        pressed.pressed = Some(0);
        for _ in 0..2 {
            pressed.push(Choice {
                name: Name::new("VO2"),
                detail: Text::new(),
            });
        }
        let mut view = View::new(Page::Workouts, 0, None);
        view.show_picker(pressed);
        view.prepare();
        let first = LAYOUT.top + LAYOUT.column.rect(0).top;
        assert_eq!(view.pixel(100, first + 30), PRESSED);
    }

    #[test]
    fn rows_and_the_sheet_lay_out_whole() {
        let screen = Rect {
            left: 0,
            top: CONTENT.top,
            right: 240,
            bottom: 320,
        };
        let page = Rect {
            top: 0,
            bottom: 100_000,
            ..screen
        };
        for cursor in 0..12 {
            picker(12, cursor).canvas().assert_laid_out(
                page,
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
