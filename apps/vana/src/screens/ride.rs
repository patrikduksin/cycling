//! The ride screen: pages that MOVE or a swipe steps through.
//!
//! The shared chrome draws the status bar (ride state, clock, page dots) and
//! toasts. This frame adds the radar strip on the left while a radar is
//! paired. A page owns [`Frame::body`] and lays out its text and shapes once per
//! frame; without a radar the frame centres the body, so pages never move
//! their own coordinates. Keep the least important readings at the bottom:
//! toasts cover `chrome::TOAST` for a moment.
//!
//!
//! To add a page: add a variant to `RidePage` and `RidePage::ALL`, then a
//! module beside `main` whose `prepare` returns its canvas, dispatched below.
//!
//! The climb page also follows the road: it comes up before a climb or once
//! one is under way, and when the road is clear it returns to the page the
//! rider was on, unless they changed page during the climb (see
//! `app::climb_page`).
use super::Layout;
use super::canvas::Canvas;
use super::chrome::{self, Align, Rect};
use super::icons::SignForm;
#[cfg(test)]
use super::scaled::BASE;
use super::scaled::{self, Scaled};
use super::workout::{Body, Graphics, Scene, Text, View};
use crate::ride::laps::Lap;
use crate::ride::zones::TrainingProfile;
use crate::sensors::radar::{self, close};
use core::fmt::Write;
use firmware_shell::shell::Parts;

mod climb;
pub mod elevation;
pub(super) mod end;
mod lap;
mod main;
mod map;
pub(super) mod paused;
pub use paused::READINGS as PAUSED_READINGS;
pub mod slots;
mod stats;
pub mod summary;
mod workout;
mod zones;

pub(super) use zones::{GAUGE_WIDTH, Mark, Marks, gauges, marks};

pub use climb::{Frame as ClimbFrame, Lent as ClimbLent};
pub use elevation::Chart as ElevationChart;
pub use lap::{LapBar, LapBars};
pub use map::{
    Cache as MapCache, Fetched, Filled as FilledMap, Frame as MapFrame, Track, Zoom as MapZoom,
};
pub use workout::{
    Chart as WorkoutChart, Regions as WorkoutRegions, Training, chart as workout_chart,
    regions as workout_regions,
};

use chrome::{ACCENT, AMBER, POWER, Row};
pub(super) use chrome::{RED, WHITE};
use firmware_shell::rendering::font::FaceRole;

pub(super) const GRID: u16 = chrome::TERTIARY;

/// The radar strip and its divider.
pub(crate) const RADAR_WIDTH: usize = 28;

/// The ride screen's frame on a panel: the radar strip, the body a page
/// draws into beside it, and where the strip's track and targets run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Frame {
    pub radar: Rect,
    pub body: Rect,
    pub strip: Strip,
    /// Without a radar the body moves left by this much, to the centre.
    pub centre_shift: usize,
    /// Text margins inside the body, as on the main page.
    pub text: Rect,
    pub cells: Cells,
}

/// A page's two columns of cells: a caption over its value, one band per
/// `pitch` rows. The next caption clears the value above it by 7 rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cells {
    /// Each column's left and right edge.
    pub columns: [(usize, usize); 2],
    /// The caption's and the value's baselines below the band's top.
    pub caption: usize,
    pub value: usize,
    pub pitch: usize,
}

pub const fn frame(at: &Scaled) -> Frame {
    use scaled::Track::{Fr, Unit};
    let chrome = chrome::regions(at);
    let content = chrome.content;
    let [radar, body] = content.cols([Unit(RADAR_WIDTH as u16), Fr(1)], Unit(0), at);
    let radius = at.unit(5);
    let [_, text, _] = body.cols([Unit(10), Fr(1), Unit(TEXT_RIGHT)], Unit(0), at);
    let middle = (text.left + text.right) / 2 + at.unit(4);
    Frame {
        radar,
        body,
        text,
        cells: Cells {
            columns: [
                (text.left, middle - at.unit(COLUMN_GAP)),
                (middle, text.right),
            ],
            caption: at.unit(12),
            value: at.unit(VALUE_BASELINE as usize),
            pitch: at.unit(CELL_PITCH),
        },
        strip: Strip {
            arrow_top: content.top + at.unit(8),
            track_top: content.top + at.unit(22),
            track_bottom: chrome.toast.top - 1 - radius,
            radius,
            centre: radar.left + at.unit(12),
            divider: at.unit(2),
        },
        centre_shift: radar.width() / 2,
    }
}

/// What a page draws into on the 240 x 320 canvas, for tests.
#[cfg(test)]
pub(super) const BODY: Rect = frame(&BASE).body;
/// The text's margin from the body's right edge.
pub(crate) const TEXT_RIGHT: u16 = 8;
/// A cell value's baseline below its band's top.
pub(super) const VALUE_BASELINE: u16 = 41;
/// Clear columns between a left cell's ink and the right column.
const COLUMN_GAP: usize = 8;
/// A caption over its value: one band of cells. The next caption clears the
/// value above it by 7 rows.
pub(super) const CELL_PITCH: usize = 48;
const _: () = assert!(
    CELL_PITCH + 12 - super::scaled::BASE.face(FaceRole::Caption).cap as usize
        > VALUE_BASELINE as usize + 6,
    "a caption needs 6 clear rows over the value above it"
);
/// Without a radar the body moves left by this much, to the centre.
#[cfg(test)]
pub(super) const CENTRE_SHIFT: usize = RADAR_WIDTH / 2;

/// The regions a page drawn from a canvas lays out in, named, in its
/// canvas's columns: the radar strip while a radar is paired, then the
/// page's own. A page lays out in the body, which the frame centres without
/// a radar. The map and the main page, which tile the panel themselves,
/// span the strip's columns without a radar. Beside one, the map's foot
/// still spans them, under the strip, which ends above it.
#[cfg(any(test, feature = "invariants"))]
pub(super) fn canvas_regions(
    scene: &Scene,
    page: RidePage,
) -> heapless::Vec<(&'static str, Rect), 3> {
    let frame = scene.frame();
    let foot = scene.map_regions().foot.rect;
    let regions = match (page, &scene.radar) {
        (RidePage::Map | RidePage::Main, None) => heapless::Vec::from_slice(&[(
            "page",
            Rect {
                left: frame.radar.left,
                ..frame.body
            },
        )]),
        (RidePage::Map, Some(_)) => heapless::Vec::from_slice(&[
            (
                "radar",
                Rect {
                    bottom: foot.top,
                    ..frame.radar
                },
            ),
            (
                "page",
                Rect {
                    bottom: foot.top,
                    ..frame.body
                },
            ),
            ("foot", foot),
        ]),
        (_, None) => heapless::Vec::from_slice(&[("page", frame.body)]),
        (_, Some(_)) => heapless::Vec::from_slice(&[("radar", frame.radar), ("page", frame.body)]),
    };
    regions.unwrap_or_default()
}

/// A ride page, in MOVE order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RidePage {
    /// Live speed, power, heart rate, cadence, distance, time and grade.
    #[default]
    Main,
    /// Ride totals: distance, moving time, averages, maxima, NP, work, ascent.
    Stats,
    /// The open lap against the last finished one.
    Lap,
    /// Grade, power and heart rate, and the climb near or under way on its
    /// profile, or the ride's elevation.
    Climb,
    /// The workout step on now; first, and only while a workout is chosen.
    Workout,
    /// The ride's own track as a breadcrumb, with the rider on it.
    Map,
}

impl RidePage {
    /// The pages of a free ride.
    pub const ALL: [Self; 5] = [Self::Main, Self::Stats, Self::Lap, Self::Climb, Self::Map];
    /// The pages while a workout is chosen or running.
    pub const TRAINING: [Self; 6] = [
        Self::Workout,
        Self::Main,
        Self::Stats,
        Self::Lap,
        Self::Climb,
        Self::Map,
    ];

    /// The page as a trace names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Main => "ride/main",
            Self::Stats => "ride/stats",
            Self::Lap => "ride/lap",
            Self::Climb => "ride/climb",
            Self::Workout => "ride/workout",
            Self::Map => "ride/map",
        }
    }

    /// The page named as a trace names it.
    pub fn named(name: &str) -> Option<Self> {
        Self::TRAINING.into_iter().find(|page| page.name() == name)
    }

    /// Which of the trace's shown readings ([`READINGS`]) the page draws
    /// live, the main page's lower line from `slots`; none for the workout
    /// page, which draws the step's target metric.
    pub fn readings(self, slots: slots::Slots) -> Option<heapless::Vec<&'static str, 7>> {
        let chosen = [slots.left, slots.right].map(crate::compose::data::Metric::shown);
        let drawn = |reading: &str| match self {
            Self::Main => {
                matches!(reading, "time" | "speed" | "power" | "hr" | "grade")
                    || chosen.contains(&Some(reading))
            }
            Self::Climb => matches!(reading, "power" | "hr" | "grade"),
            Self::Stats | Self::Map => reading == "dist",
            Self::Lap | Self::Workout => false,
        };
        (self != Self::Workout).then(|| READINGS.into_iter().filter(|r| drawn(r)).collect())
    }

    /// A number unique to the page, for touch ownership.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The bit that hides the page in a [`PageChoice`]; none for the pages
    /// that are always on.
    const fn hide_bit(self) -> Option<u8> {
        match self {
            Self::Main | Self::Workout => None,
            Self::Stats => Some(1 << 0),
            Self::Lap => Some(1 << 1),
            Self::Climb => Some(1 << 2),
            Self::Map => Some(1 << 3),
        }
    }
}

/// Which optional pages the rider turned off. Everything is on by default,
/// and the bits are stored as they are (see `app::preferences`): the main
/// page and the workout page have no bit and cannot be hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageChoice(u8);

impl PageChoice {
    /// The bits a record may carry.
    pub const BITS: u8 = 0b1111;

    /// The optional pages, in the order Settings lists them.
    pub const OPTIONAL: [RidePage; 4] = [
        RidePage::Stats,
        RidePage::Lap,
        RidePage::Climb,
        RidePage::Map,
    ];

    /// The choice a record's hidden bits stand for; none when it holds bits
    /// that mean nothing.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn shows(self, page: RidePage) -> bool {
        match page.hide_bit() {
            Some(bit) => self.0 & bit == 0,
            None => true,
        }
    }

    pub const fn showing(self, page: RidePage) -> Self {
        match page.hide_bit() {
            Some(bit) => Self(self.0 & !bit),
            None => self,
        }
    }

    /// Show or hide `page`; the always-on pages stay on.
    pub fn toggled(self, page: RidePage) -> Self {
        match page.hide_bit() {
            Some(bit) => Self(self.0 ^ bit),
            None => self,
        }
    }
}

/// The pages MOVE steps through, in order: the workout page first while a
/// workout is chosen, then every page the rider left on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pages {
    list: [RidePage; RidePage::TRAINING.len()],
    len: usize,
}

impl Pages {
    pub fn new(workout: bool, choice: PageChoice) -> Self {
        let mut list = [RidePage::Main; RidePage::TRAINING.len()];
        let mut len = 0;
        for page in RidePage::TRAINING {
            if (workout || page != RidePage::Workout) && choice.shows(page) {
                list[len] = page;
                len += 1;
            }
        }
        Self { list, len }
    }

    pub fn as_slice(&self) -> &[RidePage] {
        &self.list[..self.len]
    }

    pub fn count(&self) -> usize {
        self.len
    }

    pub fn first(&self) -> RidePage {
        self.list[0]
    }

    pub fn last(&self) -> RidePage {
        self.list[self.len - 1]
    }

    pub fn contains(&self, page: RidePage) -> bool {
        self.as_slice().contains(&page)
    }

    /// Where `page` sits, for the page dots; a page that is not there
    /// counts as the first.
    pub fn position(&self, page: RidePage) -> usize {
        self.as_slice()
            .iter()
            .position(|shown| *shown == page)
            .unwrap_or(0)
    }

    /// The page after `page`, wrapping.
    pub fn following(&self, page: RidePage) -> RidePage {
        self.list[(self.position(page) + 1) % self.len]
    }

    /// The page before `page`, wrapping.
    pub fn preceding(&self, page: RidePage) -> RidePage {
        self.list[(self.position(page) + self.len - 1) % self.len]
    }

    /// The page after `page`; none after the last.
    pub fn next(&self, page: RidePage) -> Option<RidePage> {
        self.as_slice().get(self.position(page) + 1).copied()
    }
}

/// Prepare the shown page once per frame: its canvas, and the climb page's
/// chart.
pub(super) fn prepare(view: &mut View) {
    match view.ride_page {
        RidePage::Main => super::workout::fill(view, |_| Canvas::default()),
        RidePage::Stats => super::workout::fill(view, stats::prepare),
        RidePage::Lap => super::workout::fill(view, lap::prepare),
        RidePage::Climb => {
            climb_frame(view);
            super::workout::fill(view, climb::prepare);
        }
        // The app lays out the chart from its player: the workout stays there.
        RidePage::Workout => super::workout::fill(view, workout::prepare),
        RidePage::Map => super::workout::fill(view, map::prepare),
    }
    hide_banner(view);
}

/// Hide what the course banner partly covers on the ride page shown.
pub(super) fn hide_banner(view: &mut View) {
    let layout = view.layout;
    let banner = super::sign::on_page(view.ride_page, view.nav, view.radar_paired, layout, None)
        .filter(|sign| sign.form() == SignForm::Banner);
    if let (Some(banner), Body::Canvas(canvas)) = (banner, &mut view.body) {
        canvas.hide(banner.rect().top..banner.rect().bottom);
    }
}

/// The climb chart, prepared in its own frame (#167).
#[inline(never)]
fn climb_frame(view: &mut View) {
    let climb = climb::frame(view);
    view.set_climb_frame(climb);
}

/// A caption over its value in `column` (0 or 1) of the band whose top is
/// `top`. The value steps down to the body face before it would leave its
/// column.
pub(super) fn cell(
    canvas: &mut Canvas,
    layout: &Layout,
    column: usize,
    top: usize,
    caption: &[u8],
    value: &[u8],
    color: u16,
) {
    let (at, cells) = (&layout.scaled, &layout.ride.cells);
    let (left, right) = cells.columns[column];
    let width = right - left;
    canvas.caption(at, caption, width, left, top + cells.caption, Align::Left);
    canvas.fit(
        &[at.face(FaceRole::Value), at.face(FaceRole::Body)],
        value,
        width,
        left,
        top + cells.value,
        Align::Left,
        color,
    );
}

/// Paint a content row over the black the scene laid: the radar strip
/// while a radar is paired, and the page in the body, centred without a radar.
pub(super) fn paint(scene: &Scene, page: RidePage, row: &mut Row) {
    let frame = scene.frame();
    let (strip, body) = (
        frame.radar.left..frame.radar.right - frame.strip.divider,
        frame.body,
    );
    if page == RidePage::Map {
        if let Some(radar) = &scene.radar {
            paint_radar(radar, &mut row.within(strip));
        }
        paint_map(scene, row);
        return;
    }
    if page == RidePage::Main {
        if let Some(radar) = &scene.radar {
            paint_radar(radar, &mut row.within(strip));
        }
        scene.canvas().paint(row);
        return;
    }
    if let Some(radar) = &scene.radar {
        paint_radar(radar, &mut row.within(strip));
        paint_page(scene, page, &mut row.within(body.left..body.right));
        return;
    }
    paint_page(
        scene,
        page,
        &mut row
            .shifted(frame.centre_shift)
            .within(body.left..body.right),
    );
}

pub(super) fn parts(scene: &Scene, page: RidePage, parts: &mut Parts) {
    if let Some(radar) = &scene.radar {
        let strip = &radar.strip;
        parts.add(strip.track_top..strip.track_bottom + 1, &radar.track);
        for marker in radar.markers.iter().flatten() {
            let y = usize::from(marker.0);
            parts.add(y - strip.radius..y + strip.radius + 1, marker);
        }
    }
    match page {
        RidePage::Main | RidePage::Stats | RidePage::Lap => {}
        RidePage::Workout => workout::chart_parts(scene.chart(), &scene.layout().workout, parts),
        RidePage::Climb => climb::chart_parts(scene.climb(), &scene.layout().climb, parts),
        RidePage::Map => {
            let drawn = scene.map_regions().free.drawn;
            parts.add(drawn.top..drawn.bottom, &scene.map());
        }
    }
    scene.canvas().parts(parts);
}

fn paint_page(scene: &Scene, page: RidePage, row: &mut Row) {
    match page {
        RidePage::Main | RidePage::Stats | RidePage::Lap => scene.canvas().paint(row),
        RidePage::Workout => {
            workout::paint_chart(scene.chart(), &scene.layout().workout, row);
            scene.canvas().paint(row);
        }
        RidePage::Climb => {
            climb::paint_chart(scene.climb(), &scene.layout().climb, row);
            scene.canvas().paint(row);
        }
        RidePage::Map => paint_map(scene, row),
    }
}

fn paint_map(scene: &Scene, row: &mut Row) {
    if let Some(map) = scene.map() {
        let shown = scene.map_regions().ground(scene.radar.is_some()).shown;
        map::paint(map.frame(), &mut row.within(shown.left..shown.right));
    }
    scene.canvas().paint(row);
}

/// Where the radar strip draws: the arrow, then the track from the rider at
/// the top to 200 m at the bottom. The track ends above the toast band, so a
/// toast never hides a target, a disc `radius` rows above and below its
/// range, centred on column `centre`. `divider` columns part it from the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Strip {
    arrow_top: usize,
    track_top: usize,
    track_bottom: usize,
    radius: usize,
    centre: usize,
    divider: usize,
}

const RANGE_MM: u32 = 200_000;

/// Where the gliding graphics land: each gliding tile's lit run, and each
/// radar marker's row and whether it is red. A glide needs a frame only
/// when this changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pixels {
    glides: [Option<crate::compose::Rect16>; main::GLIDES],
    markers: [Option<(u16, bool)>; 8],
}

/// The glides of `main`, when the main page shows, and of the radar strip.
pub fn pixels(
    graphics: &Graphics,
    profile: &TrainingProfile,
    main: Option<&crate::compose::builtin::Laid>,
    layout: &'static Layout,
) -> Pixels {
    let gliding = super::data::Gliding { graphics, profile };
    Pixels {
        glides: main.map_or([None; main::GLIDES], |main| main.glides(&gliding)),
        markers: radar(&graphics.radar, &layout.ride.strip).markers,
    }
}

pub use climb::{Regions as ClimbRegions, regions as climb_regions};
pub use end::{Regions as EndRegions, regions as end_regions};
pub use lap::{Regions as LapRegions, regions as lap_regions};
pub use main::{
    chip as main_chip, flags as main_flags, flags_of as main_flags_of, laid as main_laid,
    status_bar as main_status_bar,
};
pub use map::{Regions as MapRegions, regions as map_regions};
pub use paused::{Regions as PausedRegions, regions as paused_regions};
pub use stats::{Regions as StatsRegions, regions as stats_regions};

#[derive(Clone, Copy, Debug, Hash)]
pub struct Radar {
    track: u16,
    markers: [Option<(u16, bool)>; 8],
    strip: &'static Strip,
}

pub(super) fn radar(snapshot: &radar::Snapshot, strip: &'static Strip) -> Radar {
    Radar {
        track: track_color(snapshot.status),
        markers: snapshot
            .targets
            .map(|target| target.map(|target| (target_y(target, strip), close(target)))),
        strip,
    }
}

fn target_y(target: radar::Target, strip: &Strip) -> u16 {
    let span = (strip.track_bottom - strip.track_top) as u32;
    (strip.track_top as u32 + target.range_mm.min(RANGE_MM) * span / RANGE_MM) as u16
}

fn track_color(status: radar::Status) -> u16 {
    match status {
        radar::Status::Ready => ACCENT,
        radar::Status::Error | radar::Status::ReservedThreat => RED,
        _ => AMBER,
    }
}

/// The radar strip: the arrow, the track, then targets, the first on top.
fn paint_radar(radar: &Radar, row: &mut Row) {
    let y = row.y();
    let strip = &radar.strip;
    let centre = strip.centre;
    if (strip.arrow_top..strip.arrow_top + 11).contains(&y) {
        let half = (y - strip.arrow_top) / 2;
        row.span(centre - half..centre + 1 + half, WHITE);
    }
    if (strip.track_top..=strip.track_bottom).contains(&y) {
        row.span(centre - 1..centre + 2, radar.track);
    }
    let radius = strip.radius as i32;
    for &(target_y, close) in radar.markers.iter().rev().flatten() {
        let dy = y as i32 - i32::from(target_y);
        if dy * dy > radius * radius {
            continue;
        }
        let mut half = 0;
        while (half + 1) * (half + 1) + dy * dy <= radius * radius {
            half += 1;
        }
        let color = if close { RED } else { POWER };
        row.span(centre - half as usize..centre + 1 + half as usize, color);
    }
}

/// The per-pixel original of `paint`, kept as the reference it must match.
#[cfg(test)]
pub(super) fn reference_pixel(scene: &Scene, page: RidePage, x: usize, y: usize) -> chrome::Shown {
    let look = scene.look();
    let black = look.ink(chrome::BLACK);
    if page == RidePage::Map {
        let frame = scene.frame();
        return match &scene.radar {
            Some(radar) if x < frame.radar.right - frame.strip.divider => {
                let under = look.ink(reference_radar_pixel(radar, x, y));
                scene.canvas().pixel(look, x, y, under)
            }
            Some(_) if x < frame.body.left => scene.canvas().pixel(look, x, y, black),
            _ => reference_map_pixel(scene, x, y),
        };
    }
    if page == RidePage::Main {
        if let Some(radar) = &scene.radar
            && x < RADAR_WIDTH - 2
        {
            return look.ink(reference_radar_pixel(radar, x, y));
        }
        return scene.canvas().pixel(look, x, y, black);
    }
    let x = if let Some(radar) = &scene.radar {
        if x < RADAR_WIDTH - 2 {
            return look.ink(reference_radar_pixel(radar, x, y));
        }
        x
    } else {
        x + CENTRE_SHIFT
    };
    if !(BODY.left..BODY.right).contains(&x) {
        return black;
    }
    match page {
        RidePage::Main => unreachable!("the main page is painted unshifted, above"),
        RidePage::Stats | RidePage::Lap => scene.canvas().pixel(look, x, y, black),
        RidePage::Workout => {
            let chart = workout::chart_pixel(scene.chart(), &scene.layout().workout, x, y);
            let under = look.ink(chart);
            scene.canvas().pixel(look, x, y, under)
        }
        RidePage::Climb => {
            let under = look.ink(climb::chart_pixel(
                scene.climb(),
                &scene.layout().climb,
                x,
                y,
            ));
            scene.canvas().pixel(look, x, y, under)
        }
        RidePage::Map => reference_map_pixel(scene, x, y),
    }
}

#[cfg(test)]
fn reference_map_pixel(scene: &Scene, x: usize, y: usize) -> chrome::Shown {
    let look = scene.look();
    let black = look.ink(chrome::BLACK);
    let under = scene
        .map()
        .map_or(black, |map| map::pixel(map.frame(), look, x, y, black));
    scene.canvas().pixel(look, x, y, under)
}

#[cfg(test)]
fn reference_radar_pixel(radar: &Radar, x: usize, y: usize) -> u16 {
    let strip = &radar.strip;
    let (centre, radius) = (strip.centre as i32, strip.radius as i32);
    for &(target_y, close) in radar.markers.iter().flatten() {
        let dx = x as i32 - centre;
        let dy = y as i32 - i32::from(target_y);
        if dx * dx + dy * dy <= radius * radius {
            return if close { RED } else { POWER };
        }
    }
    if (strip.centre - 1..=strip.centre + 1).contains(&x)
        && (strip.track_top..=strip.track_bottom).contains(&y)
    {
        return radar.track;
    }
    let top = strip.arrow_top;
    if (top..top + 11).contains(&y) && (x as i32 - centre).abs() <= (y - top) as i32 / 2 {
        return WHITE;
    }
    chrome::BLACK
}

/// A finished lap's toast: number, time and average power.
pub fn lap_toast(lap: Lap) -> Text<19> {
    let mut toast = Text::new();
    let mut time = Text::<12>::new();
    lap_time(&mut time, lap.time_ms / 1000);
    let time = time.as_str();
    let written = match lap.avg_power() {
        Some(watts) => write!(toast, "LAP {} {time} {watts}W", lap.number),
        None => write!(toast, "LAP {} {time} --W", lap.number),
    };
    // Past lap 99 and an hour the power no longer fits.
    if written.is_err() {
        toast = Text::new();
        let _ = write!(toast, "LAP {} {time}", lap.number);
    }
    toast
}

/// km/h with one truncated decimal, as the live reading always showed, or "--".
pub(crate) fn speed(value: &mut Text<12>, mm_s: Option<u32>) {
    let _ = match mm_s {
        Some(v) => {
            let k = u64::from(v) * 36 / 1000;
            write!(value, "{}.{}", k / 10, k % 10)
        }
        None => value.push_str("--"),
    };
}

/// H:MM:SS, the way a workout clock reads.
pub(crate) fn elapsed(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = write!(value, "{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
}

/// Kilometres with two rounded decimals, one from 100 km.
pub(crate) fn kilometres(value: &mut Text<12>, mm: u64) {
    let hundredths = (mm + 5_000) / 10_000;
    let _ = if hundredths < 10_000 {
        write!(value, "{}.{:02}", hundredths / 100, hundredths % 100)
    } else {
        write!(value, "{}.{}", hundredths / 100, hundredths % 100 / 10)
    };
}

/// Lap time: MM:SS, or H:MM:SS from an hour.
pub(super) fn lap_time(value: &mut Text<12>, seconds: u64) {
    let t = seconds;
    let _ = if t < 3600 {
        write!(value, "{:02}:{:02}", t / 60, t % 60)
    } else {
        write!(value, "{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    };
}

/// A grade in percent with one decimal, or "--%".
pub(crate) fn grade(value: &mut Text<12>, tenths: Option<i32>) {
    let _ = match tenths {
        Some(v) => write!(
            value,
            "{}{}.{}%",
            if v < 0 { "-" } else { "" },
            v.unsigned_abs() / 10,
            v.unsigned_abs() % 10
        ),
        None => value.push_str("--%"),
    };
}

/// The live readings a trace's shown line names, in its order.
pub const READINGS: [&str; 7] = ["time", "speed", "power", "hr", "cad", "dist", "grade"];

pub(crate) fn shown(
    out: &mut impl Write,
    active_secs: u64,
    sample: &crate::ride::log::Sample,
    distance_mm: u64,
) -> core::fmt::Result {
    let mut values: [Text<12>; 7] = Default::default();
    elapsed(&mut values[0], active_secs);
    speed(&mut values[1], sample.speed_mm_s);
    whole(&mut values[2], sample.power_watts);
    whole(&mut values[3], sample.heart_bpm);
    whole(
        &mut values[4],
        sample.cadence_tenths.map(|tenths| (tenths + 5) / 10),
    );
    kilometres(&mut values[5], distance_mm);
    grade(&mut values[6], sample.gradient_tenths.map(i32::from));
    for (i, (name, value)) in READINGS.iter().zip(&values).enumerate() {
        let gap = if i == 0 { "" } else { " " };
        write!(out, "{gap}{name}={}", value.as_str())?;
    }
    Ok(())
}

/// A whole reading, or "--" without one.
pub(crate) fn whole(value: &mut Text<12>, reading: Option<impl core::fmt::Display>) {
    let _ = match reading {
        Some(v) => write!(value, "{v}"),
        None => value.push_str("--"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The readings `page` draws, found by drawing it with every reading
    /// at a value nothing else on the page takes.
    fn drawn(page: Option<RidePage>, slots: slots::Slots) -> std::vec::Vec<&'static str> {
        use crate::ride::log::Sample;
        use crate::screens::workout::Page;
        use std::boxed::Box;
        let sample = Sample {
            speed_mm_s: Some(10_389),
            power_watts: Some(287),
            heart_bpm: Some(153),
            cadence_tenths: Some(914),
            gradient_tenths: Some(63),
            ..Sample::default()
        };
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = page.unwrap_or_default();
        view.main_slots = slots;
        view.radar_paired = true;
        view.active_secs = 5_025;
        view.totals.add(1000, 12_345_000, &Sample::default(), None);
        view.sample = sample;
        view.power_3s = sample.power_watts;
        let crumbs = Box::leak(Box::new([
            crate::ride::breadcrumb::Crumb {
                east: 0.0,
                north: 0.0,
                along: 0.0,
            },
            crate::ride::breadcrumb::Crumb {
                east: 400.0,
                north: 300.0,
                along: 500.0,
            },
        ]));
        let track = Track {
            crumbs,
            here: Some((400.0, 300.0)),
            ..Track::default()
        };
        let frame = Box::leak(Box::default());
        let mut map = MapCache::new(frame);
        map.update(&track, &crate::screens::BASE.map);
        match page {
            Some(RidePage::Lap) => view.show_laps(heapless::Vec::new(), Lap::default(), None),
            Some(RidePage::Climb) => view.show_climb(ClimbLent {
                climbs: Box::leak(Box::default()),
                climbing: None,
                trace: Box::leak(Box::default()),
                forecast: &crate::ride::climb_ahead::Forecast::NONE,
                pinned: None,
            }),
            Some(RidePage::Map) => view.show_map(map.filled()),
            _ => {}
        }
        let texts: std::vec::Vec<std::string::String> = match page {
            None => paused::prepare(&view)
                .runs
                .iter()
                .map(|run| std::string::String::from_utf8_lossy(run.line.text()).into())
                .collect(),
            Some(page) => {
                if page == RidePage::Main {
                    view.compose(slots);
                }
                view.prepare();
                view.canvas()
                    .runs
                    .iter()
                    .map(|run| std::string::String::from_utf8_lossy(run.line.text()).into())
                    .collect()
            }
        };
        let mut line = std::string::String::new();
        shown(
            &mut line,
            view.active_secs,
            &sample,
            view.totals.distance_mm(),
        )
        .unwrap();
        READINGS
            .into_iter()
            .zip(line.split(' '))
            .filter(|(_, pair)| {
                let value = pair.split_once('=').unwrap().1;
                texts
                    .iter()
                    .any(|text| text.split(' ').any(|word| word == value))
            })
            .map(|(reading, _)| reading)
            .collect()
    }

    #[test]
    fn each_page_declares_the_readings_it_draws() {
        let chosen = slots::Slots {
            left: crate::compose::data::Metric::AvgPower,
            right: crate::compose::data::Metric::Distance,
        };
        for slots in [slots::Slots::default(), chosen] {
            for page in RidePage::ALL {
                assert_eq!(
                    page.readings(slots).unwrap().to_vec(),
                    drawn(Some(page), slots),
                    "{page:?} with {slots:?}"
                );
            }
        }
        assert_eq!(paused::READINGS.to_vec(), drawn(None, chosen), "paused");
    }

    fn choice(hidden: &[RidePage]) -> PageChoice {
        hidden
            .iter()
            .fold(PageChoice::default(), |choice, page| choice.toggled(*page))
    }

    #[test]
    fn pages_step_in_order_and_wrap_both_ways() {
        let free = Pages::new(false, PageChoice::default());
        assert_eq!(free.as_slice(), RidePage::ALL);
        assert_eq!(RidePage::default(), RidePage::Main);
        assert_eq!(free.next(RidePage::Main), Some(RidePage::Stats));
        assert_eq!(free.next(RidePage::Stats), Some(RidePage::Lap));
        assert_eq!(free.next(RidePage::Lap), Some(RidePage::Climb));
        assert_eq!(free.next(RidePage::Climb), Some(RidePage::Map));
        assert_eq!(free.next(RidePage::Map), None);
        assert_eq!(free.following(RidePage::Map), RidePage::Main);
        assert_eq!(free.preceding(RidePage::Main), RidePage::Map);
        assert_eq!(free.preceding(RidePage::Stats), RidePage::Main);
        let training = Pages::new(true, PageChoice::default());
        assert_eq!(training.as_slice(), RidePage::TRAINING);
        assert_eq!(training.following(RidePage::Workout), RidePage::Main);
        assert_eq!(training.following(RidePage::Climb), RidePage::Map);
        assert_eq!(training.following(RidePage::Map), RidePage::Workout);
        assert_eq!(training.preceding(RidePage::Workout), RidePage::Map);
        assert_eq!(training.position(RidePage::Main), 1);
        assert_eq!(free.position(RidePage::Workout), 0);
    }

    #[test]
    fn every_subset_of_optional_pages_makes_a_walkable_list() {
        for bits in 0..1u8 << PageChoice::OPTIONAL.len() {
            let hidden: std::vec::Vec<RidePage> = PageChoice::OPTIONAL
                .into_iter()
                .enumerate()
                .filter(|(i, _)| bits >> i & 1 == 1)
                .map(|(_, page)| page)
                .collect();
            for workout in [false, true] {
                let pages = Pages::new(workout, choice(&hidden));
                let list = pages.as_slice();
                assert!(list.contains(&RidePage::Main), "{hidden:?}");
                assert_eq!(list.contains(&RidePage::Workout), workout);
                assert_eq!(list.len(), pages.count());
                assert!(hidden.iter().all(|page| !pages.contains(*page)));
                assert_eq!(
                    list.len(),
                    1 + usize::from(workout) + PageChoice::OPTIONAL.len() - hidden.len()
                );
                let mut page = pages.first();
                for (i, shown) in list.iter().enumerate() {
                    assert_eq!(page, *shown);
                    assert_eq!(pages.position(page), i);
                    assert_eq!(pages.next(page), list.get(i + 1).copied());
                    assert_eq!(pages.preceding(pages.following(page)), page);
                    page = pages.following(page);
                }
                assert_eq!(page, pages.first(), "a full lap returns");
                assert_eq!(pages.following(pages.last()), pages.first());
                assert_eq!(pages.preceding(pages.first()), pages.last());
            }
        }
    }

    #[test]
    fn with_everything_hidden_only_main_remains() {
        let all_off = choice(&PageChoice::OPTIONAL);
        let pages = Pages::new(false, all_off);
        assert_eq!(pages.as_slice(), [RidePage::Main]);
        assert_eq!(pages.following(RidePage::Main), RidePage::Main);
        assert_eq!(pages.preceding(RidePage::Main), RidePage::Main);
        assert_eq!(pages.next(RidePage::Main), None);
        assert_eq!(
            Pages::new(true, all_off).as_slice(),
            [RidePage::Workout, RidePage::Main]
        );
    }

    #[test]
    fn main_and_workout_cannot_be_hidden_and_toggling_twice_restores() {
        let default = PageChoice::default();
        assert_eq!(default.bits(), 0);
        for page in [RidePage::Main, RidePage::Workout] {
            assert_eq!(default.toggled(page), default);
            assert!(default.shows(page));
        }
        for page in PageChoice::OPTIONAL {
            let off = default.toggled(page);
            assert!(!off.shows(page));
            assert_eq!(off.bits().count_ones(), 1);
            assert_eq!(off.toggled(page), default);
        }
        assert_eq!(PageChoice::from_bits(0b1_1111), None);
        assert_eq!(PageChoice::from_bits(0b10_0000), None);
    }

    /// Radar alerts are safety-critical: even the farthest target is drawn
    /// whole above the band a toast may cover.
    #[test]
    fn radar_targets_stay_clear_of_toasts() {
        for range_mm in [0, 190_000, 200_000, 400_000] {
            let target = radar::Target {
                threat: radar::Threat::Approach,
                side: radar::Side::Behind,
                range_mm,
                closing_speed_cm_s: 0,
            };
            let strip = &crate::screens::BASE.ride.strip;
            let bottom = usize::from(target_y(target, strip)) + strip.radius;
            assert!(bottom < chrome::TOAST.top, "{range_mm} mm");
            let mut snapshot = radar::Snapshot::default();
            snapshot.targets[0] = Some(target);
            for y in chrome::TOAST.top..chrome::HEIGHT {
                for x in 0..RADAR_WIDTH {
                    assert_eq!(
                        reference_radar_pixel(&radar(&snapshot, strip), x, y),
                        chrome::BLACK,
                        "({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn formats_distance_speed_and_readings() {
        let format = |write: &dyn Fn(&mut Text<12>)| {
            let mut value = Text::new();
            write(&mut value);
            value.as_bytes().to_vec()
        };
        assert_eq!(format(&|v| kilometres(v, 4_999_800)), b"5.00");
        assert_eq!(format(&|v| kilometres(v, 0)), b"0.00");
        assert_eq!(format(&|v| kilometres(v, 99_994_999)), b"99.99");
        assert_eq!(format(&|v| kilometres(v, 123_456_000)), b"123.4");
        assert_eq!(format(&|v| speed(v, Some(8334))), b"30.0");
        assert_eq!(format(&|v| speed(v, Some(8333))), b"29.9");
        assert_eq!(format(&|v| speed(v, None)), b"--");
        assert_eq!(format(&|v| elapsed(v, 3725)), b"1:02:05");
        assert_eq!(format(&|v| lap_time(v, 312)), b"05:12");
        assert_eq!(format(&|v| lap_time(v, 3725)), b"1:02:05");
        assert_eq!(format(&|v| whole(v, Some(42))), b"42");
        assert_eq!(format(&|v| whole(v, None::<u16>)), b"--");
        assert_eq!(format(&|v| grade(v, Some(-35))), b"-3.5%");
        assert_eq!(format(&|v| grade(v, Some(-5))), b"-0.5%");
        assert_eq!(format(&|v| grade(v, Some(120))), b"12.0%");
        assert_eq!(format(&|v| grade(v, None)), b"--%");
    }
}
