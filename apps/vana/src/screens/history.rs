//! Rides: the saved rides, newest first, one card each, under this week at
//! a glance. The screen is a tree (`compose::builtin::history`); here is
//! what a frame keeps of the list: the week, and the cards in view, each
//! naming its ride by its start (OCT 17  7:12) and saying how long, how far
//! and how hard it was.
use super::chrome::HEART;
use super::scroll::Layout;
use super::workout::Text;
use crate::clock::{Local, civil};
use crate::compose::data::{Entry, Glance, ListKind};
use core::ops::Range;

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

/// The rides as shown: the week, and only the cards in view, so a frame's
/// view stays small however many rides there are.
#[derive(Clone)]
pub struct Rides {
    /// The cards in view, from `shown.start`.
    rows: heapless::Vec<Row, { ListKind::Rides.most_in_view() }>,
    shown: Range<usize>,
    pub count: usize,
    pub cursor: usize,
    /// The card under a resting finger.
    pub pressed: Option<usize>,
    /// This week, and whether the index is still reading rides.
    pub glance: Glance,
}

impl Default for Rides {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Rides {
    pub const EMPTY: Self = Self {
        rows: heapless::Vec::new(),
        shown: 0..0,
        count: 0,
        cursor: 0,
        pressed: None,
        glance: Glance::UNKNOWN,
    };

    /// `count` rides on `list` focused on `cursor` and scrolled to `offset`,
    /// each card in view made by `row`.
    pub fn new(
        list: &Layout,
        count: usize,
        cursor: usize,
        offset: i32,
        row: impl Fn(usize) -> Row,
    ) -> Self {
        let shown = list.visible(count, offset);
        Self {
            rows: shown.clone().map(row).collect(),
            shown,
            count,
            cursor,
            ..Self::EMPTY
        }
    }

    /// Card `index` as the list draws it, while it is in view.
    pub fn entry(&self, index: usize) -> Option<Entry<'_>> {
        let row = self.rows.get(index.checked_sub(self.shown.start)?)?;
        Some(Entry {
            title: row.title.as_str(),
            detail: row.detail.as_str(),
            note: row.heart.as_str(),
            note_color: HEART,
            note_first: true,
            chosen: false,
        })
    }
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
mod tests;
