//! The main ride page, stacked like a workout view: elapsed time and grade,
//! then speed as the hero number, power and heart rate with zone gauges,
//! then two readings the rider chose, cadence and distance by default. Each value has its signature colour and a
//! small unit beside it; there are no boxes or dividers. During a workout a
//! white bar under the power (or heart-rate) gauge spans the step's target.
use super::slots::Reading;
use super::zones::{GAUGE_WIDTH, Mark};
use super::{BODY, Labels, RidePage, Smooth, Text, elapsed, kilometres, speed, whole};
use crate::ride::course::nav::Sign;
use crate::screens::chrome::{
    Align, BLACK, BODY as BODY_FACE, CAPTION, GRADE, HEART, HERO, LARGE, Label, Line, POWER, Rect,
    Row, SECONDARY, SPEED, TERTIARY, VALUE, WHITE, blend, rounded, zone_color,
};
use crate::screens::icons::SignForm;
use crate::screens::sign;
use crate::screens::workout::{Scene, View};
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, SPILL, TOTAL_11, TOTAL_30, TOTAL_60};
use firmware_shell::shell::Parts;

const LEFT: usize = BODY.left + 10;
const RIGHT: usize = BODY.right - 8;
const GAUGE_HEIGHT: usize = 5;
const GAUGE_GAP: usize = 3;
/// The workout target sits this far under its gauge, this tall.
const BAND_GAP: usize = 2;
const BAND_HEIGHT: usize = 2;
/// Power has seven zones, heart rate five.
const SEGMENTS: [usize; 2] = [7, 5];

/// Where the readings sit, by baseline. A ride without a course keeps the
/// hero speed; while a course is loaded the page is ride-straight.svg's,
/// whose rows 22 to 104 belong to the course sign, so the sign coming and
/// going never moves a reading.
struct Rows {
    time: usize,
    speed: usize,
    speed_faces: [&'static Face; 3],
    power: usize,
    heart: usize,
    lower: usize,
    /// The power and heart-rate gauges' top rows.
    gauges: [usize; 2],
}

fn rows(course: bool) -> Rows {
    if course {
        return Rows {
            time: 58,
            speed: 149,
            speed_faces: [&LARGE, &VALUE, &VALUE],
            power: 199,
            heart: 255,
            lower: 303,
            gauges: [208, 266],
        };
    }
    let (power, heart) = (BODY.top + 108, BODY.top + 166);
    Rows {
        time: baseline(&VALUE, BODY.top + 4),
        speed: baseline(&HERO, BODY.top + 38),
        speed_faces: [&HERO, &LARGE, &VALUE],
        power: baseline(&LARGE, power),
        heart: baseline(&LARGE, heart),
        lower: baseline(&VALUE, BODY.top + 228),
        // Zone gauges sit just under the power and heart-rate numbers.
        gauges: [power + 47, heart + 47],
    }
}

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

fn value(write: &dyn Fn(&mut Text<12>)) -> Text<12> {
    let mut text = Text::new();
    write(&mut text);
    text
}

pub(super) fn prepare(view: &View) -> Smooth {
    if view.course && view.nav == Sign::Arrived {
        return arrived(view);
    }
    let rows = rows(view.course);
    let mut smooth = Smooth {
        gauges: Some(rows.gauges),
        ..Smooth::default()
    };
    // The banner covers the time and grade: leaving them out keeps the
    // clock's tick from sending the banner's rows.
    if sign::form(RidePage::Main, view.nav) != Some(SignForm::Banner) {
        time_and_grade(&mut smooth.labels, view, rows.time);
    }
    readings(&mut smooth, view, &rows);
    smooth
}

/// The course is done: the ride's totals under the arrived banner, distance
/// first, as arrived.svg draws them. No gauges.
fn arrived(view: &View) -> Smooth {
    let totals = &view.totals;
    let mut smooth = Smooth::default();
    let labels = &mut smooth.labels;
    let caption = |labels: &mut Labels, text: &[u8], x, base, align| {
        let _ = labels.push(on_baseline(&TOTAL_11, text, x, base, align, SECONDARY));
    };
    caption(labels, b"DISTANCE", LEFT, 132, Align::Left);
    let distance = value(&|v| kilometres(v, totals.distance_mm()));
    let faces: [&Face; 2] = [&TOTAL_60, &LARGE];
    reading(
        labels,
        &faces,
        &distance,
        b"KM",
        (LEFT, RIGHT),
        184,
        Align::Left,
        WHITE,
    );
    caption(labels, b"RIDE TIME", LEFT, 210, Align::Left);
    let time = value(&|v| elapsed(v, view.active_secs));
    let _ = labels.push(on_baseline(
        &VALUE,
        time.as_bytes(),
        LEFT,
        243,
        Align::Left,
        WHITE,
    ));
    let centre = (LEFT + RIGHT - SPILL) / 2;
    let averages = [
        (
            &b"AVG KM/H"[..],
            LEFT,
            Align::Left,
            value(&|v| speed(v, totals.avg_speed_mm_s())),
            SPEED,
        ),
        (
            b"AVG W",
            centre,
            Align::Center,
            value(&|v| whole(v, totals.avg_power())),
            POWER,
        ),
        (
            b"AVG BPM",
            RIGHT - SPILL,
            Align::Right,
            value(&|v| whole(v, totals.avg_heart_bpm())),
            HEART,
        ),
    ];
    for (name, x, align, average, color) in averages {
        caption(labels, name, x, 270, align);
        let _ = labels.push(on_baseline(
            &TOTAL_30,
            average.as_bytes(),
            x,
            300,
            align,
            color,
        ));
    }
    smooth
}

/// Time and grade share a line; the grade keeps its size and the clock
/// steps down when hours of riding make it long.
fn time_and_grade(labels: &mut Labels, view: &View, base: usize) {
    let sample = view.sample;
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
}

/// Speed, power, heart rate and the lower line: the same labels in every
/// state of the course sign.
fn readings(smooth: &mut Smooth, view: &View, rows: &Rows) {
    let sample = view.sample;
    let labels = &mut smooth.labels;
    let kmh = value(&|v| speed(v, sample.speed_mm_s));
    reading(
        labels,
        &rows.speed_faces,
        &kmh,
        b"KM/H",
        (LEFT, RIGHT),
        rows.speed,
        Align::Left,
        SPEED,
    );

    let zones = [
        view.power_3s
            .map_or(0, |watts| view.profile.power_zone(watts)),
        sample
            .heart_bpm
            .map_or(0, |bpm| view.profile.heart_zone(bpm)),
    ];
    smooth.band = target_band(view);
    let watts = value(&|v| whole(v, view.power_3s));
    let bpm = value(&|v| whole(v, sample.heart_bpm));
    let lines = [
        (watts, &b"W"[..], rows.power, POWER),
        (bpm, &b"BPM"[..], rows.heart, HEART),
    ];
    for ((text, unit, base, color), zone) in lines.into_iter().zip(zones) {
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
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
    lower(labels, [left, right], &shown, rows.lower);
}

/// The last line: the chosen readings split it between them, the first
/// from the left edge and the second from the right.
fn lower(labels: &mut Labels, readings: [Reading; 2], values: &[Text<12>; 2], base: usize) {
    let middle = (LEFT + RIGHT) / 2;
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

/// The workout target's rows under the gauge whose top row is `gauge`.
fn band_rows(gauge: usize) -> core::ops::Range<usize> {
    let top = gauge + GAUGE_HEIGHT + BAND_GAP;
    top..top + BAND_HEIGHT
}

/// The workout target's rows and columns, while one shows.
fn band(smooth: &Smooth) -> Option<(core::ops::Range<usize>, core::ops::Range<usize>)> {
    let (i, columns) = smooth.band.clone()?;
    Some((band_rows(smooth.gauges?[i]), columns))
}

pub(super) fn parts(scene: &Scene, parts: &mut Parts) {
    let smooth = scene.smooth();
    for (&top, mark) in smooth.gauges.iter().flatten().zip(&scene.gauges) {
        parts.add(band_rows(top), &smooth.band);
        parts.add(top..top + GAUGE_HEIGHT, mark);
    }
    for label in &smooth.labels {
        parts.add(label.top..label.bottom(), label);
    }
}

/// Paint a body row: a gauge row, or the labels that cover it.
pub(super) fn paint(scene: &Scene, row: &mut Row) {
    let y = row.y();
    let smooth = scene.smooth();
    if let Some((rows, columns)) = band(smooth)
        && rows.contains(&y)
    {
        row.span(columns, WHITE);
    }
    for (i, &top) in smooth.gauges.iter().flatten().enumerate() {
        if (top..top + GAUGE_HEIGHT).contains(&y) {
            let mark = scene.gauges[i].unwrap_or_default();
            paint_gauge(row, mark.zone, mark.fill.into(), SEGMENTS[i], top);
            return;
        }
    }
    for label in &smooth.labels {
        label.paint(row);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(scene: &Scene, x: usize, y: usize) -> u16 {
    let smooth = scene.smooth();
    if let Some((rows, columns)) = band(smooth)
        && rows.contains(&y)
        && columns.contains(&x)
    {
        return WHITE;
    }
    for (i, &top) in smooth.gauges.iter().flatten().enumerate() {
        if (top..top + GAUGE_HEIGHT).contains(&y) {
            let mark = scene.gauges[i].unwrap_or_default();
            return gauge(mark.zone, mark.fill.into(), SEGMENTS[i], x, y, top);
        }
    }
    for label in &smooth.labels {
        if let Some(color) = label.pixel(x, y, BLACK) {
            return color;
        }
    }
    BLACK
}

/// The column where gauge `i`'s lit run ends for `mark`.
pub(super) fn gauge_end(i: usize, mark: Mark) -> usize {
    let pitch = (RIGHT - LEFT + GAUGE_GAP) / SEGMENTS[i];
    let width = pitch - GAUGE_GAP;
    LEFT + usize::from(mark.zone.saturating_sub(1)) * pitch
        + width * usize::from(mark.fill) / GAUGE_WIDTH
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
        on_course(case, None)
    }

    /// `case` with a course loaded and showing `nav`, or without a course.
    fn on_course(case: Case, nav: Option<Sign>) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
        view.course = nav.is_some();
        view.nav = nav.unwrap_or_default();
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
        view.graphics.power_3s = case.watts;
        view.graphics.heart_bpm = case.bpm;
        view.prepare();
        view
    }

    fn items(view: &View) -> Vec<(String, Rect)> {
        let mut items: Vec<_> = view.smooth().labels.iter().filter_map(text).collect();
        for (i, &top) in view.smooth().gauges.iter().flatten().enumerate() {
            let rect = Rect {
                left: LEFT,
                top,
                right: RIGHT,
                bottom: top + GAUGE_HEIGHT,
            };
            items.push((std::format!("gauge {i}"), rect));
        }
        if let Some((rows, columns)) = band(view.smooth()) {
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
                lower(&mut labels, [left, right], &texts, rows(false).lower);
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

    fn signs() -> [Sign; 4] {
        use crate::ride::course::nav::{Distance, Glyph};
        let distance = Distance::from_mm(6_400_000);
        [
            Sign::None,
            Sign::Chip {
                glyph: Glyph::Right,
                distance,
            },
            Sign::Turn {
                glyph: Glyph::Left,
                distance,
            },
            Sign::OffCourse { back: distance },
        ]
    }

    /// Speed, power, heart rate, cadence and distance keep their rects as
    /// the sign comes and goes, all of them under its band.
    #[test]
    fn the_course_sign_never_moves_or_resizes_a_reading() {
        let readings = |nav| {
            let view = on_course(WIDEST, Some(nav));
            let time = elapsed_text(WIDEST.secs);
            let rects: Vec<(String, Rect)> = items(&view)
                .into_iter()
                .filter(|(name, rect)| *name != time && !name.ends_with('%') && rect.top > 0)
                .collect();
            rects
        };
        let at_rest = readings(Sign::None);
        assert_eq!(at_rest.len(), 14, "{at_rest:?}");
        for (name, rect) in &at_rest {
            assert!(rect.top >= 104, "{name:?} {rect:?} is under the sign");
        }
        for nav in signs() {
            assert_eq!(readings(nav), at_rest, "{nav:?}");
        }
    }

    /// The banner covers the time and grade, which then send nothing; the
    /// chip leaves them showing.
    #[test]
    fn the_banner_hides_the_time_and_grade_and_the_chip_does_not() {
        for nav in signs() {
            let view = on_course(WIDEST, Some(nav));
            let names: Vec<String> = items(&view).into_iter().map(|(name, _)| name).collect();
            let shown = names.contains(&elapsed_text(WIDEST.secs));
            let banner = matches!(nav, Sign::Turn { .. } | Sign::OffCourse { .. });
            assert_eq!(shown, !banner, "{nav:?}: {names:?}");
        }
    }

    /// For 60 s after the end the page is the ride's totals, distance first.
    #[test]
    fn arrived_shows_the_ride_totals_without_gauges() {
        let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
        view.course = true;
        view.nav = Sign::Arrived;
        view.active_secs = WIDEST.secs;
        // The widest averages a ride can have: no ride averages 1000 W.
        let sample = Sample {
            speed_mm_s: Some(27_750),
            power_watts: Some(999),
            heart_bpm: WIDEST.bpm,
            ..Sample::default()
        };
        for _ in 0..36_000 {
            view.totals.add(1000, 27_750, &sample, None);
        }
        view.prepare();
        assert_eq!(view.smooth().gauges, None);
        let items = items(&view);
        let mut distance = Text::new();
        kilometres(&mut distance, view.totals.distance_mm());
        let want = [
            "DISTANCE",
            distance.as_str(),
            "RIDE TIME",
            "AVG KM/H",
            "AVG W",
            "AVG BPM",
        ];
        assert_shown(&items, &want, "arrived");
        let rows = |name: &str| items.iter().find(|(n, _)| n == name).unwrap().1.top;
        assert!(rows("DISTANCE") < rows("RIDE TIME") && rows("RIDE TIME") < rows("AVG W"));
        let bounds = Rect { top: 104, ..BODY };
        assert_apart(&items, bounds, "arrived");
    }

    fn elapsed_text(secs: u64) -> String {
        let mut text = Text::new();
        elapsed(&mut text, secs);
        String::from(text.as_str())
    }

    #[test]
    fn extreme_readings_never_overlap_or_clip() {
        let bounds = Rect {
            left: BODY.left,
            top: BODY.top,
            right: BODY.right,
            bottom: BODY.bottom,
        };
        let chip = signs()[1];
        for (case, nav) in cases()
            .into_iter()
            .flat_map(|case| [(case, None), (case, Some(chip))])
        {
            let view = on_course(case, nav);
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
            let mut items = items(&view);
            if let Some(form) = nav.and_then(|nav| sign::form(RidePage::Main, nav)) {
                let placed = sign::place(view.nav, form, true).unwrap();
                items.push(("chip".into(), placed.rect()));
            }
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
