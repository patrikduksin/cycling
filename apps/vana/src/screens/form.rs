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
use super::bests::{Message, centred, message_of};
use super::canvas::{Canvas, Shape};
use super::chrome::{Align, GREEN, POWER, RED, Rect, SECONDARY, TERTIARY, WHITE};
use super::scaled::{Scaled, Track};
use super::workout::{Text, View};
use crate::ride::fitness::{Form, TREND};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

const SCALE: i32 = 50;

/// FORM on one panel: the title, three bands of readings, then the trend
/// of the last weeks, its ends named under it and the note under those.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// FORM, its capitals ending on the bottom.
    pub title: Rect,
    /// FITNESS, FATIGUE and FORM: each caption, then its value, whose
    /// capitals end on the bottom.
    pub bands: [Rect; 3],
    pub value: Rect,
    /// What form means today, beside the FORM caption.
    pub word: Rect,
    /// The trend: bars stand up from `zero` into `rested` and hang into
    /// `tired`, a form of `SCALE` reaching across either. On a taller panel
    /// the trend takes the spare rows.
    pub rested: Rect,
    pub zero: Rect,
    pub tired: Rect,
    /// One column per week, oldest first.
    pub weeks: [Rect; TREND],
    /// "6 WEEKS AGO" and "NOW" end on its bottom.
    pub ends: Rect,
    /// That rides without power are left out, on two lines.
    pub note: [Rect; 2],
    pub message: Message,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let text = super::bests::columns(at);
    let band = Unit(32);
    let [
        title,
        _,
        a,
        b,
        c,
        _,
        rested,
        zero,
        _,
        tired,
        ends,
        first,
        second,
        _,
    ] = text.rows(
        [
            Unit(24),
            Unit(4),
            band,
            band,
            band,
            Unit(28),
            Fr(1),
            Unit(1),
            Unit(1),
            Fr(1),
            Unit(22),
            Unit(20),
            Unit(14),
            Unit(14),
        ],
        Unit(0),
        at,
    );
    let [_, word, _] = text.cols([Unit(70), Unit(100), Fr(1)], Unit(0), at);
    let content = super::chrome::regions(at).content;
    let wide = Unit(24);
    let gap = Unit(35 - 24);
    let [_, w0, _, w1, _, w2, _, w3, _, w4, _, w5, _] = content.cols(
        [
            Fr(1),
            wide,
            gap,
            wide,
            gap,
            wide,
            gap,
            wide,
            gap,
            wide,
            gap,
            wide,
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let note = super::bests::message(at).line;
    Regions {
        title,
        bands: [a, b, c],
        value: super::bests::value_column(at),
        word,
        rested,
        zero,
        tired,
        weeks: [w0, w1, w2, w3, w4, w5],
        ends,
        note: [first.with_columns(note), second.with_columns(note)],
        message: super::bests::message(at),
    }
}

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
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.form);
    let face = |role| at.face(role);
    let (body, caption) = (face(FaceRole::Body), face(FaceRole::Caption));
    let mut canvas = Canvas::default();
    let mut say = |title: &[u8], line: &[u8]| {
        message_of(&mut canvas, at, &regions.message, (title, WHITE), line);
    };
    let (form, left_out) = match view.form() {
        Mode::Form { form, left_out } => (form, left_out),
        Mode::Reading => {
            say(b"READING RIDES", b"FORM SHOWS WHEN DONE");
            return canvas;
        }
        Mode::NoClock => {
            say(b"NO DATE YET", b"GET A GPS FIX");
            return canvas;
        }
        Mode::NoPower => {
            say(b"NO POWER RIDES", b"NEEDS A POWER METER");
            return canvas;
        }
    };
    let title = regions.title;
    canvas.fit(
        &[body],
        b"FORM",
        title.width(),
        title.left,
        title.bottom,
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
    let value = regions.value;
    for ((label, text, color), band) in rows.into_iter().zip(regions.bands) {
        let base = band.bottom;
        let caption_base = base - at.unit(14);
        canvas.caption(
            at,
            label,
            band.width(),
            band.left,
            caption_base,
            Align::Left,
        );
        canvas.fit(
            &[body, caption],
            text.as_bytes(),
            value.width(),
            value.right,
            base - at.unit(3),
            Align::Right,
            color,
        );
    }
    let column = regions.word;
    canvas.fit(
        &[caption],
        word.as_bytes(),
        column.width(),
        column.left,
        regions.bands[2].bottom - at.unit(14),
        Align::Left,
        tone,
    );
    canvas.shape(Shape::Bar {
        rect: regions.zero,
        radius: 0,
        color: TERTIARY,
    });
    let reach = regions.rested.height();
    let radius = at.unit(2);
    for (&value, week) in form.trend.iter().zip(regions.weeks) {
        let high = (i32::from(value).abs().min(SCALE) as usize * reach / SCALE as usize).max(2);
        let (top, bottom) = if value >= 0 {
            (regions.rested.bottom - high, regions.rested.bottom)
        } else {
            (regions.tired.top, regions.tired.top + high)
        };
        canvas.shape(Shape::Bar {
            rect: Rect {
                top,
                bottom,
                ..week
            },
            radius,
            color: verdict(value).1,
        });
    }
    let ends = regions.ends;
    canvas.caption(
        at,
        b"6 WEEKS AGO",
        ends.width(),
        ends.left,
        ends.bottom,
        Align::Left,
    );
    let now = at.unit(60);
    canvas.caption(at, b"NOW", now, ends.right, ends.bottom, Align::Right);
    if left_out {
        for (text, row) in [&b"RIDES WITHOUT POWER"[..], b"ARE LEFT OUT"]
            .into_iter()
            .zip(regions.note)
        {
            centred(&mut canvas, &[caption], text, row, SECONDARY);
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;
    use crate::screens::{BASE, Layout, panels};

    const R: Regions = BASE.form;

    fn shown(layout: &'static Layout, mode: Mode) -> View<'static> {
        let mut view = View::new(Page::Form, 0, None);
        view.layout = layout;
        view.show_form(mode);
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
        let bars: std::vec::Vec<Rect> = view
            .canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar {
                    rect, radius: 2, ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        let (zero, reach) = (R.zero.top, R.rested.height());
        assert_eq!((zero, reach), (212, 36));
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
                .all(|b| b.left >= R.zero.left && b.right <= R.zero.right)
        );
    }
}
