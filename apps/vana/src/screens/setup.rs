//! First-run setup: a few pages that ask for what VANA cannot guess, each
//! with a short explanation and its choices as cards.
//!
//! Buttons keep their system roles: MOVE focuses the next card, ACT takes it
//! and BACK leaves setup for Home (it resumes at the same page on the next
//! start, so a ride never waits for it). Touch taps a card. Numbers open the
//! Settings editor and come back here when it finishes.
use super::canvas::{Canvas, Shape};
use super::chrome::{ACCENT, Align, CARD, Line, RAISED, Rect, SECONDARY, WHITE};
use super::scaled::{Scaled, Track};
use super::workout::View;
use firmware_shell::rendering::font::FaceRole;

/// Setup pages in order. The stored setup step is the page number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Power,
    Heart,
    Sensors,
    Done,
}

/// What a card does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Go on to the next page.
    Next,
    /// Leave setup for good, from Welcome.
    Skip,
    /// Open the editor for the page's number.
    Edit,
    /// Keep the default for the page's number and go on.
    Default,
    /// Open the sensor menu; setup resumes at Done when it closes.
    Pair,
    /// Finish setup and start a ride.
    Ride,
    /// Finish setup on Home.
    Home,
}

pub const STEPS: [Step; 5] = [
    Step::Welcome,
    Step::Power,
    Step::Heart,
    Step::Sensors,
    Step::Done,
];

impl Step {
    /// The page a stored setup step resumes, if setup is not done.
    pub fn stored(setup: u8) -> Option<Self> {
        STEPS.get(usize::from(setup).checked_sub(1)?).copied()
    }

    /// The setup step stored while this page is due.
    pub fn byte(self) -> u8 {
        STEPS.iter().position(|step| *step == self).unwrap_or(0) as u8 + 1
    }

    pub fn next(self) -> Option<Self> {
        STEPS.get(usize::from(self.byte())).copied()
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Welcome => "WELCOME",
            Self::Power => "POWER",
            Self::Heart => "HEART",
            Self::Sensors => "SENSORS",
            Self::Done => "READY",
        }
    }

    /// Two short lines under the title.
    pub const fn text(self) -> [&'static str; 2] {
        match self {
            Self::Welcome => ["A MINUTE TO MAKE VANA", "YOURS. CHANGE IT LATER."],
            Self::Power => ["FTP SETS POWER ZONES.", "NOT SURE? 215 W FOR NOW."],
            Self::Heart => ["THRESHOLD HR SETS ZONES.", "NOT SURE? 185 FOR NOW."],
            Self::Sensors => ["PAIR HEART, POWER AND", "SPEED SENSORS NEARBY."],
            Self::Done => ["ALL SET. SETTINGS KEEPS", "EVERY CHOICE."],
        }
    }

    /// The cards, in MOVE order.
    pub const fn cards(self) -> &'static [(&'static str, Choice)] {
        match self {
            Self::Welcome => &[("SET UP", Choice::Next), ("SKIP SETUP", Choice::Skip)],
            Self::Power => &[("SET FTP", Choice::Edit), ("I DON'T KNOW", Choice::Default)],
            Self::Heart => &[
                ("SET LTHR", Choice::Edit),
                ("I DON'T KNOW", Choice::Default),
            ],
            Self::Sensors => &[("PAIR SENSORS", Choice::Pair), ("LATER", Choice::Next)],
            Self::Done => &[("RIDE", Choice::Ride), ("HOME", Choice::Home)],
        }
    }
}

const CARDS: usize = 2;

const _: () = {
    let mut i = 0;
    while i < STEPS.len() {
        assert!(
            STEPS[i].cards().len() <= CARDS,
            "a setup page has more cards than its regions"
        );
        i += 1;
    }
};

/// A setup page on one panel, under the status bar: the title, two lines of
/// text and the cards. Text rows end on their capitals' baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub title: Rect,
    pub text: [Rect; 2],
    pub cards: [Rect; CARDS],
    /// Each card's label and note, inset from its edges.
    pub labels: [Rect; CARDS],
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, columns, _] = content.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [title, first, second, _, cards, _] = content.rows(
        [
            Unit(46),
            Unit(30),
            Unit(20),
            Unit(30),
            Unit(52 * CARDS as u16 + 8 * (CARDS as u16 - 1)),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let cards = cards
        .with_columns(columns)
        .rows([Unit(52); CARDS], Unit(8), at);
    let [_, inset, _] = columns.cols([Unit(16), Fr(1), Unit(16)], Unit(0), at);
    let mut labels = cards;
    let mut i = 0;
    while i < CARDS {
        labels[i] = cards[i].with_columns(inset);
        i += 1;
    }
    Regions {
        title: title.with_columns(columns),
        text: [first.with_columns(columns), second.with_columns(columns)],
        cards,
        labels,
    }
}

/// The card under a touch at panel point (`x`, `y`) on `step`.
pub fn target(regions: &Regions, x: usize, y: usize, step: Step) -> Option<usize> {
    let cards = &regions.cards[..step.cards().len()];
    cards.iter().position(|card| card.contains(x, y))
}

/// The page for `step` with `view.cursor` focused. `view.lines[slot]` is a
/// note at the right of each card, such as the value a number has now.
pub(super) fn prepare(view: &View, step: Step) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.setup);
    let [title_face, body, caption] =
        [FaceRole::Title, FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let mut canvas = Canvas::default();
    let title = regions.title;
    let label = step.title().as_bytes();
    let (left, width) = (title.left, title.width());
    canvas.fit(
        &[title_face],
        label,
        width,
        left,
        title.bottom,
        Align::Left,
        WHITE,
    );
    for (line, row) in step.text().iter().zip(regions.text) {
        let (left, width) = (row.left, row.width());
        canvas.caption(at, line.as_bytes(), width, left, row.bottom, Align::Left);
    }
    let (radius, focus) = (at.unit(12), at.unit(3));
    let rows = regions.cards.iter().zip(&regions.labels);
    for (slot, ((label, _), (&rect, &text))) in step.cards().iter().zip(rows).enumerate() {
        let focused = slot == view.cursor;
        if focused {
            canvas.shape(Shape::Panel {
                rect,
                radius,
                color: ACCENT,
            });
        }
        canvas.shape(Shape::Panel {
            rect: if focused { rect.inset(focus) } else { rect },
            radius: if focused { radius - focus } else { radius },
            color: if focused { RAISED } else { CARD },
        });
        let base = rect.top + (rect.height() + body.cap as usize) / 2;
        let title = canvas.fit(
            &[body],
            label.as_bytes(),
            text.width(),
            text.left,
            base,
            Align::Left,
            WHITE,
        );
        let note = view.lines()[slot].as_bytes();
        let room = text.right.saturating_sub(title.right() + at.unit(12));
        if !note.is_empty() && Line::<24>::fit(&[caption], note, room).text().len() == note.len() {
            canvas.fit(
                &[caption],
                note,
                room,
                text.right,
                base,
                Align::Right,
                SECONDARY,
            );
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    #[test]
    fn steps_resume_from_their_stored_byte() {
        for step in STEPS {
            assert_eq!(Step::stored(step.byte()), Some(step));
        }
        assert_eq!(Step::stored(0), None, "done");
        assert_eq!(Step::stored(9), None);
        assert_eq!(Step::Welcome.next(), Some(Step::Power));
        assert_eq!(Step::Done.next(), None);
    }

    #[test]
    fn every_page_lays_out_whole_with_notes() {
        crate::screens::check::enable(true);
        for layout in crate::screens::panels::ALL {
            let regions = &layout.setup;
            for step in STEPS {
                for cursor in 0..step.cards().len() {
                    let mut view = View::new(Page::Setup, cursor, None);
                    view.layout = layout;
                    let _ = view.show_setup(step)[0].push_str("1000 W");
                    view.prepare().check_layout();
                    let mut texts: std::vec::Vec<&str> = std::vec![step.title()];
                    if step.cards()[0].1 == Choice::Edit {
                        texts.push("1000 W");
                    }
                    texts.extend(step.text());
                    texts.extend(step.cards().iter().map(|(label, _)| *label));
                    let what = std::format!("{step:?} {cursor} on {:?}", layout.scaled.panel);
                    let page = Rect {
                        bottom: regions.cards[1].bottom,
                        ..layout.chrome.content
                    };
                    view.canvas().assert_laid_out(page, &texts, &what);
                    let card = regions.cards[cursor];
                    let x = card.center_x();
                    assert_eq!(target(regions, x, card.top + 5, step), Some(cursor));
                }
            }
            let first = regions.cards[0];
            assert_eq!(target(regions, 100, first.top - 2, Step::Welcome), None);
            let gap = regions.cards[1].top - 1;
            assert_eq!(target(regions, 100, gap, Step::Welcome), None);
        }
    }
}
