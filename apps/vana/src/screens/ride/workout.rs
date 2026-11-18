//! The workout page: the step on now and its target, the reading checked
//! against it as the hero, how much of the step is left, the next step,
//! and the whole workout as a chart.
//!
//! Top to bottom: the workout's name and the step count; the step and its
//! target band in the target's metric colour; the 3 s power (or heart rate,
//! or cadence) in its metric colour; a gauge with the band at 73 % and a
//! white disc for the reading, then UNDER / ON TARGET / OVER; the time or
//! distance left over the step's progress bar, amber for the last seconds;
//! the next step; then one bar per played step, its height the target's
//! intensity and its colour the target's zone. Past and current steps are
//! full colour, the rest 25 %.
use super::Text;
use crate::ride::zones::TrainingProfile;
use crate::screens::Layout;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BLACK, CADENCE, GREEN, HEART, POWER, Rect, Row, SECONDARY, TERTIARY, WHITE, mix,
    zone_color,
};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::View;
use crate::workout::model::{Intensity, Length, Metric, Name, Played, Ramp, Target, Test, Workout};
use crate::workout::player::{COUNTDOWN_S, Compliance, Left, Now, Player, TestReadout};
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, SPILL};

/// A workout on the ride screen: before it starts, while it runs and after.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub struct Training {
    pub name: Name,
    pub test: Option<Test>,
    pub now: Now,
    pub done: bool,
}

impl Training {
    /// What the page shows of `player`; the workout itself stays with it.
    pub fn of(player: &Player, profile: &TrainingProfile) -> Self {
        Self {
            name: player.workout().name,
            test: player.workout().test,
            now: player.now(profile),
            done: player.done(),
        }
    }
}

/// The workout page on one panel. A text row's capitals end on its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The workout's name and the step count.
    pub name: Rect,
    /// The step and its target.
    pub step: Rect,
    /// The reading the band checks.
    pub hero: Rect,
    pub gauge: Rect,
    /// UNDER / ON TARGET / OVER, and a test's running number.
    pub verdict: Rect,
    /// The time or distance left of the step.
    pub remaining: Rect,
    pub progress: Rect,
    pub next: Rect,
    /// The chart runs down to the body's edge, into the toast band, as the
    /// other charts do.
    pub chart: Rect,
    /// Rows where a tap adds to the step, across the panel: the time left
    /// and its progress.
    pub extend: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Rise, Unit};
    let text = super::frame(at).text;
    let [name, step, _, hero] = text.rows(
        [Unit(16), Unit(26), Unit(8), Rise(FaceRole::Hero)],
        Unit(0),
        at,
    );
    let below = Rect {
        top: hero.bottom,
        ..text
    };
    let [_, gauge, verdict, remaining, _, progress, next, _, chart] = below.rows(
        [
            Unit(8),
            Unit(8),
            Unit(16),
            Unit(32),
            Unit(7),
            Unit(5),
            Unit(22),
            Unit(10),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    assert!(
        chart.height() >= at.unit(20),
        "the workout chart needs some height"
    );
    Regions {
        name,
        step,
        hero,
        gauge,
        verdict,
        remaining,
        progress,
        next,
        chart,
        extend: Rect {
            top: remaining.top + at.unit(4),
            bottom: progress.bottom + at.unit(8),
            ..at.screen()
        },
    }
}

const CHART_COLUMNS: usize =
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| {
        let chart = regions(&at).chart;
        assert!(chart.height() < EMPTY as usize, "the chart's rows fit a u8");
        chart.width()
    });
/// Untimed steps take this much chart width, as if they lasted this long.
const OPEN_WEIGHT_S: u32 = 120;
/// A target of this much intensity fills the chart's height.
const FULL_PCT: u32 = 150;
/// Most stairs a ramp draws before the one on now.
const MAX_STAIRS: u32 = 40;

/// The chart's columns, prepared once per frame.
#[derive(Clone, Hash)]
pub struct Chart {
    tops: [u8; CHART_COLUMNS],
    /// Each column's colour as an index into `palette`, to keep the view small.
    colors: [u8; CHART_COLUMNS],
    palette: heapless::Vec<u16, PALETTE>,
}

/// Colours a chart can use: each zone, cadence and none, lit and at 25 %.
const PALETTE: usize = 24;

impl Chart {
    fn color(&self, column: usize) -> u16 {
        self.palette
            .get(usize::from(self.colors[column]))
            .copied()
            .unwrap_or(BLACK)
    }

    fn index(&mut self, color: u16) -> u8 {
        match self.palette.iter().position(|&c| c == color) {
            Some(index) => index as u8,
            None => {
                let _ = self.palette.push(color);
                (self.palette.len() - 1) as u8
            }
        }
    }
}

const EMPTY: u8 = u8::MAX;

impl Chart {
    pub const EMPTY: Self = Self {
        tops: [EMPTY; CHART_COLUMNS],
        colors: [0; CHART_COLUMNS],
        palette: heapless::Vec::new(),
    };
}

impl Default for Chart {
    fn default() -> Self {
        Self::EMPTY
    }
}

fn metric_color(metric: Metric) -> u16 {
    match metric {
        Metric::Power => POWER,
        Metric::Heart => HEART,
        Metric::Cadence => CADENCE,
    }
}

const fn unit(metric: Metric) -> &'static [u8] {
    match metric {
        Metric::Power => b"W",
        Metric::Heart => b"BPM",
        Metric::Cadence => b"RPM",
    }
}

/// M:SS, or H:MM:SS from an hour.
fn clock(value: &mut Text<12>, secs: u32) {
    let _ = if secs >= 3600 {
        write!(
            value,
            "{}:{:02}:{:02}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60
        )
    } else {
        write!(value, "{}:{:02}", secs / 60, secs % 60)
    };
}

fn length(value: &mut Text<12>, length: Length) {
    match length {
        Length::Time(secs) => clock(value, u32::from(secs)),
        Length::Distance(m) => {
            let _ = write!(value, "{}.{} KM", m / 1000, m % 1000 / 100);
        }
        Length::Open => {
            let _ = value.push_str("OPEN");
        }
    }
}

/// What a target asks for, in words: "265-300 W", "ALL OUT", "HOLD 260 W".
fn target_text(value: &mut Text<24>, played: Played, now: &Now, test: Option<Test>) {
    if let Some(watts) = now.ramp_watts {
        let _ = write!(value, "HOLD {watts} W");
        return;
    }
    let _ = match (played.step.target, now.band) {
        (Target::Free, _) if test == Some(Test::Twenty) => value.push_str("ALL OUT"),
        (Target::Free, _) => value.push_str("FREE RIDE"),
        (_, Some(band)) if band.low == band.high => {
            write!(value, "{} {}", band.low, unit_str(band.metric))
        }
        (_, Some(band)) => write!(
            value,
            "{}-{} {}",
            band.low,
            band.high,
            unit_str(band.metric)
        ),
        (_, None) => value.push_str("FREE RIDE"),
    };
}

fn unit_str(metric: Metric) -> &'static str {
    core::str::from_utf8(unit(metric)).unwrap_or("")
}

/// The step's name: its intensity, the run of its block when repeated, and
/// its note when there is room.
fn step_name(value: &mut Text<24>, played: Played, short: bool) {
    let _ = value.push_str(if short {
        played.step.intensity.short_label()
    } else {
        played.step.intensity.label()
    });
    if played.of > 1 {
        let _ = write!(value, " {}/{}", played.repetition + 1, played.of);
    }
}

fn next_step(next: Option<Played>, fits: impl Fn(&[u8]) -> bool) -> Text<24> {
    let mut text = Text::<24>::new();
    let Some(played) = next else {
        let _ = text.push_str("FINISH");
        return text;
    };
    let mut time = Text::<12>::new();
    length(&mut time, played.step.length);
    let intensity = played.step.intensity;
    for label in [intensity.label(), intensity.short_label()] {
        text = Text::new();
        let _ = write!(text, "{} {label}", time.as_str());
        if fits(text.as_bytes()) {
            return text;
        }
    }
    text = Text::new();
    let _ = text.push_str(time.as_str());
    text
}

fn whole_words(text: &[u8], fits: impl Fn(&[u8]) -> bool) -> Option<&[u8]> {
    if fits(text) {
        return Some(text);
    }
    (1..text.len())
        .rev()
        .filter(|&end| text[end] == b' ' && text[end - 1] != b' ')
        .map(|end| &text[..end])
        .find(|words| fits(words))
}

pub(super) fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.workout);
    let (caption, body) = (at.face(FaceRole::Caption), at.face(FaceRole::Body));
    let (left_edge, right_edge) = (regions.step.left, regions.step.right);
    let width = right_edge - left_edge;
    let mut canvas = Canvas::default();
    let Some(training) = &view.training else {
        canvas.fit(
            &[body],
            b"NO WORKOUT",
            width,
            left_edge,
            regions.step.bottom,
            Align::Left,
            SECONDARY,
        );
        return canvas;
    };
    let now = &training.now;

    let mut widest = Text::<24>::new();
    let _ = write!(widest, "STEP {}/{}", now.steps, now.steps);
    let spelled_out_fits = caption.width(training.name.as_bytes())
        + caption.width(widest.as_bytes())
        + at.unit(8)
        + 2 * SPILL
        <= width;
    let count = |step| {
        let mut count = Text::<24>::new();
        let _ = if training.done {
            count.push_str("DONE")
        } else if spelled_out_fits {
            write!(count, "STEP {}/{}", step, now.steps)
        } else {
            write!(count, "{}/{}", step, now.steps)
        };
        count
    };
    canvas.caption(
        at,
        count(now.position + 1).as_bytes(),
        width / 2,
        right_edge - SPILL,
        regions.name.bottom,
        Align::Right,
    );
    let widest_count_width = caption.width(count(now.steps).as_bytes());
    let room = width.saturating_sub(SPILL + widest_count_width + at.unit(8));
    let full_name = training.name.as_bytes();
    let name =
        whole_words(full_name, |text| caption.width(text) + SPILL <= room).unwrap_or(full_name);
    canvas.caption(at, name, room, left_edge, regions.name.bottom, Align::Left);

    // The step and its target.
    let metric = now.band.map_or(Metric::Power, |band| band.metric);
    let color = metric_color(metric);
    let mut target = Text::<24>::new();
    let mut name = Text::<24>::new();
    if training.done {
        let _ = name.push_str("WORKOUT DONE");
    } else {
        step_name(&mut name, now.played, false);
        target_text(&mut target, now.played, now, training.test);
    }
    let target_color = if now.band.is_some() { color } else { SECONDARY };
    let name_room = caption.width(name.as_bytes()) + at.unit(8) + 2 * SPILL;
    // The target is never cut: it keeps at least its smallest size.
    let target_floor = caption.width(target.as_bytes()) + SPILL;
    let name_right = if training.done {
        width
    } else {
        let target = canvas.fit(
            &[body, caption],
            target.as_bytes(),
            (width * 3 / 5)
                .min(width.saturating_sub(name_room))
                .max(target_floor),
            right_edge - SPILL,
            regions.step.bottom,
            Align::Right,
            target_color,
        );
        target.left.saturating_sub(left_edge + at.unit(8))
    };
    let name_color = if training.done { GREEN } else { WHITE };
    if !training.done && caption.width(name.as_bytes()) + SPILL > name_right {
        name = Text::new();
        step_name(&mut name, now.played, true);
    }
    canvas.fit(
        &[body, caption],
        name.as_bytes(),
        name_right,
        left_edge,
        regions.step.bottom,
        Align::Left,
        name_color,
    );

    // The reading the band checks, as the hero, its unit beside it.
    let mut reading = Text::<12>::new();
    let _ = match now.reading {
        Some(value) => write!(reading, "{value}"),
        None => reading.push_str("--"),
    };
    let unit_width = caption.width(unit(metric)) + SPILL;
    let smoothing = caption.width(b"3 S") + SPILL + at.unit(8);
    let hero = canvas.fit(
        &[at.face(FaceRole::Hero), at.face(FaceRole::Large)],
        reading.as_bytes(),
        width - unit_width - at.unit(6) - smoothing,
        left_edge,
        regions.hero.bottom,
        Align::Left,
        color,
    );
    canvas.fit(
        &[caption],
        unit(metric),
        unit_width,
        hero.right() + at.unit(6),
        regions.hero.bottom,
        Align::Left,
        SECONDARY,
    );
    if metric == Metric::Power && now.reading.is_some() {
        canvas.caption(
            at,
            b"3 S",
            at.unit(40),
            right_edge - SPILL,
            regions.hero.bottom,
            Align::Right,
        );
    }

    // The band gauge and the verdict under it.
    gauge(&mut canvas, layout, now, color);
    let mut free_from = left_edge;
    if let Some(compliance) = now.compliance {
        let (text, verdict): (&[u8], u16) = match compliance {
            Compliance::Under => (b"UNDER", AMBER),
            Compliance::In => (b"ON TARGET", GREEN),
            Compliance::Over => (b"OVER", AMBER),
        };
        let run = canvas.fit(
            &[caption],
            text,
            width / 2,
            left_edge,
            regions.verdict.bottom,
            Align::Left,
            verdict,
        );
        free_from = run.right() + at.unit(10);
    }
    if let Some(test) = now.test {
        let mut text = Text::<24>::new();
        let _ = match test {
            TestReadout::Average(w) => write!(text, "AVG {w} W"),
            TestReadout::BestMinute(w) => write!(text, "BEST {w} W"),
        };
        canvas.fit(
            &[caption],
            text.as_bytes(),
            right_edge - SPILL - free_from,
            right_edge - SPILL,
            regions.verdict.bottom,
            Align::Right,
            POWER,
        );
    }

    // What is left of the step, over its progress.
    if !training.done {
        let mut left = Text::<12>::new();
        let (words, last): (&[u8], bool) = match now.left {
            Left::Secs(secs) => {
                clock(&mut left, secs);
                (
                    b"LEFT",
                    secs <= COUNTDOWN_S + 2 && view.state == crate::ride::log::Status::Recording,
                )
            }
            Left::Metres(m) => {
                let _ = write!(left, "{}.{:02}", m / 1000, m % 1000 / 10);
                (b"KM LEFT", false)
            }
            Left::Open(secs) => {
                clock(&mut left, secs);
                let starts_test = now
                    .next
                    .is_some_and(|next| next.step.intensity == Intensity::Test);
                (
                    if starts_test {
                        b"LAP TO START"
                    } else {
                        b"LAP TO END"
                    },
                    false,
                )
            }
        };
        let value = canvas.fit(
            &[at.face(FaceRole::Value), body],
            left.as_bytes(),
            width / 2,
            left_edge,
            regions.remaining.bottom,
            Align::Left,
            if last { AMBER } else { WHITE },
        );
        canvas.caption(
            at,
            words,
            right_edge - value.right() - at.unit(12),
            value.right() + at.unit(6),
            regions.remaining.bottom,
            Align::Left,
        );
        let progress = regions.progress;
        let radius = at.unit(2);
        canvas.shape(Shape::Bar {
            rect: progress,
            radius,
            color: TERTIARY,
        });
        let lit = left_edge + width * usize::from(now.progress) / 1000;
        if lit > left_edge + at.unit(4) {
            canvas.shape(Shape::Bar {
                rect: Rect {
                    right: lit,
                    ..progress
                },
                radius,
                color: WHITE,
            });
        }

        let label = canvas.caption(
            at,
            b"NEXT",
            at.unit(50),
            left_edge,
            regions.next.bottom,
            Align::Left,
        );
        let room = right_edge - label.right() - at.unit(14);
        let next = next_step(now.next, |text| caption.width(text) + SPILL <= room);
        canvas.fit(
            &[body, caption],
            next.as_bytes(),
            room,
            label.right() + at.unit(8),
            regions.next.bottom,
            Align::Left,
            WHITE,
        );
    }
    canvas
}

/// The track from 70 % of the band's low end to 130 % of its high end, the
/// band on it at 73 % of the metric colour so it reads in sunlight, and the
/// reading as a disc.
fn gauge(canvas: &mut Canvas, layout: &Layout, now: &Now, color: u16) {
    let Some(band) = now.band else {
        return;
    };
    let (at, track) = (&layout.scaled, layout.workout.gauge);
    let low = u32::from(band.low) * 7 / 10;
    let high = (u32::from(band.high) * 13 / 10).max(low + 1);
    let x = |value: u32| {
        track.left + (value.clamp(low, high) - low) as usize * track.width() / (high - low) as usize
    };
    let radius = at.unit(4);
    canvas.shape(Shape::Bar {
        rect: track,
        radius,
        color: TERTIARY,
    });
    let (from, to) = (
        x(band.low.into()),
        x(band.high.into()).max(x(band.low.into()) + at.unit(6)),
    );
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: from,
            right: to.min(track.right),
            ..track
        },
        radius,
        color: mix(color, BLACK, 11),
    });
    if let Some(reading) = now.reading {
        let disc = at.unit(5);
        let cx = x(reading.into()).clamp(track.left + disc, track.right - disc);
        canvas.shape(Shape::Disc {
            cx: 4 * cx as i32,
            cy: 4 * (track.top + track.height() / 2) as i32,
            radius: 4 * disc as i32,
            color: WHITE,
        });
    }
}

/// How hard a step is, in percent of threshold, for the chart's height.
fn intensity(target: Target, profile: &TrainingProfile) -> Option<u32> {
    let ftp = u32::from(profile.ftp_watts.max(1));
    Some(match target {
        Target::Free => return None,
        Target::Ftp { low, high } => (u32::from(low) + u32::from(high)) / 2,
        Target::Watts { low, high } => (u32::from(low) + u32::from(high)) * 50 / ftp,
        Target::HeartZone(zone) => [55, 70, 85, 98, 110][usize::from(zone.clamp(1, 5) - 1)],
        Target::Lthr { low, high } => (u32::from(low) + u32::from(high)) / 2,
        Target::Cadence { .. } => 50,
        Target::Ramp { .. } | Target::FtpRamp { .. } => FULL_PCT,
    })
}

/// A target's chart colour: its power zone, or its heart zone.
fn zone(target: Target, profile: &TrainingProfile) -> u16 {
    match target.band(profile, 0) {
        Some(band) => match band.metric {
            Metric::Power => zone_color(profile.power_zone(band.low.midpoint(band.high))),
            Metric::Heart => zone_color(profile.heart_zone(band.low.midpoint(band.high))),
            Metric::Cadence => CADENCE,
        },
        None => TERTIARY,
    }
}

/// Stairs a ramp draws: up to the first stage that reaches the chart's top,
/// and the stage on now even past it.
fn stairs(ramp: Ramp, profile: &TrainingProfile, current: u32) -> u32 {
    let top = u32::from(profile.ftp_watts) * FULL_PCT;
    let reach = (0..MAX_STAIRS)
        .find(|&stage| u32::from(ramp.stage_watts(stage)) * 100 >= top)
        .unwrap_or(MAX_STAIRS);
    reach.max(current) + 1
}

/// Lay out the chart across `regions.chart`: each played step takes width
/// in proportion to its time, with a one-column gap before it. A ramp is a
/// stair per stage.
pub fn chart(player: &Player, profile: &TrainingProfile, regions: &Regions) -> Chart {
    let (width, rows) = (regions.chart.width(), regions.chart.height());
    let mut chart = Chart::default();
    let workout: &Workout = player.workout();
    let done = player.done();
    let now = player.now(profile);
    let position = usize::from(now.position);
    let ramp_of = |index: usize, played: &Played| {
        let ramp = played.step.target.ramp(profile)?;
        let current = match now.left {
            Left::Open(secs) if index == position && !done => ramp.stage(secs),
            _ => 0,
        };
        Some((ramp, stairs(ramp, profile, current), current))
    };
    let weight = |index: usize, played: &Played| match (ramp_of(index, played), played.step.length)
    {
        (Some((ramp, stairs, _)), _) => stairs * u32::from(ramp.every),
        (None, Length::Time(secs)) => u32::from(secs),
        (None, Length::Distance(m)) => (m / 8).max(60),
        (None, Length::Open) => OPEN_WEIGHT_S,
    };
    let total = workout
        .played()
        .enumerate()
        .map(|(index, p)| u64::from(weight(index, &p)))
        .sum::<u64>()
        .max(1);
    let mut start = 0u64;
    for (index, played) in workout.played().enumerate() {
        let from = (start * width as u64 / total) as usize;
        start += u64::from(weight(index, &played));
        let to = (start * width as u64 / total) as usize;
        let ahead = index > position && !done;
        let first = (from + 1).min(to);
        let last = to.min(width);
        let ramp = ramp_of(index, &played);
        let (color, pct) = (
            zone(played.step.target, profile),
            intensity(played.step.target, profile).unwrap_or(40),
        );
        for column in first..last {
            let (color, pct, dim) = match ramp {
                Some((ramp, stairs, current)) => {
                    let stage =
                        ((column - first) as u32 * stairs / (last - first) as u32).min(stairs - 1);
                    let watts = ramp.stage_watts(stage);
                    let dim = ahead || (index == position && !done && stage > current);
                    (
                        zone_color(profile.power_zone(watts)),
                        u32::from(watts) * 100 / u32::from(profile.ftp_watts.max(1)),
                        dim,
                    )
                }
                None => (color, pct, ahead),
            };
            let color = if dim { mix(color, BLACK, 4) } else { color };
            let height = (pct.min(FULL_PCT) as usize * rows / FULL_PCT as usize).max(3);
            chart.tops[column] = (rows - height) as u8;
            chart.colors[column] = chart.index(color);
        }
    }
    chart
}

pub(super) fn chart_parts(
    chart: &Chart,
    regions: &Regions,
    parts: &mut firmware_shell::shell::Parts,
) {
    parts.add(regions.chart.top..regions.chart.bottom, chart);
}

pub(super) fn paint_chart(chart: &Chart, regions: &Regions, row: &mut Row) {
    let (y, area) = (row.y(), regions.chart);
    if !(area.top..area.bottom).contains(&y) {
        return;
    }
    let line = (y - area.top) as u8;
    let width = area.width();
    let mut column = 0;
    while column < width {
        let filled = |c: usize| chart.tops[c] != EMPTY && line >= chart.tops[c];
        if !filled(column) {
            column += 1;
            continue;
        }
        let color = chart.colors[column];
        let start = column;
        while column < width && filled(column) && chart.colors[column] == color {
            column += 1;
        }
        row.span(area.left + start..area.left + column, chart.color(start));
    }
}

/// The per-pixel original of `paint_chart`.
#[cfg(test)]
pub(super) fn chart_pixel(chart: &Chart, regions: &Regions, x: usize, y: usize) -> u16 {
    let area = regions.chart;
    if !(area.left..area.right).contains(&x) {
        return BLACK;
    }
    let column = x - area.left;
    if (area.top..area.bottom).contains(&y)
        && chart.tops[column] != EMPTY
        && (y - area.top) as u8 >= chart.tops[column]
    {
        return chart.color(column);
    }
    BLACK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::{Sample, Status};
    use crate::screens::chrome::Rect;
    use crate::screens::ride::RidePage;
    use crate::screens::workout::Page;
    use crate::workout::library::{BUILT_IN, FTP_RAMP, FTP_TWENTY, VO2_MAX};
    use crate::workout::player::Player;

    fn page(player: &Player, watts: Option<u16>) -> View<'static> {
        page_at(
            player,
            watts,
            crate::ride::zones::TrainingProfile::default().ftp_watts,
        )
    }

    fn page_at(player: &Player, watts: Option<u16>, ftp: u16) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        view.profile.ftp_watts = ftp;
        view.ride_page = RidePage::Workout;
        view.state = Status::Recording;
        view.sample.power_watts = watts;
        view.training = Some(Training::of(player, &view.profile));
        view.show_workout(chart(player, &view.profile, &view.layout.workout));
        view.prepare();
        view
    }

    fn ride(player: &mut Player, seconds: u32, watts: u16) {
        let sample = Sample {
            power_watts: Some(watts),
            ..Sample::default()
        };
        for _ in 0..seconds {
            player.add(
                1000,
                8000,
                &sample,
                &crate::ride::zones::TrainingProfile::default(),
            );
        }
    }

    const PAGE: Rect = crate::screens::BASE.ride.body;
    const REGIONS: Regions = crate::screens::BASE.workout;

    #[test]
    fn a_chart_of_a_ramp_from_tens_of_kilowatts_still_lays_out() {
        use crate::workout::model::{Length, Step, Workout};
        let ramp = Target::Ramp {
            start: 40_000,
            step: 1,
            every: 60,
        };
        let workout = Workout::new(
            "HUGE",
            &[Step::new(Intensity::Work, Length::Open, ramp)],
            &[],
            None,
        );
        assert_eq!(workout.validate(), Ok(()));
        let chart = chart(&Player::new(workout), &TrainingProfile::default(), &REGIONS);
        assert!(chart.tops.iter().any(|&top| top != EMPTY));
    }

    #[test]
    fn a_chart_of_very_long_distance_steps_still_lays_out() {
        use crate::workout::model::{Length, Step, Workout};
        let far = Step::new(Intensity::Work, Length::Distance(u32::MAX), Target::Free);
        let workout = Workout::new("FAR", &[far; 12], &[], None);
        assert_eq!(workout.validate(), Ok(()));
        let player = Player::new(workout);
        let chart = chart(&player, &TrainingProfile::default(), &REGIONS);
        assert!(chart.tops.iter().any(|&top| top != EMPTY));
    }

    #[test]
    fn an_interval_shows_its_target_reading_verdict_and_next_step() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        let canvas = view.canvas();
        // 106-120 % of the default 215 W FTP.
        assert_eq!(canvas.find("228-258 W").map(|r| r.color), Some(POWER));
        assert_eq!(canvas.find("240").map(|r| r.color), Some(POWER));
        assert_eq!(canvas.find("ON TARGET").map(|r| r.color), Some(GREEN));
        assert!(canvas.shows("WORK 1/5") && canvas.shows("2/12"));
        assert!(canvas.shows("2:30") && canvas.shows("NEXT"));
        assert!(canvas.shows("3:00 RECOVER"));
        canvas.assert_laid_out(PAGE, &["VO2 MAX 5X3", "228-258 W", "240"], "interval");
    }

    #[test]
    fn under_target_is_amber_and_the_last_seconds_count_down_in_amber() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 177, 150);
        let view = page(&player, Some(150));
        assert_eq!(view.canvas().find("UNDER").map(|r| r.color), Some(AMBER));
        assert_eq!(view.canvas().find("0:03").map(|r| r.color), Some(AMBER));
    }

    #[test]
    fn tests_show_their_running_numbers() {
        let mut player = Player::new(FTP_TWENTY);
        while player.now(&Default::default()).played.step.intensity != Intensity::Test {
            player.skip();
        }
        ride(&mut player, 60, 262);
        let view = page(&player, Some(262));
        assert!(view.canvas().shows("ALL OUT") && view.canvas().shows("AVG 262 W"));
        let mut player = Player::new(FTP_RAMP);
        let view = page(&player, Some(140));
        assert!(view.canvas().shows("LAP TO START"), "the warm-up waits");
        player.skip();
        ride(&mut player, 130, 140);
        let view = page(&player, Some(140));
        assert!(view.canvas().shows("HOLD 134 W"), "third minute");
        assert!(view.canvas().shows("BEST 140 W"), "the best minute");
        assert!(view.canvas().shows("LAP TO END"));
    }

    #[test]
    fn a_ramp_is_a_staircase_lit_up_to_the_stair_on_now() {
        let profile = TrainingProfile::default();
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        ride(&mut player, 130, 140);
        let chart = chart(&player, &profile, &REGIONS);
        let ahead = mix(zone_color(profile.power_zone(147)), BLACK, 4);
        let to = (0..REGIONS.chart.width())
            .position(|c| chart.tops[c] != EMPTY && chart.color(c) == ahead)
            .expect("a dim stair ahead");
        let from = (0..to)
            .rev()
            .take_while(|&c| chart.tops[c] == chart.tops[to - 1])
            .last()
            .unwrap();
        assert!(to - from > 3, "a stair, not the whole ramp");
        assert!((from..to).all(|c| chart.tops[c] == chart.tops[from]));
        assert!(
            chart.tops[from - 1] > chart.tops[from],
            "the stair before is lower"
        );
        assert!(
            chart.tops[to] < chart.tops[from],
            "the next stair is higher"
        );
        assert_ne!(chart.color(from), chart.color(to));
    }

    #[test]
    fn a_finished_workout_says_so_in_full() {
        let mut player = Player::new(FTP_RAMP);
        while !player.done() {
            player.skip();
        }
        let view = page(&player, Some(120));
        assert!(view.canvas().shows("WORKOUT DONE"));
        view.canvas()
            .assert_laid_out(PAGE, &["WORKOUT DONE"], "done");
    }

    #[test]
    fn every_built_in_lays_out_at_every_step_with_extreme_readings() {
        for workout in BUILT_IN {
            let mut player = Player::new(*workout);
            loop {
                for watts in [None, Some(0), Some(1999)] {
                    let view = page(&player, watts);
                    view.canvas().assert_laid_out(
                        PAGE,
                        &[],
                        &std::format!("{:?} {watts:?}", workout.name),
                    );
                }
                if !player.done() {
                    let mut full = Text::<24>::new();
                    step_name(&mut full, player.now(&Default::default()).played, false);
                    let mut short = Text::<24>::new();
                    step_name(&mut short, player.now(&Default::default()).played, true);
                    let view = page(&player, Some(240));
                    assert!(
                        view.canvas().shows(full.as_str()) || view.canvas().shows(short.as_str()),
                        "{:?}: {} cut",
                        workout.name,
                        full.as_str()
                    );
                    assert!(
                        view.canvas().shows(workout.name.as_str()),
                        "{:?} cut at {}",
                        workout.name,
                        full.as_str()
                    );
                    for ftp in [215, 300, 400] {
                        let view = page_at(&player, Some(240), ftp);
                        let training = view.training.as_ref().unwrap();
                        let mut target = Text::<24>::new();
                        let played = training.now.played;
                        target_text(&mut target, played, &training.now, training.test);
                        assert!(
                            view.canvas().shows(target.as_str()),
                            "{:?}: {} cut at {ftp} W",
                            workout.name,
                            target.as_str()
                        );
                    }
                }
                if player.done() {
                    break;
                }
                ride(&mut player, 1, 1999);
                player.skip();
            }
        }
    }

    #[test]
    fn bars_reach_the_bottom_with_nothing_under_them() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        let chart = view.chart();
        let row = |y: usize| -> std::vec::Vec<u16> {
            (REGIONS.chart.left..REGIONS.chart.right)
                .map(|x| chart_pixel(chart, &REGIONS, x, y))
                .collect()
        };
        let last = row(REGIONS.chart.bottom - 1);
        assert!(
            (0..REGIONS.chart.width())
                .filter(|&c| chart.tops[c] != EMPTY)
                .all(|c| last[c] != BLACK),
            "every step's bar reaches the screen's last row"
        );
        assert!(
            (REGIONS.chart.top..REGIONS.chart.bottom).all(|y| !row(y).contains(&WHITE)),
            "no white marker in the chart"
        );
    }

    #[test]
    fn the_longest_workout_lays_out_on_every_panel() {
        use crate::workout::model::{Block, Step};
        let steps = [
            Step::new(
                Intensity::Cooldown,
                Length::Distance(99_900),
                Target::Watts {
                    low: 1998,
                    high: 2000,
                },
            ),
            Step::new(
                Intensity::Recovery,
                Length::Time(36_000),
                Target::Ftp { low: 99, high: 100 },
            ),
        ];
        let block = Block {
            first: 0,
            len: 2,
            count: 6,
        };
        let workout = Workout::new("WWWWWWWWWWWWWWWWWWWW", &steps, &[block], None);
        assert_eq!(workout.validate(), Ok(()));
        for layout in crate::screens::panels::ALL {
            let mut player = Player::new(workout);
            ride(&mut player, 1, 1999);
            while !player.done() {
                for radar in [false, true] {
                    let mut view = View::new(Page::Ride, 0, None);
                    view.layout = layout;
                    view.radar_paired = radar;
                    view.ride_page = RidePage::Workout;
                    view.state = Status::Recording;
                    view.sample.power_watts = Some(1999);
                    view.training = Some(Training::of(&player, &view.profile));
                    view.show_workout(chart(&player, &view.profile, &layout.workout));
                    view.prepare().check_layout();
                    let what = std::format!("{:?} radar {radar}", layout.scaled.panel);
                    let canvas = view.canvas();
                    canvas.assert_laid_out(layout.ride.body, &["1999"], &what);
                    let chart = layout.workout.chart.top;
                    assert!(
                        canvas.runs.iter().all(|run| run.bottom() <= chart),
                        "{what}: text over the chart"
                    );
                    let training = view.training.as_ref().unwrap();
                    let mut target = Text::<24>::new();
                    target_text(&mut target, training.now.played, &training.now, None);
                    assert!(
                        canvas.shows(target.as_str()),
                        "{what}: {} cut",
                        target.as_str()
                    );
                    let [full, short] = [false, true].map(|short| {
                        let mut name = Text::<24>::new();
                        step_name(&mut name, training.now.played, short);
                        name
                    });
                    assert!(
                        canvas.shows(full.as_str()) || canvas.shows(short.as_str()),
                        "{what}: {} cut",
                        full.as_str()
                    );
                    assert_next_whole(canvas, training.now.next, &what);
                }
                ride(&mut player, 1, 1999);
                player.skip();
            }
        }
    }

    fn assert_next_whole(canvas: &Canvas, next: Option<Played>, what: &str) {
        let Some(played) = next else {
            assert!(canvas.shows("FINISH"), "{what}");
            return;
        };
        let mut time = Text::<12>::new();
        length(&mut time, played.step.length);
        let intensity = played.step.intensity;
        let forms = [
            std::format!("{} {}", time.as_str(), intensity.label()),
            std::format!("{} {}", time.as_str(), intensity.short_label()),
            std::string::String::from(time.as_str()),
        ];
        assert!(
            forms.iter().any(|form| canvas.shows(form)),
            "{what}: NEXT shows none of {forms:?}"
        );
    }

    #[test]
    fn the_next_step_shortens_its_intensity_then_drops_it_rather_than_cut() {
        use crate::workout::model::Step;
        for (next, want) in [
            (Length::Time(60), "1:00 COOL DOWN"),
            (Length::Distance(99_900), "99.9 KM COOL"),
            (Length::Distance(u32::MAX), "4294967.2 KM"),
        ] {
            let steps = [
                Step::new(Intensity::Work, Length::Time(60), Target::Free),
                Step::new(Intensity::Cooldown, next, Target::Free),
            ];
            let workout = Workout::new("NEXT", &steps, &[], None);
            let mut player = Player::new(workout);
            ride(&mut player, 1, 200);
            crate::screens::check::enable(true);
            let mut view = page(&player, Some(200));
            view.prepare().check_layout();
            assert!(view.canvas().shows(want), "{want}");
            assert_next_whole(view.canvas(), player.now(&view.profile).next, want);
        }
    }

    #[test]
    fn a_long_name_keeps_its_whole_words_beside_the_step_count_all_workout_long() {
        use crate::workout::model::Step;
        let steps = [Step::new(Intensity::Work, Length::Time(60), Target::Free); 12];
        crate::screens::check::enable(true);
        for name in [
            "SWEET SPOT 2X20 WIDE",
            "THRESHOLD OVER UNDER",
            "VO2 MAX 5X4 BUILDING",
            "WWWWWWWWWWWWWWWWWWWW",
            "VO2",
        ] {
            for layout in crate::screens::panels::ALL {
                let workout = Workout::new(name, &steps, &[], None);
                let mut player = Player::new(workout);
                let mut seen = std::vec::Vec::new();
                while !player.done() {
                    ride(&mut player, 1, 200);
                    let mut view = page(&player, Some(200));
                    view.layout = layout;
                    view.show_workout(chart(&player, &view.profile, &layout.workout));
                    view.prepare().check_layout();
                    let top = layout.workout.name.bottom;
                    let shown = view
                        .canvas()
                        .runs
                        .iter()
                        .filter(|run| {
                            run.bottom() <= top + 2 && run.left == layout.workout.step.left
                        })
                        .map(|run| {
                            std::string::String::from_utf8_lossy(run.line.text()).into_owned()
                        })
                        .next()
                        .unwrap();
                    let what = std::format!("{name} on {:?}", layout.scaled.panel);
                    let whole = shown == name
                        || name.as_bytes().get(shown.len()) == Some(&b' ')
                        || !name.contains(' ');
                    assert!(whole && name.starts_with(&*shown), "{what}: {shown:?}");
                    seen.push(shown);
                    player.skip();
                }
                seen.dedup();
                assert_eq!(
                    seen.len(),
                    1,
                    "{name}: the name holds all workout long: {seen:?}"
                );
            }
        }
    }

    #[test]
    fn painted_rows_match_the_reference() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        for y in REGIONS.chart.top - 2..REGIONS.chart.bottom {
            let mut pixels = [BLACK; crate::screens::chrome::WIDTH];
            paint_chart(view.chart(), &REGIONS, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, chart_pixel(view.chart(), &REGIONS, x, y), "{x},{y}");
            }
        }
    }
}
