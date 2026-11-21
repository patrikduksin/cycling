//! This week at a glance, over the rides: the caption, the week's rides and
//! the focus's place in the list on top, then distance and moving time on
//! the left, a bar for each day on the right and last week's distance
//! under. Without a clock nothing is known (`--`); while the index still
//! reads, what it has found so far shows.
use crate::compose::data::{Data, ListKind};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::ride::week::DAYS;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, POWER, Rect, SECONDARY, TERTIARY, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct WeekGlance;

/// The text keeps this far inside the cards' columns.
const INSET_UNITS: usize = 4;
/// Each line's band, its capitals ending on the band's bottom: the
/// caption, the distance, the moving time, the stress and last week.
const BANDS: [usize; 5] = [16, 38, 19, 14, 14];
const DAYS_UNITS: usize = 94;
const LETTER_UNITS: usize = 2;
const BAR_UNITS: usize = 10;
const BAR_GAP_UNITS: usize = 4;
const BAR_TALL_UNITS: usize = 30;
/// The most training stress it names; more reads `9999+`.
const STRESS_MOST: u32 = 9999;
const DAY_LETTERS: [&[u8]; DAYS] = [b"M", b"", b"W", b"", b"F", b"", b"S"];

impl Tile for WeekGlance {
    type Arg = ();
    const NAME: &'static str = "glance";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(2 * INSET_UNITS + DAYS_UNITS + 90) as u16,
            height: at.unit(BANDS.iter().sum()) as u16,
            shapes: DAYS as u8,
            runs: 12,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let face = |role| at.face(role);
        let (left, right) = (
            rect.left + at.unit(INSET_UNITS),
            rect.right - at.unit(INSET_UNITS),
        );
        let mut bottom = rect.top;
        let [caption, value, time, stress, last] = BANDS.map(|band| {
            bottom += at.unit(band);
            bottom
        });
        // The last day's letter, centred on its bar, keeps inside the box.
        let days_left = right - at.unit(DAYS_UNITS + LETTER_UNITS);
        let count = data.list(ListKind::Rides);
        let mut place = Text::<12>::new();
        if let Some(focus) = data.listed(ListKind::Rides).focus.filter(|_| count > 0) {
            let _ = write!(place, "{}/{}", focus + 1, count);
        }
        let title = out.caption(at, b"THIS WEEK", at.unit(100), left, caption, Align::Left);
        let place = out.caption(
            at,
            place.as_bytes(),
            at.unit(80),
            right,
            caption,
            Align::Right,
        );
        let glance = data.glance();
        let weeks = glance.week;
        // Nothing found yet while reading is not a quiet week.
        let known = weeks.filter(|week| !glance.reading || week.rides > 0);
        let mut km = Text::<12>::new();
        let distance: &[u8] = match known {
            Some(weeks) => {
                let tenths = (weeks.distance_mm + 50_000) / 100_000;
                let _ = write!(km, "{}.{}", tenths / 10, tenths % 10);
                km.as_bytes()
            }
            None => b"--",
        };
        let run = out.fit(
            &[face(FaceRole::Value), face(FaceRole::Body)],
            distance,
            days_left - at.unit(30) - left,
            left,
            value,
            Align::Left,
            WHITE,
        );
        out.fit(
            &[face(FaceRole::Caption)],
            b"KM",
            at.unit(30),
            run.right() + at.unit(6),
            value,
            Align::Left,
            SECONDARY,
        );
        let mut moving = Text::<12>::new();
        match known {
            Some(weeks) => moving_time(&mut moving, weeks.active_ms / 1000),
            None => {
                let _ = moving.push_str("--");
            }
        }
        let beside_days = days_left - at.unit(8) - left;
        let body = [face(FaceRole::Body)];
        out.fit(
            &body,
            moving.as_bytes(),
            beside_days,
            left,
            time,
            Align::Left,
            WHITE,
        );
        if let Some(weeks) = known {
            let mut rides = Text::<12>::new();
            let noun = if weeks.rides == 1 { "RIDE" } else { "RIDES" };
            let _ = write!(rides, "{} {noun}", weeks.rides);
            let x = title.right() + at.unit(12);
            let room = place.left.saturating_sub(x + at.unit(8));
            out.caption(at, rides.as_bytes(), room, x, caption, Align::Left);
        }
        if let Some(weeks) = weeks {
            let mut text = Text::<20>::new();
            let kilometres = (weeks.last_mm + 500_000) / 1_000_000;
            let _ = write!(text, "LAST WEEK {kilometres} KM");
            out.caption(
                at,
                text.as_bytes(),
                rect.right - left,
                left,
                last,
                Align::Left,
            );
        }
        // This week's training stress, from the rides that had power; a week
        // without any says nothing rather than 0.
        if let Some(tss) = known.and_then(|week| week.stress) {
            let mut text = Text::<16>::new();
            let _ = if tss > STRESS_MOST {
                write!(text, "TSS {STRESS_MOST}+")
            } else {
                write!(text, "TSS {tss}")
            };
            let caption = [face(FaceRole::Caption)];
            out.fit(
                &caption,
                text.as_bytes(),
                beside_days,
                left,
                stress,
                Align::Left,
                POWER,
            );
        }
        // One bar a day, Monday first: dark stubs for days without a ride,
        // white for ridden days (blue for today), tall in proportion to the
        // most.
        let most = weeks.map_or(0, |w| w.day_m.iter().copied().max().unwrap_or(0));
        let tallest = at.unit(BAR_TALL_UNITS);
        for (day, letter) in DAY_LETTERS.iter().enumerate() {
            let bar_left = days_left + day * at.unit(BAR_UNITS + BAR_GAP_UNITS);
            let bar = Rect {
                left: bar_left,
                top: value - tallest,
                right: bar_left + at.unit(BAR_UNITS),
                bottom: value,
            };
            let today = weeks.is_some_and(|w| usize::from(w.today) == day);
            let ridden = weeks.is_some_and(|w| w.rode(day));
            let distance = weeks.map_or(0, |w| w.day_m[day]);
            let high = if ridden {
                let scaled = (u64::from(distance) * tallest as u64)
                    .checked_div(u64::from(most))
                    .unwrap_or(0);
                usize::try_from(scaled).unwrap_or(tallest).max(at.unit(6))
            } else {
                at.unit(3)
            };
            out.shape(Shape::Bar {
                rect: Rect {
                    top: value - high,
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
            let caption = [face(FaceRole::Caption)];
            let color = if today { ACCENT } else { SECONDARY };
            out.fit(
                &caption,
                letter,
                at.unit(20),
                bar.center_x(),
                time,
                Align::Center,
                color,
            );
        }
    }
}

/// Moving time as "5H 24M", or "24M" under an hour.
fn moving_time(text: &mut Text<12>, seconds: u64) {
    let minutes = seconds / 60;
    let _ = if minutes >= 60 {
        write!(text, "{}H {:02}M", minutes / 60, minutes % 60)
    } else {
        write!(text, "{minutes}M")
    };
}
