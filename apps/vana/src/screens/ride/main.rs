//! The main ride page, stacked like a workout view: elapsed time and grade,
//! then speed as the hero number, power and heart rate with zone gauges,
//! then two readings the rider chose, cadence and distance by default. Each value has its signature colour and a
//! small unit beside it; there are no boxes or dividers. During a workout a
//! white bar under the power (or heart-rate) gauge spans the step's target.
use super::slots::Reading;
use super::zones::{GAUGE_WIDTH, Mark};
use super::{Labels, RidePage, Smooth, Text, elapsed, kilometres, speed, whole};
use crate::ride::course::nav::Sign;
use crate::screens::chrome::{
    Align, BLACK, GRADE, HEART, Label, Line, POWER, Rect, Row, SECONDARY, SPEED, Shown, TERTIARY,
    WHITE, blend, rounded, zone_color,
};
use crate::screens::icons::SignForm;
use crate::screens::scaled::{Scaled, Track};
use crate::screens::sign;
use crate::screens::workout::{Scene, View};
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};
use firmware_shell::shell::Parts;

/// Power has seven zones, heart rate five.
const SEGMENTS: [usize; 2] = [7, 5];
const MOST_SEGMENTS: usize = if SEGMENTS[0] > SEGMENTS[1] {
    SEGMENTS[0]
} else {
    SEGMENTS[1]
};

/// The main page's bands on one panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The readings' text runs between these columns.
    pub text: Rect,
    /// A free ride: time and grade, speed, power, heart rate, then the
    /// rider's two readings. Each label's top is its band's.
    pub free: [Rect; 5],
    /// The power and heart-rate gauges on a free ride.
    pub gauges: [Gauge; 2],
    /// On a course: time and grade, the chip's band, speed, power, heart
    /// rate, then the rider's readings, each on a baseline inside its band.
    pub course: [Rect; 6],
}

/// The band a label is laid out in: one of the five readings, on a free
/// ride's bands or a course's, or the arrival totals, which span the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Time,
    Speed,
    Power,
    Heart,
    Lower,
    Arrived,
}

impl Regions {
    /// The bands a frame laid out on, named, in `Band` order where it has
    /// one, then the course's chip.
    #[cfg(any(test, feature = "invariants"))]
    pub fn bands(&self, course: bool) -> [(&'static str, Rect); 6] {
        let [time, speed, power, heart, lower] = self.free;
        let [on_time, chip, on_speed, on_power, on_heart, on_lower] = self.course;
        if course {
            return [
                ("time", on_time),
                ("speed", on_speed),
                ("power", on_power),
                ("heart", on_heart),
                ("lower", on_lower),
                ("chip", chip),
            ];
        }
        [
            ("time", time),
            ("speed", speed),
            ("power", power),
            ("heart", heart),
            ("lower", lower),
            (
                "rest",
                Rect {
                    top: lower.bottom,
                    ..lower
                },
            ),
        ]
    }
}

/// A zone gauge's rows, and the rows of a workout's target under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Gauge {
    pub rows: (usize, usize),
    pub band: (usize, usize),
}

impl Gauge {
    const fn under(top: usize, at: &Scaled) -> Self {
        use Track::Unit;
        let room = Rect {
            left: 0,
            top,
            right: 0,
            bottom: top + at.unit(9),
        };
        let [rows, _, band] = room.rows([Unit(5), Unit(2), Unit(2)], Unit(0), at);
        Self {
            rows: (rows.top, rows.bottom),
            band: (band.top, band.bottom),
        }
    }

    fn rows(&self) -> core::ops::Range<usize> {
        self.rows.0..self.rows.1
    }

    fn band(&self) -> core::ops::Range<usize> {
        self.band.0..self.band.1
    }
}

/// Where a gauge's segments run, and the gap between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    left: usize,
    right: usize,
    gap: usize,
}

impl Span {
    pub const NONE: Self = Self {
        left: 0,
        right: 0,
        gap: 0,
    };

    pub fn of(regions: &Regions, at: &Scaled) -> Self {
        Self {
            left: regions.text.left,
            right: regions.text.right,
            gap: at.unit(3),
        }
    }

    fn pitch(&self, segments: usize) -> usize {
        (self.right - self.left + self.gap) / segments
    }
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let body = super::frame(at).body;
    let [_, text, _] = body.cols([Unit(10), Fr(1), Unit(8)], Unit(0), at);
    let [_, time, _, speed, _, power, _, heart, _, lower] = body.rows(
        [
            Unit(4),
            Unit(34),
            Fr(1),
            Unit(70),
            Fr(1),
            Unit(58),
            Fr(1),
            Unit(62),
            Fr(1),
            Unit(68),
        ],
        Unit(0),
        at,
    );
    let [
        time_on,
        chip,
        _,
        speed_on,
        _,
        power_on,
        _,
        heart_on,
        _,
        lower_on,
    ] = body.rows(
        [
            Unit(42),
            Unit(38),
            Fr(1),
            Unit(57),
            Fr(1),
            Unit(56),
            Fr(1),
            Unit(60),
            Fr(1),
            Unit(43),
        ],
        Unit(0),
        at,
    );
    Regions {
        text,
        free: [time, speed, power, heart, lower],
        gauges: [gauge_in(power, at), gauge_in(heart, at)],
        course: [time_on, chip, speed_on, power_on, heart_on, lower_on],
    }
}

const fn gauge_in(band: Rect, at: &Scaled) -> Gauge {
    use Track::{Fr, Line, Unit};
    let [_, gap, _] = band.rows([Line(FaceRole::Large), Unit(5), Fr(1)], Unit(0), at);
    Gauge::under(gap.bottom, at)
}

struct CapsBelowBandTopUnits {
    time: usize,
    speed: usize,
    power: usize,
    heart: usize,
    lower: usize,
}

const COURSE_CAPS: CapsBelowBandTopUnits = CapsBelowBandTopUnits {
    time: 8,
    speed: 7,
    power: 0,
    heart: 0,
    lower: 0,
};

const COURSE_GAUGES_BELOW_BAND_TOP_UNITS: [usize; 2] = [47, 49];

const fn baseline_below_body_units(mockup_last_cap_row: usize) -> usize {
    let base = crate::screens::chrome::regions(&crate::screens::scaled::BASE);
    mockup_last_cap_row + 1 - base.content.top
}

struct Rows {
    time: usize,
    speed: usize,
    speed_faces: [&'static Face; 3],
    speed_count: usize,
    power: usize,
    heart: usize,
    lower: usize,
    gauges: [Gauge; 2],
    left: usize,
    right: usize,
    gap: usize,
    unit_gap: usize,
    lower_unit_gap: usize,
    lower_right: usize,
    caption: &'static Face,
    value: &'static Face,
    body: &'static Face,
    large: &'static Face,
}

/// Out of line, so its working leaves `smooth`'s frame (#167).
#[inline(never)]
fn rows(course: bool, regions: &Regions, at: &Scaled) -> Rows {
    let face = |role| at.face(role);
    let (value, large) = (face(FaceRole::Value), face(FaceRole::Large));
    let common = |rows: Rows| Rows {
        left: regions.text.left,
        right: regions.text.right,
        gap: at.unit(8),
        lower_unit_gap: at.unit(4),
        caption: face(FaceRole::Caption),
        value,
        body: face(FaceRole::Body),
        large,
        ..rows
    };
    if course {
        let [time, _, speed, power, heart, lower] = regions.course;
        let caps = |band: Rect, units: usize, face| baseline(face, band.top + at.unit(units));
        let gauge = |band: Rect, units: usize| Gauge::under(band.top + at.unit(units), at);
        let units = COURSE_CAPS;
        let [power_gauge, heart_gauge] = COURSE_GAUGES_BELOW_BAND_TOP_UNITS;
        return common(Rows {
            time: caps(time, units.time, value),
            speed: caps(speed, units.speed, large),
            speed_faces: [large, value, value],
            speed_count: 2,
            power: caps(power, units.power, large),
            heart: caps(heart, units.heart, large),
            lower: caps(lower, units.lower, value),
            gauges: [gauge(power, power_gauge), gauge(heart, heart_gauge)],
            unit_gap: at.unit(6),
            lower_right: regions.text.right + at.unit(1),
            ..BLANK
        });
    }
    let [time, speed, power, heart, lower] = regions.free;
    common(Rows {
        time: baseline(value, time.top),
        speed: baseline(face(FaceRole::Hero), speed.top),
        speed_faces: [face(FaceRole::Hero), large, value],
        speed_count: 3,
        power: baseline(large, power.top),
        heart: baseline(large, heart.top),
        lower: baseline(value, lower.top),
        gauges: regions.gauges,
        unit_gap: at.unit(4),
        lower_right: regions.text.right,
        ..BLANK
    })
}

const BLANK: Rows = Rows {
    time: 0,
    speed: 0,
    speed_faces: [&firmware_shell::rendering::font::VALUE; 3],
    speed_count: 0,
    power: 0,
    heart: 0,
    lower: 0,
    gauges: [Gauge {
        rows: (0, 0),
        band: (0, 0),
    }; 2],
    left: 0,
    right: 0,
    gap: 0,
    unit_gap: 0,
    lower_unit_gap: 0,
    lower_right: 0,
    caption: &firmware_shell::rendering::font::CAPTION,
    value: &firmware_shell::rendering::font::VALUE,
    body: &firmware_shell::rendering::font::BODY,
    large: &firmware_shell::rendering::font::LARGE,
};

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

#[allow(clippy::too_many_arguments)]
fn reading(
    labels: &mut Labels,
    caption: &'static Face,
    faces: &[&'static Face],
    value: &Text<12>,
    unit: &[u8],
    (left, right): (usize, usize),
    base: usize,
    (align, gap): (Align, usize),
    color: u16,
) {
    let unit_width = caption.width(unit) + SPILL;
    let room = right.saturating_sub(left + gap + unit_width);
    let line = Line::fit(faces, value.as_bytes(), room);
    let face = line.face();
    let top = base - face.top as usize - face.cap as usize;
    let (value, unit) = match align {
        Align::Right => {
            let unit = on_baseline(caption, unit, right - SPILL, base, Align::Right, SECONDARY);
            let value = Label::new(line, unit.left - gap, top, Align::Right, color);
            (value, unit)
        }
        _ => {
            let value = Label::new(line, left, top, Align::Left, color);
            // After the value's ink, so a "--" keeps its unit as close as
            // a number does.
            let at = left + face.ink_width(value.line.text()) + gap;
            (
                value,
                on_baseline(caption, unit, at, base, Align::Left, SECONDARY),
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
    let layout = view.layout;
    let (regions, at) = (&layout.main, &layout.scaled);
    let span = Span::of(regions, at);
    if view.course && view.nav == Sign::Arrived {
        return arrived(view, regions, at);
    }
    let rows = rows(view.course, regions, at);
    let mut smooth = Smooth {
        gauges: Some(rows.gauges),
        span,
        ..Smooth::default()
    };
    #[cfg(any(test, feature = "invariants"))]
    {
        smooth.tags.course = view.course;
    }
    let banner_covers_time_and_grade =
        sign::form(RidePage::Main, view.nav) == Some(SignForm::Banner);
    if !banner_covers_time_and_grade {
        time_and_grade(&mut smooth.labels, view, &rows);
        smooth.tag(0, Band::Time);
    }
    readings(&mut smooth, view, &rows);
    smooth
}

fn arrived(view: &View, regions: &Regions, at: &Scaled) -> Smooth {
    let totals = &view.totals;
    let mut smooth = Smooth::default();
    arrived_labels(&mut smooth, view, totals, regions, at);
    smooth.tag(0, Band::Arrived);
    smooth
}

fn arrived_labels(
    smooth: &mut Smooth,
    view: &View,
    totals: &crate::ride::totals::Totals,
    regions: &Regions,
    at: &Scaled,
) {
    let labels = &mut smooth.labels;
    let (left, right) = (regions.text.left, regions.text.right);
    let top = regions.course[0].top;
    let base =
        |mockup_last_cap_row: usize| top + at.unit(baseline_below_body_units(mockup_last_cap_row));
    let total_11 = at.face(FaceRole::Total11);
    let caption = |labels: &mut Labels, text: &[u8], x, base, align| {
        let _ = labels.push(on_baseline(total_11, text, x, base, align, SECONDARY));
    };
    caption(labels, b"DISTANCE", left, base(132), Align::Left);
    let distance = value(&|v| kilometres(v, totals.distance_mm()));
    let faces = [at.face(FaceRole::Total60), at.face(FaceRole::Large)];
    reading(
        labels,
        at.face(FaceRole::Caption),
        &faces,
        &distance,
        b"KM",
        (left, right),
        base(184),
        (Align::Left, at.unit(6)),
        WHITE,
    );
    caption(labels, b"RIDE TIME", left, base(210), Align::Left);
    let time = value(&|v| elapsed(v, view.active_secs));
    let _ = labels.push(on_baseline(
        at.face(FaceRole::Value),
        time.as_bytes(),
        left,
        base(243),
        Align::Left,
        WHITE,
    ));
    let centre = (left + right - SPILL) / 2;
    let averages = [
        (
            &b"AVG KM/H"[..],
            left,
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
            right - SPILL,
            Align::Right,
            value(&|v| whole(v, totals.avg_heart_bpm())),
            HEART,
        ),
    ];
    for (name, x, align, average, color) in averages {
        caption(labels, name, x, base(270), align);
        let _ = labels.push(on_baseline(
            at.face(FaceRole::Total30),
            average.as_bytes(),
            x,
            base(300),
            align,
            color,
        ));
    }
}

fn time_and_grade(labels: &mut Labels, view: &View, rows: &Rows) {
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
    let base = rows.time;
    let grade = on_baseline(
        rows.body,
        grade.as_bytes(),
        rows.right - SPILL,
        base,
        Align::Right,
        GRADE,
    );
    let time = value(&|v| elapsed(v, view.active_secs));
    let line = Line::fit(
        &[rows.value, rows.body],
        time.as_bytes(),
        grade.left - rows.gap - rows.left,
    );
    let top = base - line.face().top as usize - line.face().cap as usize;
    let _ = labels.push(Label::new(line, rows.left, top, Align::Left, WHITE));
    let _ = labels.push(grade);
}

fn readings(smooth: &mut Smooth, view: &View, rows: &Rows) {
    let sample = view.sample;
    let from = smooth.labels.len();
    let kmh = value(&|v| speed(v, sample.speed_mm_s));
    reading(
        &mut smooth.labels,
        rows.caption,
        &rows.speed_faces[..rows.speed_count],
        &kmh,
        b"KM/H",
        (rows.left, rows.right),
        rows.speed,
        (Align::Left, rows.unit_gap),
        SPEED,
    );
    smooth.tag(from, Band::Speed);

    let zones = [
        view.power_3s
            .map_or(0, |watts| view.profile.power_zone(watts)),
        sample
            .heart_bpm
            .map_or(0, |bpm| view.profile.heart_zone(bpm)),
    ];
    smooth.band = target_band(view, &smooth.span);
    let watts = value(&|v| whole(v, view.power_3s));
    let bpm = value(&|v| whole(v, sample.heart_bpm));
    let lines = [
        (watts, &b"W"[..], rows.power, POWER),
        (bpm, &b"BPM"[..], rows.heart, HEART),
    ];
    for (((text, unit, base, color), zone), band) in
        lines.into_iter().zip(zones).zip([Band::Power, Band::Heart])
    {
        let from = smooth.labels.len();
        let labels = &mut smooth.labels;
        let mut name = Text::<12>::new();
        let _ = match zone {
            0 => name.push_str("Z-"),
            z => write!(name, "Z{z}"),
        };
        let zone = on_baseline(
            rows.body,
            name.as_bytes(),
            rows.right - SPILL,
            base,
            Align::Right,
            zone_color(zone),
        );
        let faces = [rows.large, rows.value];
        let span = (rows.left, zone.left - rows.gap);
        let side = (Align::Left, rows.unit_gap);
        reading(
            labels,
            rows.caption,
            &faces,
            &text,
            unit,
            span,
            base,
            side,
            color,
        );
        let _ = labels.push(zone);
        smooth.tag(from, band);
    }

    let [left, right] = [view.main_slots.left, view.main_slots.right];
    let mut shown = [Text::new(), Text::new()];
    for (reading, text) in [left, right].into_iter().zip(&mut shown) {
        reading.write(view, text);
    }
    let from = smooth.labels.len();
    lower(&mut smooth.labels, [left, right], &shown, rows);
    smooth.tag(from, Band::Lower);
}

/// The last line: the chosen readings split it between them, the first
/// from the left edge and the second from the right.
fn lower(labels: &mut Labels, readings: [Reading; 2], values: &[Text<12>; 2], rows: &Rows) {
    let base = rows.lower;
    let middle = (rows.left + rows.right) / 2;
    let faces = [rows.value, rows.body];
    let [left, right] = readings;
    let span = (rows.left, middle - rows.gap / 2);
    reading(
        labels,
        rows.caption,
        &faces,
        &values[0],
        left.unit(),
        span,
        base,
        (Align::Left, rows.unit_gap),
        left.color(),
    );
    let span = (middle + rows.gap / 2, rows.lower_right);
    reading(
        labels,
        rows.caption,
        &faces,
        &values[1],
        right.unit(),
        span,
        base,
        (Align::Right, rows.lower_unit_gap),
        right.color(),
    );
}

/// Where a reading falls on gauge `i`: the zone it is in, and the share of
/// that zone's segment below it.
fn gauge_x(view: &View, span: &Span, i: usize, value: u16) -> usize {
    let profile = &view.profile;
    let (zone, (low, high)) = if i == 0 {
        let zone = profile.power_zone(value);
        (zone, profile.power_range(zone))
    } else {
        let zone = profile.heart_zone(value);
        (zone, profile.heart_range(zone))
    };
    let pitch = span.pitch(SEGMENTS[i]);
    let width = pitch - span.gap;
    let within = u32::from(value).clamp(low, high) - low;
    span.left
        + usize::from(zone - 1) * pitch
        + width * within as usize / (high - low).max(1) as usize
}

/// The step's target band under its gauge, while a workout step has one.
fn target_band(view: &View, span: &Span) -> Option<(usize, core::ops::Range<usize>)> {
    use crate::workout::model::Metric;
    let training = view.training.as_ref().filter(|t| !t.done)?;
    let band = training.now.band?;
    let i = match band.metric {
        Metric::Power => 0,
        Metric::Heart => 1,
        Metric::Cadence => return None,
    };
    let from = gauge_x(view, span, i, band.low);
    let to = gauge_x(view, span, i, band.high)
        .max(from + 3)
        .min(span.right);
    Some((i, from..to))
}

fn band(smooth: &Smooth) -> Option<(core::ops::Range<usize>, core::ops::Range<usize>)> {
    let (i, columns) = smooth.band.clone()?;
    Some((smooth.gauges?[i].band(), columns))
}

pub(super) fn parts(scene: &Scene, parts: &mut Parts) {
    let smooth = scene.smooth();
    for (gauge, mark) in smooth.gauges.iter().flatten().zip(&scene.gauges) {
        parts.add(gauge.band(), &smooth.band);
        parts.add(gauge.rows(), mark);
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
    for (i, gauge) in smooth.gauges.iter().flatten().enumerate() {
        if gauge.rows().contains(&y) {
            let mark = scene.gauges[i].unwrap_or_default();
            paint_gauge(row, &smooth.span, mark, SEGMENTS[i], gauge);
            return;
        }
    }
    for label in &smooth.labels {
        label.paint(row);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(scene: &Scene, x: usize, y: usize) -> Shown {
    let look = scene.look();
    let black = look.ink(BLACK);
    let smooth = scene.smooth();
    if let Some((rows, columns)) = band(smooth)
        && rows.contains(&y)
        && columns.contains(&x)
    {
        return look.ink(WHITE);
    }
    for (i, gauge) in smooth.gauges.iter().flatten().enumerate() {
        if gauge.rows().contains(&y) {
            let mark = scene.gauges[i].unwrap_or_default();
            let Some(at) = self::gauge(&smooth.span, mark, SEGMENTS[i], x, y, gauge) else {
                return black;
            };
            let color = segment_color(at.segment, at.lit);
            return blend(look.ink(color), black, at.coverage);
        }
    }
    for label in &smooth.labels {
        if let Some(color) = label.pixel(look, x, y, black) {
            return color;
        }
    }
    black
}

/// The column where gauge `i`'s lit run ends for `mark`.
pub(super) fn gauge_end(span: &Span, i: usize, mark: Mark) -> usize {
    let pitch = span.pitch(SEGMENTS[i]);
    let width = pitch - span.gap;
    span.left
        + usize::from(mark.zone.saturating_sub(1)) * pitch
        + width * usize::from(mark.fill) / GAUGE_WIDTH
}

/// `gauge` as spans: each segment lit up to its split, then the dim track,
/// with only its rounded ends worked out per pixel.
fn paint_gauge(row: &mut Row, span: &Span, mark: Mark, segments: usize, gauge: &Gauge) {
    let pitch = span.pitch(segments);
    let width = pitch - span.gap;
    let (zone, fill) = (mark.zone, usize::from(mark.fill));
    let lit = usize::from(zone);
    let black = row.ink(BLACK);
    let inks: [[Shown; 2]; MOST_SEGMENTS] = core::array::from_fn(|segment| {
        [false, true].map(|lit| row.ink(segment_color(segment, lit)))
    });
    let y = row.y();
    for (segment, &[track, color]) in inks.iter().enumerate().take(segments) {
        let left = span.left + segment * pitch;
        let split = if segment + 1 < lit {
            width
        } else if segment + 1 == lit {
            width * fill / GAUGE_WIDTH
        } else {
            0
        };
        row.span_shown(left..left + split, color);
        row.span_shown(left + split..left + width, track);
        let end = |x| {
            let at = self::gauge(span, mark, segments, x, y, gauge)?;
            let shown = inks[at.segment][usize::from(at.lit)];
            Some(blend(shown, black, at.coverage))
        };
        row.each(left..left + 2, end);
        row.each(left + width - 2..left + width, end);
    }
}

struct GaugePixel {
    segment: usize,
    lit: bool,
    coverage: u8,
}

fn segment_color(segment: usize, lit: bool) -> u16 {
    if lit {
        zone_color(segment as u8 + 1)
    } else {
        TERTIARY
    }
}

fn gauge(
    span: &Span,
    mark: Mark,
    segments: usize,
    x: usize,
    y: usize,
    gauge: &Gauge,
) -> Option<GaugePixel> {
    if !(span.left..span.right).contains(&x) {
        return None;
    }
    let pitch = span.pitch(segments);
    let segment = (x - span.left) / pitch;
    let left = span.left + segment * pitch;
    let rect = Rect {
        left,
        top: gauge.rows.0,
        right: left + pitch - span.gap,
        bottom: gauge.rows.1,
    };
    let zone = usize::from(mark.zone);
    let width = pitch - span.gap;
    let fill = usize::from(mark.fill);
    let lit = segment + 1 < zone || (segment + 1 == zone && x - left < width * fill / GAUGE_WIDTH);
    Some(GaugePixel {
        segment,
        lit,
        coverage: rounded(rect, 2, x, y),
    })
}

#[cfg(test)]
mod tests {
    use super::super::BODY;
    use super::*;
    use crate::ride::log::{Sample, Status};
    use crate::screens::scaled::BASE;

    const UNIT_GAP: usize = 4;
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
        for (i, gauge) in view.smooth().gauges.iter().flatten().enumerate() {
            let rect = Rect {
                left: crate::screens::BASE.ride.text.left,
                top: gauge.rows.0,
                right: crate::screens::BASE.ride.text.right,
                bottom: gauge.rows.1,
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
                assert_eq!(
                    columns.start,
                    gauge_x(&view, &view.smooth().span, 0, band.low)
                );
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
                lower(
                    &mut labels,
                    [left, right],
                    &texts,
                    &rows(false, &regions(&BASE), &BASE),
                );
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

    #[test]
    fn arrived_shows_the_ride_totals_without_gauges() {
        let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
        view.course = true;
        view.nav = Sign::Arrived;
        view.active_secs = WIDEST.secs;
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
                let placed = sign::place(view.nav, form, true, view.layout).unwrap();
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
