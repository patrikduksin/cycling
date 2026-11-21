//! FORM: fitness, fatigue and form from the last four months of rides with
//! power. Opened from Settings, PROFILE; BACK returns there.
//!
//! FITNESS and FATIGUE are training stress in `POWER`, the 42-day and 7-day
//! averages (`ride::fitness`). FORM is fitness minus fatigue, with a word for
//! what it means today beside the caption. Only the two ends of the range
//! draw the eye: green for FRESH, red for VERY TIRED; the rest of the normal
//! training range stays white. Under them one bar a week shows how form stood
//! at the end of each of the last six weeks, up for rested and down for
//! tired, on a baseline at zero. When some rides had no power the page says
//! they are left out. While the ride index is still reading, or when there is
//! no date or no ride with power, it says that instead.
use super::chrome::{GREEN, RED, WHITE};
use super::workout::Text;
use crate::ride::fitness::Form;
use core::fmt::Write;

/// What the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The ride index has not read every ride yet.
    #[default]
    Reading,
    /// No date, so no ride can be placed on a day.
    NoClock,
    /// No ride with power in the window.
    NoPower,
    Form {
        form: Form,
        /// Some rides in the window had no power, so they do not count.
        left_out: bool,
    },
}

impl Mode {
    pub const fn asks(self) -> crate::compose::builtin::Asks {
        match self {
            Self::Form { .. } => crate::compose::builtin::Asks::Own,
            _ => crate::compose::builtin::Asks::Note,
        }
    }
}

/// Why FORM does not show, and what it waits for.
pub const NOTES: [[&str; 2]; 3] = [
    ["READING RIDES", "FORM SHOWS WHEN DONE"],
    ["NO DATE YET", "GET A GPS FIX"],
    ["NO POWER RIDES", "NEEDS A POWER METER"],
];

/// The note `mode` shows, as its place in `NOTES`.
pub const fn note(mode: Mode) -> Option<u8> {
    match mode {
        Mode::Reading => Some(0),
        Mode::NoClock => Some(1),
        Mode::NoPower => Some(2),
        Mode::Form { .. } => None,
    }
}

/// What form means today, and its colour.
pub fn verdict(form: i16) -> (&'static str, u16) {
    match form {
        26.. => ("RESTED", WHITE),
        5..=25 => ("FRESH", GREEN),
        -10..=4 => ("EVEN", WHITE),
        -30..=-11 => ("BUILDING", WHITE),
        _ => ("VERY TIRED", RED),
    }
}

pub fn signed(text: &mut Text<12>, value: i16) {
    let _ = if value > 0 {
        write!(text, "+{value}")
    } else {
        write!(text, "{value}")
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::fitness::TREND;
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::{Rect, TERTIARY};
    use crate::screens::workout::{Page, View};
    use crate::screens::{BASE, Layout, panels};

    fn shown(layout: &'static Layout, mode: Mode) -> View<'static> {
        let mut view = View::new(Page::Form, 0, None);
        view.layout = layout;
        view.show_form(mode);
        view.compose_main();
        view.prepare();
        view
    }

    fn view(mode: Mode) -> View<'static> {
        shown(&BASE, mode)
    }

    fn full() -> Rect {
        BASE.chrome.content
    }

    #[test]
    fn the_page_names_each_number_and_what_form_means() {
        let view = view(Mode::Form {
            form: Form {
                fitness: 57,
                fatigue: 71,
                form: -14,
                trend: [3, 8, -6, -20, -9, -14],
            },
            left_out: true,
        });
        assert!(view.cues.is_empty());
        view.canvas().assert_laid_out(
            full(),
            &[
                "FORM",
                "FITNESS",
                "FATIGUE",
                "57",
                "71",
                "-14",
                "BUILDING",
                "6 WEEKS AGO",
                "NOW",
                "RIDES WITHOUT POWER",
                "ARE LEFT OUT",
            ],
            "form",
        );
    }

    #[test]
    fn the_view_carries_a_small_mode() {
        assert_eq!(core::mem::size_of::<Mode>(), 20);
    }

    #[test]
    fn the_left_out_note_shows_only_when_rides_were_left_out() {
        let page = |left_out| {
            view(Mode::Form {
                form: Form::default(),
                left_out,
            })
        };
        assert!(page(true).canvas().shows("ARE LEFT OUT"));
        assert!(!page(false).canvas().shows("LEFT OUT"));
    }

    #[test]
    fn form_reads_by_its_band() {
        assert_eq!(verdict(26), ("RESTED", WHITE));
        assert_eq!(verdict(25), ("FRESH", GREEN));
        assert_eq!(verdict(5), ("FRESH", GREEN));
        assert_eq!(verdict(4), ("EVEN", WHITE));
        assert_eq!(verdict(-10), ("EVEN", WHITE));
        assert_eq!(verdict(-11), ("BUILDING", WHITE));
        assert_eq!(verdict(-30), ("BUILDING", WHITE));
        assert_eq!(verdict(-31), ("VERY TIRED", RED));
        let fresh = view(Mode::Form {
            form: Form {
                form: 9,
                ..Form::default()
            },
            left_out: false,
        });
        assert!(fresh.canvas().shows("+9"));
        assert!(fresh.canvas().shows("FRESH"));
    }

    #[test]
    fn waiting_and_empty_pages_say_why_and_show_no_numbers() {
        for (mode, want) in [
            (Mode::Reading, ["READING RIDES", "FORM SHOWS WHEN DONE"]),
            (Mode::NoClock, ["NO DATE YET", "GET A GPS FIX"]),
            (Mode::NoPower, ["NO POWER RIDES", "NEEDS A POWER METER"]),
        ] {
            let view = view(mode);
            view.canvas().assert_laid_out(full(), &want, "state");
            assert!(!view.canvas().shows("FITNESS"));
        }
    }

    #[test]
    fn the_widest_form_and_every_waiting_page_lay_out_whole() {
        let widest = Mode::Form {
            form: Form {
                fitness: 999,
                fatigue: 999,
                form: -999,
                trend: [i16::MIN, i16::MAX, -50, 50, -1, 0],
            },
            left_out: true,
        };
        let waiting = [
            (Mode::Reading, ["READING RIDES", "FORM SHOWS WHEN DONE"]),
            (Mode::NoClock, ["NO DATE YET", "GET A GPS FIX"]),
            (Mode::NoPower, ["NO POWER RIDES", "NEEDS A POWER METER"]),
        ];
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let what = std::format!("{:?}", layout.scaled);
            let mut view = shown(layout, widest);
            view.prepare().check_layout();
            view.canvas().assert_laid_out(
                layout.chrome.content,
                &[
                    "FITNESS",
                    "999",
                    "-999",
                    "VERY TIRED",
                    "NOW",
                    "ARE LEFT OUT",
                ],
                &what,
            );
            for (mode, want) in waiting {
                let mut view = shown(layout, mode);
                view.prepare().check_layout();
                view.canvas()
                    .assert_laid_out(layout.chrome.content, &want, &what);
            }
        }
    }

    #[test]
    fn trend_bars_stand_on_the_zero_line_by_sign_and_stop_at_the_scale() {
        let view = view(Mode::Form {
            form: Form {
                trend: [i16::MAX, 0, i16::MIN, 25, -25, 5],
                ..Form::default()
            },
            left_out: false,
        });
        let shapes = &view.canvas().shapes;
        let bars: std::vec::Vec<Rect> = shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar {
                    rect, radius: 2, ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        let line = shapes.iter().find_map(|shape| match shape {
            Shape::Bar {
                rect,
                radius: 0,
                color: TERTIARY,
            } => Some(*rect),
            _ => None,
        });
        let line = line.expect("the zero line");
        let zero = line.top;
        let reach = bars[0].bottom - bars[0].top;
        assert!(reach > 30, "{reach}: the trend takes the spare rows");
        assert_eq!(bars.len(), TREND);
        assert_eq!(bars[0].top, zero - reach);
        assert_eq!(bars[0].bottom, zero);
        assert_eq!(bars[1].bottom - bars[1].top, 2);
        assert_eq!(bars[2].top, zero + 2);
        assert_eq!(bars[2].bottom, zero + 2 + reach);
        assert_eq!(bars[3].bottom - bars[3].top, reach / 2);
        assert!(bars[4].top > zero);
        assert!(
            bars.iter()
                .all(|b| b.left >= line.left && b.right <= line.right)
        );
    }
}
