//! First-run setup: a few pages that ask for what VANA cannot guess, each
//! with a short explanation and its choices as cards.
//!
//! Buttons keep their system roles: MOVE focuses the next card, ACT takes it
//! and BACK leaves setup for Home (it resumes at the same page on the next
//! start, so a ride never waits for it). Touch taps a card. Numbers open the
//! Settings editor and come back here when it finishes.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, Align, BODY, CAPTION, CARD, Line, RAISED, Rect, SECONDARY, TITLE, WHITE,
};
use super::workout::View;

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

const RADIUS: usize = 12;
const LEFT: usize = 10;
const RIGHT: usize = 230;
const INSET: usize = 16;
const TITLE_BASE: usize = 70;
const TEXT_BASE: usize = 100;
const TEXT_PITCH: usize = 20;
const CARD_TOP: usize = 150;
const CARD_HEIGHT: usize = 52;
const CARD_PITCH: usize = 60;
const FOCUS: usize = 3;

fn card_rect(slot: usize) -> Rect {
    let top = CARD_TOP + slot * CARD_PITCH;
    Rect {
        left: LEFT,
        top,
        right: RIGHT,
        bottom: top + CARD_HEIGHT,
    }
}

/// The card under a touch at panel point (`x`, `y`) on `step`.
pub fn target(x: usize, y: usize, step: Step) -> Option<usize> {
    if !(LEFT..RIGHT).contains(&x) {
        return None;
    }
    (0..step.cards().len()).find(|slot| {
        let rect = card_rect(*slot);
        (rect.top..rect.bottom).contains(&y)
    })
}

/// The page for `step` with `view.cursor` focused. `view.lines[slot]` is a
/// note at the right of each card, such as the value a number has now.
pub(super) fn prepare(view: &View, step: Step) -> Canvas {
    let mut canvas = Canvas::default();
    canvas.fit(
        &[&TITLE],
        step.title().as_bytes(),
        RIGHT - LEFT,
        LEFT,
        TITLE_BASE,
        Align::Left,
        WHITE,
    );
    for (i, line) in step.text().iter().enumerate() {
        canvas.caption(
            line.as_bytes(),
            RIGHT - LEFT,
            LEFT,
            TEXT_BASE + i * TEXT_PITCH,
            Align::Left,
        );
    }
    for (slot, (label, _)) in step.cards().iter().enumerate() {
        let rect = card_rect(slot);
        let focused = slot == view.cursor;
        if focused {
            canvas.shape(Shape::Panel {
                rect,
                radius: RADIUS,
                color: ACCENT,
            });
        }
        canvas.shape(Shape::Panel {
            rect: if focused { rect.inset(FOCUS) } else { rect },
            radius: if focused { RADIUS - FOCUS } else { RADIUS },
            color: if focused { RAISED } else { CARD },
        });
        let base = rect.top + (CARD_HEIGHT + BODY.cap as usize) / 2;
        let title = canvas.fit(
            &[&BODY],
            label.as_bytes(),
            RIGHT - LEFT - 2 * INSET,
            LEFT + INSET,
            base,
            Align::Left,
            WHITE,
        );
        let note = view.lines()[slot].as_bytes();
        let room = (RIGHT - INSET).saturating_sub(title.right() + 12);
        if !note.is_empty() && Line::<24>::fit(&[&CAPTION], note, room).text().len() == note.len() {
            canvas.fit(
                &[&CAPTION],
                note,
                room,
                RIGHT - INSET,
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
        let screen = Rect {
            left: 0,
            top: 24,
            right: 240,
            bottom: 280,
        };
        for step in STEPS {
            for cursor in 0..step.cards().len() {
                let mut view = View::new(Page::Setup, cursor, None);
                let _ = view.show_setup(step)[0].push_str("1000 W");
                view.prepare();
                let canvas = prepare(&view, step);
                let mut texts: std::vec::Vec<&str> = std::vec![step.title()];
                texts.extend(step.text());
                texts.extend(step.cards().iter().map(|(label, _)| *label));
                canvas.assert_laid_out(screen, &texts, &std::format!("{step:?} {cursor}"));
                assert_eq!(target(100, card_rect(cursor).top + 5, step), Some(cursor));
            }
        }
        assert_eq!(target(100, CARD_TOP - 2, Step::Welcome), None);
    }
}
