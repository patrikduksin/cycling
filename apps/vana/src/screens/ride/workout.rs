//! The workout page: the step on now and its target, the reading checked
//! against it as the hero, how much of the step is left, the next step,
//! and the whole workout as a chart.
//!
//! Top to bottom: the workout's name and the step count; the step and its
//! target band in the target's metric colour; the 3 s power (or heart rate,
//! or cadence) in its metric colour; a gauge with the band at 25 % and a
//! white disc for the reading, then UNDER / ON TARGET / OVER; the time or
//! distance left over the step's progress bar, amber for the last seconds;
//! the next step; then one bar per played step, its height the target's
//! intensity and its colour the target's zone. Past and current steps are
//! full colour, the rest 25 %, and a white bar marks the step on now.
use super::{BODY, LEFT, RIGHT, Text};
use crate::ride::zones::TrainingProfile;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BLACK, BODY as BODY_FACE, CADENCE, CAPTION, GREEN, HEART, HERO, LARGE, POWER,
    Rect, Row, SECONDARY, TERTIARY, VALUE, WHITE, blend, zone_color,
};
use crate::screens::workout::View;
use crate::workout::model::{Length, Metric, Name, Played, Target, Test, Workout};
use crate::workout::player::{COUNTDOWN_S, Compliance, Left, Now, Player, TestReadout};
use core::fmt::Write;
use firmware_shell::rendering::font::SPILL;

/// A workout on the ride screen: before it starts, while it runs and after.
#[derive(Clone, Copy)]
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

const NAME_BASE: usize = BODY.top + 16;
const STEP_BASE: usize = NAME_BASE + 26;
const HERO_BASE: usize = STEP_BASE + 8 + HERO.cap as usize + HERO.top as usize;
const GAUGE_TOP: usize = HERO_BASE + 8;
const GAUGE_HEIGHT: usize = 8;
const VERDICT_BASE: usize = GAUGE_TOP + GAUGE_HEIGHT + 16;
const LEFT_BASE: usize = VERDICT_BASE + 32;
const PROGRESS_TOP: usize = LEFT_BASE + 7;
const PROGRESS_HEIGHT: usize = 5;
const NEXT_BASE: usize = PROGRESS_TOP + PROGRESS_HEIGHT + 22;
/// Rows where a tap adds to the step: the time left and its progress.
pub const EXTEND_ROWS: core::ops::Range<usize> =
    VERDICT_BASE + 4..PROGRESS_TOP + PROGRESS_HEIGHT + 8;
/// The chart reaches into the toast band, as the other charts do.
pub(super) const CHART_TOP: usize = NEXT_BASE + 10;
pub(super) const CHART_BOTTOM: usize = BODY.bottom - 12;
const CHART_HEIGHT: usize = CHART_BOTTOM - CHART_TOP;
/// The white bar under the step on now.
const NOW_TOP: usize = CHART_BOTTOM + 3;
const NOW_HEIGHT: usize = 3;
pub(super) const CHART_WIDTH: usize = RIGHT - LEFT;
const _: () = assert!(CHART_HEIGHT >= 20, "the workout chart needs some height");
const _: () = assert!(NOW_TOP + NOW_HEIGHT <= BODY.bottom);
/// Untimed steps take this much chart width, as if they lasted this long.
const OPEN_WEIGHT_S: u32 = 120;
/// A target of this much intensity fills the chart's height.
const FULL_PCT: u32 = 150;

/// The chart's columns, prepared once per frame.
#[derive(Clone)]
pub struct Chart {
    /// Rows below `CHART_TOP` where each column's bar starts; `EMPTY` for a gap.
    tops: [u8; CHART_WIDTH],
    /// Each column's colour as an index into `palette`, to keep the view small.
    colors: [u8; CHART_WIDTH],
    palette: heapless::Vec<u16, PALETTE>,
    /// The columns of the step on now.
    now: (usize, usize),
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

impl Default for Chart {
    fn default() -> Self {
        Self {
            tops: [EMPTY; CHART_WIDTH],
            colors: [0; CHART_WIDTH],
            palette: heapless::Vec::new(),
            now: (0, 0),
        }
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
    let _ = match (played.step.target, now.band) {
        (Target::Ramp { .. }, Some(band)) => write!(value, "HOLD {} W", band.low),
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

pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let Some(training) = &view.training else {
        canvas.fit(
            &[&BODY_FACE],
            b"NO WORKOUT",
            RIGHT - LEFT,
            LEFT,
            STEP_BASE,
            Align::Left,
            SECONDARY,
        );
        return canvas;
    };
    let now = &training.now;
    let width = RIGHT - LEFT;

    // The workout and where in it the rider is.
    let mut count = Text::<24>::new();
    // Decided on the widest count, so the format holds all workout long.
    let mut widest = Text::<24>::new();
    let _ = write!(widest, "STEP {}/{}", now.steps, now.steps);
    let word =
        CAPTION.width(training.name.as_bytes()) + CAPTION.width(widest.as_bytes()) + 8 + 2 * SPILL
            <= width;
    let _ = if training.done {
        count.push_str("DONE")
    } else if word {
        write!(count, "STEP {}/{}", now.position + 1, now.steps)
    } else {
        write!(count, "{}/{}", now.position + 1, now.steps)
    };
    let count = canvas.caption(
        count.as_bytes(),
        width / 2,
        RIGHT - SPILL,
        NAME_BASE,
        Align::Right,
    );
    canvas.caption(
        training.name.as_bytes(),
        count.left.saturating_sub(LEFT + 8),
        LEFT,
        NAME_BASE,
        Align::Left,
    );

    // The step and its target.
    let metric = now.band.map_or(Metric::Power, |band| band.metric);
    let color = metric_color(metric);
    let mut target = Text::<24>::new();
    let mut name = Text::<24>::new();
    if training.done {
        let _ = name.push_str("WORKOUT DONE");
        let _ = target.push_str("FREE RIDE");
    } else {
        step_name(&mut name, now.played, false);
        target_text(&mut target, now.played, now, training.test);
    }
    let target_color = if now.band.is_some() { color } else { SECONDARY };
    let name_room = CAPTION.width(name.as_bytes()) + 8 + 2 * SPILL;
    // The target is never cut: it keeps at least its smallest size.
    let target_floor = CAPTION.width(target.as_bytes()) + SPILL;
    let target = canvas.fit(
        &[&BODY_FACE, &CAPTION],
        target.as_bytes(),
        (width * 3 / 5)
            .min(width.saturating_sub(name_room))
            .max(target_floor),
        RIGHT - SPILL,
        STEP_BASE,
        Align::Right,
        target_color,
    );
    let name_color = if training.done { GREEN } else { WHITE };
    if !training.done
        && CAPTION.width(name.as_bytes()) + SPILL > target.left.saturating_sub(LEFT + 8)
    {
        name = Text::new();
        step_name(&mut name, now.played, true);
    }
    canvas.fit(
        &[&BODY_FACE, &CAPTION],
        name.as_bytes(),
        target.left.saturating_sub(LEFT + 8),
        LEFT,
        STEP_BASE,
        Align::Left,
        name_color,
    );

    // The reading the band checks, as the hero, its unit beside it.
    let mut reading = Text::<12>::new();
    let _ = match now.reading {
        Some(value) => write!(reading, "{value}"),
        None => reading.push_str("--"),
    };
    let unit_width = CAPTION.width(unit(metric)) + SPILL;
    let smoothing = CAPTION.width(b"3 S") + SPILL + 8;
    let hero = canvas.fit(
        &[&HERO, &LARGE],
        reading.as_bytes(),
        width - unit_width - 6 - smoothing,
        LEFT,
        HERO_BASE,
        Align::Left,
        color,
    );
    canvas.fit(
        &[&CAPTION],
        unit(metric),
        unit_width,
        hero.right() + 6,
        HERO_BASE,
        Align::Left,
        SECONDARY,
    );
    if metric == Metric::Power && now.reading.is_some() {
        canvas.caption(b"3 S", 40, RIGHT - SPILL, HERO_BASE, Align::Right);
    }

    // The band gauge and the verdict under it.
    gauge(&mut canvas, now, color);
    let mut free_from = LEFT;
    if let Some(compliance) = now.compliance {
        let (text, verdict): (&[u8], u16) = match compliance {
            Compliance::Under => (b"UNDER", AMBER),
            Compliance::In => (b"ON TARGET", GREEN),
            Compliance::Over => (b"OVER", AMBER),
        };
        let run = canvas.fit(
            &[&CAPTION],
            text,
            width / 2,
            LEFT,
            VERDICT_BASE,
            Align::Left,
            verdict,
        );
        free_from = run.right() + 10;
    }
    if let Some(test) = now.test {
        let mut text = Text::<24>::new();
        let _ = match test {
            TestReadout::Average(w) => write!(text, "AVG {w} W"),
            TestReadout::BestMinute(w) => write!(text, "BEST {w} W"),
        };
        canvas.fit(
            &[&CAPTION],
            text.as_bytes(),
            RIGHT - SPILL - free_from,
            RIGHT - SPILL,
            VERDICT_BASE,
            Align::Right,
            POWER,
        );
    }

    // What is left of the step, over its progress.
    if !training.done {
        let mut left = Text::<12>::new();
        let (caption, last): (&[u8], bool) = match now.left {
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
                (b"LAP TO END", false)
            }
        };
        let value = canvas.fit(
            &[&VALUE, &BODY_FACE],
            left.as_bytes(),
            width / 2,
            LEFT,
            LEFT_BASE,
            Align::Left,
            if last { AMBER } else { WHITE },
        );
        canvas.caption(
            caption,
            RIGHT - value.right() - 12,
            value.right() + 6,
            LEFT_BASE,
            Align::Left,
        );
        let rect = |left: usize, right: usize| Rect {
            left,
            top: PROGRESS_TOP,
            right,
            bottom: PROGRESS_TOP + PROGRESS_HEIGHT,
        };
        canvas.shape(Shape::Bar {
            rect: rect(LEFT, RIGHT),
            radius: 2,
            color: TERTIARY,
        });
        let lit = LEFT + width * usize::from(now.progress) / 1000;
        if lit > LEFT + 4 {
            canvas.shape(Shape::Bar {
                rect: rect(LEFT, lit),
                radius: 2,
                color: WHITE,
            });
        }

        // The next step.
        let mut next = Text::<24>::new();
        match now.next {
            Some(played) => {
                let mut time = Text::<12>::new();
                length(&mut time, played.step.length);
                let _ = write!(next, "{} {}", time.as_str(), played.step.intensity.label());
            }
            None => {
                let _ = next.push_str("FINISH");
            }
        }
        let caption = canvas.caption(b"NEXT", 50, LEFT, NEXT_BASE, Align::Left);
        canvas.fit(
            &[&BODY_FACE, &CAPTION],
            next.as_bytes(),
            RIGHT - caption.right() - 14,
            caption.right() + 8,
            NEXT_BASE,
            Align::Left,
            WHITE,
        );
    }
    canvas
}

/// The track from 70 % of the band's low end to 130 % of its high end, the
/// band on it at 25 % of the metric colour, and the reading as a disc.
fn gauge(canvas: &mut Canvas, now: &Now, color: u16) {
    let Some(band) = now.band else {
        return;
    };
    let low = u32::from(band.low) * 7 / 10;
    let high = (u32::from(band.high) * 13 / 10).max(low + 1);
    let x = |value: u32| {
        LEFT + (value.clamp(low, high) - low) as usize * (RIGHT - LEFT) / (high - low) as usize
    };
    let rect = |left: usize, right: usize| Rect {
        left,
        top: GAUGE_TOP,
        right,
        bottom: GAUGE_TOP + GAUGE_HEIGHT,
    };
    canvas.shape(Shape::Bar {
        rect: rect(LEFT, RIGHT),
        radius: 4,
        color: TERTIARY,
    });
    let (from, to) = (
        x(band.low.into()),
        x(band.high.into()).max(x(band.low.into()) + 6),
    );
    canvas.shape(Shape::Bar {
        rect: rect(from, to.min(RIGHT)),
        radius: 4,
        color: blend(color, BLACK, 6),
    });
    if let Some(reading) = now.reading {
        let cx = x(reading.into()).clamp(LEFT + 5, RIGHT - 5);
        canvas.shape(Shape::Disc {
            cx: 4 * cx as i32,
            cy: 4 * (GAUGE_TOP + GAUGE_HEIGHT / 2) as i32,
            radius: 4 * 5,
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
        Target::Ramp { .. } => FULL_PCT,
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

/// Lay out the chart: each played step takes width in proportion to its
/// time, with a one-column gap before it.
pub fn chart(player: &Player, profile: &TrainingProfile) -> Chart {
    let mut chart = Chart::default();
    let weight = |played: &Played| match played.step.length {
        Length::Time(secs) => u32::from(secs),
        Length::Distance(m) => (m / 8).max(60),
        Length::Open => OPEN_WEIGHT_S,
    };
    let workout: &Workout = player.workout();
    let done = player.done();
    let total = workout
        .played()
        .map(|p| u64::from(weight(&p)))
        .sum::<u64>()
        .max(1);
    let mut start = 0u64;
    let position = player.now(profile).position;
    for (index, played) in workout.played().enumerate() {
        let from = (start * CHART_WIDTH as u64 / total) as usize;
        start += u64::from(weight(&played));
        let to = (start * CHART_WIDTH as u64 / total) as usize;
        let color = zone(played.step.target, profile);
        let ahead = index > position && !done;
        let color = if ahead { blend(color, BLACK, 4) } else { color };
        let pct = intensity(played.step.target, profile).unwrap_or(40);
        let shade = chart.index(color);
        for column in (from + 1).min(to)..to.min(CHART_WIDTH) {
            let pct = if let Target::Ramp { .. } = played.step.target {
                // A ramp rises across its width.
                60 + (FULL_PCT - 60) * (column - from) as u32 / (to - from).max(1) as u32
            } else {
                pct
            };
            let height = (pct.min(FULL_PCT) as usize * CHART_HEIGHT / FULL_PCT as usize).max(3);
            chart.tops[column] = (CHART_HEIGHT - height) as u8;
            chart.colors[column] = shade;
        }
        if index == position && !done {
            chart.now = ((from + 1).min(to), to.min(CHART_WIDTH));
        }
    }
    chart
}

/// Paint the chart's row: its bars, then the white bar under the step on now.
pub(super) fn paint_chart(chart: &Chart, row: &mut Row) {
    let y = row.y();
    if (NOW_TOP..NOW_TOP + NOW_HEIGHT).contains(&y) && chart.now.1 > chart.now.0 {
        row.span(LEFT + chart.now.0..LEFT + chart.now.1, WHITE);
        return;
    }
    if !(CHART_TOP..CHART_BOTTOM).contains(&y) {
        return;
    }
    let line = (y - CHART_TOP) as u8;
    let mut column = 0;
    while column < CHART_WIDTH {
        let filled = |c: usize| chart.tops[c] != EMPTY && line >= chart.tops[c];
        if !filled(column) {
            column += 1;
            continue;
        }
        let color = chart.colors[column];
        let start = column;
        while column < CHART_WIDTH && filled(column) && chart.colors[column] == color {
            column += 1;
        }
        row.span(LEFT + start..LEFT + column, chart.color(start));
    }
}

/// The per-pixel original of `paint_chart`.
#[cfg(test)]
pub(super) fn chart_pixel(chart: &Chart, x: usize, y: usize) -> u16 {
    if !(LEFT..RIGHT).contains(&x) {
        return BLACK;
    }
    let column = x - LEFT;
    if (NOW_TOP..NOW_TOP + NOW_HEIGHT).contains(&y) {
        return if (chart.now.0..chart.now.1).contains(&column) {
            WHITE
        } else {
            BLACK
        };
    }
    if (CHART_TOP..CHART_BOTTOM).contains(&y)
        && chart.tops[column] != EMPTY
        && (y - CHART_TOP) as u8 >= chart.tops[column]
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
    use crate::workout::model::Intensity;
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
        view.chart = chart(player, &view.profile);
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

    const PAGE: Rect = Rect {
        left: BODY.left,
        top: BODY.top,
        right: BODY.right,
        bottom: BODY.bottom,
    };

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
        let chart = chart(&Player::new(workout), &TrainingProfile::default());
        assert!(chart.now.1 > chart.now.0);
    }

    #[test]
    fn a_chart_of_very_long_distance_steps_still_lays_out() {
        use crate::workout::model::{Length, Step, Workout};
        let far = Step::new(Intensity::Work, Length::Distance(u32::MAX), Target::Free);
        let workout = Workout::new("FAR", &[far; 12], &[], None);
        assert_eq!(workout.validate(), Ok(()));
        let player = Player::new(workout);
        let chart = chart(&player, &TrainingProfile::default());
        assert!(chart.now.1 > chart.now.0);
    }

    #[test]
    fn an_interval_shows_its_target_reading_verdict_and_next_step() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        let canvas = &view.canvas;
        // 106-120 % of the default 215 W FTP.
        assert_eq!(canvas.find("228-258 W").map(|r| r.color), Some(POWER));
        assert_eq!(canvas.find("240").map(|r| r.color), Some(POWER));
        assert_eq!(canvas.find("ON TARGET").map(|r| r.color), Some(GREEN));
        assert!(canvas.shows("WORK 1/5") && canvas.shows("2/12"));
        assert!(canvas.shows("2:30") && canvas.shows("NEXT"));
        assert!(canvas.shows("3:00 RECOVER"));
        canvas.assert_laid_out(PAGE, &["VO2 MAX 5X3", "228-258 W", "240"], "interval");
        // The step on now is marked under its bar, and past bars are lit.
        assert!(view.chart.now.1 > view.chart.now.0);
    }

    #[test]
    fn under_target_is_amber_and_the_last_seconds_count_down_in_amber() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 177, 150);
        let view = page(&player, Some(150));
        assert_eq!(view.canvas.find("UNDER").map(|r| r.color), Some(AMBER));
        assert_eq!(view.canvas.find("0:03").map(|r| r.color), Some(AMBER));
    }

    #[test]
    fn tests_show_their_running_numbers() {
        let mut player = Player::new(FTP_TWENTY);
        while player.now(&Default::default()).played.step.intensity != Intensity::Test {
            player.skip();
        }
        ride(&mut player, 60, 262);
        let view = page(&player, Some(262));
        assert!(view.canvas.shows("ALL OUT") && view.canvas.shows("AVG 262 W"));
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        ride(&mut player, 130, 140);
        let view = page(&player, Some(140));
        assert!(view.canvas.shows("HOLD 140 W"), "third minute");
        assert!(view.canvas.shows("BEST 140 W"), "the best minute");
        assert!(view.canvas.shows("LAP TO END"));
    }

    #[test]
    fn every_built_in_lays_out_at_every_step_with_extreme_readings() {
        for workout in BUILT_IN {
            let mut player = Player::new(*workout);
            loop {
                for watts in [None, Some(0), Some(1999)] {
                    let view = page(&player, watts);
                    view.canvas.assert_laid_out(
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
                        view.canvas.shows(full.as_str()) || view.canvas.shows(short.as_str()),
                        "{:?}: {} cut",
                        workout.name,
                        full.as_str()
                    );
                    assert!(
                        view.canvas.shows(workout.name.as_str()),
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
                            view.canvas.shows(target.as_str()),
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
    fn painted_rows_match_the_reference() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        for y in CHART_TOP - 2..BODY.bottom {
            let mut pixels = [BLACK; crate::screens::chrome::WIDTH];
            paint_chart(&view.chart, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, chart_pixel(&view.chart, x, y), "{x},{y}");
            }
        }
    }
}
