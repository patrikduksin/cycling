//! The ride screen: pages that MOVE or a swipe steps through.
//!
//! The shared chrome draws the status bar (ride state, clock, page dots) and
//! toasts. While a radar is paired its strip paints the [`radar_columns`]
//! every ride tree keeps clear. Keep the least important readings at the
//! bottom: toasts cover `chrome::toast_band` for a moment.
//!
//! To add a page: add a variant to `RidePage` and `RidePage::ALL` and its
//! tree in `compose::builtin`.
//!
//! The climb page also follows the road: it comes up before a climb or once
//! one is under way, and when the road is clear it returns to the page the
//! rider was on, unless they changed page during the climb (see
//! `app::climb_page`).
use super::Device;
use super::chrome::{self, Rect};
use super::icons::SignForm;
use super::scaled::Scaled;
use super::workout::{Body, Graphics, Scene, Text, View};
use crate::compose::builtin::ScreenId;
use crate::ride::laps::Lap;
use crate::ride::zones::TrainingProfile;
use crate::sensors::radar::{self, close};
use core::fmt::Write;
use firmware_shell::shell::Parts;

pub mod climb;
pub mod elevation;
mod lap;
mod main;
pub mod map;
pub mod slots;
mod stats;
pub mod summary;
pub mod workout;
mod zones;

pub(super) use zones::{GAUGE_WIDTH, Mark, Marks, gauges, marks};

pub use climb::{Frame as ClimbFrame, Lent as ClimbLent, Road as ClimbRoad};
pub use elevation::Chart as ElevationChart;
pub use lap::{LapBar, LapBars};
pub use map::{
    Cache as MapCache, Fetched, Filled as FilledMap, Frame as MapFrame, Track, Zoom as MapZoom,
};
pub use workout::{Chart as WorkoutChart, Training, chart as workout_chart};

use chrome::{ACCENT, AMBER, POWER, Row};
pub(super) use chrome::{RED, WHITE};

pub(super) const GRID: u16 = chrome::TERTIARY;

/// The radar strip and its divider.
pub(crate) const RADAR_WIDTH: usize = 28;

/// The columns the radar strip paints under the status bar while a radar
/// is paired: the first `RADAR_WIDTH` units, which every ride tree keeps
/// clear then, the strip's divider in their last columns.
pub const fn radar_columns(at: &Scaled) -> Rect {
    Rect {
        right: at.unit(RADAR_WIDTH),
        ..chrome::content(at)
    }
}

/// Where the radar strip's arrow, track and targets run on `at`.
pub const fn strip(at: &Scaled) -> Strip {
    let (content, radar) = (chrome::content(at), radar_columns(at));
    let radius = at.unit(5);
    Strip {
        arrow_top: content.top + at.unit(8),
        track_top: content.top + at.unit(22),
        track_bottom: chrome::toast_band(at).top - 1 - radius,
        radius,
        centre: radar.left + at.unit(12),
        left: radar.left,
        right: radar.right - at.unit(2),
        body: radar.right,
    }
}

/// The text's margins from the body's left and right edges.
pub(crate) const TEXT_LEFT: u16 = 10;
pub(crate) const TEXT_RIGHT: u16 = 8;

/// The regions a page drawn from a canvas lays out in, named, in its
/// canvas's columns: the radar strip while a radar is paired, then the
/// page's own. Without a radar a page lays out across the whole content.
/// The map and the main page, which tile the panel themselves, span the
/// strip's columns without a radar. Beside one, the map's foot
/// still spans them, under the strip, which ends above it.
#[cfg(any(test, feature = "invariants"))]
pub(super) fn canvas_regions(
    scene: &Scene,
    page: RidePage,
) -> heapless::Vec<(&'static str, Rect), 3> {
    let at = &scene.device().scaled;
    let radar = radar_columns(at);
    let body = Rect {
        left: radar.right,
        ..chrome::content(at)
    };
    let foot = Rect {
        top: map::ground_rect(at).bottom,
        ..at.screen()
    };
    let regions = match (page, &scene.radar) {
        (_, None) => heapless::Vec::from_slice(&[(
            "page",
            Rect {
                left: radar.left,
                ..body
            },
        )]),
        (RidePage::Map, Some(_)) => heapless::Vec::from_slice(&[
            (
                "radar",
                Rect {
                    bottom: foot.top,
                    ..radar
                },
            ),
            (
                "page",
                Rect {
                    bottom: foot.top,
                    ..body
                },
            ),
            ("foot", foot),
        ]),
        (_, Some(_)) => heapless::Vec::from_slice(&[("radar", radar), ("page", body)]),
    };
    regions.unwrap_or_default()
}

/// A ride page, in MOVE order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RidePage {
    /// Stats: power and heart rate in their zones, speed, cadence, grade,
    /// the ride's climbing, distance and NP.
    #[default]
    Main,
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
    pub const ALL: [Self; 4] = [Self::Main, Self::Lap, Self::Climb, Self::Map];
    /// The pages while a workout is chosen or running.
    pub const TRAINING: [Self; 5] = [Self::Workout, Self::Main, Self::Lap, Self::Climb, Self::Map];

    /// The page as a trace names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Main => "ride/main",
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
    /// live; none for the workout page, which draws the step's target
    /// metric.
    pub fn readings(self) -> Option<heapless::Vec<&'static str, 7>> {
        let id = match self {
            Self::Main => ScreenId::RideMain,
            Self::Lap => ScreenId::RideLap,
            Self::Climb => ScreenId::RideClimb,
            Self::Workout => ScreenId::RideWorkout,
            Self::Map => return Some(READINGS.into_iter().filter(|&r| r == "dist").collect()),
        };
        tree_readings(id)
    }

    /// A number unique to the page, for touch ownership.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// The ride pages MOVE steps through, in order. The rider's deck holds the
/// main page, first and always, then the optional pages left on in the
/// rider's order; a record stores it without the workout bit (see
/// `app::preferences`). The deck
/// shown puts the workout page first while a workout is chosen. Two bytes,
/// as `View` holds one every frame: three bits a slot, each an optional
/// page's index or 0 for none, and the top bit for the workout page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deck(u16);

impl Default for Deck {
    fn default() -> Self {
        Self::OPTIONAL
            .into_iter()
            .fold(Self::MAIN, |deck, page| deck.pushed(page))
    }
}

/// The shown readings the tiles of `id`'s built-in read; none when a tile
/// reads the workout step's target, whose metric the step picks.
fn tree_readings(id: ScreenId) -> Option<heapless::Vec<&'static str, 7>> {
    use crate::compose::data::Reads;
    use crate::compose::node::What;
    let builtin = id.builtin();
    let nodes = (builtin.tree)(&crate::compose::builtin::Choices::default());
    let mut read = heapless::Vec::<&str, 7>::new();
    for node in &nodes {
        let Some(What::Leaf { component, arg }) = node.what() else {
            continue;
        };
        match crate::compose::registry::tile(component).reads(arg) {
            Some(Reads::Target) => return None,
            Some(Reads::Metric(metric)) => {
                if let Some(name) = metric.shown().filter(|name| !read.contains(name)) {
                    let _ = read.push(name);
                }
            }
            None => {}
        }
    }
    Some(READINGS.into_iter().filter(|r| read.contains(r)).collect())
}

impl Deck {
    /// The optional pages in Settings' order; a page turned back on returns
    /// to its place in it.
    pub const OPTIONAL: [RidePage; 3] = [RidePage::Lap, RidePage::Climb, RidePage::Map];
    /// Bytes a record stores: each optional page shown by its index, in
    /// order, then [`Self::END`].
    pub const LENGTH: usize = Self::OPTIONAL.len();
    const END: u8 = 0xff;
    const SLOT_BITS: u32 = 3;
    const WORKOUT: u16 = 1 << 15;
    const MAIN: Self = Self(0);

    fn optional(self) -> impl Iterator<Item = RidePage> + Clone {
        (0..Self::LENGTH as u32)
            .map(move |slot| usize::from(self.0 >> (slot * Self::SLOT_BITS) & 0b111))
            .take_while(|&index| index != 0)
            .filter_map(|index| {
                Self::OPTIONAL
                    .into_iter()
                    .find(|page| page.index() == index)
            })
    }

    fn pushed(self, page: RidePage) -> Self {
        let slot = self.optional().count() as u32;
        Self(self.0 | (page.index() as u16) << (slot * Self::SLOT_BITS))
    }

    pub fn encode(self) -> [u8; Self::LENGTH] {
        let mut bytes = [Self::END; Self::LENGTH];
        for (byte, page) in bytes.iter_mut().zip(self.optional()) {
            *byte = page.index() as u8;
        }
        bytes
    }

    /// The deck a record's bytes stand for; none when a byte names no
    /// optional page, names one twice, or follows the end.
    pub fn decode(bytes: [u8; Self::LENGTH]) -> Option<Self> {
        let mut deck = Self::MAIN;
        let shown = bytes.iter().take_while(|&&byte| byte != Self::END);
        for &byte in shown.clone() {
            let page = *Self::OPTIONAL
                .iter()
                .find(|page| page.index() == usize::from(byte))?;
            if deck.contains(page) {
                return None;
            }
            deck = deck.pushed(page);
        }
        let rest = &bytes[shown.count()..];
        rest.iter().all(|&byte| byte == Self::END).then_some(deck)
    }

    /// The pages in MOVE order.
    pub fn pages(self) -> heapless::Vec<RidePage, { RidePage::TRAINING.len() }> {
        let workout = (self.0 & Self::WORKOUT != 0).then_some(RidePage::Workout);
        workout
            .into_iter()
            .chain([RidePage::Main])
            .chain(self.optional())
            .collect()
    }

    pub fn shows(self, page: RidePage) -> bool {
        page == RidePage::Workout || self.contains(page)
    }

    /// `page` on, in its place among the optional pages if it was off:
    /// after the last page shown that comes before it, else after main.
    pub fn showing(self, page: RidePage) -> Self {
        if self.contains(page) || !Self::OPTIONAL.contains(&page) {
            return self;
        }
        let rank = |page: RidePage| Self::OPTIONAL.iter().position(|&optional| optional == page);
        let before = self
            .optional()
            .filter(|&shown| rank(shown) < rank(page))
            .last();
        let after = before.map_or(0, |before| {
            self.optional()
                .position(|shown| shown == before)
                .map_or(0, |at| at + 1)
        });
        let deck = self
            .optional()
            .take(after)
            .fold(self.only_workout(), Self::pushed);
        let deck = deck.pushed(page);
        self.optional().skip(after).fold(deck, Self::pushed)
    }

    /// Show or hide `page`; the main page and the workout page stay on.
    pub fn toggled(self, page: RidePage) -> Self {
        if !self.contains(page) {
            return self.showing(page);
        }
        self.optional()
            .filter(|&shown| shown != page)
            .fold(self.only_workout(), Self::pushed)
    }

    fn only_workout(self) -> Self {
        Self(self.0 & Self::WORKOUT)
    }

    /// The deck MOVE steps through: the workout page first while a workout
    /// is chosen, then this one.
    pub fn shown(self, workout: bool) -> Self {
        if workout {
            Self(self.0 | Self::WORKOUT)
        } else {
            self
        }
    }

    pub fn count(self) -> usize {
        self.pages().len()
    }

    pub fn first(self) -> RidePage {
        self.pages()[0]
    }

    pub fn last(self) -> RidePage {
        self.pages()[self.count() - 1]
    }

    pub fn contains(self, page: RidePage) -> bool {
        self.pages().contains(&page)
    }

    /// Where `page` sits, for the page dots; a page that is not there
    /// counts as the first.
    pub fn position(self, page: RidePage) -> usize {
        self.pages()
            .iter()
            .position(|shown| *shown == page)
            .unwrap_or(0)
    }

    /// The page after `page`, wrapping.
    pub fn following(self, page: RidePage) -> RidePage {
        self.pages()[(self.position(page) + 1) % self.count()]
    }

    /// The page before `page`, wrapping.
    pub fn preceding(self, page: RidePage) -> RidePage {
        self.pages()[(self.position(page) + self.count() - 1) % self.count()]
    }

    /// The page after `page`; none after the last.
    pub fn next(self, page: RidePage) -> Option<RidePage> {
        self.pages().get(self.position(page) + 1).copied()
    }
}

/// Two bytes, three bits a slot, each slot an optional page's index.
const _: () = {
    assert!(core::mem::size_of::<Deck>() == 2);
    assert!(Deck::LENGTH as u32 * Deck::SLOT_BITS < 15);
    let mut at = 0;
    while at < Deck::OPTIONAL.len() {
        let index = Deck::OPTIONAL[at] as usize;
        assert!(index > 0 && index < 1 << Deck::SLOT_BITS);
        at += 1;
    }
};

/// What a ride page drawn from its tiles prepares before they draw: the
/// climb chart, in the profile tile's box. The app lays out the workout
/// chart from its player.
pub(super) fn prepare_tiles(view: &mut View, laid: &crate::compose::builtin::Laid) {
    if view.ride_page == RidePage::Climb
        && let Some(chart) = laid.box_of(crate::compose::registry::CLIMB_PROFILE)
    {
        climb_frame(view, chart);
    }
}

/// Hide what the course banner partly covers on the ride page shown; on a
/// page drawn from its tree, every tile it touches, so no value shows
/// without its caption.
pub(super) fn hide_banner(view: &mut View, laid: Option<&crate::compose::builtin::Laid>) {
    use crate::compose::node::What;
    use crate::compose::registry::{SPACE, STATUS, TOP_LINE};
    let device = view.device;
    let banner = super::sign::on_page(view.ride_page, view.nav, view.radar_paired, device, None)
        .filter(|sign| sign.form() == SignForm::Banner);
    let (Some(banner), Body::Canvas(canvas)) = (banner, &mut view.body) else {
        return;
    };
    let covered = banner.rect();
    let mut rows = covered.top..covered.bottom;
    let nodes = laid.map_or(&[][..], |laid| laid.screen().tree().nodes());
    let tiles = laid.map_or(&[][..], |laid| laid.leaves());
    for placed in tiles {
        let drawn = match nodes[usize::from(placed.node)].what() {
            Some(What::Leaf { component, .. }) => ![SPACE, STATUS, TOP_LINE].contains(&component),
            _ => false,
        };
        let (top, bottom) = (
            usize::from(placed.rect.top),
            usize::from(placed.rect.bottom),
        );
        if drawn && top < covered.bottom && covered.top < bottom {
            rows = rows.start.min(top)..rows.end.max(bottom);
        }
    }
    canvas.hide(rows);
}

/// The climb chart, prepared in its own frame (#167).
#[inline(never)]
fn climb_frame(view: &mut View, chart: Rect) {
    let climb = climb::frame(view, chart);
    view.set_climb_frame(climb);
}

/// Paint a content row over the black the scene laid: the radar strip
/// while a radar is paired, then the page's tiles over its chart.
pub(super) fn paint(scene: &Scene, page: RidePage, row: &mut Row) {
    if let Some(radar) = &scene.radar {
        paint_radar(radar, &mut row.within(radar.columns()));
    }
    if page == RidePage::Map {
        paint_map(scene, row);
        return;
    }
    match page {
        RidePage::Climb => climb::paint_chart(scene.climb(), row),
        RidePage::Workout => workout::paint_chart(scene.chart(), row),
        _ => {}
    }
    scene.paint_canvas(row);
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
        RidePage::Main | RidePage::Lap => {}
        RidePage::Workout => workout::chart_parts(scene.chart(), parts),
        RidePage::Climb => climb::chart_parts(scene.climb(), parts),
        RidePage::Map => {
            if let Some(ground) = scene.map().and_then(|map| map.frame().ground()) {
                parts.add(ground.drawn.top..ground.drawn.bottom, &scene.map());
            }
        }
    }
    scene.canvas().parts(parts);
}

fn paint_map(scene: &Scene, row: &mut Row) {
    if let Some((map, ground)) = scene
        .map()
        .and_then(|map| Some((map.frame(), map.frame().ground()?)))
    {
        let shown = ground.shown;
        map::paint(map, &mut row.within(shown.left..shown.right));
    }
    scene.paint_canvas(row);
}

/// Where the radar strip draws: the arrow, then the track from the rider at
/// the top to 200 m at the bottom. The track ends above the toast band, so a
/// toast never hides a target, a disc `radius` rows above and below its
/// range, centred on column `centre`. It paints columns `left..right`, and
/// black divides it from the body, which starts at column `body`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Strip {
    arrow_top: usize,
    track_top: usize,
    track_bottom: usize,
    radius: usize,
    centre: usize,
    left: usize,
    right: usize,
    body: usize,
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
    device: &'static Device,
) -> Pixels {
    let gliding = super::data::Gliding { graphics, profile };
    Pixels {
        glides: main.map_or([None; main::GLIDES], |main| main.glides(&gliding)),
        markers: radar(&graphics.radar, &device.scaled).markers,
    }
}

pub use main::{
    chip as main_chip, flags as main_flags, flags_of as main_flags_of, laid as main_laid,
    status_bar as main_status_bar,
};

#[derive(Clone, Copy, Debug, Hash)]
pub struct Radar {
    track: u16,
    markers: [Option<(u16, bool)>; 8],
    strip: Strip,
}

impl Radar {
    /// The columns the strip paints.
    fn columns(&self) -> core::ops::Range<usize> {
        self.strip.left..self.strip.right
    }
}

pub(super) fn radar(snapshot: &radar::Snapshot, at: &Scaled) -> Radar {
    let strip = strip(at);
    Radar {
        track: track_color(snapshot.status),
        markers: snapshot
            .targets
            .map(|target| target.map(|target| (target_y(target, &strip), close(target)))),
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
    if let Some(theme) = scene.theme() {
        let under = match &scene.radar {
            Some(radar) if radar.columns().contains(&x) => {
                match reference_radar_pixel(radar, x, y) {
                    chrome::BLACK => chrome::Shown(theme.bg),
                    mark => look.ink(mark),
                }
            }
            _ => chrome::Shown(theme.bg),
        };
        return scene.canvas().pixel(chrome::Look::PLAIN, x, y, under);
    }
    if page == RidePage::Map {
        return match &scene.radar {
            Some(radar) if radar.columns().contains(&x) => {
                let under = look.ink(reference_radar_pixel(radar, x, y));
                scene.canvas().pixel(look, x, y, under)
            }
            Some(radar) if x < radar.strip.body => scene.canvas().pixel(look, x, y, black),
            _ => reference_map_pixel(scene, x, y),
        };
    }
    if let Some(radar) = &scene.radar
        && x < RADAR_WIDTH - 2
    {
        return look.ink(reference_radar_pixel(radar, x, y));
    }
    let under = match page {
        RidePage::Climb => look.ink(climb::chart_pixel(scene.climb(), x, y)),
        RidePage::Workout => look.ink(workout::chart_pixel(scene.chart(), x, y)),
        _ => black,
    };
    scene.canvas().pixel(look, x, y, under)
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
pub(crate) fn lap_time(value: &mut Text<12>, seconds: u64) {
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

/// Which of the trace's shown readings the paused screen draws.
pub fn paused_readings() -> heapless::Vec<&'static str, 7> {
    tree_readings(ScreenId::RidePaused).unwrap_or_default()
}

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

/// The largest box accept gives tile `component` on `device`'s panel: the
/// tile alone, grown over the whole screen.
#[cfg(test)]
pub(crate) fn largest_box(device: &super::Device, component: u8) -> Rect {
    use crate::compose::input::Controls;
    use crate::compose::node::{Flags, Node, Size};
    use crate::compose::parse::Screen;
    use crate::compose::solve::Solved;
    let bytes = Node::leaf(component, 0, Size::grow(1)).bytes();
    let screen = Screen::parse(0, &bytes, &device.scaled, Controls::FREE).expect("accepted");
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    let rect = solved.leaves()[0].rect.rect();
    assert_eq!(rect, device.scaled.screen());
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::{Choices, Laid, ScreenId};
    use crate::compose::node::{Flags, What};
    use crate::compose::registry::{self, SPACE, STATUS};
    use crate::screens::workout::Page;

    /// `view` solved and drawn, as the runtime shows its page.
    pub(super) fn prepared(mut view: View<'static>) -> View<'static> {
        view.compose_main();
        view.prepare();
        view
    }

    /// The balance rule on `id`: the room its tiles leave down its column
    /// is even gaps, above the first row and below the last as well as
    /// between them, on every panel, with and without a radar. A chart
    /// ends the column above it.
    pub(super) fn balanced(id: ScreenId) {
        for device in crate::screens::panels::ALL {
            for flags in [Flags::NONE, Flags::RADAR] {
                let laid = Laid::new(id, device, Choices::default(), flags, None).unwrap();
                let nodes = laid.screen().tree().nodes();
                let (mut start, mut end) = (0, device.scaled.panel.height as i32);
                let mut rows: std::vec::Vec<(i32, i32)> = std::vec::Vec::new();
                for placed in laid.leaves() {
                    let Some(What::Leaf { component, .. }) = nodes[usize::from(placed.node)].what()
                    else {
                        continue;
                    };
                    let (top, bottom) = (i32::from(placed.rect.top), i32::from(placed.rect.bottom));
                    match component {
                        STATUS => start = bottom,
                        SPACE => {}
                        _ if registry::tile(component).heavy() => end = top,
                        _ => match rows.iter_mut().find(|row| top < row.1 && row.0 < bottom) {
                            Some(row) => *row = (row.0.min(top), row.1.max(bottom)),
                            None => rows.push((top, bottom)),
                        },
                    }
                }
                rows.sort();
                let mut edges = std::vec![start];
                for (top, bottom) in &rows {
                    edges.extend([*top, *bottom]);
                }
                edges.push(end);
                let gaps: std::vec::Vec<i32> =
                    edges.chunks(2).map(|pair| pair[1] - pair[0]).collect();
                let (least, most) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
                assert!(
                    most - least <= 1 && *least > 0,
                    "{id:?} {flags} on {:?}: gaps {gaps:?}",
                    device.scaled.panel
                );
            }
        }
    }

    #[test]
    fn the_paused_screen_and_end_ride_are_centred() {
        for id in [ScreenId::RidePaused, ScreenId::RideEnd] {
            for device in crate::screens::panels::ALL {
                let laid = Laid::new(id, device, Choices::default(), Flags::NONE, None).unwrap();
                let leaves = laid.leaves();
                let above = i32::from(leaves[0].rect.top);
                let bottom = i32::from(leaves[leaves.len() - 1].rect.bottom);
                let height = device.scaled.panel.height;
                let floor = laid
                    .controls()
                    .cues()
                    .into_iter()
                    .flat_map(|cues| cues.shown())
                    .map(|cue| cue.tab(&device.keys, &device.scaled))
                    .filter(|tab| tab.bottom == height)
                    .map(|tab| tab.top)
                    .min()
                    .unwrap_or(height);
                let below = floor as i32 - bottom;
                assert!(
                    (above - below).abs() <= 1,
                    "{id:?}: {above} above, {below} below"
                );
            }
        }
    }

    #[test]
    fn longest_pause_lays_out_whole_between_the_cues() {
        for device in crate::screens::panels::ALL {
            for auto in [false, true] {
                let mut view = View::new(Page::Ride, 0, None);
                view.device = device;
                view.pause_screen = true;
                view.auto_paused = auto;
                view.paused_secs = 359_999;
                view.active_secs = 359_999;
                let sample = crate::ride::log::Sample::default();
                view.totals.add(1000, 999_999_999, &sample, None);
                let mut view = prepared(view);
                let scene = view.prepare();
                scene.check_layout();
                let cues = scene.cues().to_vec();
                assert_eq!(cues.len(), 2, "resume and end");
                let canvas = view.canvas();
                let what = std::format!("paused on {:?}", device.scaled.panel);
                for shown in ["FOR 99:59:59", "99:59:59", "1000.0 KM", "RIDE TIME"] {
                    assert!(canvas.shows(shown), "{what}: {shown}");
                }
                assert_eq!(canvas.shows("RIDE ON TO RESUME"), auto, "{what}");
                assert_eq!(canvas.shows("AUTO PAUSED"), auto, "{what}");
                for run in &canvas.runs {
                    let rect = crate::screens::chrome::Rect {
                        left: run.columns().start,
                        top: run.top,
                        right: run.columns().end,
                        bottom: run.bottom(),
                    };
                    let text = core::str::from_utf8(run.line.text());
                    for cue in &cues {
                        assert!(!cue.covers(rect, device), "{what}: {text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn end_ride_offers_the_trash_only_where_a_ride_can_be_discarded() {
        use crate::screens::cue::Icon;
        use crate::screens::workout::Discard;
        for (discard, icons) in [
            (Discard::Unavailable, &[Icon::Check, Icon::Back][..]),
            (
                Discard::Offered,
                &[Icon::Check, Icon::Back, Icon::Trash][..],
            ),
        ] {
            let mut view = View::new(Page::EndRide, 0, None);
            view.discard = discard;
            let mut view = prepared(view);
            let scene = view.prepare();
            scene.check_layout();
            let mut shown: std::vec::Vec<Icon> = scene.cues().iter().map(|cue| cue.icon).collect();
            shown.sort_by_key(|icon| *icon as u8);
            let mut icons = icons.to_vec();
            icons.sort_by_key(|icon| *icon as u8);
            assert_eq!(shown, icons, "{discard:?}");
            for line in ["END RIDE?", "YOUR RIDE WILL BE SAVED", "0:00:00", "0.00 KM"] {
                assert!(view.canvas().shows(line), "{line}");
            }
        }
    }

    /// The readings `page` draws, found by drawing it with every reading
    /// at a value nothing else on the page takes.
    fn drawn(page: Option<RidePage>) -> std::vec::Vec<&'static str> {
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
        let at = &crate::screens::scaled::BASE;
        map.update(&track, map::ground(at, map::ground_rect(at)));
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
        view.pause_screen = page.is_none();
        view.compose(slots::Slots::default());
        view.prepare();
        let texts = runs_and_neighbour_pairs(view.canvas());
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
            .filter(|(reading, pair)| {
                let value = pair.split_once('=').unwrap().1;
                let distance_in_tenths = |word: &str| {
                    *reading == "dist" && value.len() == word.len() + 1 && value.starts_with(word)
                };
                texts.iter().any(|text| {
                    text.split(' ')
                        .any(|word| word == value || distance_in_tenths(word))
                })
            })
            .map(|(reading, _)| reading)
            .collect()
    }

    /// Each run's text, then each two neighbouring runs joined, as a grade
    /// and its `%` draw.
    fn runs_and_neighbour_pairs(
        canvas: &crate::screens::canvas::Canvas,
    ) -> std::vec::Vec<std::string::String> {
        let runs: std::vec::Vec<std::string::String> = canvas
            .runs
            .iter()
            .map(|run| std::string::String::from_utf8_lossy(run.line.text()).into())
            .collect();
        let pairs = runs
            .windows(2)
            .map(|pair| std::format!("{}{}", pair[0], pair[1]));
        runs.iter().cloned().chain(pairs).collect()
    }

    #[test]
    fn each_page_declares_the_readings_it_draws() {
        for page in RidePage::ALL {
            assert_eq!(
                page.readings().unwrap().to_vec(),
                drawn(Some(page)),
                "{page:?}"
            );
        }
        assert_eq!(paused_readings().to_vec(), drawn(None), "paused");
    }

    fn choice(hidden: &[RidePage]) -> Deck {
        hidden
            .iter()
            .fold(Deck::default(), |choice, page| choice.toggled(*page))
    }

    #[test]
    fn pages_step_in_order_and_wrap_both_ways() {
        let free = Deck::default().shown(false);
        assert_eq!(free.pages(), RidePage::ALL);
        assert_eq!(RidePage::default(), RidePage::Main);
        assert_eq!(free.next(RidePage::Main), Some(RidePage::Lap));
        assert_eq!(free.next(RidePage::Lap), Some(RidePage::Climb));
        assert_eq!(free.next(RidePage::Climb), Some(RidePage::Map));
        assert_eq!(free.next(RidePage::Map), None);
        assert_eq!(free.following(RidePage::Map), RidePage::Main);
        assert_eq!(free.preceding(RidePage::Main), RidePage::Map);
        assert_eq!(free.preceding(RidePage::Lap), RidePage::Main);
        let training = Deck::default().shown(true);
        assert_eq!(training.pages(), RidePage::TRAINING);
        assert_eq!(training.following(RidePage::Workout), RidePage::Main);
        assert_eq!(training.following(RidePage::Climb), RidePage::Map);
        assert_eq!(training.following(RidePage::Map), RidePage::Workout);
        assert_eq!(training.preceding(RidePage::Workout), RidePage::Map);
        assert_eq!(training.position(RidePage::Main), 1);
        assert_eq!(free.position(RidePage::Workout), 0);
    }

    #[test]
    fn every_subset_of_optional_pages_makes_a_walkable_list() {
        for bits in 0..1u8 << Deck::OPTIONAL.len() {
            let hidden: std::vec::Vec<RidePage> = Deck::OPTIONAL
                .into_iter()
                .enumerate()
                .filter(|(i, _)| bits >> i & 1 == 1)
                .map(|(_, page)| page)
                .collect();
            for workout in [false, true] {
                let pages = choice(&hidden).shown(workout);
                let list = pages.pages();
                assert!(list.contains(&RidePage::Main), "{hidden:?}");
                assert_eq!(list.contains(&RidePage::Workout), workout);
                assert_eq!(list.len(), pages.count());
                assert!(hidden.iter().all(|page| !pages.contains(*page)));
                assert_eq!(
                    list.len(),
                    1 + usize::from(workout) + Deck::OPTIONAL.len() - hidden.len()
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
        let all_off = choice(&Deck::OPTIONAL);
        let pages = all_off.shown(false);
        assert_eq!(pages.pages(), [RidePage::Main]);
        assert_eq!(pages.following(RidePage::Main), RidePage::Main);
        assert_eq!(pages.preceding(RidePage::Main), RidePage::Main);
        assert_eq!(pages.next(RidePage::Main), None);
        assert_eq!(
            all_off.shown(true).pages(),
            [RidePage::Workout, RidePage::Main]
        );
    }

    #[test]
    fn main_and_workout_cannot_be_hidden_and_toggling_twice_restores() {
        let default = Deck::default();
        assert_eq!(default.pages(), RidePage::ALL);
        for page in [RidePage::Main, RidePage::Workout] {
            assert_eq!(default.toggled(page), default);
            assert!(default.shows(page));
        }
        for page in Deck::OPTIONAL {
            let off = default.toggled(page);
            assert!(!off.shows(page));
            assert_eq!(off.count(), default.count() - 1);
            assert_eq!(off.toggled(page), default, "back in its place");
        }
    }

    #[test]
    fn a_deck_round_trips_its_bytes_and_refuses_what_means_nothing() {
        for bits in 0..1u8 << Deck::OPTIONAL.len() {
            let hidden: std::vec::Vec<RidePage> = Deck::OPTIONAL
                .into_iter()
                .enumerate()
                .filter(|(i, _)| bits >> i & 1 == 1)
                .map(|(_, page)| page)
                .collect();
            let deck = choice(&hidden);
            assert_eq!(Deck::decode(deck.encode()), Some(deck), "{hidden:?}");
        }
        assert_eq!(Deck::default().encode(), [1, 2, 4]);
        assert_eq!(choice(&Deck::OPTIONAL).encode(), [0xff; 3]);
        let reordered = Deck::decode([4, 1, 0xff]).unwrap();
        assert_eq!(
            reordered.pages(),
            [RidePage::Main, RidePage::Map, RidePage::Lap]
        );
        assert_eq!(
            reordered.showing(RidePage::Climb).pages(),
            [
                RidePage::Main,
                RidePage::Map,
                RidePage::Lap,
                RidePage::Climb
            ]
        );
        for bytes in [
            [0, 0xff, 0xff],
            [3, 0xff, 0xff],
            [1, 1, 0xff],
            [0xff, 1, 0xff],
            [5, 0xff, 0xff],
        ] {
            assert_eq!(Deck::decode(bytes), None, "{bytes:?}");
        }
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
            let at = &crate::screens::scaled::BASE;
            let strip = &strip(at);
            let bottom = usize::from(target_y(target, strip)) + strip.radius;
            assert!(bottom < chrome::TOAST.top, "{range_mm} mm");
            let mut snapshot = radar::Snapshot::default();
            snapshot.targets[0] = Some(target);
            for y in chrome::TOAST.top..chrome::HEIGHT {
                for x in 0..RADAR_WIDTH {
                    assert_eq!(
                        reference_radar_pixel(&radar(&snapshot, at), x, y),
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
