//! First-run setup: a few pages that ask for what VANA cannot guess, each
//! with a short explanation and its choices as cards.
//!
//! Buttons keep their system roles: MOVE focuses the next card, ACT takes it
//! and BACK leaves setup for Home (it resumes at the same page on the next
//! start, so a ride never waits for it). Touch taps a card. Numbers open the
//! Settings editor and come back here when it finishes. The Setup screen's
//! tree (`compose::builtin::setup`) lays a step out.

use crate::ride::zones::TrainingProfile;
use crate::screens::workout::Text;
use core::fmt::Write;

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

/// Cards a step shows.
pub const CARDS: usize = 2;

/// The note beside a step's first card: the number it sets, or how many
/// sensors are saved.
pub fn note(step: Step, profile: TrainingProfile, sensors: usize) -> Text<38> {
    let mut note = Text::new();
    let _ = match step {
        Step::Power => write!(note, "{} W", profile.ftp_watts),
        Step::Heart => write!(note, "{} BPM", profile.lthr_bpm),
        Step::Sensors if sensors > 0 => write!(note, "{sensors} SAVED"),
        Step::Welcome | Step::Sensors | Step::Done => Ok(()),
    };
    note
}

/// `note` on a narrow card: the sensors' count without SAVED, the others
/// whole.
pub fn narrow(step: Step, note: &[u8]) -> &[u8] {
    match step {
        Step::Sensors => note.split(|&byte| byte == b' ').next().unwrap_or(note),
        _ => note,
    }
}

/// The widest `note` writes for `step`: each number at its end.
pub fn widest_note(step: Step) -> Text<38> {
    let profile = TrainingProfile {
        ftp_watts: *TrainingProfile::FTP_WATTS.end(),
        lthr_bpm: *TrainingProfile::LTHR_BPM.end(),
    };
    note(step, profile, device_api::ant::CHANNEL_CAPACITY)
}

const _: () = {
    let mut i = 0;
    while i < STEPS.len() {
        assert!(
            STEPS[i].cards().len() <= CARDS,
            "a setup page has more cards than its tree"
        );
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::ScreenId;
    use crate::compose::input::{Action, OnPage, hit};
    use crate::compose::registry::CHOICE;
    use crate::screens::chrome::{RAISED, Rect};
    use crate::screens::workout::{Page, View};

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
    fn every_page_lays_out_whole_with_notes_and_taps_find_cards() {
        crate::screens::check::enable(true);
        for layout in crate::screens::panels::ALL {
            for step in STEPS {
                let mut view = View::new(Page::Setup, 0, None);
                view.layout = layout;
                view.compose_main();
                let _ = view.show_setup(step)[0].push_str("1000 W");
                view.prepare().check_layout();
                let mut texts: std::vec::Vec<&str> = std::vec![step.title()];
                if step.cards()[0].1 == Choice::Edit {
                    texts.push("1000 W");
                }
                texts.extend(step.text());
                texts.extend(step.cards().iter().map(|(label, _)| *label));
                let what = std::format!("{step:?} on {:?}", layout.scaled.panel);
                let page = Rect {
                    bottom: layout.scaled.panel.height,
                    ..layout.chrome.content
                };
                view.canvas().assert_laid_out(page, &texts, &what);
                let laid = view
                    .laids
                    .and_then(|laids| laids.get(ScreenId::Setup))
                    .unwrap();
                let first = laid.box_of(CHOICE).unwrap();
                let inside = (first.left + 8, (first.top + first.bottom) / 2);
                assert_eq!(view.pixel(inside.0, inside.1), RAISED, "{what}");
                let solved = laid.solved();
                for (slot, placed) in solved.focus_order().enumerate() {
                    let rect = placed.rect;
                    let (x, y) = (rect.left + rect.width() / 2, rect.top + 5);
                    let (_, action) = hit(laid.buttons(), &solved, x, y).unwrap();
                    assert_eq!(action, Action::Page(OnPage::Item(slot as u8)), "{what}");
                }
                let gap = first.bottom as u16 + 1;
                assert!(hit(laid.buttons(), &solved, 100, gap).is_none(), "{what}");
            }
        }
    }
}
