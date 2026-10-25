//! Rides: the saved rides, newest first, one card each.
//!
//! MOVE focuses the next ride (wrapping), ACT opens it, BACK goes Home, and
//! a tap opens the ride under the finger. Each card names the ride by its
//! start (OCT 17  7:12) and says how long, how far and how hard it was.
//! Above the first cards sits this week at a glance: distance, moving time,
//! rides, the week's training stress (TSS, from the rides that had power),
//! last week's distance and a bar for each day. Scrolled past the
//! first cards, the header gives way to the RIDES headline and a fourth card.
//! Without a saved ride the screen says how one gets here.
use super::canvas::{Canvas, Shape};
use super::chrome::HEIGHT as SCREEN_HEIGHT;
use super::chrome::{
    ACCENT, Align, BODY, CAPTION, CARD, CONTENT, HEART, POWER, RAISED, Rect, SECONDARY, TERTIARY,
    TITLE, VALUE, WHITE, WIDTH,
};
use super::scroll::Column;
use super::workout::Text;
use crate::clock::{Local, civil};
use crate::ride::week::{DAYS, Weeks};
use core::fmt::Write;

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
}

const LEFT: usize = 10;
const RIGHT: usize = WIDTH - 10;
const RADIUS: usize = 12;
const INSET: usize = 14;
const HEADLINE: usize = CONTENT.top + 36;
const HEIGHT: usize = 54;
const PITCH: usize = HEIGHT + 6;

const HEADER_FIRST: usize = CONTENT.top + 111;
const CAPTION_BASE: usize = CONTENT.top + 16;
const VALUE_BASE: usize = CONTENT.top + 54;
const TIME_BASE: usize = CONTENT.top + 73;
const STRESS_BASE: usize = CONTENT.top + 87;
const LAST_BASE: usize = CONTENT.top + 101;
/// The day bars end on `BARS_BASE`, up to `BARS_HIGH` tall, `BAR_PITCH` apart.
const BARS_BASE: usize = VALUE_BASE;
const BARS_HIGH: usize = 30;
const BAR_WIDE: usize = 10;
const BAR_PITCH: usize = 14;
const BARS_LEFT: usize = RIGHT - 4 - (DAYS - 1) * BAR_PITCH - BAR_WIDE;
/// The most training stress the header names; more reads `9999+`.
const STRESS_MOST: u32 = 9999;
/// Every other day is named: the letters are as wide as the bars are apart.
const DAY_LETTERS: [&[u8]; DAYS] = [b"M", b"", b"W", b"", b"F", b"", b"S"];

/// The week header and every card make one page that scrolls under the
/// status bar: `VIEWPORT` pixels of it show at once. Cards follow the
/// header in this column, in screen coordinates while the page is at the top.
pub const VIEWPORT: usize = SCREEN_HEIGHT - CONTENT.top;
const COLUMN: Column = Column {
    left: LEFT,
    right: RIGHT,
    top: HEADER_FIRST,
    height: HEIGHT,
    pitch: PITCH,
};
/// Room under the last card when the page is scrolled to its end.
const END_MARGIN: usize = 10;
/// A card revealed for the focus keeps this much room from the viewport's edge.
const REVEAL_MARGIN: usize = 6;

/// The page's height, from the top of the viewport.
pub fn content(count: usize) -> usize {
    if count == 0 {
        return VIEWPORT;
    }
    COLUMN.bottom(count) + END_MARGIN - CONTENT.top
}

/// Card `index`'s top and bottom on the page, from the top of the viewport.
fn span(index: usize) -> (i32, i32) {
    let rect = COLUMN.rect(index);
    let top = CONTENT.top as i32;
    (rect.top as i32 - top, rect.bottom as i32 - top)
}

/// Once the header's top line has scrolled under the status bar, a band
/// this tall pins RIDES and the focus's place at the top of the viewport, as
/// iOS folds a large title into its navigation bar. The band's runs sit in
/// the page's first rows, above anything that scrolls, and paint there.
pub const PIN: usize = CONTENT.top;
const PIN_BASE: usize = 18;

/// Whether the band pins with the page scrolled to `offset`.
pub fn pinned(offset: i32) -> bool {
    offset > (CAPTION_BASE - CONTENT.top) as i32
}

/// The viewport's first row that shows the page, past the band.
fn inset(offset: i32) -> i32 {
    if pinned(offset) { PIN as i32 } else { 0 }
}

/// The page's row that screen row `y` shows with the page scrolled to
/// `offset`: the pinned band's own rows inside it, none above the page.
pub fn page_row(y: usize, offset: i32) -> Option<usize> {
    if pinned(offset) && y < CONTENT.top + PIN {
        return Some(y - CONTENT.top);
    }
    usize::try_from(y as i32 + offset).ok()
}

/// Whether any of the week header shows with the page scrolled to `offset`.
pub fn header_shows(offset: i32) -> bool {
    offset < span(0).0
}

/// Whether card `index` shows whole with the page scrolled to `offset`.
pub fn whole(index: usize, offset: i32) -> bool {
    let (top, bottom) = span(index);
    top >= offset + inset(offset) && bottom <= offset + VIEWPORT as i32
}

/// The scroll that shows card `index` whole, moving as little as it can from
/// `offset`. The first card shows the page from its top, header and all.
pub fn reveal(index: usize, offset: i32) -> i32 {
    if index == 0 {
        return 0;
    }
    let (top, bottom) = span(index);
    let margin = REVEAL_MARGIN as i32;
    if top - margin - inset(offset) < offset {
        top - margin - PIN as i32
    } else if bottom + margin > offset + VIEWPORT as i32 {
        bottom + margin - VIEWPORT as i32
    } else {
        offset
    }
}

/// The card to focus once the page stops at `offset` with the focus on
/// `cursor`: the focus itself while it shows whole, else the nearest card
/// that does.
pub fn settled(count: usize, cursor: usize, offset: i32) -> usize {
    if count == 0 || whole(cursor, offset) {
        return cursor;
    }
    let (top, _) = span(cursor);
    let pick = if top < offset {
        (0..count).find(|&index| whole(index, offset))
    } else {
        (0..count).rev().find(|&index| whole(index, offset))
    };
    pick.unwrap_or(cursor)
}

/// The list index of the ride under a tap at screen (`x`, `y`) with the
/// page scrolled to `offset`, if any.
pub fn row_at(count: usize, offset: i32, x: usize, y: usize) -> Option<usize> {
    if y < CONTENT.top + inset(offset) as usize {
        return None;
    }
    let y = usize::try_from(y as i32 + offset).ok()?;
    COLUMN.slot_at(count, x, y)
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
    // Only what shows: the header while any of it does, and the cards whose
    // rows reach the viewport.
    let (top, bottom) = (list.offset, list.offset + VIEWPORT as i32);
    if header_shows(list.offset) {
        header(&mut canvas, list, count.as_bytes());
    }
    if pinned(list.offset) {
        canvas.fit(
            &[&BODY],
            b"RIDES",
            RIGHT - LEFT - 80,
            LEFT + 4,
            PIN_BASE,
            Align::Left,
            WHITE,
        );
        canvas.caption(count.as_bytes(), 80, RIGHT - 4, PIN_BASE, Align::Right);
        // A hairline where the cards pass under the band.
        canvas.shape(Shape::Bar {
            rect: Rect {
                left: 0,
                top: PIN - 1,
                right: WIDTH,
                bottom: PIN,
            },
            radius: 0,
            color: CARD,
        });
    }
    // Card i shows while its bottom is below the top and its top above the
    // bottom: i > (top - bottom of card 0) / pitch, i < (bottom - its top) / pitch.
    let pitch = PITCH as i32;
    let first = ((top - span(0).1).div_euclid(pitch) + 1).max(0) as usize;
    let end = ((bottom - span(0).0 - 1).div_euclid(pitch) + 1).clamp(0, list.count as i32);
    for index in first..end as usize {
        let row = (list.row)(index);
        let rect = COLUMN.rect(index);
        let focused = index == list.cursor;
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
            &[&CAPTION],
            stress.as_bytes(),
            BARS_LEFT - left - 8,
            left,
            STRESS_BASE,
            Align::Left,
            POWER,
        );
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
            offset: 0,
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

    /// The page's whole height in screen rows: runs below the panel are
    /// cards scrolled part way in.
    fn page() -> Rect {
        Rect {
            top: 0,
            bottom: 10_000,
            ..bounds()
        }
    }

    /// The canvas's card panels, top edges in page rows.
    fn cards(canvas: &Canvas) -> std::vec::Vec<usize> {
        canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Panel { rect, .. } => Some(rect.top),
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
        // Three whole cards and the top of a fourth reach the panel.
        assert_eq!(cards(&canvas), [0, 1, 2, 3].map(|i| COLUMN.rect(i).top));
        assert!((0..3).all(|i| whole(i, 0)) && !whole(3, 0));
        let empty = prepare(&list(0, 0));
        empty.assert_laid_out(
            bounds(),
            &["NO RIDES YET", "SAVED RIDES SHOW HERE"],
            "empty",
        );
    }

    #[test]
    fn scrolled_only_the_cards_in_view_are_made_and_taps_find_them() {
        for offset in [-40, 0, 1, 60, 111, 200, 301, reveal(ROWS - 1, 0)] {
            let mut scrolled = list(ROWS, 0);
            scrolled.offset = offset;
            scrolled.weeks = Some(week());
            let canvas = prepare(&scrolled);
            canvas.assert_laid_out(page(), &[], &std::format!("{offset}"));
            assert_eq!(canvas.shows("THIS WEEK"), header_shows(offset), "{offset}");
            // Once the header's top line is gone, RIDES and the place pin.
            assert_eq!(canvas.shows("RIDES"), pinned(offset), "{offset}");
            assert_eq!(canvas.shows("1/8"), header_shows(offset) || pinned(offset));
            let band = CONTENT.top + inset(offset) as usize;
            let (top, bottom) = (CONTENT.top as i32 + offset, 320 + offset);
            let made = cards(&canvas);
            for index in 0..ROWS {
                let rect = COLUMN.rect(index);
                let reaches = (rect.bottom as i32) > top && (rect.top as i32) < bottom;
                assert_eq!(made.contains(&rect.top), reaches, "{offset} {index}");
                // A tap on a card's middle, where it shows on the panel.
                let y = (rect.top + rect.bottom) as i32 / 2 - offset;
                if reaches && y >= band as i32 && y < 320 {
                    assert_eq!(row_at(ROWS, offset, 120, y as usize), Some(index));
                }
            }
            assert_eq!(row_at(ROWS, offset, 120, band - 1), None, "bar or band");
            // The band's rows show the band, the rest the page.
            let shown = if pinned(offset) {
                Some(3)
            } else {
                usize::try_from(CONTENT.top as i32 + 3 + offset).ok()
            };
            assert_eq!(page_row(CONTENT.top + 3, offset), shown);
        }
        // The gap between cards and the margins are no card.
        let gap = COLUMN.rect(0).bottom + 2;
        assert_eq!(row_at(ROWS, 0, 120, gap), None);
        assert_eq!(row_at(ROWS, 0, 4, COLUMN.rect(0).top + 10), None);
        assert_eq!(row_at(1, 0, 120, COLUMN.rect(1).top + 10), None);
    }

    #[test]
    fn the_page_ends_under_the_last_card_and_reveals_move_as_little_as_they_can() {
        let end = content(ROWS) as i32 - VIEWPORT as i32;
        assert_eq!(
            reveal(ROWS - 1, 0),
            end - END_MARGIN as i32 + REVEAL_MARGIN as i32
        );
        assert!(whole(ROWS - 1, reveal(ROWS - 1, 0)));
        // A card already whole needs no scroll; one above scrolls up to it.
        assert_eq!(reveal(1, 0), 0);
        let low = reveal(ROWS - 1, 0);
        assert_eq!(reveal(ROWS - 2, low), low);
        let up = reveal(2, low);
        assert!(up < low && whole(2, up) && !whole(1, up));
        assert_eq!(reveal(0, low), 0, "the first card brings the header");
        assert_eq!(content(0), VIEWPORT);
    }

    #[test]
    fn once_the_page_rests_the_focus_moves_onto_a_card_that_shows_whole() {
        assert_eq!(settled(ROWS, 1, 0), 1);
        // Scrolled down past the focus: the first whole card.
        let low = reveal(ROWS - 1, 0);
        let first = (0..ROWS).find(|&i| whole(i, low)).unwrap();
        assert_eq!(settled(ROWS, 0, low), first);
        // Scrolled up past the focus: the last whole card.
        let last = (0..ROWS).rev().find(|&i| whole(i, 0)).unwrap();
        assert_eq!(settled(ROWS, ROWS - 1, 0), last);
        assert_eq!(settled(0, 0, 0), 0);
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
}
