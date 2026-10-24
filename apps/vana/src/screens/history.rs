//! Rides: the saved rides, newest first, one card each.
//!
//! MOVE focuses the next ride (wrapping), ACT opens it, BACK goes Home, and
//! a tap opens the ride under the finger. Each card names the ride by its
//! start (OCT 17  7:12) and says how long, how far and how hard it was.
//! Above the first cards sits this week at a glance: distance, moving time,
//! rides, last week's distance and a bar for each day. Scrolled past the
//! first cards, the header gives way to the RIDES headline and a fourth card.
//! Without a saved ride the screen says how one gets here.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, Align, BODY, CAPTION, CARD, CONTENT, HEART, RAISED, Rect, SECONDARY, TERTIARY, TITLE,
    VALUE, WHITE, WIDTH,
};
use super::workout::Text;
use crate::clock::{Local, civil};
use crate::ride::week::{DAYS, Weeks};
use core::fmt::Write;

/// Cards on screen at once; the list scrolls to keep the focused one shown.
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

/// The list: how many rides, which is focused, and each card made on
/// demand, so only the cards on screen exist at once.
pub struct List<F: Fn(usize) -> Row> {
    pub count: usize,
    pub cursor: usize,
    pub row: F,
    /// This week and last, or none while the clock is unknown.
    pub weeks: Option<Weeks>,
    /// The index has not read every ride yet, so the weeks may grow.
    pub reading: bool,
}

const LEFT: usize = 10;
const RIGHT: usize = WIDTH - 10;
const RADIUS: usize = 12;
const INSET: usize = 14;
const HEADLINE: usize = CONTENT.top + 36;
const FIRST: usize = CONTENT.top + 50;
const HEIGHT: usize = 54;
const PITCH: usize = HEIGHT + 6;

/// While the focus is on one of the first `HEADED` rides the week header
/// shows, over that many cards; the rest of the list has the headline.
pub const HEADED: usize = 3;
const HEADER_FIRST: usize = CONTENT.top + 100;
const CAPTION_BASE: usize = CONTENT.top + 16;
const VALUE_BASE: usize = CONTENT.top + 54;
const TIME_BASE: usize = CONTENT.top + 73;
const LAST_BASE: usize = CONTENT.top + 90;
/// The day bars end on `BARS_BASE`, up to `BARS_HIGH` tall, `BAR_PITCH` apart.
const BARS_BASE: usize = VALUE_BASE;
const BARS_HIGH: usize = 30;
const BAR_WIDE: usize = 10;
const BAR_PITCH: usize = 14;
const BARS_LEFT: usize = RIGHT - 4 - (DAYS - 1) * BAR_PITCH - BAR_WIDE;
/// Every other day is named: the letters are as wide as the bars are apart.
const DAY_LETTERS: [&[u8]; DAYS] = [b"M", b"", b"W", b"", b"F", b"", b"S"];

/// Whether the week header shows with the focus on `cursor`.
fn headed(cursor: usize) -> bool {
    cursor < HEADED
}

/// The first row on screen, so that the focused one shows.
fn first_shown(cursor: usize) -> usize {
    if headed(cursor) {
        0
    } else {
        cursor - (SHOWN - 1).min(cursor)
    }
}

/// Cards on screen with the focus on `cursor`.
fn shown(cursor: usize) -> usize {
    if headed(cursor) { HEADED } else { SHOWN }
}

fn card(headed: bool, slot: usize) -> Rect {
    let top = if headed { HEADER_FIRST } else { FIRST } + slot * PITCH;
    Rect {
        left: LEFT,
        top,
        right: RIGHT,
        bottom: top + HEIGHT,
    }
}

/// The list index of the ride under a tap, if any.
pub fn row_at(count: usize, cursor: usize, x: usize, y: usize) -> Option<usize> {
    let headed = headed(cursor);
    let slot = (0..shown(cursor)).find(|slot| card(headed, *slot).contains(x, y))?;
    let index = first_shown(cursor) + slot;
    (index < count).then_some(index)
}

pub fn prepare<F: Fn(usize) -> Row>(list: &List<F>) -> Canvas {
    let mut canvas = Canvas::default();
    if list.count == 0 {
        canvas.fit(
            &[&TITLE, &BODY],
            b"RIDES",
            RIGHT - LEFT,
            LEFT + 4,
            HEADLINE,
            Align::Left,
            WHITE,
        );
        let centre = WIDTH / 2;
        canvas.fit(
            &[&BODY],
            b"NO RIDES YET",
            RIGHT - LEFT,
            centre,
            150,
            Align::Center,
            WHITE,
        );
        canvas.caption(
            b"SAVED RIDES SHOW HERE",
            RIGHT - LEFT,
            centre,
            174,
            Align::Center,
        );
        return canvas;
    }
    let mut count = Text::<12>::new();
    let _ = write!(count, "{}/{}", list.cursor + 1, list.count);
    let headed = headed(list.cursor);
    if headed {
        header(&mut canvas, list, count.as_bytes());
    } else {
        canvas.fit(
            &[&TITLE, &BODY],
            b"RIDES",
            RIGHT - LEFT,
            LEFT + 4,
            HEADLINE,
            Align::Left,
            WHITE,
        );
        canvas.caption(count.as_bytes(), 80, RIGHT - 4, HEADLINE - 4, Align::Right);
    }
    let first = first_shown(list.cursor);
    for (slot, index) in (first..list.count.min(first + shown(list.cursor))).enumerate() {
        let row = (list.row)(index);
        let rect = card(headed, slot);
        let focused = first + slot == list.cursor;
        canvas.shape(Shape::Panel {
            rect,
            radius: RADIUS,
            color: if focused { RAISED } else { CARD },
        });
        let width = rect.right - rect.left - 2 * INSET;
        let title_base = rect.top + 8 + BODY.cap as usize;
        // Heart rate at the right of the title line, the date beside it.
        let heart = canvas.fit(
            &[&CAPTION],
            row.heart.as_bytes(),
            width / 2,
            rect.right - INSET,
            title_base,
            Align::Right,
            HEART,
        );
        let room = if row.heart.as_bytes().is_empty() {
            width
        } else {
            heart
                .left
                .saturating_sub(rect.left + INSET + super::canvas::GAP)
        };
        canvas.fit(
            &[&BODY, &CAPTION],
            row.title.as_bytes(),
            room,
            rect.left + INSET,
            title_base,
            Align::Left,
            if focused { WHITE } else { SECONDARY },
        );
        canvas.caption(
            row.detail.as_bytes(),
            width,
            rect.left + INSET,
            rect.bottom - 10,
            Align::Left,
        );
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
    let left = LEFT + 4;
    let title = canvas.caption(b"THIS WEEK", 100, left, CAPTION_BASE, Align::Left);
    let place = canvas.caption(count, 80, RIGHT - 4, CAPTION_BASE, Align::Right);
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
        &[&VALUE, &BODY],
        value,
        BARS_LEFT - left - 30,
        left,
        VALUE_BASE,
        Align::Left,
        WHITE,
    );
    canvas.fit(
        &[&CAPTION],
        b"KM",
        30,
        run.right() + 6,
        VALUE_BASE,
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
        &[&BODY],
        time.as_bytes(),
        BARS_LEFT - left - 8,
        left,
        TIME_BASE,
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
        canvas.caption(
            rides.as_bytes(),
            place.left.saturating_sub(title.right() + 12 + 8),
            title.right() + 12,
            CAPTION_BASE,
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
        canvas.caption(last.as_bytes(), RIGHT - left, left, LAST_BASE, Align::Left);
    }
    // One bar a day, Monday first: dark stubs for days without a ride, white
    // for ridden days (blue for today), tall in proportion to the most.
    let most = weeks.map_or(0, |w| w.this.day_mm.iter().copied().max().unwrap_or(0));
    for (day, letter) in DAY_LETTERS.iter().enumerate() {
        let today = weeks.is_some_and(|w| w.today == day);
        let ridden = weeks.is_some_and(|w| w.this.day_rides[day] > 0);
        let distance = weeks.map_or(0, |w| w.this.day_mm[day]);
        let high = if ridden {
            let scaled = (distance * BARS_HIGH as u64).checked_div(most).unwrap_or(0);
            usize::try_from(scaled).unwrap_or(BARS_HIGH).max(6)
        } else {
            3
        };
        let x = BARS_LEFT + day * BAR_PITCH;
        canvas.shape(Shape::Bar {
            rect: Rect {
                left: x,
                top: BARS_BASE - high,
                right: x + BAR_WIDE,
                bottom: BARS_BASE,
            },
            radius: if ridden { 3 } else { 1 },
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
            &[&CAPTION],
            letter,
            BAR_PITCH + 6,
            x + BAR_WIDE / 2,
            TIME_BASE,
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

    fn list(count: usize, cursor: usize) -> List<fn(usize) -> Row> {
        List {
            count,
            cursor,
            row,
            weeks: None,
            reading: false,
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

    fn bounds() -> Rect {
        Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: 320,
        }
    }

    #[test]
    fn rows_lay_out_whole_and_the_focused_one_stays_shown() {
        let bounds = bounds();
        let mut full = list(ROWS, 0);
        full.weeks = Some(week());
        let want = [
            "THIS WEEK",
            "DEC 31  12:59",
            "1/8",
            "10:07:12  132.4 KM",
            "141 BPM",
        ];
        prepare(&full).assert_laid_out(bounds, &want, "list");
        // Scrolled on, the headline and a fourth card replace the header.
        let last = list(ROWS, ROWS - 1);
        assert_eq!(first_shown(last.cursor), ROWS - SHOWN);
        prepare(&last).assert_laid_out(bounds, &["RIDES", "8/8"], "scrolled");
        let end = ROWS - 1;
        assert_eq!(
            row_at(ROWS, end, 120, card(false, SHOWN - 1).top + 4),
            Some(end)
        );
        assert_eq!(
            row_at(ROWS, end, 120, card(false, 0).top + 4),
            Some(ROWS - SHOWN)
        );
        assert_eq!(row_at(1, 0, 120, card(true, 1).top + 4), None);
        assert_eq!(row_at(ROWS, end, 120, CONTENT.top), None);
        let empty = prepare(&list(0, 0));
        empty.assert_laid_out(bounds, &["NO RIDES YET", "SAVED RIDES SHOW HERE"], "empty");
    }

    #[test]
    fn the_list_scrolls_through_the_header_and_back() {
        // The header and three cards while the focus is on the first three
        // rides; from the fourth, four cards and the headline.
        for cursor in 0..ROWS {
            let mut list = list(ROWS, cursor);
            list.weeks = Some(week());
            let canvas = prepare(&list);
            canvas.assert_laid_out(bounds(), &[], "scroll");
            assert_eq!(canvas.shows("THIS WEEK"), cursor < HEADED, "{cursor}");
            let first = first_shown(cursor);
            assert!(
                first <= cursor && cursor < first + shown(cursor),
                "{cursor}"
            );
            let panels = canvas
                .shapes
                .iter()
                .filter(|s| matches!(s, Shape::Panel { .. }))
                .count();
            assert_eq!(panels, shown(cursor));
            // Every card on screen answers a tap with its own row.
            for slot in 0..shown(cursor) {
                let at = card(headed(cursor), slot);
                assert_eq!(
                    row_at(ROWS, cursor, at.center_x(), at.top + 20),
                    Some(first + slot)
                );
            }
            let below = card(headed(cursor), shown(cursor) - 1).bottom;
            assert!(below <= 320 - 4, "{cursor}: cards end at {below}");
        }
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
        quiet.assert_laid_out(bounds(), &["THIS WEEK", "LAST WEEK 0 KM"], "quiet");
        // No clock: nothing is known, and there is no last week.
        list.weeks = None;
        let unknown = prepare(&list);
        assert!(unknown.shows("--") && !unknown.shows("LAST WEEK 0 KM"));
        unknown.assert_laid_out(bounds(), &["THIS WEEK"], "no clock");
        // A long week in a big number still fits.
        let mut big = Weeks::new(1_789_000_000_000, &|_| -3 * 3600);
        big.add(
            Some(1_788_900_000_000),
            400 * 3_600_000,
            Some(1_234_500_000),
            &|_| -3 * 3600,
        );
        list.weeks = Some(big);
        prepare(&list).assert_laid_out(bounds(), &["THIS WEEK", "1234.5"], "big");
    }
}
