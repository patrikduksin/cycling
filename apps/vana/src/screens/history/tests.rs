use super::*;
use crate::ride::week::Weeks;
use crate::screens::Device;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, PRESSED, RAISED, Rect, TERTIARY, WHITE};
use crate::screens::workout::{Page, View};
use core::fmt::Write;

/// A list longer than the screen.
const ROWS: usize = 8;

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

/// Short titles beside their heart rates, and a detail of its own on each.
fn short(index: usize) -> Row {
    let mut row = Row::default();
    let _ = write!(row.title, "RIDE {}", index + 1);
    let _ = write!(row.detail, "{}:07:12  32.4 KM", index + 1);
    let _ = row.heart.push_str("141 BPM");
    row
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

/// What a frame of Rides shows.
struct Shown {
    count: usize,
    cursor: usize,
    offset: i32,
    weeks: Option<Weeks>,
    reading: bool,
    pressed: Option<usize>,
}

const TOP: Shown = Shown {
    count: ROWS,
    cursor: 0,
    offset: 0,
    weeks: None,
    reading: false,
    pressed: None,
};

/// Rides composed on `device`, its cards made by `make`.
fn rides_on(
    device: &'static Device,
    shown: Shown,
    make: fn(usize) -> Row,
) -> (View<'static>, Layout) {
    let mut view = View::new(Page::History, 0, None);
    view.device = device;
    view.compose_main();
    let list = *view.layout().expect("Rides scrolls a list");
    view.scroll = shown.offset;
    let mut rides = Rides::new(&list, shown.count, shown.cursor, shown.offset, make);
    rides.glance = Glance::of(shown.weeks.as_ref(), shown.reading);
    rides.pressed = shown.pressed;
    view.show_rides(rides);
    view.prepare();
    (view, list)
}

fn rides(shown: Shown) -> View<'static> {
    rides_on(&crate::screens::C606, shown, row).0
}

/// The screen's rows and the canvas rows its list's cards in view reach.
fn page(view: &View) -> Rect {
    let list = view.layout().expect("Rides scrolls a list");
    Rect {
        bottom: list.stretched_bottom(),
        ..view.device.scaled.screen()
    }
}

fn bars(canvas: &Canvas, color: u16) -> usize {
    let lit = |shape: &&Shape| matches!(shape, Shape::Bar { color: c, .. } if *c == color);
    canvas.shapes.iter().filter(lit).count()
}

#[test]
fn starts_read_as_month_day_and_twelve_hour_time() {
    let october_17: u64 = 20_743 * 86_400;
    let at = |seconds| started(Local(october_17 + seconds));
    assert_eq!(at(7 * 3600 + 12 * 60).as_str(), "OCT 17  7:12");
    assert_eq!(at(21 * 3600).as_str(), "OCT 17  9:00");
    assert_eq!(at(0).as_str(), "OCT 17  12:00");
}

#[test]
fn the_week_and_the_first_cards_show_with_the_place_of_the_focus() {
    let view = rides(Shown {
        weeks: Some(week()),
        ..TOP
    });
    let want = [
        "THIS WEEK",
        "1/8",
        "DEC 31  12:59",
        "10:07:12  132.4 KM",
        "141 BPM",
    ];
    view.canvas().assert_laid_out(page(&view), &want, "top");
}

#[test]
fn the_glance_reads_the_week_and_what_is_known() {
    assert!(
        core::mem::size_of::<Weeks>() <= 208,
        "{}",
        core::mem::size_of::<Weeks>()
    );
    let view = rides(Shown {
        weeks: Some(week()),
        ..TOP
    });
    let canvas = view.canvas();
    // 42.0 + 30.0 + 12.3 km in 3 rides over 2 days; 84 km last week.
    for text in ["84.3", "KM", "3H 30M", "3 RIDES", "LAST WEEK 84 KM"] {
        assert!(canvas.shows(text), "{text}");
    }
    // Monday and Tuesday white (today is Wednesday), the rest dark.
    let lit = (
        bars(canvas, WHITE),
        bars(canvas, ACCENT),
        bars(canvas, TERTIARY),
    );
    assert_eq!(lit, (2, 0, 5));
    // Still reading with nothing found: not a quiet week yet.
    let empty = Weeks::new(1_789_000_000_000, &|_| -3 * 3600);
    let reading = rides(Shown {
        weeks: Some(empty),
        reading: true,
        ..TOP
    });
    assert!(reading.canvas().shows("--") && !reading.canvas().shows("0.0"));
    // The same once everything is read: a week without rides.
    let quiet = rides(Shown {
        weeks: Some(empty),
        ..TOP
    });
    let canvas = quiet.canvas();
    assert!(canvas.shows("0.0") && canvas.shows("0M") && canvas.shows("0 RIDES"));
    canvas.assert_laid_out(page(&quiet), &["THIS WEEK", "LAST WEEK 0 KM"], "quiet");
    // No clock: nothing is known, and there is no last week.
    let unknown = rides(TOP);
    assert!(unknown.canvas().shows("--") && !unknown.canvas().shows("LAST WEEK"));
    // A long week in a big number still fits.
    let mut big = empty;
    big.add(
        Some(1_788_900_000_000),
        400 * 3_600_000,
        Some(1_234_500_000),
        &|_| -3 * 3600,
    );
    let big = rides(Shown {
        weeks: Some(big),
        ..TOP
    });
    big.canvas()
        .assert_laid_out(page(&big), &["THIS WEEK", "1234.5"], "big");
}

#[test]
fn training_stress_shows_only_for_a_week_with_power() {
    let without = rides(Shown {
        weeks: Some(week()),
        ..TOP
    });
    assert!(!without.canvas().shows("TSS"));
    let zone = |_| -3 * 3600;
    let tuesday = (20_746 * 86_400 + 15 * 3600) * 1000;
    let last_week = tuesday - 7 * 86_400_000;
    // Three- and four-digit last-week distances with three- to five-digit
    // stress: none is cut and none overlaps.
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
        let view = rides(Shown {
            weeks: Some(weeks),
            ..TOP
        });
        let last = std::format!("LAST WEEK {km} KM");
        assert!(view.canvas().shows(shown), "{shown}");
        view.canvas()
            .assert_laid_out(page(&view), &[&last, shown, "THIS WEEK"], "stress");
    }
}

#[test]
fn the_focused_card_is_raised_and_ringed_and_one_under_a_resting_finger_pressed() {
    let (mut view, list) = rides_on(
        &crate::screens::C606,
        Shown {
            cursor: 1,
            pressed: Some(0),
            ..TOP
        },
        row,
    );
    let (first, second) = (list.column.rect(0), list.column.rect(1));
    // Left of the words, under the corners' curve.
    let x = list.column.left + 6;
    assert_eq!(view.pixel(x, list.top + first.top + 20), PRESSED);
    assert_eq!(view.pixel(x, list.top + second.top + 20), RAISED);
    assert_eq!(view.pixel(100, list.top + second.top - 2), ACCENT);
}

#[test]
fn long_rides_a_long_week_and_no_rides_lay_out_on_every_panel() {
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
    for device in crate::screens::panels::ALL {
        let (_, list) = rides_on(device, TOP, row);
        let end = list.reveal(ROWS - 1, 0);
        for (count, offset) in [(ROWS, -40), (ROWS, 0), (ROWS, 60), (ROWS, end), (0, 0)] {
            let shown = Shown {
                count,
                cursor: 1,
                offset,
                weeks: Some(weeks),
                reading: false,
                pressed: Some(2),
            };
            let (mut view, _) = rides_on(device, shown, row);
            view.prepare().check_layout();
            let what = std::format!("{count} rides at {offset} on {:?}", device.scaled.panel);
            let want: &[&str] = match (count, offset) {
                (0, _) => &["THIS WEEK", "NO RIDES YET", "SAVED RIDES SHOW HERE"],
                (_, 0) => &["THIS WEEK", "TSS 9999+", "DEC 31  12:59", "141 BPM"],
                _ => &["THIS WEEK"],
            };
            view.canvas().assert_laid_out(page(&view), want, &what);
        }
    }
}

#[test]
fn every_panel_keeps_the_words_of_every_card_in_view() {
    let zone = |_| -3 * 3600;
    let tuesday = (20_746 * 86_400 + 15 * 3600) * 1000;
    let mut powered = week();
    powered.add_stress(Some(tuesday), 1, Some(1), Some(87), &zone);
    let count = 12;
    for device in crate::screens::panels::ALL {
        let (_, list) = rides_on(device, TOP, short);
        for weeks in [None, Some(powered)] {
            for offset in -40..=list.reveal(count - 1, 0) {
                let shown = Shown {
                    count,
                    cursor: 1,
                    offset,
                    weeks,
                    reading: false,
                    pressed: Some(2),
                };
                let (mut view, _) = rides_on(device, shown, short);
                view.prepare().check_layout();
                let what = std::format!("{offset} on {:?}", device.scaled.panel);
                // Every card that shows whole says all it has; check_layout
                // has held the rest to the canvas.
                let whole = list
                    .visible(count, offset)
                    .filter(|&index| list.whole(index, offset));
                for index in whole {
                    for text in [short(index).title, short(index).detail] {
                        let text = text.as_str();
                        assert!(view.canvas().shows(text), "{text} at {what}");
                    }
                }
            }
        }
    }
}
