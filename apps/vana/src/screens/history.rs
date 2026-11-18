//! Rides: the saved rides, newest first, one card each.
//!
//! The week header and every card make one page that scrolls under the
//! finger (`scroll::Layout`). MOVE focuses the next ride (wrapping), ACT
//! opens it, BACK goes Home, and a tap opens the ride under the finger. Each
//! card names the ride by its start (OCT 17  7:12) and says how long, how far
//! and how hard it was. At the page's top sits this week at a glance:
//! distance, moving time, rides, the week's training stress (TSS, from the
//! rides that had power), last week's distance and a bar for each day. Once
//! its top line has scrolled away, RIDES and the focus's place pin at the
//! top. Without a saved ride the screen says how one gets here.
use super::Layout as Screens;
use super::canvas::{Canvas, Shape, on_baseline};
use super::chrome::{
    ACCENT, Align, CARD, HEART, Line, POWER, PRESSED, RAISED, Rect, SECONDARY, TERTIARY, WHITE,
};
use super::scaled::{Scaled, Track};
use super::scroll::{Column, Layout, PAGE, Pin};
use super::workout::Text;
use crate::clock::{Local, civil};
use crate::ride::week::{DAYS, Weeks};
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, SPILL};

/// Held MOVE steps this many cards on.
pub const SHOWN: usize = 4;

/// One saved ride as its card shows it.
#[derive(Clone, Default)]
pub struct Row {
    /// When it started, or RIDE n without a clock.
    pub title: Text<20>,
    /// Recorded time and distance, such as "1:07:12  32.4 KM".
    pub detail: Text<20>,
    /// Average heart rate, such as "141 BPM", at the card's right.
    pub heart: Text<8>,
}

/// The list: how many rides, which is focused, how far it is scrolled and
/// each card made on demand, so only the cards on screen exist at once.
pub struct List<F: Fn(usize) -> Row> {
    pub layout: &'static Screens,
    pub count: usize,
    pub cursor: usize,
    /// How far the page is scrolled (`Kinetic::offset`): negative past the
    /// top, as the rubber band stretches.
    pub offset: i32,
    pub row: F,
    /// This week and last, or none while the clock is unknown.
    pub weeks: Option<Weeks>,
    /// The index has not read every ride yet, so the weeks may grow.
    pub reading: bool,
    /// The card under a resting finger.
    pub pressed: Option<usize>,
}

/// The rides on one panel. The page's parts are in screen rows as they lie
/// with the page at its top; each text row's capitals end on its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The page under the status bar: the week header, then the cards.
    pub list: Layout,
    /// The week header's text, inside the cards' columns.
    pub text: Rect,
    pub caption: Rect,
    /// The week's distance, left of the day bars.
    pub value: Rect,
    pub time: Rect,
    pub stress: Rect,
    pub last: Rect,
    /// Each day's bar at its tallest, Monday first; bars end on the value's
    /// bottom.
    pub bars: [Rect; DAYS],
    /// The band RIDES and the focus's place pin in once the header's top
    /// line has scrolled away.
    pub band: Rect,
    /// Without a ride: RIDES, then what the list will hold.
    pub empty: [Rect; 3],
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, cards, _] = content.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [_, text, _] = content.cols([Unit(14), Fr(1), Unit(14)], Unit(0), at);
    let [_, days, _] = content.cols([Fr(1), Unit(94), Unit(14)], Unit(0), at);
    let [caption, value, time, stress, last, _, column] = text.rows(
        [
            Unit(16),
            Unit(38),
            Unit(19),
            Unit(14),
            Unit(14),
            Unit(10),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, tall] = value.rows([Fr(1), Unit(30)], Unit(0), at);
    let bars = tall.with_columns(days).cols([Unit(10); DAYS], Unit(4), at);
    let [band, _] = content.rows([Unit(24), Fr(1)], Unit(0), at);
    let [title, _, nothing, saved, _] =
        text.rows([Unit(36), Unit(60), Unit(30), Unit(24), Fr(1)], Unit(0), at);
    let height = at.unit(54);
    Regions {
        list: Layout {
            top: content.top,
            bottom: content.bottom,
            column: Column {
                left: cards.left,
                right: cards.right,
                top: column.top - content.top,
                height,
                pitch: height + at.unit(6),
            },
            end: at.unit(10),
            margin: at.unit(6),
            pin: Some(Pin {
                after: (caption.bottom - content.top) as i32,
                height: band.height(),
            }),
        },
        text,
        caption,
        value: Rect {
            right: days.left - at.unit(30),
            ..value
        },
        time: Rect {
            right: days.left - at.unit(8),
            ..time
        },
        stress: Rect {
            right: days.left - at.unit(8),
            ..stress
        },
        last: Rect {
            right: cards.right,
            ..last
        },
        bars,
        band,
        empty: [
            title.with_columns(cards),
            nothing.with_columns(cards),
            saved.with_columns(cards),
        ],
    }
}

/// The most training stress the header names; more reads `9999+`.
const STRESS_MOST: u32 = 9999;
/// Every other day is named: the letters are as wide as the bars are apart.
const DAY_LETTERS: [&[u8]; DAYS] = [b"M", b"", b"W", b"", b"F", b"", b"S"];

/// Whether any of the week header shows with the page scrolled to `offset`.
pub fn header_shows(list: &Layout, offset: i32) -> bool {
    offset < list.span(0).0
}

pub fn prepare<F: Fn(usize) -> Row>(list: &List<F>) -> Canvas {
    let (at, regions) = (&list.layout.scaled, &list.layout.history);
    let layout = &regions.list;
    let face = |role| at.face(role);
    let page_row = |y: usize| PAGE + y - layout.top;
    let mut canvas = Canvas::default();
    if list.count == 0 {
        let [title, nothing, saved] = regions.empty;
        canvas.fit(
            &[face(FaceRole::Title), face(FaceRole::Body)],
            b"RIDES",
            title.width(),
            regions.text.left,
            page_row(title.bottom),
            Align::Left,
            WHITE,
        );
        canvas.fit(
            &[face(FaceRole::Body)],
            b"NO RIDES YET",
            nothing.width(),
            nothing.center_x(),
            page_row(nothing.bottom),
            Align::Center,
            WHITE,
        );
        canvas.caption(
            at,
            b"SAVED RIDES SHOW HERE",
            saved.width(),
            saved.center_x(),
            page_row(saved.bottom),
            Align::Center,
        );
        return canvas;
    }
    let mut count = Text::<12>::new();
    let _ = write!(count, "{}/{}", list.cursor + 1, list.count);
    // Only what shows: the header while any of it does, and the cards whose
    // rows reach the viewport. The runs the viewport leaves out go as each
    // part is made: a tall panel's cards and the whole header would overfill
    // the canvas together.
    let cull = |canvas: &mut Canvas| {
        canvas
            .runs
            .retain(|run| layout.shows(run.top..run.bottom(), list.offset));
    };
    if header_shows(layout, list.offset) {
        header(&mut canvas, list, count.as_bytes());
        cull(&mut canvas);
    }
    if layout.pinned(list.offset) {
        let (band, text) = (regions.band, regions.text);
        let base = band.top + at.unit(18);
        let place = at.unit(80);
        canvas.fit(
            &[face(FaceRole::Body)],
            b"RIDES",
            text.width() + at.unit(8) - place,
            text.left,
            base,
            Align::Left,
            WHITE,
        );
        canvas.caption(at, count.as_bytes(), place, text.right, base, Align::Right);
        // A hairline where the cards pass under the band.
        canvas.shape(Shape::Bar {
            rect: Rect {
                top: band.bottom - 1,
                ..band
            },
            radius: 0,
            color: CARD,
        });
    }
    let (radius, ring, inset) = (at.unit(12), at.unit(3), at.unit(14));
    for index in layout.visible(list.count, list.offset) {
        let row = (list.row)(index);
        let rect = layout.rect(index);
        let focused = index == list.cursor;
        // The focused card is ringed, as on every list (`docs/ui.md`).
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
            color: if list.pressed == Some(index) {
                PRESSED
            } else if focused {
                RAISED
            } else {
                CARD
            },
        });
        let width = rect.width() - 2 * inset;
        let body = face(FaceRole::Body);
        let title_base = rect.top + at.unit(8) + body.cap as usize;
        let caption = face(FaceRole::Caption);
        let heart = Line::<24>::fit(&[caption], row.heart.as_bytes(), width / 2);
        let (start, end) = (rect.left + inset, rect.right - inset);
        let beside = (end - heart.width()).saturating_sub(start + super::canvas::GAP);
        let room = if row.heart.as_bytes().is_empty()
            || caption.width(row.title.as_bytes()) + SPILL > beside
        {
            width
        } else {
            canvas.run(on_baseline(heart, end, title_base, Align::Right, HEART));
            beside
        };
        canvas.fit(
            &[body, caption],
            row.title.as_bytes(),
            room,
            rect.left + inset,
            title_base,
            Align::Left,
            if focused { WHITE } else { SECONDARY },
        );
        canvas.caption(
            at,
            row.detail.as_bytes(),
            width,
            rect.left + inset,
            rect.bottom - at.unit(10),
            Align::Left,
        );
        cull(&mut canvas);
    }
    canvas
}

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// "OCT 17 2026" for a day count since 1970-01-01 on the local clock.
pub fn date(days: u16) -> Text<20> {
    let (year, month, day) = civil(i64::from(days));
    let mut text = Text::new();
    let _ = core::fmt::Write::write_fmt(
        &mut text,
        format_args!("{} {} {}", MONTHS[usize::from(month - 1)], day, year),
    );
    text
}

/// This week at a glance: the caption, rides and the focus's place in the
/// list on top, then distance and moving time on the left, a bar for each day
/// on the right and last week's distance under.
/// Without a clock nothing is known (`--`); while the index still reads, what
/// it has found so far shows.
fn header<F: Fn(usize) -> Row>(canvas: &mut Canvas, list: &List<F>, count: &[u8]) {
    let (at, regions) = (&list.layout.scaled, &list.layout.history);
    let face = |role| at.face(role);
    let page_row = |y: usize| PAGE + y - regions.list.top;
    let left = regions.text.left;
    let caption = page_row(regions.caption.bottom);
    let title = canvas.caption(at, b"THIS WEEK", at.unit(100), left, caption, Align::Left);
    let place = canvas.caption(
        at,
        count,
        at.unit(80),
        regions.text.right,
        caption,
        Align::Right,
    );
    let weeks = list.weeks.as_ref();
    // Nothing found yet while reading is not a quiet week.
    let known = weeks.filter(|weeks| !list.reading || weeks.this.rides > 0);
    let mut km = Text::<12>::new();
    let value: &[u8] = match known {
        Some(weeks) => {
            let tenths = (weeks.this.distance_mm + 50_000) / 100_000;
            let _ = write!(km, "{}.{}", tenths / 10, tenths % 10);
            km.as_bytes()
        }
        None => b"--",
    };
    let run = canvas.fit(
        &[face(FaceRole::Value), face(FaceRole::Body)],
        value,
        regions.value.width(),
        left,
        page_row(regions.value.bottom),
        Align::Left,
        WHITE,
    );
    canvas.fit(
        &[face(FaceRole::Caption)],
        b"KM",
        at.unit(30),
        run.right() + at.unit(6),
        page_row(regions.value.bottom),
        Align::Left,
        SECONDARY,
    );
    let time = match known {
        Some(weeks) => {
            let mut time = Text::<12>::new();
            moving(&mut time, weeks.this.active_ms / 1000);
            time
        }
        None => {
            let mut time = Text::new();
            let _ = time.push_str("--");
            time
        }
    };
    canvas.fit(
        &[face(FaceRole::Body)],
        time.as_bytes(),
        regions.time.width(),
        left,
        page_row(regions.time.bottom),
        Align::Left,
        WHITE,
    );
    if let Some(weeks) = known {
        let mut rides = Text::<12>::new();
        let _ = write!(
            rides,
            "{} {}",
            weeks.this.rides,
            if weeks.this.rides == 1 {
                "RIDE"
            } else {
                "RIDES"
            }
        );
        let x = title.right() + at.unit(12);
        canvas.caption(
            at,
            rides.as_bytes(),
            place.left.saturating_sub(x + at.unit(8)),
            x,
            caption,
            Align::Left,
        );
    }
    if let Some(weeks) = weeks {
        let mut last = Text::<20>::new();
        let _ = write!(
            last,
            "LAST WEEK {} KM",
            (weeks.last.distance_mm + 500_000) / 1_000_000
        );
        canvas.caption(
            at,
            last.as_bytes(),
            regions.last.width(),
            left,
            page_row(regions.last.bottom),
            Align::Left,
        );
    }
    // This week's training stress, from the rides that had power; a week
    // without any says nothing rather than 0. Its own line, under the time,
    // so no distance can crowd it out.
    if let Some(weeks) = known.filter(|weeks| weeks.this.powered > 0) {
        let mut stress = Text::<16>::new();
        let _ = if weeks.this.stress > STRESS_MOST {
            write!(stress, "TSS {STRESS_MOST}+")
        } else {
            write!(stress, "TSS {}", weeks.this.stress)
        };
        canvas.fit(
            &[face(FaceRole::Caption)],
            stress.as_bytes(),
            regions.stress.width(),
            left,
            page_row(regions.stress.bottom),
            Align::Left,
            POWER,
        );
    }
    // One bar a day, Monday first: dark stubs for days without a ride, white
    // for ridden days (blue for today), tall in proportion to the most.
    let most = weeks.map_or(0, |w| w.this.day_mm.iter().copied().max().unwrap_or(0));
    for (day, (letter, bar)) in DAY_LETTERS.iter().zip(regions.bars).enumerate() {
        let today = weeks.is_some_and(|w| w.today == day);
        let ridden = weeks.is_some_and(|w| w.this.day_rides[day] > 0);
        let distance = weeks.map_or(0, |w| w.this.day_mm[day]);
        let tallest = bar.height();
        let high = if ridden {
            let scaled = (distance * tallest as u64).checked_div(most).unwrap_or(0);
            usize::try_from(scaled).unwrap_or(tallest).max(at.unit(6))
        } else {
            at.unit(3)
        };
        let base = page_row(bar.bottom);
        canvas.shape(Shape::Bar {
            rect: Rect {
                top: base - high,
                bottom: base,
                ..bar
            },
            radius: at.unit(if ridden { 3 } else { 1 }),
            color: match (ridden, today) {
                (false, _) => TERTIARY,
                (true, true) => ACCENT,
                (true, false) => WHITE,
            },
        });
        if letter.is_empty() {
            continue;
        }
        canvas.fit(
            &[face(FaceRole::Caption)],
            letter,
            at.unit(20),
            bar.center_x(),
            page_row(regions.time.bottom),
            Align::Center,
            if today { ACCENT } else { SECONDARY },
        );
    }
}

/// Moving time as "5H 24M", or "24M" under an hour.
fn moving(text: &mut Text<12>, seconds: u64) {
    let minutes = seconds / 60;
    let _ = if minutes >= 60 {
        write!(text, "{}H {:02}M", minutes / 60, minutes % 60)
    } else {
        write!(text, "{minutes}M")
    };
}

/// "OCT 17  7:12", a local start time.
pub fn started(local: Local) -> Text<20> {
    let (_, month, day) = civil((local.0 / 86_400) as i64);
    let seconds = local.of_day();
    let (hour, minute) = (seconds / 3600, seconds / 60 % 60);
    let mut text = Text::new();
    let _ = core::fmt::Write::write_fmt(
        &mut text,
        format_args!(
            "{} {}  {}:{:02}",
            MONTHS[usize::from(month - 1)],
            day,
            (hour + 11) % 12 + 1,
            minute
        ),
    );
    text
}

#[cfg(test)]
mod tests {
    /// A list longer than the screen.
    const ROWS: usize = 8;
    use super::*;

    fn row(index: usize) -> Row {
        let mut row = Row::default();
        let _ = row.title.push_str(if index == 0 {
            "DEC 31  12:59"
        } else {
            "RIDE 4"
        });
        let _ = row.detail.push_str("10:07:12  132.4 KM");
        let _ = row.heart.push_str("141 BPM");
        row
    }

    const LAYOUT: Layout = crate::screens::BASE.history.list;

    fn list(count: usize, cursor: usize) -> List<fn(usize) -> Row> {
        List {
            layout: &crate::screens::BASE,
            count,
            cursor,
            offset: 0,
            row,
            weeks: None,
            reading: false,
            pressed: None,
        }
    }

    /// Wednesday 2026-10-21, with rides Monday and Tuesday and last week's.
    fn week() -> Weeks {
        let day = |d: i64| ((20_746 + d) * 86_400 + 15 * 3600) as u64 * 1000;
        let zone = |_| -3 * 3600;
        let mut weeks = Weeks::new(day(2), &zone);
        weeks.add(Some(day(0)), 5_400_000, Some(42_000_000), &zone);
        weeks.add(Some(day(1)), 3_600_000, Some(30_000_000), &zone);
        weeks.add(Some(day(1) + 1000), 3_600_000, Some(12_300_000), &zone);
        weeks.add(Some(day(-3)), 3_600_000, Some(84_000_000), &zone);
        weeks
    }

    #[test]
    fn starts_read_as_month_day_and_twelve_hour_time() {
        let october_17: u64 = 20_743 * 86_400;
        let at = |seconds| started(Local(october_17 + seconds));
        assert_eq!(at(7 * 3600 + 12 * 60).as_str(), "OCT 17  7:12");
        assert_eq!(at(21 * 3600).as_str(), "OCT 17  9:00");
        assert_eq!(at(0).as_str(), "OCT 17  12:00");
    }

    /// The whole canvas: the page sits below the screen's rows from `PAGE`.
    fn page() -> Rect {
        Rect {
            left: 0,
            top: 0,
            right: 240,
            bottom: 100_000,
        }
    }

    /// The canvas's card panels, top edges in canvas rows, with their fill.
    fn cards(canvas: &Canvas) -> std::vec::Vec<(usize, u16)> {
        canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Panel { rect, color, .. } => Some((rect.top, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn at_the_top_the_week_header_and_the_first_cards_show() {
        let mut full = list(ROWS, 0);
        full.weeks = Some(week());
        let canvas = prepare(&full);
        let want = [
            "THIS WEEK",
            "DEC 31  12:59",
            "1/8",
            "10:07:12  132.4 KM",
            "141 BPM",
        ];
        canvas.assert_laid_out(page(), &want, "top");
        // The header sits on the page, above three whole cards and the top
        // of a fourth.
        let title = canvas.find("THIS WEEK").unwrap();
        assert!(title.top >= PAGE);
        let tops: std::vec::Vec<_> = cards(&canvas)
            .iter()
            .filter(|(_, c)| *c != ACCENT)
            .map(|(top, _)| *top)
            .collect();
        assert_eq!(tops, [0, 1, 2, 3].map(|i| LAYOUT.rect(i).top));
        assert!((0..3).all(|i| LAYOUT.whole(i, 0)) && !LAYOUT.whole(3, 0));
        let empty = prepare(&list(0, 0));
        empty.assert_laid_out(page(), &["NO RIDES YET", "SAVED RIDES SHOW HERE"], "empty");
    }

    #[test]
    fn scrolled_only_cards_in_view_are_made_and_the_title_pins() {
        let end = LAYOUT.reveal(ROWS - 1, 0);
        for offset in [-40, 0, 1, 16, 17, 60, 111, 200, 301, end] {
            let mut scrolled = list(ROWS, 0);
            scrolled.offset = offset;
            scrolled.weeks = Some(week());
            let canvas = prepare(&scrolled);
            canvas.assert_laid_out(page(), &[], &std::format!("{offset}"));
            // Once the header's top line is gone, RIDES and the place pin in
            // its stead.
            assert_eq!(
                canvas.shows("THIS WEEK"),
                !LAYOUT.pinned(offset),
                "{offset}"
            );
            assert_eq!(canvas.shows("RIDES"), LAYOUT.pinned(offset), "{offset}");
            assert!(canvas.shows("1/8"), "{offset}");
            let made: std::vec::Vec<_> = cards(&canvas)
                .iter()
                .filter(|(_, c)| *c != ACCENT)
                .map(|(top, _)| *top)
                .collect();
            let want: std::vec::Vec<_> = LAYOUT
                .visible(ROWS, offset)
                .map(|i| LAYOUT.rect(i).top)
                .collect();
            assert_eq!(made, want, "{offset}");
        }
        assert!(LAYOUT.pinned(17) && !LAYOUT.pinned(16));
    }

    #[test]
    fn the_focused_card_is_raised_and_ringed_and_one_under_a_resting_finger_pressed() {
        let mut shown = list(ROWS, 1);
        shown.pressed = Some(2);
        let fills: std::vec::Vec<_> = cards(&prepare(&shown)).iter().map(|(_, c)| *c).collect();
        assert_eq!(
            fills,
            [CARD, ACCENT, RAISED, PRESSED, CARD],
            "the focus ringed"
        );
        shown.pressed = Some(1);
        let fills: std::vec::Vec<_> = cards(&prepare(&shown)).iter().map(|(_, c)| *c).collect();
        assert_eq!(fills[1..3], [ACCENT, PRESSED], "pressed keeps the ring");
    }

    #[test]
    fn the_header_reads_the_week_and_what_is_known() {
        assert!(
            core::mem::size_of::<Weeks>() <= 208,
            "{}",
            core::mem::size_of::<Weeks>()
        );
        let mut list = list(ROWS, 0);
        list.weeks = Some(week());
        let canvas = prepare(&list);
        // 42.0 + 30.0 + 12.3 km in 3 rides over 2 days; 84 km last week.
        for text in ["84.3", "KM", "3H 30M", "3 RIDES", "LAST WEEK 84 KM"] {
            assert!(canvas.shows(text), "{text}");
        }
        let lit = |color| {
            canvas
                .shapes
                .iter()
                .filter(|s| matches!(s, Shape::Bar { color: c, .. } if *c == color))
                .count()
        };
        // Monday and Tuesday white (today is Wednesday), the rest dark.
        assert_eq!((lit(WHITE), lit(ACCENT), lit(TERTIARY)), (2, 0, 5));
        // Still reading with nothing found: not a quiet week yet.
        list.weeks = Some(Weeks::new(1_789_000_000_000, &|_| -3 * 3600));
        list.reading = true;
        let reading = prepare(&list);
        assert!(reading.shows("--") && !reading.shows("0.0"));
        // The same once everything is read: a week without rides.
        list.reading = false;
        let quiet = prepare(&list);
        assert!(quiet.shows("0.0") && quiet.shows("0M") && quiet.shows("0 RIDES"));
        quiet.assert_laid_out(page(), &["THIS WEEK", "LAST WEEK 0 KM"], "quiet");
        // No clock: nothing is known, and there is no last week.
        list.weeks = None;
        let unknown = prepare(&list);
        assert!(unknown.shows("--") && !unknown.shows("LAST WEEK 0 KM"));
        unknown.assert_laid_out(page(), &["THIS WEEK"], "no clock");
        // A long week in a big number still fits.
        let mut big = Weeks::new(1_789_000_000_000, &|_| -3 * 3600);
        big.add(
            Some(1_788_900_000_000),
            400 * 3_600_000,
            Some(1_234_500_000),
            &|_| -3 * 3600,
        );
        list.weeks = Some(big);
        prepare(&list).assert_laid_out(page(), &["THIS WEEK", "1234.5"], "big");
    }

    #[test]
    fn training_stress_shows_only_for_a_week_with_power() {
        let mut list = list(ROWS, 0);
        list.weeks = Some(week());
        assert!(!prepare(&list).shows("TSS"));
        let zone = |_| -3 * 3600;
        let tuesday = (20_746 * 86_400 + 15 * 3600) * 1000;
        let last_week = tuesday - 7 * 86_400_000;
        // Three- and four-digit last-week distances with three- to
        // five-digit stress: none is cut and none overlaps.
        for (km, stress, shown) in [
            (84u64, 87, "TSS 87"),
            (123, 456, "TSS 456"),
            (1234, 1234, "TSS 1234"),
            (1234, 9999, "TSS 9999"),
            (1234, u32::MAX, "TSS 9999+"),
        ] {
            let mut weeks = week();
            weeks.last = Default::default();
            weeks.add(Some(last_week), 1, Some(km * 1_000_000), &zone);
            weeks.add_stress(Some(tuesday), 1, Some(1), Some(stress), &zone);
            list.weeks = Some(weeks);
            let canvas = prepare(&list);
            let last = std::format!("LAST WEEK {km} KM");
            assert!(canvas.shows(shown), "{shown}");
            canvas.assert_laid_out(page(), &[&last, shown, "THIS WEEK"], "stress");
        }
    }

    #[test]
    fn long_rides_and_a_long_week_lay_out_on_every_panel() {
        use crate::screens::workout::{Page, View};
        let zone = |_| -3 * 3600;
        let tuesday = (20_746 * 86_400 + 15 * 3600) * 1000;
        let mut weeks = week();
        weeks.add(
            Some(tuesday - 7 * 86_400_000),
            1,
            Some(9_999_000_000),
            &zone,
        );
        weeks.add_stress(
            Some(tuesday),
            400 * 3_600_000,
            Some(1_234_500_000),
            Some(u32::MAX),
            &zone,
        );
        for layout in crate::screens::panels::ALL {
            let end = layout.history.list.reveal(ROWS - 1, 0);
            for (count, offset) in [(ROWS, -40), (ROWS, 0), (ROWS, 60), (ROWS, end), (0, 0)] {
                let mut rides = list(count, 1);
                rides.layout = layout;
                rides.offset = offset;
                rides.weeks = Some(weeks);
                rides.pressed = Some(2);
                let mut view = View::new(Page::History, 0, None);
                view.layout = layout;
                view.scroll = offset;
                view.set_canvas(prepare(&rides));
                view.prepare().check_layout();
                let what = std::format!("{count} rides at {offset} on {:?}", layout.scaled.panel);
                let want: &[&str] = match (count, offset) {
                    (0, _) => &["RIDES", "NO RIDES YET", "SAVED RIDES SHOW HERE"],
                    (_, 0) => &["THIS WEEK", "TSS 9999+", "DEC 31  12:59", "141 BPM"],
                    _ => &[],
                };
                let page = Rect {
                    top: 0,
                    bottom: 100_000,
                    ..layout.scaled.screen()
                };
                view.canvas().assert_laid_out(page, want, &what);
            }
        }
    }

    /// Short titles beside their heart rates, so every card makes its three
    /// runs, and a detail of its own on each.
    fn short(index: usize) -> Row {
        let mut row = Row::default();
        let _ = write!(row.title, "RIDE {}", index + 1);
        let _ = write!(row.detail, "{}:07:12  32.4 KM", index + 1);
        let _ = row.heart.push_str("141 BPM");
        row
    }

    #[test]
    fn every_panel_keeps_the_words_of_every_card_in_view() {
        use crate::screens::workout::{Page, View};
        let zone = |_| -3 * 3600;
        let tuesday = (20_746 * 86_400 + 15 * 3600) * 1000;
        let mut powered = week();
        powered.add_stress(Some(tuesday), 1, Some(1), Some(87), &zone);
        let count = 12;
        for layout in crate::screens::panels::ALL {
            let list = &layout.history.list;
            let (at, body) = (&layout.scaled, layout.scaled.face(FaceRole::Body));
            for weeks in [None, Some(powered)] {
                for offset in -40..=list.reveal(count - 1, 0) {
                    let rides = List {
                        layout,
                        count,
                        cursor: 1,
                        offset,
                        row: short as fn(usize) -> Row,
                        weeks,
                        reading: false,
                        pressed: Some(2),
                    };
                    let what = std::format!(
                        "{count} rides at {offset} on {:?}, weeks {}",
                        at.panel,
                        weeks.is_some()
                    );
                    let mut view = View::new(Page::History, 0, None);
                    view.layout = layout;
                    view.scroll = offset;
                    view.set_canvas(prepare(&rides));
                    view.prepare().check_layout();
                    // A word whose baseline row shows is on the canvas.
                    let canvas = view.canvas();
                    for index in list.visible(count, offset) {
                        let rect = list.rect(index);
                        let title = rect.top + at.unit(8) + body.cap as usize;
                        let detail = rect.bottom - at.unit(10);
                        for (base, text) in
                            [(title, short(index).title), (detail, short(index).detail)]
                        {
                            if list.shows(base - 1..base, offset) {
                                assert!(canvas.shows(text.as_str()), "{} at {what}", text.as_str());
                            }
                        }
                    }
                }
            }
        }
    }
}
