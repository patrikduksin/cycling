use super::chrome::{Rect, Row};
use super::sensors::Glance;
use super::theme::Theme;
use super::workout::Text;
use crate::clock::Local;
use crate::ride::zones::TrainingProfile;
use crate::workout::library::{PIECES, Piece};
use crate::workout::model::Name;
use core::fmt::Write;
use heapless::Vec;

/// The sensors the widget shows, after the fix.
pub const SENSORS: usize = 3;

/// What Home shows, prepared once per frame.
#[derive(Clone, Default)]
pub struct Home {
    pub local: Option<Local>,
    pub sensors: Vec<Glance, SENSORS>,
    pub today: Option<Today>,
    pub strip: Strip,
}

/// The workout planned for today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Today {
    pub name: Name,
    pub planned_secs: u32,
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// 1970-01-01, day 0, was a Thursday.
const DAY_ZERO_WEEKDAY: u64 = 4;
const DAY_SECS: u64 = 86_400;

/// "Sunday 24 Nov".
pub fn date(local: Local) -> Text<20> {
    let days = local.0 / DAY_SECS;
    let (_, month, day) = crate::clock::civil(days as i64);
    let weekday = WEEKDAYS[((days + DAY_ZERO_WEEKDAY) % 7) as usize];
    let mut text = Text::new();
    let _ = write!(text, "{weekday} {day} {}", MONTHS[usize::from(month - 1)]);
    text
}

/// "7:10": twelve-hour time without a suffix.
pub fn time(local: Option<Local>) -> Text<8> {
    let mut text = Text::new();
    match local {
        Some(local) => {
            let minutes = local.of_day() / 60;
            let hour = minutes / 60;
            let _ = write!(text, "{}:{:02}", (hour + 11) % 12 + 1, minutes % 60);
        }
        None => {
            let _ = text.push_str("--:--");
        }
    }
    text
}

/// The planned time as hours and minutes, none for an untimed workout.
pub fn planned(secs: u32) -> Option<Text<8>> {
    let minutes = secs.div_ceil(60);
    let mut text = Text::new();
    let _ = write!(text, "{}:{:02}", minutes / 60, minutes % 60);
    (secs > 0).then_some(text)
}

/// Today's workout as its card's strip draws it, in pixels: one bar a
/// step, as wide as it is long and as tall as it is hard, in its power
/// zone's colour.
#[derive(Clone, Debug, Default, Hash)]
pub struct Strip {
    bars: Vec<Bar, PIECES>,
}

#[derive(Clone, Copy, Debug, Hash)]
struct Bar {
    left: u16,
    right: u16,
    top: u16,
    bottom: u16,
    zone: u8,
}

/// Heights are in fifteenths of the strip.
const SHARES: u32 = 15;
/// The hardest step's height, and the least any step keeps.
const TALLEST: u32 = 14;
const LOWEST: u32 = 4;
const SEAM: usize = 1;

impl Strip {
    pub fn lay(pieces: &[Piece], rect: Rect) -> Self {
        let total: u32 = pieces.iter().map(|piece| u32::from(piece.secs)).sum();
        let hardest = pieces.iter().map(|piece| piece.pct).max().unwrap_or(0);
        let mut bars = Vec::new();
        if total == 0 || rect.width() == 0 {
            return Self { bars };
        }
        let percent = TrainingProfile {
            ftp_watts: 100,
            ..TrainingProfile::default()
        };
        let height = rect.height() as u32;
        let width = rect.width() as u32;
        let mut elapsed = 0u32;
        for piece in pieces.iter().filter(|piece| piece.secs > 0) {
            let left = rect.left + (elapsed * width / total) as usize;
            elapsed += u32::from(piece.secs);
            let right = rect.left + (elapsed * width / total) as usize;
            let share = u32::from(piece.pct) * TALLEST / u32::from(hardest.max(1));
            let tall = height * share.max(LOWEST) / SHARES;
            let right = right.saturating_sub(SEAM).max(left + 1);
            let _ = bars.push(Bar {
                left: left as u16,
                right: right.min(rect.right) as u16,
                top: (rect.bottom - tall as usize) as u16,
                bottom: rect.bottom as u16,
                zone: percent.power_zone(u16::from(piece.pct)),
            });
        }
        Self { bars }
    }

    #[cfg(test)]
    pub fn pixel(&self, theme: &Theme, x: usize, y: usize) -> Option<u16> {
        let (x, y) = (x as u16, y as u16);
        let bar =
            self.bars.iter().rev().find(|bar| {
                (bar.left..bar.right).contains(&x) && (bar.top..bar.bottom).contains(&y)
            })?;
        Some(theme.zone(bar.zone))
    }

    pub fn paint(&self, theme: &Theme, row: &mut Row) {
        let y = row.y() as u16;
        for bar in &self.bars {
            if (bar.top..bar.bottom).contains(&y) {
                let x = usize::from(bar.left)..usize::from(bar.right);
                row.span(x, theme.zone(bar.zone));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_reads_as_a_watch_face_says_it() {
        let days = 20_781;
        assert_eq!(
            date(Local(days * DAY_SECS + 3_600)).as_str(),
            "Tuesday 24 Nov"
        );
        assert_eq!(date(Local(0)).as_str(), "Thursday 1 Jan");
        assert_eq!(time(Some(Local(19 * 3_600 + 10 * 60))).as_str(), "7:10");
        assert_eq!(time(None).as_str(), "--:--");
    }

    #[test]
    fn the_planned_time_is_hours_and_minutes() {
        assert_eq!(planned(64 * 60).unwrap().as_str(), "1:04");
        assert_eq!(planned(59).unwrap().as_str(), "0:01");
        assert!(planned(0).is_none());
    }

    #[test]
    fn each_step_is_a_bar_as_wide_as_it_is_long_in_its_zone() {
        let pieces = [
            Piece { secs: 600, pct: 65 },
            Piece { secs: 720, pct: 92 },
            Piece { secs: 240, pct: 50 },
            Piece { secs: 0, pct: 0 },
        ];
        let rect = Rect {
            left: 10,
            top: 0,
            right: 166,
            bottom: 20,
        };
        let strip = Strip::lay(&pieces, rect);
        let bars = &strip.bars;
        assert_eq!(bars.len(), 3, "a step of no length draws nothing");
        assert_eq!((bars[0].left, bars[0].right), (10, 69));
        assert_eq!((bars[1].left, bars[1].right), (70, 141));
        assert_eq!(bars[2].right, 165);
        let zones: std::vec::Vec<_> = bars.iter().map(|bar| bar.zone).collect();
        assert_eq!(zones, [2, 4, 1]);
        assert_eq!(u32::from(bars[1].top), 20 - 20 * TALLEST / SHARES);
        assert!(bars[2].top > bars[0].top && bars[0].top > bars[1].top);
    }
}
