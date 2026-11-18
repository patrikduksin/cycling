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
use super::chrome::{AMBER, Align, GRADE, POWER, Rect, SECONDARY, SPEED, TERTIARY, WHITE};
use super::cue::{Corner, Cue, Icon};
use super::history::date;
use super::ride::summary::effort_label;
use super::ride::{elapsed, kilometres};
use super::scaled::{Scaled, Track};
use super::workout::{Text, View};
use crate::kept::bests::Mark;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole};

/// BESTS on one panel: the title, then one band per best under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// BESTS, its capitals ending on the bottom.
    pub title: Rect,
    /// One band per best: its caption, then its value and the day it was
    /// set, whose capitals end on the bottom.
    pub bands: [Rect; 7],
    /// The value's column on the right; the day beside it on the left is
    /// shorter.
    pub value: Rect,
    /// W/kg on the caption's line, left of the value, which is below it. A
    /// four-digit value starts 68 columns in from the right, so 12 stay
    /// clear.
    pub per_kg: Rect,
    /// What shows in place of the list.
    pub message: Message,
}

/// Two centred lines in place of a page, each with its capitals ending on
/// its region's bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Message {
    pub title: Rect,
    /// The caption under a title in the body face.
    pub line: Rect,
    /// The caption under a title in the taller title face.
    pub under_title: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let text = columns(at);
    let band = Unit(32);
    let [title, _, a, b, c, d, e, f, g, _] = text.rows(
        [
            Unit(24),
            Unit(4),
            band,
            band,
            band,
            band,
            band,
            band,
            band,
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, per_kg, _] = text.cols([Fr(1), Unit(80), Unit(80)], Unit(0), at);
    Regions {
        title,
        bands: [a, b, c, d, e, f, g],
        value: value_column(at),
        per_kg,
        message: message(at),
    }
}

pub(super) const fn columns(at: &Scaled) -> Rect {
    use Track::{Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, text, _] = content.cols([Unit(16), Fr(1), Unit(16)], Unit(0), at);
    text
}

pub(super) const fn value_column(at: &Scaled) -> Rect {
    use Track::{Fr, Unit};
    let [_, value] = columns(at).cols([Fr(1), Unit(100)], Unit(0), at);
    value
}

pub(super) const fn message(at: &Scaled) -> Message {
    use Track::{Fr, Unit};
    let [_, title, line, deeper, _, _] = columns(at).rows(
        [Fr(1), Unit(30), Unit(24), Unit(4), Unit(46), Fr(1)],
        Unit(0),
        at,
    );
    Message {
        title,
        line,
        under_title: Rect {
            bottom: deeper.bottom,
            ..line
        },
    }
}

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
    /// The file is of another version; only FORMAT replaces it.
    Foreign,
    /// The store would not take the bests; they are in memory only.
    NotSaved,
}

/// BACK keeps the bests, ACT clears them.
pub const CONFIRM_CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Back),
    Cue::new(Corner::BottomRight, Icon::Check),
];

pub(super) fn centred(
    canvas: &mut Canvas,
    faces: &[&'static Face],
    text: &[u8],
    row: Rect,
    color: u16,
) {
    canvas.fit(
        faces,
        text,
        row.width(),
        row.center_x(),
        row.bottom,
        Align::Center,
        color,
    );
}

pub(super) fn message_of(
    canvas: &mut Canvas,
    at: &Scaled,
    message: &Message,
    (title, color): (&[u8], u16),
    line: &[u8],
) {
    let face = |role| at.face(role);
    let faces = [face(FaceRole::Body), face(FaceRole::Caption)];
    centred(canvas, &faces, title, message.title, color);
    let caption = [face(FaceRole::Caption)];
    centred(canvas, &caption, line, message.line, SECONDARY);
}

pub fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.bests);
    let face = |role| at.face(role);
    let (body, caption) = (face(FaceRole::Body), face(FaceRole::Caption));
    let message = &regions.message;
    let mut canvas = Canvas::default();
    let message_shown: Option<(&[u8], &[u8])> = match view.bests_mode() {
        Mode::Confirm => {
            let faces = [face(FaceRole::Title), body];
            centred(&mut canvas, &faces, b"CLEAR BESTS?", message.title, WHITE);
            let line = b"THEY CANNOT COME BACK";
            centred(
                &mut canvas,
                &[caption],
                line,
                message.under_title,
                SECONDARY,
            );
            return canvas;
        }
        Mode::Foreign => {
            let faces = [body, caption];
            centred(
                &mut canvas,
                &faces,
                b"BESTS NOT FORMATTED",
                message.title,
                AMBER,
            );
            let line = b"OFF AND ON TO FORMAT";
            centred(&mut canvas, &[caption], line, message.line, SECONDARY);
            return canvas;
        }
        Mode::Unavailable => Some((b"BESTS NOT KEPT", b"NO PLACE TO KEEP THEM")),
        Mode::Damaged => Some((b"BESTS FILE DAMAGED", b"IT CANNOT BE READ")),
        Mode::NotSaved => Some((b"BESTS NOT SAVED", b"STORAGE REFUSED THEM")),
        Mode::List => None,
    };
    if let Some((title, line)) = message_shown {
        message_of(&mut canvas, at, message, (title, AMBER), line);
        return canvas;
    }
    let title = regions.title;
    canvas.fit(
        &[body],
        b"BESTS",
        title.width(),
        title.left,
        title.bottom,
        Align::Left,
        WHITE,
    );
    let bests = view.bests();
    for (index, mark) in bests.power.iter().enumerate() {
        let mut value = Text::<12>::new();
        let _ = write!(value, "{} W", mark.value);
        line_of(
            &mut canvas,
            (at, regions),
            index,
            effort_label(index).as_bytes(),
            (&value, *mark, POWER),
        );
        if mark.value != 0
            && let Some(tenths) = view.rider.watts_per_kg_tenths(mark.value)
        {
            let mut per_kg = Text::<12>::new();
            let _ = write!(per_kg, "{}.{} W/KG", tenths / 10, tenths % 10);
            canvas.caption(
                at,
                per_kg.as_bytes(),
                regions.per_kg.width(),
                regions.per_kg.right,
                regions.bands[index].bottom - at.unit(14),
                Align::Right,
            );
        }
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
        line_of(
            &mut canvas,
            (at, regions),
            4 + offset,
            label,
            (value, mark, color),
        );
    }
    canvas
}

fn line_of(
    canvas: &mut Canvas,
    (at, regions): (&Scaled, &Regions),
    row: usize,
    label: &[u8],
    (value, mark, color): (&Text<12>, Mark, u16),
) {
    let body = at.face(FaceRole::Body);
    let band = regions.bands[row];
    let column = regions.value;
    let base = band.bottom;
    let caption_base = base - at.unit(14);
    let label = canvas.caption(
        at,
        label,
        band.width(),
        band.left,
        caption_base,
        Align::Left,
    );
    let value_base = base - at.unit(3);
    if mark.value == 0 {
        let width = at.unit(60);
        let right = column.right;
        canvas.fit(
            &[body],
            b"--",
            width,
            right,
            value_base,
            Align::Right,
            TERTIARY,
        );
        return;
    }
    let room = column.right - (label.right() + at.unit(4));
    canvas.fit(
        &[body, at.face(FaceRole::Caption)],
        value.as_bytes(),
        column.width().min(room),
        column.right,
        value_base,
        Align::Right,
        color,
    );
    if mark.day != 0 {
        let day = date(mark.day);
        let width = column.left - band.left;
        canvas.caption(at, day.as_bytes(), width, band.left, base, Align::Left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kept::bests::Bests;
    use crate::screens::workout::Page;
    use crate::screens::{Layout, panels};

    fn shown(layout: &'static Layout, bests: Bests, mode: Mode) -> View<'static> {
        let mut view = View::new(Page::Bests, 0, None);
        view.layout = layout;
        view.show_bests(bests, mode);
        view.prepare();
        view
    }

    fn view(bests: Bests) -> View<'static> {
        shown(&crate::screens::BASE, bests, Mode::List)
    }

    fn page(layout: &Layout) -> Rect {
        layout.chrome.content
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
        let page = page(&crate::screens::BASE);
        view.canvas().assert_laid_out(page, &want, "bests");
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
        view.show_bests(bests, Mode::List);
        view.prepare();
        assert!(!view.canvas().shows("W/KG"));
        view.rider = crate::ride::rider::Rider::default().step_weight(1, true);
        view.prepare();
        view.canvas().assert_laid_out(
            page(&crate::screens::BASE),
            &["28.6 W/KG", "6.4 W/KG", "4.5 W/KG", "1999 W", "312 W"],
            "bests with weight",
        );
    }

    #[test]
    fn clearing_asks_first_and_missing_storage_says_so() {
        let full = page(&crate::screens::BASE);
        let confirm = shown(&crate::screens::BASE, Bests::default(), Mode::Confirm);
        confirm.canvas().assert_laid_out(
            full,
            &["CLEAR BESTS?", "THEY CANNOT COME BACK"],
            "confirm",
        );
        assert_eq!(confirm.cues.len(), 2);
        let none = shown(&crate::screens::BASE, Bests::default(), Mode::Unavailable);
        none.canvas()
            .assert_laid_out(full, &["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"], "none");
        assert!(none.cues.is_empty());
    }

    /// A 20 minute best under 10 W/kg at the default weight, whose W/kg keeps
    /// a word's gap from "20 MIN". From 10.0 W/kg, which no rider holds for
    /// 20 minutes, it comes closer than a word on the C606.
    const TWENTY_MINUTES_UNDER_10_W_KG: u32 = 650;

    #[test]
    fn the_longest_bests_and_every_message_lay_out_whole() {
        let widest = Bests {
            power: [
                mark(1999, 20_000),
                mark(1999, 20_000),
                mark(999, 20_000),
                mark(TWENTY_MINUTES_UNDER_10_W_KG, 20_000),
            ],
            distance_m: mark(9_999_900, 20_022),
            time_s: mark(99 * 3600 + 59 * 60 + 59, 20_023),
            ascent_m: mark(99_999, 20_024),
        };
        let messages = [
            (Mode::Confirm, ["CLEAR BESTS?", "THEY CANNOT COME BACK"]),
            (
                Mode::Unavailable,
                ["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"],
            ),
            (Mode::Damaged, ["BESTS FILE DAMAGED", "IT CANNOT BE READ"]),
            (
                Mode::Foreign,
                ["BESTS NOT FORMATTED", "OFF AND ON TO FORMAT"],
            ),
            (Mode::NotSaved, ["BESTS NOT SAVED", "STORAGE REFUSED THEM"]),
        ];
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let what = std::format!("{:?}", layout.scaled);
            let mut view = shown(layout, widest, Mode::List);
            view.prepare().check_layout();
            let want = ["BESTS", "9999.9 KM", "99:59:59", "MOST ASCENT", "99999 M"];
            view.canvas().assert_laid_out(page(layout), &want, &what);
            view.rider = crate::ride::rider::Rider::default().step_weight(1, true);
            view.prepare().check_layout();
            let canvas = view.canvas();
            assert!(canvas.shows("28.6 W/KG"), "{what}");
            let tenths = view.rider.watts_per_kg_tenths(TWENTY_MINUTES_UNDER_10_W_KG);
            let tenths = tenths.unwrap();
            assert!(
                tenths < 100,
                "{what}: 20 minutes at {tenths} tenths of a W/kg"
            );
            let per_kg = std::format!("{}.{} W/KG", tenths / 10, tenths % 10);
            let rect = |text: &str| {
                let run = canvas.find(text).unwrap();
                crate::screens::chrome::layout::text(run).unwrap().1
            };
            let word = layout.scaled.face(FaceRole::Caption).glyph(b' ').advance;
            let gap = rect(&per_kg).left - rect("20 MIN").right;
            assert!(gap >= usize::from(word), "{what}: {per_kg} meets 20 MIN");
            for (mode, want) in messages {
                let mut view = shown(layout, Bests::default(), mode);
                view.prepare().check_layout();
                view.canvas().assert_laid_out(page(layout), &want, &what);
            }
        }
    }

    #[test]
    fn a_best_not_set_yet_says_so_and_a_missing_day_shows_none() {
        let empty = view(Bests::default());
        assert!(empty.canvas().shows("--"));
        assert!(!empty.canvas().shows(" W"));
        let undated = view(Bests {
            power: [
                mark(300, 0),
                Mark::default(),
                Mark::default(),
                Mark::default(),
            ],
            ..Bests::default()
        });
        assert!(undated.canvas().shows("300 W"));
        assert!(!undated.canvas().shows("1970"));
    }
}
