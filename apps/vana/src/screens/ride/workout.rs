//! The workout page's words and its chart: one bar per played step, its
//! height the target's intensity and its colour the target's zone. Past and
//! current steps are full colour, the rest 25 %.
use super::Text;
use crate::ride::zones::TrainingProfile;
use crate::screens::chrome::{BLACK, CADENCE, HEART, POWER, Rect, Row, TERTIARY, mix, zone_color};
use crate::workout::model::{Length, Metric, Name, Played, Ramp, Target, Test, Workout};
use crate::workout::player::{Left, Now, Player};
use core::fmt::Write;

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

/// The widest the chart may be: the whole panel, which a tree may give it.
const CHART_COLUMNS: usize =
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.width);
/// Untimed steps take this much chart width, as if they lasted this long.
const OPEN_WEIGHT_S: u32 = 120;
/// A target of this much intensity fills the chart's height.
const FULL_PCT: u32 = 150;
/// Most stairs a ramp draws before the one on now.
const MAX_STAIRS: u32 = 40;

/// The chart's columns, prepared once per frame.
#[derive(Clone, Hash)]
pub struct Chart {
    /// The chart tile's box.
    pub rect: Rect,
    /// Each column in 16 bits, to keep the view small: its top row, and
    /// above it its colour as an index into `palette`.
    columns: [u16; CHART_COLUMNS],
    palette: heapless::Vec<u16, PALETTE>,
}

/// Colours a chart can use: each zone, cadence and none, lit and at 25 %.
const PALETTE: usize = 24;

const TOP_BITS: u32 = 9;
/// A column with no bar.
const EMPTY: u16 = (1 << TOP_BITS) - 1;
const _: () = assert!(PALETTE <= 1 << (16 - TOP_BITS));
const _: () = assert!(
    crate::screens::scaled::largest!(crate::screens::limits::PANELS, |at| at.panel.height)
        < EMPTY as usize
);

impl Chart {
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

impl Chart {
    pub const EMPTY: Self = Self {
        rect: Rect::EMPTY,
        columns: [EMPTY; CHART_COLUMNS],
        palette: heapless::Vec::new(),
    };
}

impl Default for Chart {
    fn default() -> Self {
        Self::EMPTY
    }
}

pub fn metric_color(metric: Metric) -> u16 {
    match metric {
        Metric::Power => POWER,
        Metric::Heart => HEART,
        Metric::Cadence => CADENCE,
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

pub fn length(value: &mut Text<12>, length: Length) {
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
pub fn target_text(value: &mut Text<24>, played: Played, now: &Now, test: Option<Test>) {
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

pub fn unit_str(metric: Metric) -> &'static str {
    core::str::from_utf8(unit(metric)).unwrap_or("")
}

/// The step's name: its intensity, the run of its block when repeated, and
/// its note when there is room.
pub fn step_name(value: &mut Text<24>, played: Played, short: bool) {
    let _ = value.push_str(if short {
        played.step.intensity.short_label()
    } else {
        played.step.intensity.label()
    });
    if played.of > 1 {
        let _ = write!(value, " {}/{}", played.repetition + 1, played.of);
    }
}

pub fn next_step(next: Option<Played>, fits: impl Fn(&[u8]) -> bool) -> Text<24> {
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

pub fn whole_words(text: &[u8], fits: impl Fn(&[u8]) -> bool) -> Option<&[u8]> {
    if fits(text) {
        return Some(text);
    }
    (1..text.len())
        .rev()
        .filter(|&end| text[end] == b' ' && text[end - 1] != b' ')
        .map(|end| &text[..end])
        .find(|words| fits(words))
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

/// Lay out the chart across `rect`, the chart tile's box: each played step
/// takes width in proportion to its time, with a one-column gap before it.
/// A ramp is a stair per stage.
pub fn chart(player: &Player, profile: &TrainingProfile, rect: Rect) -> Chart {
    let (width, rows) = (rect.width(), rect.height());
    debug_assert!(width <= CHART_COLUMNS && rows < usize::from(EMPTY));
    let mut chart = Chart {
        rect,
        ..Chart::default()
    };
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
            let index = u16::from(chart.index(color));
            chart.columns[column] = (rows - height) as u16 | index << TOP_BITS;
        }
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
}

/// The per-pixel original of `paint_chart`.
#[cfg(test)]
pub(super) fn chart_pixel(chart: &Chart, x: usize, y: usize) -> u16 {
    let area = chart.rect;
    if !(area.left..area.right).contains(&x) {
        return BLACK;
    }
    let column = x - area.left;
    if (area.top..area.bottom).contains(&y)
        && chart.top(column) != EMPTY
        && (y - area.top) as u16 >= chart.top(column)
    {
        return chart.color(column);
    }
    BLACK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::{Choices, Laid, ScreenId};
    use crate::compose::registry::{STEP_LEFT, WORKOUT_CHART, WORKOUT_LINE};
    use crate::ride::log::{Sample, Status};
    use crate::screens::Layout;
    use crate::screens::canvas::Canvas;
    use crate::screens::chrome::{AMBER, GREEN, WHITE};
    use crate::screens::ride::RidePage;
    use crate::screens::workout::{Page, View};
    use crate::workout::library::{BUILT_IN, FTP_RAMP, FTP_TWENTY, VO2_MAX};
    use crate::workout::model::Intensity;
    use crate::workout::player::Player;

    /// The page's tiles on `layout`.
    fn laid(layout: &'static Layout, radar: bool) -> Laid {
        let flags = crate::screens::ride::main_flags(false, radar, true, false);
        Laid::new(
            ScreenId::RideWorkout,
            layout,
            Choices::default(),
            flags,
            None,
        )
        .unwrap()
    }

    /// The chart tile's box on the C606 without a radar.
    fn chart_box() -> Rect {
        laid(&crate::screens::BASE, false)
            .box_of(WORKOUT_CHART)
            .unwrap()
    }

    /// Show `player` on `view`'s workout page, as the app does.
    fn show(view: &mut View, player: &Player) {
        view.compose_main();
        let laid = view.laids.unwrap().get(ScreenId::RideWorkout).unwrap();
        let rect = laid.box_of(WORKOUT_CHART).unwrap();
        view.show_workout(chart(player, &view.profile, rect));
    }

    fn page(player: &Player, watts: Option<u16>) -> View<'static> {
        page_at(
            player,
            watts,
            crate::ride::zones::TrainingProfile::default().ftp_watts,
        )
    }

    fn page_at(player: &Player, watts: Option<u16>, ftp: u16) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        // A radar keeps the text as narrow as it gets on the C606.
        view.radar_paired = true;
        view.profile.ftp_watts = ftp;
        view.ride_page = RidePage::Workout;
        view.state = Status::Recording;
        view.sample.power_watts = watts;
        view.training = Some(Training::of(player, &view.profile));
        show(&mut view, player);
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

    const PAGE: Rect = crate::screens::BASE.scaled.screen();

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
        let chart = chart(
            &Player::new(workout),
            &TrainingProfile::default(),
            chart_box(),
        );
        assert!((0..CHART_COLUMNS).any(|c| chart.top(c) != EMPTY));
    }

    #[test]
    fn a_chart_of_very_long_distance_steps_still_lays_out() {
        use crate::workout::model::{Length, Step, Workout};
        let far = Step::new(Intensity::Work, Length::Distance(u32::MAX), Target::Free);
        let workout = Workout::new("FAR", &[far; 12], &[], None);
        assert_eq!(workout.validate(), Ok(()));
        let player = Player::new(workout);
        let chart = chart(&player, &TrainingProfile::default(), chart_box());
        assert!((0..CHART_COLUMNS).any(|c| chart.top(c) != EMPTY));
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
        let chart = chart(&player, &profile, chart_box());
        let ahead = mix(zone_color(profile.power_zone(147)), BLACK, 4);
        let to = (0..chart.rect.width())
            .position(|c| chart.top(c) != EMPTY && chart.color(c) == ahead)
            .expect("a dim stair ahead");
        let from = (0..to)
            .rev()
            .take_while(|&c| chart.top(c) == chart.top(to - 1))
            .last()
            .unwrap();
        assert!(to - from > 3, "a stair, not the whole ramp");
        assert!((from..to).all(|c| chart.top(c) == chart.top(from)));
        assert!(
            chart.top(from - 1) > chart.top(from),
            "the stair before is lower"
        );
        assert!(chart.top(to) < chart.top(from), "the next stair is higher");
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
        let area = chart.rect;
        let row = |y: usize| -> std::vec::Vec<u16> {
            (area.left..area.right)
                .map(|x| chart_pixel(chart, x, y))
                .collect()
        };
        let last = row(area.bottom - 1);
        assert_eq!(area.bottom, view.layout.scaled.panel.height);
        assert!(
            (0..area.width())
                .filter(|&c| chart.top(c) != EMPTY)
                .all(|c| last[c] != BLACK),
            "every step's bar reaches the screen's last row"
        );
        assert!(
            (area.top..area.bottom).all(|y| !row(y).contains(&WHITE)),
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
                    show(&mut view, &player);
                    view.prepare().check_layout();
                    let what = std::format!("{:?} radar {radar}", layout.scaled.panel);
                    let canvas = view.canvas();
                    canvas.assert_laid_out(layout.scaled.screen(), &["1999"], &what);
                    let chart = view.chart().rect.top;
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
                    show(&mut view, &player);
                    view.prepare().check_layout();
                    let title = laid(layout, true).box_of(WORKOUT_LINE).unwrap();
                    let shown = view
                        .canvas()
                        .runs
                        .iter()
                        .filter(|run| run.bottom() <= title.bottom + 2 && run.left == title.left)
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

    /// A tree may grow the chart over the whole panel: on every panel, every
    /// built-in workout fills that box and paints every row as its
    /// per-pixel reference does.
    #[test]
    fn the_chart_at_the_largest_box_accept_allows_paints_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            let rect = crate::screens::ride::largest_box(layout, WORKOUT_CHART);
            for (index, &workout) in BUILT_IN.iter().enumerate() {
                let mut player = Player::new(*workout);
                ride(&mut player, 30, 240);
                let chart = chart(&player, &TrainingProfile::default(), rect);
                let what = std::format!("workout {index} on {:?}", layout.scaled.panel);
                let tops: std::vec::Vec<u16> = (0..rect.width()).map(|c| chart.top(c)).collect();
                assert!(tops.iter().any(|&top| top != EMPTY), "{what}");
                assert!(
                    tops.iter()
                        .all(|&top| top == EMPTY || usize::from(top) < rect.height()),
                    "{what}"
                );
                for y in rect.top..rect.bottom {
                    let mut pixels = [BLACK; crate::screens::chrome::WIDTH];
                    paint_chart(&chart, &mut Row::new(y, &mut pixels));
                    for (x, &pixel) in pixels.iter().enumerate() {
                        assert_eq!(pixel, chart_pixel(&chart, x, y), "{what}: {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn painted_rows_match_the_reference() {
        let mut player = Player::new(VO2_MAX);
        player.skip();
        ride(&mut player, 30, 240);
        let view = page(&player, Some(240));
        let area = view.chart().rect;
        for y in area.top - 2..area.bottom {
            let mut pixels = [BLACK; crate::screens::chrome::WIDTH];
            paint_chart(view.chart(), &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, chart_pixel(view.chart(), x, y), "{x},{y}");
            }
        }
    }

    /// The balance rule: the rows the words leave over the chart are even
    /// gaps, above the first and below the last as well as between them.
    #[test]
    fn the_words_gaps_are_even_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            for radar in [false, true] {
                let laid = laid(layout, radar);
                let bar = crate::screens::ride::main_status_bar(&laid).unwrap();
                let chart = laid.box_of(WORKOUT_CHART).unwrap();
                let mut edges = std::vec![bar.bottom as i32];
                for placed in &laid.leaves()[1..] {
                    let rect = placed.rect;
                    if rect.top as usize >= chart.top || rect.top as usize == bar.bottom {
                        continue;
                    }
                    edges.extend([i32::from(rect.top), i32::from(rect.bottom)]);
                }
                edges.push(chart.top as i32);
                let gaps: std::vec::Vec<i32> =
                    edges.chunks(2).map(|pair| pair[1] - pair[0]).collect();
                let (least, most) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
                assert!(
                    most - least <= 1 && *least > 0,
                    "radar {radar} on {:?}: gaps {gaps:?}",
                    layout.scaled.panel
                );
            }
        }
    }

    /// A tap on the time left adds to the step; nothing else on the page
    /// acts.
    #[test]
    fn only_the_time_left_answers_a_tap() {
        let laid = laid(&crate::screens::BASE, false);
        let solved = laid.solved();
        let stops: std::vec::Vec<_> = solved
            .focus_order()
            .map(|placed| (placed.node, solved.action(placed)))
            .collect();
        let left = laid.box_of(STEP_LEFT).unwrap();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].1, Some(crate::compose::input::Action::ExtendStep));
        let (x, y) = (left.left as u16 + 4, left.top as u16 + 4);
        let hit = crate::compose::input::hit(laid.buttons(), &solved, x, y);
        assert_eq!(hit.map(|(_, action)| action), stops[0].1);
        assert_eq!(
            laid.buttons().held,
            Some(crate::compose::input::Action::ExtendStep)
        );
    }

    #[test]
    fn the_page_reads_the_steps_target() {
        let slots = crate::screens::ride::slots::Slots::default();
        assert_eq!(RidePage::Workout.readings(slots), None);
    }
}
