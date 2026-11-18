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
use super::Layout as Screens;
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, Align, CARD, GREEN, MUTED, POWER, PRESSED, RAISED, Rect, SECONDARY, WHITE,
};
use super::scaled::{Scaled, Track};
use super::scroll::{Column, Layout, PAGE, Pin};
use super::workout::{Text, View};
use crate::workout::model::Name;
use core::fmt::Write;
use core::ops::Range;
use firmware_shell::rendering::font::FaceRole;

/// Held MOVE pages by this many cards.
pub const VISIBLE: usize = 4;

/// The picker and the FTP sheet on one panel; each text row's capitals end
/// on its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The page under the status bar: the title, then the cards 8 units
    /// apart.
    pub list: Layout,
    /// WORKOUTS and the focus's place on the page at rest, in screen rows.
    pub title: Rect,
    /// The band they pin in once the title has scrolled away.
    pub band: Rect,
    pub sheet: Sheet,
}

/// The FTP sheet over the whole panel: KEEP by BACK, the new FTP, the hero,
/// the old FTP and SAVE? down the middle, and ACT's corner lit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sheet {
    pub keep: Rect,
    pub new: Rect,
    pub hero: Rect,
    pub now: Rect,
    pub save: Rect,
    pub act: [Rect; 2],
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, cards, _] = content.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [title, _, column] = cards.rows([Unit(38), Unit(12), Fr(1)], Unit(0), at);
    let [band, _] = content.rows([Unit(24), Fr(1)], Unit(0), at);
    let height = at.unit(52);
    Regions {
        list: Layout {
            top: content.top,
            bottom: content.bottom,
            column: Column {
                left: cards.left,
                right: cards.right,
                top: column.top - content.top,
                height,
                pitch: height + at.unit(8),
            },
            end: at.unit(10),
            margin: at.unit(6),
            pin: Some(Pin {
                after: at.unit(20) as i32,
                height: band.height(),
            }),
        },
        title,
        band,
        sheet: sheet(at),
    }
}

const fn sheet(at: &Scaled) -> Sheet {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [keep, new, hero, now, save, _, _] = text.rows(
        [
            Unit(20),
            Fr(2),
            Unit(80),
            Unit(34),
            Unit(56),
            Fr(1),
            Unit(22),
        ],
        Unit(0),
        at,
    );
    let [_, by_back, _] = screen.cols([Unit(8), Unit(60), Fr(1)], Unit(0), at);
    let [_, along] = screen.cols([Fr(1), Unit(70)], Unit(0), at);
    let [_, edge] = screen.cols([Fr(1), Unit(4)], Unit(0), at);
    let [_, low] = screen.rows([Fr(1), Unit(4)], Unit(0), at);
    let [_, up] = screen.rows([Fr(1), Unit(50)], Unit(0), at);
    Sheet {
        keep: keep.with_columns(by_back),
        new,
        hero,
        now,
        save,
        act: [low.with_columns(along), up.with_columns(edge)],
    }
}

const MOST_CARDS_IN_VIEW_ON_ANY_PANEL: usize =
    super::scaled::largest!(super::limits::PANELS, |at| {
        let list = regions(&at).list;
        list.viewport() / list.column.pitch + 2
    });

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
    rows: heapless::Vec<Choice, MOST_CARDS_IN_VIEW_ON_ANY_PANEL>,
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
}

pub(super) fn prepare(view: &View) -> Canvas {
    let picker = &view.picker();
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.workouts);
    let list = &regions.list;
    let face = |role| at.face(role);
    let mut canvas = Canvas::default();
    let mut position = Text::<12>::new();
    let _ = write!(position, "{}/{}", picker.cursor + 1, picker.count);
    // The title scrolls with the page; once it is gone, a band pins it.
    let (title_base, faces): (usize, &[&'static firmware_shell::rendering::font::Face]) =
        if list.pinned(picker.offset) {
            let band = regions.band;
            canvas.shape(Shape::Bar {
                rect: Rect {
                    top: band.bottom - 1,
                    ..band
                },
                radius: 0,
                color: CARD,
            });
            (band.top + at.unit(18), &[face(FaceRole::Body)])
        } else {
            (
                PAGE + regions.title.bottom - list.top,
                &[face(FaceRole::Title), face(FaceRole::Body)],
            )
        };
    let title = regions.title;
    let place = at.unit(60);
    canvas.fit(
        faces,
        b"WORKOUTS",
        title.width() - place,
        title.left,
        title_base,
        Align::Left,
        WHITE,
    );
    canvas.caption(
        at,
        position.as_bytes(),
        place,
        title.right,
        title_base,
        Align::Right,
    );
    let (radius, ring, inset) = (at.unit(12), at.unit(3), at.unit(16));
    for (row, choice) in picker.shown.clone().zip(picker.rows.iter()) {
        let rect = list.rect(row);
        let focused = row == picker.cursor;
        if focused {
            canvas.shape(Shape::Panel {
                rect: Rect {
                    left: rect.left - ring,
                    top: rect.top - ring,
                    right: rect.right + ring,
                    bottom: rect.bottom + ring,
                },
                radius: radius + ring,
                color: ACCENT,
            });
        }
        canvas.shape(Shape::Panel {
            rect,
            radius,
            color: if picker.pressed == Some(row) {
                PRESSED
            } else if focused {
                RAISED
            } else {
                CARD
            },
        });
        let chosen = row == picker.chosen;
        let text_right = rect.right - inset - if chosen { at.unit(16) } else { 0 };
        if chosen {
            canvas.shape(Shape::Disc {
                cx: 4 * (rect.right - inset - at.unit(4)) as i32,
                cy: 4 * (rect.top + rect.height() / 2) as i32,
                radius: 4 * at.unit(4) as i32,
                color: GREEN,
            });
        }
        let left = rect.left + inset;
        canvas.fit(
            &[face(FaceRole::Body), face(FaceRole::Caption)],
            choice.name.as_bytes(),
            text_right - left,
            left,
            rect.top + at.unit(24),
            Align::Left,
            if focused { WHITE } else { MUTED },
        );
        canvas.caption(
            at,
            choice.detail.as_bytes(),
            text_right - left,
            left,
            rect.top + at.unit(42),
            Align::Left,
        );
    }
    canvas
}

/// The FTP sheet over the whole panel.
pub fn ftp_sheet(layout: &Screens, new: u16, old: u16) -> Canvas {
    let (at, sheet) = (&layout.scaled, &layout.workouts.sheet);
    let face = |role| at.face(role);
    let mut canvas = Canvas::default();
    let centre = sheet.new.center_x();
    let width = sheet.new.width();
    canvas.fit(
        &[face(FaceRole::Body)],
        b"NEW FTP",
        width,
        centre,
        sheet.new.bottom,
        Align::Center,
        SECONDARY,
    );
    let mut value = Text::<12>::new();
    let _ = write!(value, "{new}");
    let hero = canvas.fit(
        &[face(FaceRole::Hero)],
        value.as_bytes(),
        width - at.unit(40),
        centre,
        sheet.hero.bottom,
        Align::Center,
        POWER,
    );
    canvas.fit(
        &[face(FaceRole::Caption)],
        b"W",
        at.unit(30),
        hero.right() + at.unit(6),
        sheet.hero.bottom,
        Align::Left,
        SECONDARY,
    );
    let mut was = Text::<24>::new();
    let _ = write!(was, "NOW {old} W");
    canvas.fit(
        &[face(FaceRole::Body), face(FaceRole::Caption)],
        was.as_bytes(),
        width,
        centre,
        sheet.now.bottom,
        Align::Center,
        SECONDARY,
    );
    canvas.fit(
        &[face(FaceRole::Title), face(FaceRole::Body)],
        b"SAVE?",
        width,
        centre,
        sheet.save.bottom,
        Align::Center,
        WHITE,
    );
    // BACK keeps the old FTP, by its button; ACT saves, its corner lit.
    let keep = sheet.keep;
    canvas.caption(
        at,
        b"KEEP",
        keep.width(),
        keep.left,
        keep.bottom,
        Align::Left,
    );
    for rect in sheet.act {
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

    const LAYOUT: Layout = crate::screens::BASE.workouts.list;

    fn picker_on(layout: &'static Screens, rows: usize, cursor: usize) -> View<'static> {
        let mut view = View::new(Page::Workouts, 0, None);
        view.layout = layout;
        let list = &layout.workouts.list;
        let offset = list.reveal(cursor, 0);
        view.scroll = offset;
        let mut picker = Picker::new(list, rows, cursor, 2, offset);
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

    fn picker(rows: usize, cursor: usize) -> View<'static> {
        picker_on(&crate::screens::BASE, rows, cursor)
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
        assert!(title.bottom() <= crate::screens::BASE.workouts.band.bottom);
        let mut short = picker(2, 1);
        // The focused card is raised and ringed where it shows.
        let top = LAYOUT.top + LAYOUT.column.rect(1).top;
        assert_eq!(short.pixel(100, top + 30), RAISED);
        assert_eq!(short.pixel(100, top - 2), ACCENT);
        // A card under a resting finger is pressed.
        let mut pressed = Picker::new(&LAYOUT, 2, 1, 2, 0);
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
    fn rows_and_the_sheet_lay_out_whole_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            let screen = layout.scaled.screen();
            let page = Rect {
                top: 0,
                bottom: 100_000,
                ..screen
            };
            for cursor in 0..12 {
                let mut view = picker_on(layout, 12, cursor);
                view.prepare().check_layout();
                let what = std::format!("{cursor} on {:?}", layout.scaled.panel);
                let want: &[&str] = match cursor {
                    0 => &["WORKOUTS", "SWEET SPOT 2X20 WIDE", "100 MIN  96 STEPS"],
                    _ => &["WORKOUTS"],
                };
                view.canvas().assert_laid_out(page, want, &what);
            }
            for (new, old) in [(247, 215), (600, 50), (50, 600), (999, 1000)] {
                let mut view = View::new(Page::Workouts, 0, None);
                view.layout = layout;
                view.show_picker(Picker::default());
                view.ftp_offer = Some((new, old));
                view.prepare().check_layout();
                let sheet = view.canvas();
                let what = std::format!("sheet on {:?}", layout.scaled.panel);
                sheet.assert_laid_out(screen, &["NEW FTP", "SAVE?", "KEEP"], &what);
                assert!(sheet.shows(&std::format!("{new}")), "{what}");
            }
        }
    }

    #[test]
    fn every_scroll_keeps_the_words_of_every_card_in_view_on_every_panel() {
        let count = 12;
        let detail = |i: usize| {
            let mut detail = Text::new();
            let _ = write!(detail, "{} MIN  96 STEPS", 100 + i);
            detail
        };
        for layout in crate::screens::panels::ALL {
            let list = &layout.workouts.list;
            for offset in -40..=list.reveal(count - 1, 0) {
                let mut view = View::new(Page::Workouts, 0, None);
                view.layout = layout;
                view.scroll = offset;
                let mut picker = Picker::new(list, count, 1, 2, offset);
                for i in 0..count {
                    picker.push(Choice {
                        name: Name::new("VO2"),
                        detail: detail(i),
                    });
                }
                view.show_picker(picker);
                view.prepare().check_layout();
                let what = std::format!("{offset} on {:?}", layout.scaled.panel);
                for index in list.visible(count, offset) {
                    let text = detail(index);
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
