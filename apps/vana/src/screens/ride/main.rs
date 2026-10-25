//! The main ride page, stacked like a workout view: elapsed time and grade,
//! then speed as the hero number, power and heart rate with zone gauges,
//! then two readings the rider chose, cadence and distance by default. Each value has its signature colour and a
//! small unit beside it; there are no boxes or dividers. During a workout a
//! white bar under the power (or heart-rate) gauge spans the step's target.
use super::slots::Reading;
use super::zones::GAUGE_WIDTH;
use super::{BODY, Labels, Smooth, Text, elapsed, speed, whole};
use crate::screens::chrome::{
    Align, BLACK, BODY as BODY_FACE, CAPTION, GRADE, HEART, HERO, LARGE, Label, Line, POWER, Rect,
    Row, SECONDARY, SPEED, TERTIARY, VALUE, WHITE, blend, rounded, zone_color,
};
use crate::screens::workout::View;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, SPILL};

const LEFT: usize = BODY.left + 10;
const RIGHT: usize = BODY.right - 8;
const TIME_TOP: usize = BODY.top + 4;
const SPEED_TOP: usize = BODY.top + 38;
const POWER_TOP: usize = BODY.top + 108;
const HEART_TOP: usize = BODY.top + 166;
const LOWER_TOP: usize = BODY.top + 228;
/// Zone gauges sit just under the power and heart-rate numbers.
const GAUGES: [usize; 2] = [POWER_TOP + 47, HEART_TOP + 47];
const GAUGE_HEIGHT: usize = 5;
const GAUGE_GAP: usize = 3;
/// The workout target sits this far under its gauge, this tall.
const BAND_GAP: usize = 2;
const BAND_HEIGHT: usize = 2;
/// Power has seven zones, heart rate five.
const SEGMENTS: [usize; 2] = [7, 5];

/// The bottom of `face`'s capitals for a label whose top is `top`.
const fn baseline(face: &Face, top: usize) -> usize {
    top + face.top as usize + face.cap as usize
}

/// A label whose capitals end on `base`.
fn on_baseline(
    face: &'static Face,
    text: &[u8],
    x: usize,
    base: usize,
    align: Align,
    color: u16,
) -> Label<12> {
    let top = base - face.top as usize - face.cap as usize;
    Label::new(Line::new(face, text), x, top, align, color)
}

/// The least space between two readings on one line.
const GAP: usize = 8;
/// Space between a value and its unit.
const UNIT_GAP: usize = 4;

/// A value and its unit just after it on baseline `base`, both inside
/// `left..right`: the value takes the largest of `faces` that leaves room.
#[allow(clippy::too_many_arguments)]
fn reading(
    labels: &mut Labels,
    faces: &[&'static Face],
    value: &Text<12>,
    unit: &[u8],
    (left, right): (usize, usize),
    base: usize,
    align: Align,
    color: u16,
) {
    let unit_width = CAPTION.width(unit) + SPILL;
    let room = right.saturating_sub(left + UNIT_GAP + unit_width);
    let line = Line::fit(faces, value.as_bytes(), room);
    let face = line.face();
    let top = base - face.top as usize - face.cap as usize;
    let (value, unit) = match align {
        Align::Right => {
            let unit = on_baseline(&CAPTION, unit, right - SPILL, base, Align::Right, SECONDARY);
            let value = Label::new(line, unit.left - UNIT_GAP, top, Align::Right, color);
            (value, unit)
        }
        _ => {
            let value = Label::new(line, left, top, Align::Left, color);
            // After the value's ink, so a "--" keeps its unit as close as
            // a number does.
            let at = left + face.ink_width(value.line.text()) + UNIT_GAP;
            (
                value,
                on_baseline(&CAPTION, unit, at, base, Align::Left, SECONDARY),
            )
        }
    };
    let _ = labels.push(value);
    let _ = labels.push(unit);
}

pub(super) fn prepare(view: &View) -> Smooth {
    let sample = view.sample;
    let mut smooth = Smooth::default();
    let labels = &mut smooth.labels;
    let value = |write: &dyn Fn(&mut Text<12>)| {
        let mut text = Text::new();
        write(&mut text);
        text
    };

    // Time and grade share a line; the grade keeps its size and the clock
    // steps down when hours of riding make it long.
    let grade = value(&|v| {
        let _ = match sample.gradient_tenths {
            Some(g) => write!(
                v,
                "{}{}.{}%",
                if g < 0 { "-" } else { "" },
                g.unsigned_abs() / 10,
                g.unsigned_abs() % 10
            ),
            None => v.push_str("--%"),
        };
    });
    let base = baseline(&VALUE, TIME_TOP);
    let grade = on_baseline(
        &BODY_FACE,
        grade.as_bytes(),
        RIGHT - SPILL,
        base,
        Align::Right,
        GRADE,
    );
    let time = value(&|v| elapsed(v, view.active_secs));
    let line = Line::fit(
        &[&VALUE, &BODY_FACE],
        time.as_bytes(),
        grade.left - GAP - LEFT,
    );
    let top = base - line.face().top as usize - line.face().cap as usize;
    let _ = labels.push(Label::new(line, LEFT, top, Align::Left, WHITE));
    let _ = labels.push(grade);

    let kmh = value(&|v| speed(v, sample.speed_mm_s));
    let base = baseline(&HERO, SPEED_TOP);
    let faces: [&Face; 3] = [&HERO, &LARGE, &VALUE];
    reading(
        labels,
        &faces,
        &kmh,
        b"KM/H",
        (LEFT, RIGHT),
        base,
        Align::Left,
        SPEED,
    );

    smooth.zones = [
        view.power_3s
            .map_or(0, |watts| view.profile.power_zone(watts)),
        sample
            .heart_bpm
            .map_or(0, |bpm| view.profile.heart_zone(bpm)),
    ];
    smooth.band = target_band(view);
    let watts = value(&|v| whole(v, view.power_3s));
    let bpm = value(&|v| whole(v, sample.heart_bpm));
    let rows = [
        (watts, &b"W"[..], POWER_TOP, POWER),
        (bpm, &b"BPM"[..], HEART_TOP, HEART),
    ];
    for ((text, unit, top, color), zone) in rows.into_iter().zip(smooth.zones) {
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
        let base = baseline(&LARGE, top);
        let zone = on_baseline(
            &BODY_FACE,
            name.as_bytes(),
            RIGHT - SPILL,
            base,
            Align::Right,
            zone_color(zone),
        );
        let faces: [&Face; 2] = [&LARGE, &VALUE];
        let span = (LEFT, zone.left - GAP);
        reading(labels, &faces, &text, unit, span, base, Align::Left, color);
        let _ = labels.push(zone);
    }

    let [left, right] = [view.main_slots.left, view.main_slots.right];
    let mut shown = [Text::new(), Text::new()];
    for (reading, text) in [left, right].into_iter().zip(&mut shown) {
        reading.write(view, text);
    }
    lower(labels, [left, right], &shown);
    smooth
}

/// The last line: the chosen readings split it between them, the first
/// from the left edge and the second from the right.
fn lower(labels: &mut Labels, readings: [Reading; 2], values: &[Text<12>; 2]) {
    let middle = (LEFT + RIGHT) / 2;
    let base = baseline(&VALUE, LOWER_TOP);
    let faces: [&Face; 2] = [&VALUE, &BODY_FACE];
    let [left, right] = readings;
    let span = (LEFT, middle - GAP / 2);
    reading(
        labels,
        &faces,
        &values[0],
        left.unit(),
        span,
        base,
        Align::Left,
        left.color(),
    );
    let span = (middle + GAP / 2, RIGHT);
    reading(
        labels,
        &faces,
        &values[1],
        right.unit(),
        span,
        base,
        Align::Right,
        right.color(),
    );
}

/// Where a reading falls on gauge `i`: the zone it is in, and the share of
/// that zone's segment below it.
fn gauge_x(view: &View, i: usize, value: u16) -> usize {
    let profile = &view.profile;
    let (zone, (low, high)) = if i == 0 {
        let zone = profile.power_zone(value);
        (zone, profile.power_range(zone))
    } else {
        let zone = profile.heart_zone(value);
        (zone, profile.heart_range(zone))
    };
    let pitch = (RIGHT - LEFT + GAUGE_GAP) / SEGMENTS[i];
    let width = pitch - GAUGE_GAP;
    let within = u32::from(value).clamp(low, high) - low;
    LEFT + usize::from(zone - 1) * pitch + width * within as usize / (high - low).max(1) as usize
}

/// The step's target band under its gauge, while a workout step has one.
fn target_band(view: &View) -> Option<(usize, core::ops::Range<usize>)> {
    use crate::workout::model::Metric;
    let training = view.training.as_ref().filter(|t| !t.done)?;
    let band = training.now.band?;
    let i = match band.metric {
        Metric::Power => 0,
        Metric::Heart => 1,
        Metric::Cadence => return None,
    };
    let from = gauge_x(view, i, band.low);
    let to = gauge_x(view, i, band.high).max(from + 3).min(RIGHT);
    Some((i, from..to))
}

fn band_rows(i: usize) -> core::ops::Range<usize> {
    let top = GAUGES[i] + GAUGE_HEIGHT + BAND_GAP;
    top..top + BAND_HEIGHT
}

/// Paint a body row: a gauge row, or the labels that cover it.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    let smooth = view.smooth();
    if let Some((i, columns)) = &smooth.band
        && band_rows(*i).contains(&y)
    {
        row.span(columns.clone(), WHITE);
    }
    for (i, &top) in GAUGES.iter().enumerate() {
        if (top..top + GAUGE_HEIGHT).contains(&y) {
            let mark = [view.marks.power, view.marks.heart][i];
            let fill = mark.map_or(0, |mark| mark.fill);
            paint_gauge(row, smooth.zones[i], fill, SEGMENTS[i], top);
            return;
        }
    }
    for label in &smooth.labels {
        label.paint(row);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(view: &View, x: usize, y: usize) -> u16 {
    let smooth = view.smooth();
    if let Some((i, columns)) = &smooth.band
        && band_rows(*i).contains(&y)
        && columns.contains(&x)
    {
        return WHITE;
    }
    for (i, &top) in GAUGES.iter().enumerate() {
        if (top..top + GAUGE_HEIGHT).contains(&y) {
            let mark = [view.marks.power, view.marks.heart][i];
            let fill = mark.map_or(0, |mark| mark.fill);
            return gauge(smooth.zones[i], fill, SEGMENTS[i], x, y, top);
        }
    }
    for label in &smooth.labels {
        if let Some(color) = label.pixel(x, y, BLACK) {
            return color;
        }
    }
    BLACK
}

/// `gauge` as spans: each segment lit up to its split, then the dim track,
/// with only its rounded ends worked out per pixel.
fn paint_gauge(row: &mut Row, zone: u8, fill: usize, segments: usize, top: usize) {
    let y = row.y();
    let pitch = (RIGHT - LEFT + GAUGE_GAP) / segments;
    let width = pitch - GAUGE_GAP;
    let lit = usize::from(zone);
    for segment in 0..segments {
        let left = LEFT + segment * pitch;
        let split = if segment + 1 < lit {
            width
        } else if segment + 1 == lit {
            width * fill / GAUGE_WIDTH
        } else {
            0
        };
        row.span(left..left + split, zone_color(segment as u8 + 1));
        row.span(left + split..left + width, TERTIARY);
        let end = |x| Some(gauge(zone, fill, segments, x, y, top));
        row.each(left..left + 2, end);
        row.each(left + width - 2..left + width, end);
    }
}

/// A row of rounded segments, one per zone: those below `zone` lit in their
/// colours, the current one lit `fill` / `GAUGE_WIDTH` of the way through
/// its range, the rest a dim track.
fn gauge(zone: u8, fill: usize, segments: usize, x: usize, y: usize, top: usize) -> u16 {
    if !(LEFT..RIGHT).contains(&x) {
        return BLACK;
    }
    let pitch = (RIGHT - LEFT + GAUGE_GAP) / segments;
    let segment = (x - LEFT) / pitch;
    let left = LEFT + segment * pitch;
    let rect = Rect {
        left,
        top,
        right: left + pitch - GAUGE_GAP,
        bottom: top + GAUGE_HEIGHT,
    };
    let zone = usize::from(zone);
    let width = pitch - GAUGE_GAP;
    let lit = segment + 1 < zone || (segment + 1 == zone && x - left < width * fill / GAUGE_WIDTH);
    let color = if lit {
        zone_color(segment as u8 + 1)
    } else {
        TERTIARY
    };
    blend(color, BLACK, rounded(rect, 2, x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::{Sample, Status};
    use crate::screens::chrome::layout::{assert_apart, assert_shown, text};
    use crate::screens::ride::kilometres;
    use crate::screens::ride::slots::Slots;
    use crate::screens::workout::Page;
    use std::string::String;
    use std::vec::Vec;

    /// One extreme reading of each kind the main page shows.
    #[derive(Clone, Copy)]
    struct Case {
        speed_mm_s: Option<u32>,
        watts: Option<u16>,
        bpm: Option<u16>,
        cadence_tenths: Option<u16>,
        distance_mm: u64,
        secs: u64,
        grade_tenths: Option<i16>,
        state: Status,
    }

    const SPEEDS: [Option<u32>; 5] = [None, Some(0), Some(2_750), Some(27_750), Some(33_334)];
    const WATTS: [Option<u16>; 4] = [None, Some(0), Some(999), Some(1999)];
    const BPMS: [Option<u16>; 3] = [None, Some(40), Some(220)];
    const CADENCES: [Option<u16>; 4] = [None, Some(0), Some(1000), Some(1800)];
    const DISTANCES: [u64; 4] = [0, 15_000_000, 150_000_000, 1_000_000_000];
    const TIMES: [u64; 4] = [0, 3_599, 36_000, 359_999];
    const GRADES: [Option<i16>; 4] = [None, Some(0), Some(-255), Some(199)];
    const STATES: [Status; 4] = [
        Status::Ready,
        Status::Recording,
        Status::Paused,
        Status::Saved,
    ];

    /// The widest of everything at once.
    const WIDEST: Case = Case {
        speed_mm_s: Some(33_334),
        watts: Some(1999),
        bpm: Some(220),
        cadence_tenths: Some(1800),
        distance_mm: 1_000_000_000,
        secs: 359_999,
        grade_tenths: Some(-255),
        state: Status::Recording,
    };

    fn view(case: Case) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
        view.state = case.state;
        view.active_secs = case.secs;
        let sample = Sample {
            speed_mm_s: case.speed_mm_s,
            power_watts: case.watts,
            heart_bpm: case.bpm,
            cadence_tenths: case.cadence_tenths,
            gradient_tenths: case.grade_tenths,
            ..Sample::default()
        };
        view.totals.add(1000, case.distance_mm, &sample, None);
        view.sample = sample;
        view.power_3s = case.watts;
        view.prepare();
        view
    }

    fn items(view: &View) -> Vec<(String, Rect)> {
        let mut items: Vec<_> = view.smooth().labels.iter().filter_map(text).collect();
        for (i, &top) in GAUGES.iter().enumerate() {
            let rect = Rect {
                left: LEFT,
                top,
                right: RIGHT,
                bottom: top + GAUGE_HEIGHT,
            };
            items.push((std::format!("gauge {i}"), rect));
        }
        if let Some((i, columns)) = &view.smooth().band {
            let rows = band_rows(*i);
            let rect = Rect {
                left: columns.start,
                top: rows.start,
                right: columns.end,
                bottom: rows.end,
            };
            items.push(("band".into(), rect));
        }
        items
    }

    /// A workout's target sits under the gauge of its metric, over the
    /// watts it asks for, clear of every reading.
    #[test]
    fn a_workout_target_spans_its_watts_under_the_power_gauge() {
        use crate::screens::ride::Training;
        use crate::workout::library::{ENDURANCE, VO2_MAX};
        use crate::workout::player::Player;
        for workout in [VO2_MAX, ENDURANCE] {
            let mut player = Player::new(workout);
            player.skip();
            for case in cases() {
                let mut view = view(case);
                view.training = Some(Training::of(&player, &view.profile));
                view.prepare();
                let (gauge, columns) = view.smooth().band.clone().expect("a power target");
                assert_eq!(gauge, 0);
                let band = player.now(&view.profile).band.unwrap();
                assert_eq!(columns.start, gauge_x(&view, 0, band.low));
                assert!(columns.end > columns.start + 2);
                let bounds = Rect {
                    left: BODY.left,
                    top: BODY.top,
                    right: BODY.right,
                    bottom: BODY.bottom,
                };
                assert_apart(&items(&view), bounds, "workout band");
            }
        }
    }

    /// Every case: the widest of everything, then each extreme of one
    /// reading with the rest at their widest.
    fn cases() -> Vec<Case> {
        let mut cases = std::vec![WIDEST];
        let mut vary = |set: &dyn Fn(&mut Case)| {
            let mut case = WIDEST;
            set(&mut case);
            cases.push(case);
        };
        for v in SPEEDS {
            vary(&|c| c.speed_mm_s = v);
        }
        for v in WATTS {
            vary(&|c| c.watts = v);
        }
        for v in BPMS {
            vary(&|c| c.bpm = v);
        }
        for v in CADENCES {
            vary(&|c| c.cadence_tenths = v);
        }
        for v in DISTANCES {
            vary(&|c| c.distance_mm = v);
        }
        for v in TIMES {
            vary(&|c| c.secs = v);
        }
        for v in GRADES {
            vary(&|c| c.grade_tenths = v);
        }
        for v in STATES {
            vary(&|c| c.state = v);
        }
        cases
    }

    /// The widest value each reading can plausibly take, by stored id: a
    /// 9999 kcal, a 1999 W reading, 220 bpm, 120 km/h, 29 999 m of climb.
    const WIDEST_VALUES: [&str; 9] = [
        "255", "1000.0", "1999", "1999", "220", "120.0", "9999", "99999", "29999",
    ];

    /// Every reading in either slot, against every reading in the other, at
    /// its widest value, keeps its value whole and clear of its neighbours.
    #[test]
    fn every_reading_fits_either_slot_at_its_widest_value() {
        let bounds = Rect {
            left: BODY.left,
            top: BODY.top,
            right: BODY.right,
            bottom: BODY.bottom,
        };
        for left in Reading::ALL {
            for right in Reading::ALL {
                let mut labels = Labels::new();
                let mut texts = [Text::<12>::new(), Text::new()];
                for (reading, text) in [left, right].into_iter().zip(&mut texts) {
                    let _ = text.push_str(WIDEST_VALUES[usize::from(reading.id())]);
                }
                lower(&mut labels, [left, right], &texts);
                let items: Vec<_> = labels.iter().filter_map(text).collect();
                let what = std::format!("{} | {}", left.name(), right.name());
                assert_apart(&items, bounds, &what);
                let units = [
                    core::str::from_utf8(left.unit()).unwrap(),
                    core::str::from_utf8(right.unit()).unwrap(),
                ];
                let want = [
                    WIDEST_VALUES[usize::from(left.id())],
                    WIDEST_VALUES[usize::from(right.id())],
                    units[0],
                    units[1],
                ];
                assert_shown(&items, &want, &what);
            }
        }
    }

    /// The defaults draw the cadence and the distance, as the page always did.
    #[test]
    fn the_default_slots_show_cadence_then_distance() {
        let view = view(WIDEST);
        assert_eq!(view.main_slots, Slots::default());
        let items = items(&view);
        let at = |name: &str| items.iter().find(|(n, _)| n == name).unwrap().1;
        assert!(at("RPM").right < at("KM").left);
    }

    /// Chosen readings replace the defaults, with the totals' own values.
    #[test]
    fn chosen_readings_show_their_values_and_units() {
        let mut view = view(WIDEST);
        view.main_slots = Slots {
            left: Reading::AvgSpeed,
            right: Reading::Ascent,
        };
        view.prepare();
        let items = items(&view);
        let names: Vec<&str> = items.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"AVG") && names.contains(&"M"), "{names:?}");
        assert!(
            !names.contains(&"RPM") && !names.contains(&"KM"),
            "{names:?}"
        );
        assert!(names.contains(&"120.0"), "{names:?}");
    }

    /// A unit follows the ink of its value: "--" keeps it as close as a
    /// number does.
    #[test]
    fn units_follow_the_value_they_belong_to() {
        let gaps = |case: Case| {
            let items = items(&view(case));
            ["KM/H", "W", "BPM", "RPM"].map(|unit| {
                let unit = items.iter().find(|(n, _)| n == unit).unwrap().1;
                // The nearest ink before the unit on its line: its value.
                // Dashes sit at mid height, above the unit's capitals.
                let value = items
                    .iter()
                    .filter(|(name, _)| !name.starts_with("gauge"))
                    .map(|(_, rect)| *rect)
                    .filter(|rect| rect.right <= unit.left)
                    .filter(|rect| rect.top < unit.bottom && unit.top < rect.bottom + 40)
                    .max_by_key(|rect| rect.right)
                    .unwrap();
                unit.left - value.right
            })
        };
        let empty = gaps(Case {
            speed_mm_s: None,
            watts: None,
            bpm: None,
            cadence_tenths: None,
            ..WIDEST
        });
        let full = gaps(Case {
            speed_mm_s: Some(8334),
            watts: Some(250),
            bpm: Some(140),
            cadence_tenths: Some(900),
            ..WIDEST
        });
        for (empty, full) in empty.iter().zip(&full) {
            assert!(empty.abs_diff(*full) <= 2, "{empty:?} vs {full:?}");
            assert!((UNIT_GAP..=UNIT_GAP + 3).contains(empty), "{empty:?}");
        }
    }

    #[test]
    fn extreme_readings_never_overlap_or_clip() {
        let bounds = Rect {
            left: BODY.left,
            top: BODY.top,
            right: BODY.right,
            bottom: BODY.bottom,
        };
        for case in cases() {
            let view = view(case);
            let what = std::format!(
                "{:?} km/h, {:?} W, {:?} bpm, {:?} rpm/10, {} mm, {} s, {:?}",
                case.speed_mm_s,
                case.watts,
                case.bpm,
                case.cadence_tenths,
                case.distance_mm,
                case.secs,
                case.grade_tenths
            );
            let items = items(&view);
            assert_apart(&items, bounds, &what);
            let mut want = Vec::new();
            let mut format = |write: &dyn Fn(&mut Text<12>)| {
                let mut text = Text::new();
                write(&mut text);
                want.push(String::from(text.as_str()));
            };
            format(&|v| speed(v, case.speed_mm_s));
            format(&|v| whole(v, case.watts));
            format(&|v| whole(v, case.bpm));
            format(&|v| whole(v, case.cadence_tenths.map(|t| (t + 5) / 10)));
            format(&|v| kilometres(v, case.distance_mm));
            format(&|v| elapsed(v, case.secs));
            let want: Vec<&str> = want
                .iter()
                .map(String::as_str)
                .chain(["KM/H", "W", "BPM", "RPM", "KM"])
                .collect();
            assert_shown(&items, &want, &what);
        }
    }
}
