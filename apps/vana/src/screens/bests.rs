//! BESTS: the rider's all-time bests, one line each with its value and the
//! day it was set. Opened from Settings, PROFILE; BACK returns there. ACT asks
//! CLEAR BESTS?, which ACT (check cue) confirms and BACK (back cue) cancels.
//! Where bests cannot be kept, the page says so instead of listing them.
//!
//! Power over 5 s, 1, 5 and 20 min comes first in `POWER`, then the longest
//! ride by distance and by time and the most ascent. A best not set yet
//! shows `--` in `TERTIARY`, and a best set before the clock was known shows
//! no day.
use super::canvas::Canvas;
use super::chrome::{
    AMBER, Align, BODY, CAPTION, CONTENT, GRADE, POWER, SECONDARY, SPEED, TERTIARY, TITLE, WHITE,
    WIDTH,
};
use super::cue::{Corner, Cue, Icon};
use super::history::date;
use super::ride::summary::effort_label;
use super::ride::{elapsed, kilometres};
use super::workout::{Text, View};
use crate::ride::bests::Mark;
use core::fmt::Write;

const LEFT: usize = 16;
const RIGHT: usize = WIDTH - 16;
/// The room a value has; the day beside it on the left is shorter.
const VALUE_WIDTH: usize = 100;
/// W/kg sits on the label's line, left of the value, which is below it. A
/// four-digit value starts 68 px in from the right edge, so 12 px stay clear.
const PER_KG_WIDTH: usize = 80;
const FIRST: usize = CONTENT.top + 60;
const PITCH: usize = 32;

/// What the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    List,
    /// ACT asked to clear them.
    Confirm,
    /// There is no place to keep bests.
    Unavailable,
    /// The file cannot be read; ACT asks to start over.
    Damaged,
    /// The store would not take the bests; they are in memory only.
    NotSaved,
}

/// BACK keeps the bests, ACT clears them.
pub const CONFIRM_CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Back),
    Cue::new(Corner::BottomRight, Icon::Check),
];

fn centred(
    canvas: &mut Canvas,
    faces: &[&'static firmware_shell::rendering::font::Face],
    text: &[u8],
    base: usize,
    color: u16,
) {
    canvas.fit(
        faces,
        text,
        RIGHT - LEFT,
        WIDTH / 2,
        base,
        Align::Center,
        color,
    );
}

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    match view.bests_mode {
        Mode::Confirm => {
            centred(&mut canvas, &[&TITLE, &BODY], b"CLEAR BESTS?", 150, WHITE);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"THEY CANNOT COME BACK",
                178,
                SECONDARY,
            );
            return canvas;
        }
        Mode::Unavailable => {
            centred(&mut canvas, &[&BODY], b"BESTS NOT KEPT", 150, AMBER);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"NO PLACE TO KEEP THEM",
                174,
                SECONDARY,
            );
            return canvas;
        }
        Mode::Damaged => {
            centred(&mut canvas, &[&BODY], b"BESTS FILE DAMAGED", 150, AMBER);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"IT CANNOT BE READ",
                174,
                SECONDARY,
            );
            return canvas;
        }
        Mode::NotSaved => {
            centred(&mut canvas, &[&BODY], b"BESTS NOT SAVED", 150, AMBER);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"STORAGE REFUSED THEM",
                174,
                SECONDARY,
            );
            return canvas;
        }
        Mode::List => {}
    }
    canvas.fit(
        &[&BODY],
        b"BESTS",
        RIGHT - LEFT,
        LEFT,
        CONTENT.top + 24,
        Align::Left,
        WHITE,
    );
    let bests = &view.bests;
    for (index, mark) in bests.power.iter().enumerate() {
        let mut value = Text::<12>::new();
        let _ = write!(value, "{} W", mark.value);
        if mark.value != 0
            && let Some(tenths) = view.rider.watts_per_kg_tenths(mark.value)
        {
            let mut per_kg = Text::<12>::new();
            let _ = write!(per_kg, "{}.{} W/KG", tenths / 10, tenths % 10);
            canvas.caption(
                per_kg.as_bytes(),
                PER_KG_WIDTH,
                RIGHT - PER_KG_WIDTH,
                FIRST + index * PITCH - 14,
                Align::Right,
            );
        }
        line(
            &mut canvas,
            index,
            effort_label(index).as_bytes(),
            &value,
            *mark,
            POWER,
        );
    }
    let mut distance = Text::<12>::new();
    kilometres(&mut distance, u64::from(bests.distance_m.value) * 1000);
    let _ = distance.push_str(" KM");
    let mut time = Text::<12>::new();
    elapsed(&mut time, u64::from(bests.time_s.value));
    let mut ascent = Text::<12>::new();
    let _ = write!(ascent, "{} M", bests.ascent_m.value);
    let rows = [
        (
            b"LONGEST RIDE".as_slice(),
            &distance,
            bests.distance_m,
            SPEED,
        ),
        (b"LONGEST TIME", &time, bests.time_s, SPEED),
        (b"MOST ASCENT", &ascent, bests.ascent_m, GRADE),
    ];
    for (offset, (label, value, mark, color)) in rows.into_iter().enumerate() {
        line(&mut canvas, 4 + offset, label, value, mark, color);
    }
    canvas
}

/// One best: its caption over the day it was set on the left, its value in
/// the metric's colour on the right.
fn line(canvas: &mut Canvas, row: usize, label: &[u8], value: &Text<12>, mark: Mark, color: u16) {
    let base = FIRST + row * PITCH;
    canvas.caption(label, RIGHT - LEFT, LEFT, base - 14, Align::Left);
    if mark.value == 0 {
        canvas.fit(&[&BODY], b"--", 60, RIGHT, base - 3, Align::Right, TERTIARY);
        return;
    }
    canvas.fit(
        &[&BODY, &CAPTION],
        value.as_bytes(),
        VALUE_WIDTH,
        RIGHT,
        base - 3,
        Align::Right,
        color,
    );
    if mark.day != 0 {
        let day = date(mark.day);
        canvas.caption(
            day.as_bytes(),
            RIGHT - LEFT - VALUE_WIDTH,
            LEFT,
            base,
            Align::Left,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::bests::Bests;
    use crate::screens::chrome::{HEIGHT, Rect};
    use crate::screens::workout::Page;

    fn view(bests: Bests) -> View<'static> {
        let mut view = View::new(Page::Bests, 0, None);
        view.bests = bests;
        view.prepare();
        view
    }

    fn mark(value: u32, day: u16) -> Mark {
        Mark { value, day }
    }

    #[test]
    fn bests_list_each_value_with_its_day() {
        let bests = Bests {
            power: [
                mark(1999, 20_000),
                mark(450, 20_001),
                mark(312, 20_020),
                mark(255, 20_021),
            ],
            distance_m: mark(123_400, 20_022),
            time_s: mark(5 * 3600 + 20 * 60, 20_023),
            ascent_m: mark(1_850, 20_024),
        };
        let view = view(bests);
        assert!(view.cues.is_empty());
        let full = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT,
        };
        let want = [
            "BESTS",
            "5 S",
            "20 MIN",
            "1999 W",
            "312 W",
            "123.4 KM",
            "5:20:00",
            "1850 M",
            "OCT 4 2024",
            "OCT 28 2024",
        ];
        view.canvas.assert_laid_out(full, &want, "bests");
    }

    #[test]
    fn power_bests_show_watts_per_kilogram_only_once_the_weight_is_set() {
        let bests = Bests {
            power: [
                mark(1999, 20_000),
                mark(450, 20_001),
                mark(312, 20_020),
                Mark::default(),
            ],
            ..Bests::default()
        };
        let mut view = View::new(Page::Bests, 0, None);
        view.bests = bests;
        view.prepare();
        assert!(!view.canvas.shows("W/KG"));
        view.rider = crate::ride::rider::Rider::default().step_weight(1, true);
        view.prepare();
        let full = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT,
        };
        view.canvas.assert_laid_out(
            full,
            &["28.6 W/KG", "6.4 W/KG", "4.5 W/KG", "1999 W", "312 W"],
            "bests with weight",
        );
    }

    #[test]
    fn clearing_asks_first_and_missing_storage_says_so() {
        let full = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT,
        };
        let mut confirm = View::new(Page::Bests, 0, None);
        confirm.bests_mode = Mode::Confirm;
        confirm.prepare();
        confirm
            .canvas
            .assert_laid_out(full, &["CLEAR BESTS?", "THEY CANNOT COME BACK"], "confirm");
        assert_eq!(confirm.cues.len(), 2);
        let mut none = View::new(Page::Bests, 0, None);
        none.bests_mode = Mode::Unavailable;
        none.prepare();
        none.canvas
            .assert_laid_out(full, &["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"], "none");
        assert!(none.cues.is_empty());
        for (mode, want) in [
            (Mode::Damaged, ["BESTS FILE DAMAGED", "IT CANNOT BE READ"]),
            (Mode::NotSaved, ["BESTS NOT SAVED", "STORAGE REFUSED THEM"]),
        ] {
            let mut view = View::new(Page::Bests, 0, None);
            view.bests_mode = mode;
            view.prepare();
            view.canvas.assert_laid_out(full, &want, "state");
        }
    }

    #[test]
    fn a_best_not_set_yet_says_so_and_a_missing_day_shows_none() {
        let empty = view(Bests::default());
        assert!(empty.canvas.shows("--"));
        assert!(!empty.canvas.shows(" W"));
        let undated = view(Bests {
            power: [
                mark(300, 0),
                Mark::default(),
                Mark::default(),
                Mark::default(),
            ],
            ..Bests::default()
        });
        assert!(undated.canvas.shows("300 W"));
        assert!(!undated.canvas.shows("1970"));
    }
}
