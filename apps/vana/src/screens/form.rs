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
use super::canvas::{Canvas, Shape};
use super::chrome::{
    Align, BODY, CAPTION, CONTENT, GREEN, POWER, RED, Rect, SECONDARY, TERTIARY, WHITE, WIDTH,
};
use super::workout::{Text, View};
use crate::ride::fitness::{Form, TREND};
use core::fmt::Write;

const LEFT: usize = 16;
const RIGHT: usize = WIDTH - 16;
const FIRST: usize = CONTENT.top + 60;
const PITCH: usize = 32;
/// The zero line of the trend, and the bar height a form of `SCALE` reaches.
const ZERO: usize = 212;
const REACH: usize = 36;
const SCALE: i32 = 50;
const BAR_WIDE: usize = 24;
const BAR_PITCH: usize = 35;
const BARS_LEFT: usize = (WIDTH - (TREND - 1) * BAR_PITCH - BAR_WIDE) / 2;
const ENDS_BASE: usize = 272;
const NOTE_BASE: usize = 292;

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

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let (form, left_out) = match view.form {
        Mode::Form { form, left_out } => (form, left_out),
        Mode::Reading => {
            super::bests::centred(&mut canvas, &[&BODY], b"READING RIDES", 150, WHITE);
            super::bests::centred(
                &mut canvas,
                &[&CAPTION],
                b"FORM SHOWS WHEN DONE",
                174,
                SECONDARY,
            );
            return canvas;
        }
        Mode::NoClock => {
            super::bests::centred(&mut canvas, &[&BODY], b"NO DATE YET", 150, WHITE);
            super::bests::centred(&mut canvas, &[&CAPTION], b"GET A GPS FIX", 174, SECONDARY);
            return canvas;
        }
        Mode::NoPower => {
            super::bests::centred(&mut canvas, &[&BODY], b"NO POWER RIDES", 150, WHITE);
            super::bests::centred(
                &mut canvas,
                &[&CAPTION],
                b"NEEDS A POWER METER",
                174,
                SECONDARY,
            );
            return canvas;
        }
    };
    canvas.fit(
        &[&BODY],
        b"FORM",
        RIGHT - LEFT,
        LEFT,
        CONTENT.top + 24,
        Align::Left,
        WHITE,
    );
    let (word, tone) = verdict(form.form);
    let mut signed_form = Text::<12>::new();
    signed(&mut signed_form, form.form);
    let mut fitness = Text::<12>::new();
    let _ = write!(fitness, "{}", form.fitness);
    let mut fatigue = Text::<12>::new();
    let _ = write!(fatigue, "{}", form.fatigue);
    let rows = [
        (b"FITNESS".as_slice(), &fitness, POWER),
        (b"FATIGUE", &fatigue, POWER),
        (b"FORM", &signed_form, tone),
    ];
    for (row, (label, value, color)) in rows.into_iter().enumerate() {
        let base = FIRST + row * PITCH;
        canvas.caption(label, RIGHT - LEFT, LEFT, base - 14, Align::Left);
        canvas.fit(
            &[&BODY, &CAPTION],
            value.as_bytes(),
            100,
            RIGHT,
            base - 3,
            Align::Right,
            color,
        );
    }
    canvas.fit(
        &[&CAPTION],
        word.as_bytes(),
        100,
        LEFT + 70,
        FIRST + 2 * PITCH - 14,
        Align::Left,
        tone,
    );
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: LEFT,
            top: ZERO,
            right: RIGHT,
            bottom: ZERO + 1,
        },
        radius: 0,
        color: TERTIARY,
    });
    for (week, &value) in form.trend.iter().enumerate() {
        let high = (i32::from(value).abs().min(SCALE) as usize * REACH / SCALE as usize).max(2);
        let left = BARS_LEFT + week * BAR_PITCH;
        let (top, bottom) = if value >= 0 {
            (ZERO - high, ZERO)
        } else {
            (ZERO + 2, ZERO + 2 + high)
        };
        canvas.shape(Shape::Bar {
            rect: Rect {
                left,
                top,
                right: left + BAR_WIDE,
                bottom,
            },
            radius: 2,
            color: verdict(value).1,
        });
    }
    canvas.caption(b"6 WEEKS AGO", RIGHT - LEFT, LEFT, ENDS_BASE, Align::Left);
    canvas.caption(b"NOW", 60, RIGHT, ENDS_BASE, Align::Right);
    let note: &[&[u8]] = if left_out {
        &[b"RIDES WITHOUT POWER", b"ARE LEFT OUT"]
    } else {
        &[]
    };
    for (line, text) in note.iter().enumerate() {
        canvas.caption(
            text,
            RIGHT - LEFT,
            WIDTH / 2,
            NOTE_BASE + line * 14,
            Align::Center,
        );
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::HEIGHT;
    use crate::screens::workout::Page;

    fn view(mode: Mode) -> View<'static> {
        let mut view = View::new(Page::Form, 0, None);
        view.form = mode;
        view.prepare();
        view
    }

    fn full() -> Rect {
        Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT,
        }
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
        view.canvas.assert_laid_out(
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
        assert!(page(true).canvas.shows("ARE LEFT OUT"));
        assert!(!page(false).canvas.shows("LEFT OUT"));
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
        assert!(fresh.canvas.shows("+9"));
        assert!(fresh.canvas.shows("FRESH"));
    }

    #[test]
    fn waiting_and_empty_pages_say_why_and_show_no_numbers() {
        for (mode, want) in [
            (Mode::Reading, ["READING RIDES", "FORM SHOWS WHEN DONE"]),
            (Mode::NoClock, ["NO DATE YET", "GET A GPS FIX"]),
            (
                Mode::NoPower,
                ["NO POWER RIDES", "NEEDS A POWER METER"],
            ),
        ] {
            let view = view(mode);
            view.canvas.assert_laid_out(full(), &want, "state");
            assert!(!view.canvas.shows("FITNESS"));
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
        let bars: std::vec::Vec<Rect> = view
            .canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar {
                    rect, radius: 2, ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(bars.len(), TREND);
        assert_eq!(bars[0].top, ZERO - REACH);
        assert_eq!(bars[0].bottom, ZERO);
        assert_eq!(bars[1].bottom - bars[1].top, 2);
        assert_eq!(bars[2].top, ZERO + 2);
        assert_eq!(bars[2].bottom, ZERO + 2 + REACH);
        assert_eq!(bars[3].bottom - bars[3].top, REACH / 2);
        assert!(bars[4].top > ZERO);
        assert!(bars.iter().all(|b| b.left >= LEFT && b.right <= RIGHT));
    }
}
