//! The workout page's step and its chart: one bar per played step, its
//! height the target's intensity and its colour the target's zone. Steps
//! already ridden are dimmed, and a line marks where the ride is now.
use super::Text;
use crate::ride::zones::TrainingProfile;
use crate::screens::chrome::{Rect, Row};
use crate::screens::theme::{LAYOUT, Theme};
use crate::workout::model::{Length, Metric, Name, Played, Ramp, Target, Workout};
use crate::workout::player::{Left, Now, Player};
use core::fmt::Write;
use firmware_shell::rendering::font::mix;

/// A workout on the ride screen: before it starts, while it runs and after.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub struct Training {
    pub name: Name,
    pub now: Now,
    pub done: bool,
    /// The zone whose colour the step's target takes, from 1; 0 for a free
    /// step, which takes the unlit colour.
    pub zone: u8,
    /// The same for the step after this one.
    pub next_zone: u8,
}

impl Training {
    /// What the page shows of `player`; the workout itself stays with it.
    pub fn of(player: &Player, profile: &TrainingProfile) -> Self {
        let now = player.now(profile);
        Self {
            name: player.workout().name,
            zone: now.band.map_or(0, |band| {
                zone(band.metric, band.low.midpoint(band.high), profile)
            }),
            next_zone: now
                .next
                .map_or(0, |next| target_zone(next.step.target, profile)),
            now,
            done: player.done(),
        }
    }
}

/// The widest the chart may be: the whole panel, which a tree may give it.
const CHART_COLUMNS: usize =
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.width);
/// Untimed steps take this much chart width, as if they lasted this long.
const OPEN_WEIGHT_S: u32 = 120;
/// The most intensity the chart's height holds, in percent of threshold.
const FULL_PCT: u32 = 150;
/// Most stairs a ramp draws before the one on now.
const MAX_STAIRS: u32 = 40;
/// How much of its colour a ridden step keeps over the background, of 15.
const RIDDEN_COVERAGE: u8 = 5;

/// The chart's columns, prepared once per frame.
#[derive(Clone, Hash)]
pub struct Chart {
    /// The chart tile's box.
    pub rect: Rect,
    /// Each column in 16 bits, to keep the view small: its top row, and
    /// above it its colour as an index into `palette`.
    columns: [u16; CHART_COLUMNS],
    /// The line's colour first.
    palette: heapless::Vec<u16, PALETTE>,
    /// The first column of the line where the ride is now; none past the
    /// chart's width.
    now: u16,
}

/// Colours a chart can use: the line's, then each zone and the unlit
/// colour, lit and ridden.
const PALETTE: usize = 17;
/// The columns the line where the ride is now takes.
const LINE: usize = 2 * LAYOUT.space.rule as usize;

const TOP_BITS: u32 = 9;
/// A column with no bar.
const EMPTY: u16 = (1 << TOP_BITS) - 1;
const _: () = assert!(PALETTE <= 1 << (16 - TOP_BITS));
const _: () = assert!(
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.height)
        < EMPTY as usize
);

impl Chart {
    pub const EMPTY: Self = Self {
        rect: Rect::EMPTY,
        columns: [EMPTY; CHART_COLUMNS],
        palette: heapless::Vec::new(),
        now: u16::MAX,
    };

    fn top(&self, column: usize) -> u16 {
        self.columns[column] & EMPTY
    }

    fn index_of(&self, column: usize) -> u16 {
        self.columns[column] >> TOP_BITS
    }

    fn color(&self, column: usize) -> u16 {
        self.palette
            .get(usize::from(self.index_of(column)))
            .copied()
            .unwrap_or(self.ink())
    }

    fn ink(&self) -> u16 {
        self.palette.first().copied().unwrap_or_default()
    }

    /// The columns of the line where the ride is now.
    fn line(&self) -> core::ops::Range<usize> {
        let start = usize::from(self.now).min(self.rect.width());
        start..(start + LINE).min(self.rect.width())
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

impl Default for Chart {
    fn default() -> Self {
        Self::EMPTY
    }
}

pub const fn unit(metric: Metric) -> &'static [u8] {
    match metric {
        Metric::Power => b"W",
        Metric::Heart => b"BPM",
        Metric::Cadence => b"RPM",
    }
}

/// M:SS, or H:MM:SS from an hour.
pub fn clock(value: &mut Text<12>, secs: u32) {
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

/// Kilometres with two decimals, as the step's distance left reads.
pub fn km(value: &mut Text<12>, metres: u32) {
    let _ = write!(value, "{}.{:02}", metres / 1000, metres % 1000 / 10);
}

/// Cadence has no zones; its targets take Z3's green, cadence's colour at night.
const CADENCE_ZONE: u8 = 3;

/// The zone of a target reading of `metric`.
fn zone(metric: Metric, reading: u16, profile: &TrainingProfile) -> u8 {
    match metric {
        Metric::Power => profile.power_zone(reading),
        Metric::Heart => profile.heart_zone(reading),
        Metric::Cadence => CADENCE_ZONE,
    }
}

fn target_zone(target: Target, profile: &TrainingProfile) -> u8 {
    target.band(profile, 0).map_or(0, |band| {
        zone(band.metric, band.low.midpoint(band.high), profile)
    })
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

/// Stairs a ramp draws: up to the first stage that reaches the chart's top,
/// and the stage on now even past it.
fn stairs(ramp: Ramp, profile: &TrainingProfile, current: u32) -> u32 {
    let top = u32::from(profile.ftp_watts) * FULL_PCT;
    let reach = (0..MAX_STAIRS)
        .find(|&stage| u32::from(ramp.stage_watts(stage)) * 100 >= top)
        .unwrap_or(MAX_STAIRS);
    reach.max(current) + 1
}

/// Lay out the chart across `rect`, the chart tile's box, in `theme`'s
/// colours: each played step takes width in proportion to its time, with a
/// one-column gap before it, and the hardest step, up to the cap, reaches
/// nearly the top.
/// A ramp is a stair per stage.
pub fn chart(player: &Player, profile: &TrainingProfile, rect: Rect, theme: &Theme) -> Chart {
    let (width, rows) = (rect.width(), rect.height());
    debug_assert!(width <= CHART_COLUMNS && rows < usize::from(EMPTY));
    let mut chart = Chart {
        rect,
        ..Chart::default()
    };
    chart.index(theme.text);
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
    let hardest = workout
        .played()
        .filter_map(|played| intensity(played.step.target, profile))
        .max()
        .unwrap_or(FULL_PCT);
    let full = (hardest + hardest / 16).clamp(1, FULL_PCT) as usize;
    let mut start = 0u64;
    for (index, played) in workout.played().enumerate() {
        let from = (start * width as u64 / total) as usize;
        start += u64::from(weight(index, &played));
        let to = (start * width as u64 / total) as usize;
        let ridden = index < position || done;
        let first = (from + 1).min(to);
        let last = to.min(width);
        let ramp = ramp_of(index, &played);
        let lit = |zone: u8| {
            let color = theme.zone(zone);
            if ridden {
                mix(color, theme.bg, RIDDEN_COVERAGE)
            } else {
                color
            }
        };
        let (color, pct) = (
            lit(target_zone(played.step.target, profile)),
            intensity(played.step.target, profile).unwrap_or(40),
        );
        if index == position && !done {
            let step = match ramp {
                Some((_, stairs, current)) => (current * 1000 / stairs) as usize,
                None => usize::from(now.progress),
            };
            let at = (first + (last - first) * step / 1000).min(width.saturating_sub(1));
            chart.now = at as u16;
        }
        for column in first..last {
            let (color, pct) = match ramp {
                Some((ramp, stairs, _)) => {
                    let stage =
                        ((column - first) as u32 * stairs / (last - first) as u32).min(stairs - 1);
                    let watts = ramp.stage_watts(stage);
                    (
                        lit(profile.power_zone(watts)),
                        u32::from(watts) * 100 / u32::from(profile.ftp_watts.max(1)),
                    )
                }
                None => (color, pct),
            };
            let height = (pct as usize).min(full) * rows / full;
            let index = u16::from(chart.index(color));
            chart.columns[column] = (rows - height.max(3).min(rows)) as u16 | index << TOP_BITS;
        }
    }
    if done {
        chart.now = width.saturating_sub(LINE) as u16;
    }
    chart
}

pub(super) fn chart_parts(chart: &Chart, parts: &mut firmware_shell::shell::Parts) {
    parts.add(chart.rect.top..chart.rect.bottom, chart);
}

pub(super) fn paint_chart(chart: &Chart, row: &mut Row) {
    let (y, area) = (row.y(), chart.rect);
    if !(area.top..area.bottom).contains(&y) {
        return;
    }
    let line = (y - area.top) as u16;
    let width = area.width();
    let mut column = 0;
    while column < width {
        let filled = |c: usize| chart.top(c) != EMPTY && line >= chart.top(c);
        if !filled(column) {
            column += 1;
            continue;
        }
        let color = chart.index_of(column);
        let start = column;
        while column < width && filled(column) && chart.index_of(column) == color {
            column += 1;
        }
        row.span(area.left + start..area.left + column, chart.color(start));
    }
    let line = chart.line();
    if !line.is_empty() {
        row.span(area.left + line.start..area.left + line.end, chart.ink());
    }
}

#[cfg(test)]
pub(super) fn chart_pixel(chart: &Chart, x: usize, y: usize) -> Option<u16> {
    let area = chart.rect;
    if !(area.left..area.right).contains(&x) || !(area.top..area.bottom).contains(&y) {
        return None;
    }
    let column = x - area.left;
    if chart.line().contains(&column) {
        return Some(chart.ink());
    }
    (chart.top(column) != EMPTY && (y - area.top) as u16 >= chart.top(column))
        .then(|| chart.color(column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::ScreenId;
    use crate::compose::kit::chart::plotted;
    use crate::compose::registry::{CHART, STEP};
    use crate::ride::log::{Sample, Status};
    use crate::screens::Device;
    use crate::screens::ride::RidePage;
    use crate::screens::theme::{DARK, LIGHT};
    use crate::screens::workout::{Page, View};
    use crate::workout::library::{BUILT_IN, FTP_RAMP, FTP_TWENTY, VO2_MAX};
    use crate::workout::model::{Block, Intensity, Step};
    use std::string::String;
    use std::vec::Vec;

    /// Show `player` on `view`'s workout page, as the app does.
    fn show(view: &mut View, player: &Player) {
        view.training = Some(Training::of(player, &view.profile));
        view.compose_main();
        let laid = view.laids.unwrap().get(ScreenId::RideWorkout).unwrap();
        let rect = plotted(laid.box_of(CHART).unwrap(), &view.device.scaled);
        let theme = crate::compose::data::Data::theme(&*view);
        view.show_workout(chart(player, &view.profile, rect, theme));
    }

    fn page_on(
        device: &'static Device,
        player: &Player,
        watts: Option<u16>,
        radar: bool,
        light: bool,
    ) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        view.device = device;
        view.radar_paired = radar;
        view.light = light;
        view.ride_page = RidePage::Workout;
        view.state = Status::Recording;
        view.sample.power_watts = watts;
        show(&mut view, player);
        view.prepare().check_layout();
        view
    }

    fn page(player: &Player, watts: Option<u16>) -> View<'static> {
        page_on(&crate::screens::C606, player, watts, false, false)
    }

    fn ride(player: &mut Player, seconds: u32, watts: u16) {
        let sample = Sample {
            power_watts: Some(watts),
            ..Sample::default()
        };
        for _ in 0..seconds {
            player.add(1000, 8000, &sample, &TrainingProfile::default());
        }
    }

    fn interval(watts: u16) -> Player {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, watts);
        player
    }

    fn texts(view: &View) -> Vec<String> {
        view.canvas()
            .runs
            .iter()
            .map(|run| String::from_utf8_lossy(run.line.text()).into_owned())
            .collect()
    }

    fn chart_box(device: &'static Device) -> Rect {
        page_on(device, &interval(240), Some(240), false, false)
            .chart()
            .rect
    }

    #[test]
    fn an_interval_shows_its_range_power_time_left_and_next_step() {
        let view = page(&interval(240), Some(240));
        let canvas = view.canvas();
        // 106-120 % of the default 215 W FTP.
        let [high, low, hero] = ["258", "228", "240"].map(|text| canvas.find(text).unwrap());
        assert!(high.bottom() < low.bottom() && high.left == low.left);
        assert!(hero.left > low.right(), "the range sits left of the power");
        assert_eq!(hero.color, DARK.power);
        for words in ["W", "3s", "2:30", "LEFT", "NEXT", "3:00", "RECOVER"] {
            assert!(canvas.shows(words), "{words}: {:?}", texts(&view));
        }
        for gone in ["VO2 MAX 5X3", "ON TARGET", "WORK 1/5", "2/12"] {
            assert!(!canvas.shows(gone), "{gone}");
        }
    }

    #[test]
    fn light_writes_every_word_black_and_keeps_colour_in_the_band_and_chart() {
        let view = page_on(
            &crate::screens::C606,
            &interval(240),
            Some(240),
            false,
            true,
        );
        let inks = [LIGHT.text, LIGHT.sub, LIGHT.good];
        assert!(
            view.canvas()
                .runs
                .iter()
                .all(|run| inks.contains(&run.color)),
            "{:?}",
            texts(&view)
        );
        let zone = LIGHT.zone(5);
        assert!(
            view.canvas().shapes.iter().any(|shape| matches!(
                shape,
                crate::screens::canvas::Shape::Bar { color, .. } if *color == zone
            )),
            "the band in Z5"
        );
        let chart = view.chart();
        assert!((0..chart.rect.width()).any(|c| chart.top(c) != EMPTY && chart.color(c) == zone));
    }

    #[test]
    fn beside_a_radar_the_hero_steps_down_one_face_and_a_sprint_one_more() {
        use firmware_shell::rendering::font::FaceRole;
        let at = &crate::screens::C606.scaled;
        let hero_face = |player: &Player, watts: u16, radar: bool| {
            let view = page_on(&crate::screens::C606, player, Some(watts), radar, false);
            let text = std::format!("{}", view.training.unwrap().now.reading.unwrap());
            core::ptr::from_ref(view.canvas().find(&text).unwrap().line.face())
        };
        let face = |role| core::ptr::from_ref(at.face(role));
        let (steady, sprint) = (interval(243), interval(1999));
        assert_eq!(hero_face(&steady, 243, false), face(FaceRole::Hero));
        assert_eq!(hero_face(&steady, 243, true), face(FaceRole::Large));
        assert_eq!(hero_face(&sprint, 1999, false), face(FaceRole::Large));
        assert_eq!(hero_face(&sprint, 1999, true), face(FaceRole::Digits40));
    }

    #[test]
    fn ridden_steps_dim_and_a_line_marks_where_the_ride_is() {
        let player = interval(240);
        let rect = chart_box(&crate::screens::C606);
        let chart = chart(&player, &TrainingProfile::default(), rect, &DARK);
        let warm_up = 3;
        let dim = mix(DARK.zone(2), DARK.bg, RIDDEN_COVERAGE);
        assert_eq!(chart.color(warm_up), dim, "the warm-up is ridden");
        let now = chart.line().start;
        assert_eq!(
            chart.color(now + 3),
            DARK.zone(5),
            "the interval on now is lit"
        );
        let last = rect.width() - 3;
        assert_eq!(
            chart.color(last),
            DARK.zone(1),
            "the cool-down ahead is lit"
        );
        for y in rect.top..rect.bottom {
            assert_eq!(chart_pixel(&chart, rect.left + now, y), Some(DARK.text));
        }
    }

    #[test]
    fn tests_show_their_running_numbers_and_open_steps_wait_for_a_lap() {
        let mut player = Player::new(FTP_TWENTY);
        while player.now(&Default::default()).played.step.intensity != Intensity::Test {
            player.skip();
        }
        ride(&mut player, 60, 262);
        let view = page(&player, Some(262));
        assert!(view.canvas().shows("262") && view.canvas().shows("AVG"));
        let mut player = Player::new(FTP_RAMP);
        let view = page(&player, Some(140));
        assert!(view.canvas().shows("LAP TO START"), "the warm-up waits");
        player.skip();
        ride(&mut player, 130, 140);
        let view = page(&player, Some(140));
        assert!(view.canvas().shows("134"), "the third minute's stage");
        assert!(view.canvas().shows("LAP TO END"));
    }

    #[test]
    fn a_finished_workout_says_so() {
        let mut player = Player::new(FTP_RAMP);
        while !player.done() {
            player.skip();
        }
        let view = page(&player, Some(120));
        assert!(view.canvas().shows("WORKOUT DONE"));
        assert!(!view.canvas().shows("NEXT"));
    }

    #[test]
    fn every_built_in_lays_out_at_every_step_with_extreme_readings_on_every_panel() {
        for device in crate::screens::panels::ALL {
            for workout in BUILT_IN {
                let mut player = Player::new(*workout);
                loop {
                    for (watts, radar) in [(None, true), (Some(0), false), (Some(1999), true)] {
                        let view = page_on(device, &player, watts, radar, false);
                        let what = std::format!(
                            "{:?} {watts:?} radar {radar} on {:?}",
                            workout.name,
                            device.scaled.panel
                        );
                        let canvas = view.canvas();
                        canvas.assert_laid_out_at(
                            &device.scaled,
                            device.scaled.screen(),
                            &[],
                            &what,
                        );
                        let chart = view.chart().rect.top;
                        assert!(
                            canvas.runs.iter().all(|run| run.bottom() <= chart),
                            "{what}: text over the chart"
                        );
                        let reading = match view.training.unwrap().now.reading {
                            Some(reading) => std::format!("{reading}"),
                            None => String::from("--"),
                        };
                        assert!(canvas.shows(&reading), "{what}: {:?}", texts(&view));
                    }
                    if player.done() {
                        break;
                    }
                    ride(&mut player, 1, 1999);
                    player.skip();
                }
            }
        }
    }

    #[test]
    fn the_next_step_shortens_its_name_rather_than_cut_it() {
        for (next, want) in [
            (Length::Time(60), "COOL"),
            (Length::Distance(99_900), "KM COOL"),
        ] {
            let steps = [
                Step::new(Intensity::Work, Length::Time(60), Target::Free),
                Step::new(Intensity::Cooldown, next, Target::Free),
            ];
            let player = Player::new(Workout::new("NEXT", &steps, &[], None));
            for device in crate::screens::panels::ALL {
                let view = page_on(device, &player, Some(200), true, false);
                let shown = texts(&view);
                assert!(
                    shown.iter().any(|text| text.starts_with(want)),
                    "{want}: {shown:?}"
                );
            }
        }
    }

    #[test]
    fn a_chart_of_a_ramp_from_tens_of_kilowatts_still_lays_out() {
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
        let rect = chart_box(&crate::screens::C606);
        let chart = chart(
            &Player::new(workout),
            &TrainingProfile::default(),
            rect,
            &DARK,
        );
        assert!((0..rect.width()).any(|c| chart.top(c) != EMPTY));
    }

    #[test]
    fn a_chart_of_very_long_distance_steps_still_lays_out() {
        let far = Step::new(Intensity::Work, Length::Distance(u32::MAX), Target::Free);
        let workout = Workout::new("FAR", &[far; 12], &[], None);
        assert_eq!(workout.validate(), Ok(()));
        let rect = chart_box(&crate::screens::C606);
        let chart = chart(
            &Player::new(workout),
            &TrainingProfile::default(),
            rect,
            &DARK,
        );
        assert!((0..rect.width()).any(|c| chart.top(c) != EMPTY));
    }

    #[test]
    fn a_ramp_is_a_staircase() {
        let profile = TrainingProfile::default();
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        ride(&mut player, 130, 140);
        let chart = chart(&player, &profile, chart_box(&crate::screens::C606), &DARK);
        let mut stairs: Vec<u16> = (chart.line().end..chart.rect.width())
            .map(|c| chart.top(c))
            .take_while(|&top| top != EMPTY)
            .collect();
        stairs.dedup();
        assert!(stairs.len() > 3, "{stairs:?}");
        assert!(
            stairs.windows(2).all(|pair| pair[1] < pair[0]),
            "each stair higher: {stairs:?}"
        );
    }

    #[test]
    fn the_chart_at_the_largest_box_accept_allows_paints_on_every_panel() {
        for device in crate::screens::panels::ALL {
            let rect = device.scaled.screen();
            for (index, &workout) in BUILT_IN.iter().enumerate() {
                let mut player = Player::new(*workout);
                ride(&mut player, 30, 240);
                for theme in [&DARK, &LIGHT] {
                    let chart = chart(&player, &TrainingProfile::default(), rect, theme);
                    let what = std::format!("workout {index} on {:?}", device.scaled.panel);
                    let tops: Vec<u16> = (0..rect.width()).map(|c| chart.top(c)).collect();
                    assert!(tops.iter().any(|&top| top != EMPTY), "{what}");
                    assert!(
                        tops.iter()
                            .all(|&top| top == EMPTY || usize::from(top) < rect.height()),
                        "{what}"
                    );
                    for y in rect.top..rect.bottom {
                        let mut pixels = [theme.bg; crate::screens::chrome::WIDTH];
                        paint_chart(&chart, &mut Row::new(y, &mut pixels));
                        for (x, &pixel) in pixels.iter().enumerate() {
                            let want = chart_pixel(&chart, x, y).unwrap_or(theme.bg);
                            assert_eq!(pixel, want, "{what}: {x},{y}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bars_stand_on_the_chart_s_bottom_row() {
        let view = page(&interval(240), Some(240));
        let chart = view.chart();
        let bottom = chart.rect.bottom - 1;
        assert!(
            (0..chart.rect.width())
                .filter(|&c| chart.top(c) != EMPTY)
                .all(|c| chart_pixel(chart, chart.rect.left + c, bottom).is_some())
        );
    }

    /// A tap on the time left adds to the step; nothing else on the page
    /// acts.
    #[test]
    fn only_the_time_left_answers_a_tap() {
        let view = page(&interval(240), Some(240));
        let laid = view.laids.unwrap().get(ScreenId::RideWorkout).unwrap();
        let solved = laid.solved();
        let stops: Vec<_> = solved
            .focus_order()
            .map(|placed| solved.action(placed))
            .collect();
        assert_eq!(stops, [Some(crate::compose::input::Action::ExtendStep)]);
        assert!(laid.box_of(STEP).is_some());
        assert_eq!(
            laid.buttons().held,
            Some(crate::compose::input::Action::ExtendStep)
        );
    }

    #[test]
    fn the_page_reads_the_steps_target() {
        assert_eq!(RidePage::Workout.readings(), None);
    }

    #[test]
    fn a_block_s_repeats_each_take_a_bar() {
        let steps = [
            Step::new(
                Intensity::Work,
                Length::Time(60),
                Target::Ftp {
                    low: 100,
                    high: 110,
                },
            ),
            Step::new(
                Intensity::Recovery,
                Length::Time(60),
                Target::Ftp { low: 40, high: 50 },
            ),
        ];
        let block = Block {
            first: 0,
            len: 2,
            count: 3,
        };
        let workout = Workout::new("BLOCK", &steps, &[block], None);
        let rect = chart_box(&crate::screens::C606);
        let chart = chart(
            &Player::new(workout),
            &TrainingProfile::default(),
            rect,
            &DARK,
        );
        let mut tops: Vec<u16> = (0..rect.width()).map(|c| chart.top(c)).collect();
        tops.dedup();
        let bars = tops.iter().filter(|&&top| top != EMPTY).count();
        assert_eq!(bars, 6, "{tops:?}");
    }
}
