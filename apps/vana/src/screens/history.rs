//! Rides: the saved rides, newest first, one card each.
//!
//! MOVE focuses the next ride (wrapping), ACT opens it, BACK goes Home, and
//! a tap opens the ride under the finger. Each card names the ride by its
//! start (OCT 17  7:12) and says how long, how far and how hard it was.
//! Without a saved ride the screen says how one gets here.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    Align, BODY, CAPTION, CARD, CONTENT, HEART, RAISED, Rect, SECONDARY, TITLE, WHITE, WIDTH,
};
use super::workout::Text;

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
}

const LEFT: usize = 10;
const RIGHT: usize = WIDTH - 10;
const RADIUS: usize = 12;
const INSET: usize = 14;
const HEADLINE: usize = CONTENT.top + 36;
const FIRST: usize = CONTENT.top + 50;
const HEIGHT: usize = 54;
const PITCH: usize = HEIGHT + 6;

/// The first row on screen, so that the focused one shows.
fn first_shown(cursor: usize) -> usize {
    cursor.saturating_sub(SHOWN - 1)
}

fn card(slot: usize) -> Rect {
    let top = FIRST + slot * PITCH;
    Rect {
        left: LEFT,
        top,
        right: RIGHT,
        bottom: top + HEIGHT,
    }
}

/// The list index of the ride under a tap, if any.
pub fn row_at(count: usize, cursor: usize, x: usize, y: usize) -> Option<usize> {
    let slot = (0..SHOWN).find(|slot| card(*slot).contains(x, y))?;
    let index = first_shown(cursor) + slot;
    (index < count).then_some(index)
}

pub fn prepare<F: Fn(usize) -> Row>(list: &List<F>) -> Canvas {
    let mut canvas = Canvas::default();
    canvas.fit(
        &[&TITLE, &BODY],
        b"RIDES",
        RIGHT - LEFT,
        LEFT + 4,
        HEADLINE,
        Align::Left,
        WHITE,
    );
    if list.count == 0 {
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
    let _ = core::fmt::Write::write_fmt(
        &mut count,
        format_args!("{}/{}", list.cursor + 1, list.count),
    );
    canvas.caption(count.as_bytes(), 80, RIGHT - 4, HEADLINE - 4, Align::Right);
    let first = first_shown(list.cursor);
    for (slot, index) in (first..list.count.min(first + SHOWN)).enumerate() {
        let row = (list.row)(index);
        let rect = card(slot);
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

/// "OCT 17  7:12" in the clock's time zone, from UTC milliseconds.
pub fn started(utc_ms: u64) -> Text<20> {
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    let local = (utc_ms / 1000).saturating_sub(super::workout::UTC_OFFSET_S);
    let (days, seconds) = (local / 86_400, local % 86_400);
    let (_, month, day) = civil(days);
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

/// The proleptic Gregorian date of a day count since 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u8, u8) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
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
        List { count, cursor, row }
    }

    #[test]
    fn dates_read_in_the_clock_time_zone() {
        // 2026-10-17 10:12 UTC is 7:12 in the clock's zone (UTC-3).
        let utc = (20_743 * 86_400 + 10 * 3600 + 12 * 60) * 1000;
        assert_eq!(started(utc).as_str(), "OCT 17  7:12");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        // Midnight UTC is still the day before here.
        assert_eq!(started(20_744 * 86_400_000).as_str(), "OCT 17  9:00");
    }

    #[test]
    fn rows_lay_out_whole_and_the_focused_one_stays_shown() {
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: 320,
        };
        let full = list(ROWS, 0);
        let want = [
            "RIDES",
            "DEC 31  12:59",
            "1/8",
            "10:07:12  132.4 KM",
            "141 BPM",
        ];
        prepare(&full).assert_laid_out(bounds, &want, "list");
        let last = list(ROWS, ROWS - 1);
        assert_eq!(first_shown(last.cursor), ROWS - SHOWN);
        assert!(prepare(&last).shows("8/8"));
        let end = ROWS - 1;
        assert_eq!(row_at(ROWS, end, 120, card(SHOWN - 1).top + 4), Some(end));
        assert_eq!(row_at(ROWS, end, 120, card(0).top + 4), Some(ROWS - SHOWN));
        assert_eq!(row_at(1, 0, 120, card(1).top + 4), None);
        assert_eq!(row_at(ROWS, end, 120, CONTENT.top), None);
        let empty = prepare(&list(0, 0));
        empty.assert_laid_out(bounds, &["NO RIDES YET", "SAVED RIDES SHOW HERE"], "empty");
    }
}
