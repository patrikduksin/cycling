mod arrow;
mod pose;
mod raster;
mod roads;

use super::kilometres;
use crate::ride::course::follow::{self, Placement};
use crate::ride::course::nav::{Glyph, Guidance, Sign, Spot};
use crate::ride::homing::{Crumb, Frame as RideFrame, TURN, bearing, distance};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BLACK, BODY as BODY_FACE, HEIGHT, Rect, Row, SECONDARY, WHITE, WIDTH,
    blend, paint_disc_over, rgb,
};
use crate::screens::icons::PUCK;
use crate::screens::workout::{Text, View};
use arrow::{ARROW_OUTLINE_PX, ARROW_SHAFT_HALF_PX, ARROW_STROKES, arrow_shape};
use firmware_shell::rendering::font::FOOT_20;
pub use pose::{GROUND, Reach, Zoom};
use pose::{Pose, Projection};
use raster::{Bar, Class, Cover, Layer, Stroke, Triangle};
use roads::{Detail, Roads, Source};

type Point = (f32, f32);

#[derive(Clone, Copy, Debug, Default)]
pub struct Track<'a> {
    /// The device has memory set aside for the breadcrumb.
    pub available: bool,
    /// Kept points, metres east and north of the start, oldest first.
    pub crumbs: &'a [Crumb],
    /// The rider, while the fix is fresh.
    pub here: Option<Point>,
    /// When the receiver took `here`.
    pub here_taken_ms: Option<u64>,
    /// The way the rider is riding, in `TURN`ths clockwise from north.
    pub heading: Option<u16>,
    /// The ride's live speed, when a sensor or the receiver reports one.
    pub speed_mm_s: Option<u32>,
    /// The loaded course, end first, each point's `along` its metres to the
    /// end; empty without one.
    pub course: &'a [Crumb],
    pub placement: Placement,
    /// Following the course: what is ahead is its first points up to this
    /// index, from its end, and then the matched point if any.
    pub ahead: Option<(usize, Option<Point>)>,
    pub guidance: Option<Guidance>,
    /// Metres left to the course's end, while following it.
    pub left_m: Option<u32>,
    pub reach: Reach,
    /// A radar's strip takes the screen's left edge.
    pub radar: bool,
}

/// What a frame was filled from: refill it only when this changes, about
/// once a second, not on every radar redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    available: bool,
    crumbs: usize,
    newest: u32,
    here: Option<(u32, u32)>,
    heading: Option<u16>,
    placement: Placement,
    ahead: Option<(usize, Option<(u32, u32)>)>,
    leg: Option<usize>,
    sign: Option<Sign>,
    turn: Option<u32>,
    junction: Option<u32>,
    left_m: Option<u32>,
    reach: Reach,
    radar: bool,
    roads: u32,
}

impl Track<'_> {
    fn key(&self, leg: Option<usize>) -> Key {
        let bits = |(x, y): Point| (x.to_bits(), y.to_bits());
        Key {
            available: self.available,
            crumbs: self.crumbs.len(),
            newest: self.crumbs.last().map_or(0, |c| c.along.to_bits()),
            here: self.here.map(bits),
            heading: self.heading,
            placement: self.placement,
            ahead: self
                .ahead
                .map(|(segment, matched)| (segment, matched.map(bits))),
            leg,
            sign: self.guidance.map(|g| g.sign),
            turn: self.turn_on_course().map(|(spot, _)| spot.along_mm),
            junction: self.junction().map(|j| j.along_mm),
            left_m: self.left_m,
            reach: self.reach,
            radar: self.radar,
            roads: 0,
        }
    }

    fn rider_or_last_crumb(&self) -> Option<Point> {
        self.here
            .or_else(|| self.crumbs.last().map(|c| (c.east, c.north)))
    }

    fn following(&self) -> bool {
        self.ahead.is_some() && self.course.len() >= 2
    }

    fn junction(&self) -> Option<Spot> {
        self.guidance
            .and_then(|g| g.junction)
            .filter(|_| self.following())
    }

    fn turn_on_course(&self) -> Option<(Spot, Glyph)> {
        self.guidance
            .and_then(|g| g.turn)
            .filter(|_| self.following() && self.junction().is_none())
    }

    fn split(&self) -> Option<(usize, Point)> {
        let (segment, matched) = self.ahead.filter(|_| self.following())?;
        if let Some(junction) = self.junction() {
            return Some((self.index_of(junction), junction.at));
        }
        let end = segment.clamp(1, self.course.len() - 1);
        let c = self.course[end];
        Some((end, matched.unwrap_or((c.east, c.north))))
    }

    fn metres_to_end(&self, spot: Spot) -> f32 {
        let total = self.course.last().map_or(0.0, |c| c.along);
        (total - spot.along_mm as f32 / 1000.0).max(0.0)
    }

    fn index_of(&self, spot: Spot) -> usize {
        let to_end = self.metres_to_end(spot);
        self.course
            .partition_point(|c| c.along < to_end)
            .clamp(1, self.course.len() - 1)
    }

    fn place(&self, centre: Point, held: Option<usize>) -> Option<(usize, Point)> {
        if let Some(place) = self.split() {
            return Some(place);
        }
        let joining = self.joining(centre, held)?;
        let leg = joining.held.unwrap_or(joining.best);
        Some((leg.end, leg.at))
    }

    fn ahead_points(
        &self,
        (end, at): (usize, Point),
        reach_m: f32,
    ) -> impl Iterator<Item = (Point, bool)> + '_ {
        let turn = self
            .turn_on_course()
            .map(|(spot, _)| self.metres_to_end(spot));
        let before = move |along: f32| turn.is_none_or(|t| along > t);
        let next = self.course[end - 1];
        let start = (at, next.along + distance(at, (next.east, next.north)));
        let stop = start.1 - reach_m;
        let first = self.course[..end].partition_point(|c| c.along < stop);
        let walked = |c: &Crumb| ((c.east, c.north), c.along);
        let last = self.course[first..end].first().map_or(start, walked);
        let cut = first.checked_sub(1).map(|k| {
            let ((x, y), from) = last;
            let to = self.course[k];
            let t = (from - stop) / (from - to.along);
            ((x + t * (to.east - x), y + t * (to.north - y)), stop)
        });
        core::iter::once((at, true)).chain(
            self.course[first..end]
                .iter()
                .rev()
                .map(walked)
                .chain(cut)
                .map(move |(p, along)| (p, before(along))),
        )
    }

    fn crumbs_back(&self) -> impl Iterator<Item = Point> + '_ {
        self.here
            .into_iter()
            .chain(self.crumbs.iter().rev().map(|c| (c.east, c.north)))
    }

    fn joining(&self, centre: Point, held: Option<usize>) -> Option<Joining> {
        let legs = 1..self.course.len();
        let leg = |end: usize| {
            let (a, b) = (self.course[end - 1], self.course[end]);
            let at = follow::project(centre, (a.east, a.north), (b.east, b.north)).0;
            Leg {
                end,
                at,
                gap_m: distance(at, centre),
            }
        };
        let held = held.filter(|end| legs.contains(end)).map(leg);
        let Some(heading) = self.heading else {
            let best = held.or_else(|| closest(legs.map(leg)))?;
            return Some(Joining {
                held,
                best,
                heading_against_held: false,
                held_leg_too_far: false,
            });
        };
        let (sin, cos) = pose::rotation(heading);
        let our_way = |&end: &usize| {
            let (a, b) = (self.course[end - 1], self.course[end]);
            (a.east - b.east) * sin + (a.north - b.north) * cos > 0.0
        };
        let ours =
            closest(legs.clone().filter(our_way).map(leg)).filter(|l| l.gap_m <= JOIN_WITHIN_M);
        let best = ours.or_else(|| closest(legs.map(leg)))?;
        let heading_against_held = held.is_some_and(|held| {
            let (a, b) = (self.course[held.end - 1], self.course[held.end]);
            let off_its_way =
                (heading + TURN - bearing((b.east, b.north), (a.east, a.north))) % TURN;
            MORE_THAN_135_DEG_OFF.contains(&off_its_way)
        });
        let held_leg_too_far = held.is_some_and(|held| {
            best.end != held.end
                && (held.gap_m > JOIN_WITHIN_M || held.gap_m - best.gap_m > JOIN_SWITCH_M)
        });
        Some(Joining {
            held,
            best,
            heading_against_held,
            held_leg_too_far,
        })
    }
}

#[derive(Clone, Copy)]
struct Leg {
    end: usize,
    at: Point,
    gap_m: f32,
}

struct Joining {
    held: Option<Leg>,
    best: Leg,
    heading_against_held: bool,
    held_leg_too_far: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fixed {
    at: Point,
    taken_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Moving,
    TooSlowToTell,
    Stopped,
}

#[derive(Clone, Copy, Debug, Default)]
struct HeldLeg {
    placement: Placement,
    end: Option<usize>,
    last_fix: Option<Fixed>,
    counted_from: Option<Point>,
    against_fixes: u8,
    too_far_fixes: u8,
}

impl HeldLeg {
    fn observe(&mut self, track: &Track) {
        if self.placement != track.placement {
            *self = Self {
                placement: track.placement,
                ..Self::default()
            };
        }
        let fix = track.here.map(|at| Fixed {
            at,
            taken_ms: track.here_taken_ms,
        });
        let motion = fix.and_then(|fix| self.motion(fix, track.speed_mm_s));
        if let Some((end, _)) = track.split() {
            self.hold(end);
            return;
        }
        if let (Some(fix), Some(motion)) = (fix, motion)
            && let Some(joining) = track.joining(fix.at, self.end)
        {
            self.choose(joining, motion);
        }
    }

    fn motion(&mut self, fix: Fixed, speed_mm_s: Option<u32>) -> Option<Motion> {
        let last = self.last_fix.replace(fix);
        if last == Some(fix) {
            return None;
        }
        let counted_from = *self.counted_from.get_or_insert(fix.at);
        let travelled_from = |from: Point| distance(from, fix.at) >= MOVING_M;
        let motion = match speed_mm_s {
            Some(v) if !pose::moving(v) => Motion::Stopped,
            Some(_) if travelled_from(counted_from) => Motion::Moving,
            Some(_) => Motion::TooSlowToTell,
            None if last.is_some_and(|last| travelled_from(last.at)) => Motion::Moving,
            None => Motion::Stopped,
        };
        if motion != Motion::TooSlowToTell {
            self.counted_from = Some(fix.at);
        }
        Some(motion)
    }

    fn hold(&mut self, end: usize) {
        self.end = Some(end);
        self.against_fixes = 0;
        self.too_far_fixes = 0;
    }

    fn choose(
        &mut self,
        Joining {
            held,
            best,
            heading_against_held,
            held_leg_too_far,
        }: Joining,
        motion: Motion,
    ) {
        if held.is_none() {
            self.hold(best.end);
            return;
        }
        let count = |n: u8, holds: bool| match motion {
            Motion::Moving if holds => n.saturating_add(1),
            Motion::TooSlowToTell => n,
            _ => 0,
        };
        self.against_fixes = count(self.against_fixes, heading_against_held);
        self.too_far_fixes = count(self.too_far_fixes, held_leg_too_far);
        if self.against_fixes >= TURN_AROUND_FIXES || self.too_far_fixes >= TOO_FAR_FIXES {
            self.hold(best.end);
        }
    }
}

fn closest(legs: impl Iterator<Item = Leg>) -> Option<Leg> {
    legs.min_by(|a, b| a.gap_m.total_cmp(&b.gap_m))
}

pub struct Cache<'a> {
    frame: &'a mut Frame,
    held: Option<Key>,
}

impl<'a> Cache<'a> {
    pub fn new(frame: &'a mut Frame) -> Self {
        Self { frame, held: None }
    }

    /// Fill the frame again if `track` or the roads changed; whether it did.
    pub fn update(&mut self, track: &Track) -> bool {
        self.frame.route_leg.observe(track);
        let key = Key {
            roads: self.frame.roads.generation(),
            ..track.key(self.frame.route_leg.end)
        };
        let stale = self.held != Some(key);
        if stale {
            fill(self.frame, track);
            self.held = Some(key);
        }
        stale
    }

    /// One pass of fetching the roads under the map from the terrain
    /// cache, in the ride's `frame`: ask for the tiles the view lacks and
    /// copy one that is cached.
    pub fn fetch_roads(&mut self, engine: &mut terrain::Lookahead, frame: RideFrame) -> Fetched {
        let pack = engine.pack().map(|(pack, header)| (pack, *header));
        self.frame
            .roads
            .hold(pack.map(|(pack, _)| Source { pack, frame }));
        let Some((_, header)) = pack else {
            return Fetched::Nothing;
        };
        let Some(keys) = self.frame.road_tiles(&header, frame) else {
            engine.want([]);
            return Fetched::Nothing;
        };
        let detail = Detail::of(self.frame.last_pose.step);
        let roads = &mut self.frame.roads;
        engine.want(roads.lacking(&keys, detail));
        let ready = roads
            .lacking(&keys, detail)
            .find(|&key| engine.roads(key).is_some());
        let Some((key, tile)) = ready.and_then(|key| Some((key, engine.roads(key)?))) else {
            return Fetched::Nothing;
        };
        let (south, west) = header.origin(key);
        if roads.extract(&keys, key, detail, (south * 10, west * 10), tile) {
            Fetched::Whole
        } else {
            Fetched::Cut
        }
    }

    pub fn start_ride(&mut self) {
        self.frame.route_leg = HeldLeg::default();
    }

    pub fn filled(&self) -> Option<Filled<'_>> {
        let key = self.held?;
        Some(Filled {
            frame: self.frame,
            key,
        })
    }
}

/// What a pass of [`Cache::fetch_roads`] copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetched {
    Nothing,
    /// A tile with every road its detail draws.
    Whole,
    /// A tile whose minor roads did not all fit.
    Cut,
}

#[derive(Clone, Copy)]
pub struct Filled<'a> {
    frame: &'a Frame,
    key: Key,
}

impl<'a> Filled<'a> {
    pub(in crate::screens) fn frame(&self) -> &'a Frame {
        self.frame
    }
}

impl core::hash::Hash for Filled<'_> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.frame.last_pose.hash(state);
    }
}

const GROUND_COLOR: u16 = rgb(0x10, 0x10, 0x12);
const MINOR_ROAD: u16 = rgb(0x2a, 0x2a, 0x2d);
const MAJOR_ROAD: u16 = rgb(0x44, 0x44, 0x48);
const RIDDEN: u16 = rgb(0x1f, 0x4a, 0x78);
const CASING: u16 = rgb(0x04, 0x26, 0x4c);
const BAND_ROWS: usize = 16;
const BAND_COUNT: usize = (GROUND.end - GROUND.start).div_ceil(BAND_ROWS);
const STRAY_PX: f32 = 0.35;
const STRAIGHT_RUN: usize = 16;
const RIDDEN_SEGMENTS: usize = 255;
const ROUTE_SEGMENTS: usize = 160;
const STROKES: usize = RIDDEN_SEGMENTS + ROUTE_SEGMENTS;
const BAND_ENTRIES: usize = 2048;
const LOOKAHEAD_M: f32 = 4_000.0;
const ROUTE_REACH: f32 = 1.5;
const JOIN_WITHIN_M: f32 = 25.0;
const JOIN_SWITCH_M: f32 = 10.0;
const TURN_AROUND_FIXES: u8 = 3;
const TOO_FAR_FIXES: u8 = 2;
const MOVING_M: f32 = 2.0;
const MORE_THAN_135_DEG_OFF: core::ops::Range<u16> = TURN * 3 / 8 + 1..TURN * 5 / 8;
const ROAD_SEGMENTS: usize = 2_048;
const MINOR_ROAD_HALF_PX: f32 = 2.5;
const MAJOR_ROAD_HALF_PX: f32 = 4.5;
const GROUND_ROWS: usize = GROUND.end - GROUND.start;
/// Roads are fetched for the view and this share of it more each way.
const ROAD_FETCH_MARGIN: f32 = 0.25;
struct ArrowLayer {
    shaft_half: f32,
    head_outline: f32,
    color: u16,
}
const ARROW_LAYERS: [ArrowLayer; 2] = [
    ArrowLayer {
        shaft_half: ARROW_SHAFT_HALF_PX,
        head_outline: ARROW_OUTLINE_PX,
        color: CASING,
    },
    ArrowLayer {
        shaft_half: 2.5,
        head_outline: 0.5,
        color: WHITE,
    },
];
const CHEVRONS: usize = 3;
const CHEVRON_PITCH: f32 = 15.0;
const CHEVRON_DEPTH: f32 = 6.0;
const CHEVRON_HALF: f32 = 5.0;
const CHEVRON_STROKE: f32 = 1.75;
const PUCK_RINGS_QUARTER_PX: [(i32, u16); 2] = [(58, BLACK), (50, WHITE)];
const PUCK_CLEAR: f32 = 14.5;
const JUNCTION_CLEAR: f32 = 12.0;
const JUNCTION_RINGS_QUARTER_PX: [(i32, u16); 4] =
    [(48, GROUND_COLOR), (40, AMBER), (22, BLACK), (14, WHITE)];
const _: () = assert!(
    BAND_ENTRIES <= u16::MAX as usize,
    "strokes are bucketed as u16"
);

const FOOT_BASE: usize = 309;
const BAR_LEFT: usize = 12;
const BAR_TOP: usize = 303;
const BAR_THICKNESS: usize = 3;
const BAR_GAP: usize = 6;
const DISTANCE_RIGHT: usize = 203;
const UNIT_RIGHT: usize = 228;
const MID_Y: usize = (GROUND.start + GROUND.end) / 2;
const _: () = assert!(GROUND.end <= HEIGHT);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    /// The device keeps no track.
    #[default]
    Unavailable,
    /// No position yet.
    NoGps,
    Map,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Ridden,
    Route,
}

impl Line {
    const ALL: [Self; 2] = [Self::Ridden, Self::Route];

    const fn room(self) -> core::ops::Range<usize> {
        match self {
            Self::Ridden => 0..RIDDEN_SEGMENTS,
            Self::Route => RIDDEN_SEGMENTS..STROKES,
        }
    }

    const fn layers(self) -> &'static [(f32, u16)] {
        match self {
            Self::Ridden => &[(2.5, RIDDEN)],
            Self::Route => &[(9.0, CASING), (6.0, ACCENT)],
        }
    }

    const fn reach(self) -> f32 {
        self.layers()[0].0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Puck {
    centre_quarter_px: (i32, i32),
    arrow: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct Arrow {
    shaft: [Stroke; ARROW_STROKES],
    strokes: usize,
    head: Triangle,
    rows: (f32, f32),
}

/// The map in screen space: refilled when the track's key changes,
/// painted every frame until then. Composition places it in PSRAM,
/// zeroed, so all zero bytes are a valid `Frame`: nothing shown yet.
#[derive(Clone)]
pub struct Frame {
    show: Show,
    last_pose: Pose,
    last_centre: Point,
    layer: Layer<GROUND_ROWS>,
    pairs: [[u16; 2]; 256],
    roads_drawn: [usize; 2],
    strokes: [Stroke; STROKES],
    counts: [usize; 2],
    band_offsets: [[u16; BAND_COUNT + 1]; 2],
    items: [u16; BAND_ENTRIES],
    arrow: Arrow,
    chevrons: [Stroke; 2 * CHEVRONS],
    chevron_count: usize,
    junction_quarter_px: Option<(i32, i32)>,
    puck: Option<Puck>,
    has_track: bool,
    left_m: Option<u32>,
    route_leg: HeldLeg,
    roads: Roads,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unavailable,
            last_pose: Pose::default(),
            last_centre: (0.0, 0.0),
            layer: Layer::new(GROUND.start),
            pairs: [[0; 2]; 256],
            roads_drawn: [0; 2],
            strokes: [Stroke::default(); STROKES],
            counts: [0; 2],
            band_offsets: [[0; BAND_COUNT + 1]; 2],
            items: [0; BAND_ENTRIES],
            arrow: Arrow::default(),
            chevrons: [Stroke::default(); 2 * CHEVRONS],
            chevron_count: 0,
            junction_quarter_px: None,
            puck: None,
            has_track: false,
            left_m: None,
            route_leg: HeldLeg::default(),
            roads: Roads::default(),
        }
    }
}

impl Frame {
    fn line(&self, line: Line) -> &[Stroke] {
        let room = line.room();
        &self.strokes[room.start..room.start + self.counts[line as usize]]
    }

    fn band(&self, line: Line, band: usize) -> &[u16] {
        let band_offsets = &self.band_offsets[line as usize];
        &self.items[usize::from(band_offsets[band])..usize::from(band_offsets[band + 1])]
    }

    fn add(&mut self, line: Line, at: &Projection, points: impl Iterator<Item = Point>) {
        let room = line.room();
        let (strokes, count) = (&mut self.strokes, &mut self.counts[line as usize]);
        project(at, line.reach(), points, |a, b| {
            if *count == room.len() {
                return false;
            }
            strokes[room.start + *count] = Stroke::new(a, b);
            *count += 1;
            true
        });
    }

    /// The ground the last fill drew, `margin` of it wider each way, in
    /// the ride's frame.
    fn view(&self, margin: f32) -> Option<[Point; 4]> {
        if !self.has_pose || self.show != Show::Map {
            return None;
        }
        let at = self.last_pose.projection(self.last_centre);
        let (wide, high) = (WIDTH as f32, (GROUND.end - GROUND.start) as f32);
        let (dx, dy) = (wide * margin, high * margin);
        let (left, right) = (-dx, wide + dx);
        let (top, bottom) = (GROUND.start as f32 - dy, GROUND.end as f32 + dy);
        Some([(left, top), (right, top), (right, bottom), (left, bottom)].map(|p| at.back(p)))
    }

    /// The tiles of `header` the roads are fetched from, in the ride's
    /// frame `ride`: those under the screen, then the margin's while there
    /// is room.
    fn road_tiles(
        &self,
        header: &terrain::Header,
        ride: RideFrame,
    ) -> Option<heapless::Vec<terrain::TileKey, { roads::TILES }>> {
        let screen = self.view(0.0)?.map(|p| ride.locate(p));
        let margin = self.view(ROAD_FETCH_MARGIN)?.map(|p| ride.locate(p));
        let mut keys = heapless::Vec::new();
        for key in header
            .tiles_under(&screen)
            .chain(header.tiles_under(&margin))
        {
            if !keys.contains(&key) && keys.push(key).is_err() {
                break;
            }
        }
        Some(keys)
    }
}

/// Project `points` as one polyline `reach` wide and hand each segment on
/// the screen to `put` while it says there is room for more. A point is
/// dropped while a straight segment past it strays less than `STRAY_PX`
/// from it.
fn project(
    at: &Projection,
    reach: f32,
    points: impl Iterator<Item = Point>,
    mut put: impl FnMut(Point, Point) -> bool,
) {
    let reach = reach + 1.0;
    let mut points = points.map(|p| at.at(p));
    let Some(mut anchor) = points.next() else {
        return;
    };
    let mut passed: heapless::Vec<Point, STRAIGHT_RUN> = heapless::Vec::new();
    let mut draw = |a: Point, b: Point| !visible(a, b, reach) || put(a, b);
    for p in points {
        let straight = !passed.is_full() && straight_past(anchor, p, &passed);
        if !straight {
            let Some(&end) = passed.last() else {
                continue;
            };
            if !draw(anchor, end) {
                return;
            }
            anchor = end;
            passed.clear();
        }
        let _ = passed.push(p);
    }
    if let Some(&end) = passed.last() {
        draw(anchor, end);
    }
}

/// Whether the segment `a`-`b` passes within `STRAY_PX` of every point of
/// `passed`.
fn straight_past(a: Point, b: Point, passed: &[Point]) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let squared = dx * dx + dy * dy;
    let per = if squared > 0.0 { 1.0 / squared } else { 0.0 };
    passed.iter().all(|&(px, py)| {
        let t = (((px - a.0) * dx + (py - a.1) * dy) * per).clamp(0.0, 1.0);
        let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
        qx * qx + qy * qy <= STRAY_PX * STRAY_PX
    })
}

fn visible(a: Point, b: Point, reach: f32) -> bool {
    let (left, right) = (-reach, WIDTH as f32 + reach);
    let (top, bottom) = (GROUND.start as f32 - reach, GROUND.end as f32 + reach);
    a.0.max(b.0) >= left && a.0.min(b.0) <= right && a.1.max(b.1) >= top && a.1.min(b.1) <= bottom
}

fn quarter((x, y): Point) -> (i32, i32) {
    (libm::roundf(4.0 * x) as i32, libm::roundf(4.0 * y) as i32)
}

/// Rasterise the held roads the pose's step draws onto the ground, nearest
/// tile first, until `ROAD_SEGMENTS` are drawn.
fn draw_roads(frame: &mut Frame, at: &Projection, centre: Point) {
    let Frame {
        roads,
        layer,
        pairs,
        roads_drawn,
        last_pose,
        ..
    } = frame;
    layer.clear(GROUND.start);
    *pairs = raster::pairs(GROUND_COLOR, MINOR_ROAD, MAJOR_ROAD);
    *roads_drawn = [0; 2];
    let room = |drawn: &[usize; 2]| drawn[0] + drawn[1] < ROAD_SEGMENTS;
    for road in roads.each(Detail::of(last_pose.step), centre) {
        if !room(roads_drawn) {
            break;
        }
        let (class, half) = if road.class.major() {
            (Class::Major, MAJOR_ROAD_HALF_PX)
        } else {
            (Class::Minor, MINOR_ROAD_HALF_PX)
        };
        project(at, half, road.points(), |a, b| {
            layer.add(&Bar::new(a, b, half), class);
            roads_drawn[class as usize] += 1;
            room(roads_drawn)
        });
    }
}

fn fill(frame: &mut Frame, track: &Track) {
    frame.show = if !track.available {
        Show::Unavailable
    } else if track.rider_or_last_crumb().is_none() {
        Show::NoGps
    } else {
        Show::Map
    };
    frame.counts = [0; 2];
    frame.arrow.strokes = 0;
    frame.chevron_count = 0;
    frame.junction_quarter_px = None;
    frame.puck = None;
    frame.left_m = track.left_m.filter(|_| track.following());
    let Some(centre) = track
        .rider_or_last_crumb()
        .filter(|_| frame.show == Show::Map)
    else {
        frame.band_offsets = [[0; BAND_COUNT + 1]; 2];
        return;
    };
    let heading = track.heading.unwrap_or(frame.last_pose.heading);
    let pose = Pose::new(track.reach, map_left(track.radar), heading);
    draw(frame, track, centre, pose);
}

fn draw(frame: &mut Frame, track: &Track, centre: Point, pose: Pose) {
    frame.last_pose = pose;
    frame.last_centre = centre;
    let place = track.place(centre, frame.route_leg.end);
    let at = pose.projection(centre);
    draw_roads(frame, &at, centre);
    match place {
        Some(place) => {
            let to_junction = track.junction().map_or(0.0, |j| distance(centre, j.at));
            let reach_m = ROUTE_REACH * pose.reach.ahead_m() + to_junction;
            let ahead = track.ahead_points(place, reach_m);
            frame.add(Line::Route, &at, ahead.map(|(p, _)| p));
        }
        None => frame.add(Line::Ridden, &at, track.crumbs_back()),
    }
    frame.has_track = track.crumbs.len() >= 2 || place.is_some();
    if let (Some(place), Some((turn, _))) = (place, track.turn_on_course()) {
        arrow(frame, &at, track, place, at.at(turn.at));
    }
    let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
    if let Some(junction) = track.junction() {
        let junction = at.at(junction.at);
        let reach = JUNCTION_RINGS_QUARTER_PX[0].0 as f32 / 4.0 + 1.0;
        frame.junction_quarter_px = visible(junction, junction, reach).then(|| quarter(junction));
        let ring = frame.junction_quarter_px.is_some();
        chevrons(
            frame,
            rider,
            clipped(rider, junction, map_left(track.radar)),
            ring,
        );
    }
    let tint = match (track.here, track.junction()) {
        (None, _) => SECONDARY,
        (Some(_), Some(_)) => AMBER,
        (Some(_), None) => ACCENT,
    };
    frame.puck = Some(Puck {
        centre_quarter_px: quarter(rider),
        arrow: tint,
    });
    bucket(frame);
}

fn arrow(frame: &mut Frame, at: &Projection, track: &Track, split: (usize, Point), turn: Point) {
    let route = |visit: &mut dyn FnMut(Point, bool)| {
        track
            .ahead_points(split, LOOKAHEAD_M)
            .for_each(|(p, before)| visit(at.at(p), before));
    };
    let Some(shape) = arrow_shape(&route, turn) else {
        return;
    };
    frame.arrow.head = Triangle(shape.head);
    frame.arrow.strokes = 0;
    for pair in shape.shaft.windows(2) {
        frame.arrow.shaft[frame.arrow.strokes] = Stroke::new(pair[0], pair[1]);
        frame.arrow.strokes += 1;
    }
    let reach = ARROW_LAYERS[0].shaft_half + 1.0;
    frame.arrow.rows = shape
        .shaft
        .iter()
        .chain(&frame.arrow.head.0)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(top, bottom), p| {
            (top.min(p.1 - reach), bottom.max(p.1 + reach))
        });
}

fn clipped(from: Point, to: Point, left: usize) -> Point {
    let leaves = |from: f32, to: f32, low: f32, high: f32| match to - from {
        d if d > 0.0 => (high - from) / d,
        d if d < 0.0 => (low - from) / d,
        _ => f32::INFINITY,
    };
    let t = leaves(from.0, to.0, left as f32, WIDTH as f32)
        .min(leaves(from.1, to.1, GROUND.start as f32, GROUND.end as f32))
        .min(1.0);
    (from.0 + t * (to.0 - from.0), from.1 + t * (to.1 - from.1))
}

fn chevrons(frame: &mut Frame, rider: Point, junction: Point, ring: bool) {
    frame.chevron_count = 0;
    for (wing, tip) in chevron_wings(rider, junction, ring) {
        frame.chevrons[frame.chevron_count] = Stroke::new(wing, tip);
        frame.chevron_count += 1;
    }
}

fn chevron_wings(
    rider: Point,
    towards: Point,
    ring: bool,
) -> heapless::Vec<(Point, Point), { CHEVRONS * 2 }> {
    let mut wings = heapless::Vec::new();
    let (dx, dy) = (towards.0 - rider.0, towards.1 - rider.1);
    let length = libm::sqrtf(dx * dx + dy * dy);
    if length < 1.0 {
        return wings;
    }
    let (ux, uy) = (dx / length, dy / length);
    let nearest = (length / 2.0 - CHEVRON_PITCH).max(PUCK_CLEAR + CHEVRON_STROKE + CHEVRON_DEPTH);
    let furthest = length - CHEVRON_STROKE - if ring { JUNCTION_CLEAR } else { 0.0 };
    for k in 0..CHEVRONS {
        let along = nearest + k as f32 * CHEVRON_PITCH;
        if along > furthest {
            break;
        }
        let tip = (rider.0 + ux * along, rider.1 + uy * along);
        for side in [-1.0, 1.0] {
            let wing = (
                tip.0 - ux * CHEVRON_DEPTH - uy * CHEVRON_HALF * side,
                tip.1 - uy * CHEVRON_DEPTH + ux * CHEVRON_HALF * side,
            );
            let _ = wings.push((wing, tip));
        }
    }
    wings
}

fn bands(stroke: &Stroke, half: f32) -> Option<(usize, usize)> {
    let (top, bottom) = stroke.rows(half);
    let low = (top - 1.0).max(GROUND.start as f32);
    let high = (bottom + 1.0).min((GROUND.end - 1) as f32);
    if low > high {
        return None;
    }
    let band = |row: f32| (row as usize - GROUND.start) / BAND_ROWS;
    Some((band(low), band(high)))
}

fn bucket(frame: &mut Frame) {
    let mut at = 0usize;
    for line in [Line::Route, Line::Ridden] {
        let room = line.room();
        let mut counts = [0usize; BAND_COUNT];
        let mut entries = 0;
        let mut kept = 0;
        for stroke in frame.line(line) {
            if let Some((first, last)) = bands(stroke, line.reach()) {
                if at + entries + last - first + 1 > BAND_ENTRIES {
                    break;
                }
                entries += last - first + 1;
                counts[first..=last].iter_mut().for_each(|c| *c += 1);
            }
            kept += 1;
        }
        frame.counts[line as usize] = kept;
        let band_offsets = &mut frame.band_offsets[line as usize];
        for (band, count) in counts.iter().enumerate() {
            band_offsets[band] = at as u16;
            at += count;
        }
        band_offsets[BAND_COUNT] = at as u16;
        let mut next = *band_offsets;
        for i in room.start..room.start + kept {
            if let Some((first, last)) = bands(&frame.strokes[i], line.reach()) {
                for slot in &mut next[first..=last] {
                    frame.items[usize::from(*slot)] = i as u16;
                    *slot += 1;
                }
            }
        }
    }
}

fn disc(row: &mut Row, (cx, cy): (i32, i32), radius: i32, color: u16) {
    paint_disc_over(row, (cx, cy, radius), color, |_, coverage, under| {
        Some(blend(color, under, coverage))
    });
}

pub(super) fn paint(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if frame.show != Show::Map || !GROUND.contains(&y) {
        return;
    }
    frame.layer.paint(row, &frame.pairs);
    let mut cover = Cover::new(y);
    let band = (y - GROUND.start) / BAND_ROWS;
    for line in Line::ALL {
        let items = frame.band(line, band);
        for &(half, color) in line.layers() {
            for &item in items {
                cover.stroke(&frame.strokes[usize::from(item)], half);
            }
            cover.paint(row, color);
        }
    }
    let arrow = &frame.arrow;
    let centre = y as f32 + 0.5;
    if arrow.strokes > 0 && (arrow.rows.0..=arrow.rows.1).contains(&centre) {
        for ArrowLayer {
            shaft_half: half,
            head_outline: outline,
            color,
        } in ARROW_LAYERS
        {
            for stroke in &arrow.shaft[..arrow.strokes] {
                cover.stroke(stroke, half);
            }
            cover.triangle(&arrow.head, outline);
            cover.paint(row, color);
        }
    }
    for stroke in &frame.chevrons[..frame.chevron_count] {
        cover.stroke(stroke, CHEVRON_STROKE);
    }
    cover.paint(row, AMBER);
    if let Some(at) = frame.junction_quarter_px {
        for (radius, color) in JUNCTION_RINGS_QUARTER_PX {
            disc(row, at, radius, color);
        }
    }
    if let Some(Puck {
        centre_quarter_px: at,
        arrow: tint,
    }) = frame.puck
    {
        for (radius, color) in PUCK_RINGS_QUARTER_PX {
            disc(row, at, radius, color);
        }
        let (left, top) = puck_top_left(at);
        PUCK.paint(row, left, top, tint);
    }
}

fn map_left(radar: bool) -> usize {
    if radar { super::BODY.left } else { 0 }
}

fn puck_top_left((cx, cy): (i32, i32)) -> (usize, usize) {
    let half = i32::from(PUCK.size) * 2;
    (((cx - half) / 4) as usize, ((cy - half) / 4) as usize)
}

#[cfg(test)]
pub(super) fn pixel(frame: &Frame, x: usize, y: usize, under: u16) -> u16 {
    if frame.show != Show::Map || !GROUND.contains(&y) {
        return under;
    }
    let over = |color: u16, paint: u16, coverage: u8| match coverage {
        0 => color,
        c => blend(paint, color, c),
    };
    let mut color = frame.pairs[usize::from(frame.layer.nibble(x, y))][0];
    for line in Line::ALL {
        for &(half, paint) in line.layers() {
            let c = frame
                .line(line)
                .iter()
                .map(|s| raster::coverage(s, half, x, y))
                .max()
                .unwrap_or(0);
            color = over(color, paint, c);
        }
    }
    let arrow = &frame.arrow;
    if arrow.strokes > 0 {
        for ArrowLayer {
            shaft_half: half,
            head_outline: outline,
            color: paint,
        } in ARROW_LAYERS
        {
            let c = arrow.shaft[..arrow.strokes]
                .iter()
                .map(|s| raster::coverage(s, half, x, y))
                .chain([arrow.head.coverage(outline, x, y)])
                .max()
                .unwrap_or(0);
            color = over(color, paint, c);
        }
    }
    let c = frame.chevrons[..frame.chevron_count]
        .iter()
        .map(|s| raster::coverage(s, CHEVRON_STROKE, x, y))
        .max()
        .unwrap_or(0);
    color = over(color, AMBER, c);
    let rings = |color: u16, (cx, cy): (i32, i32), rings: &[(i32, u16)]| {
        rings.iter().fold(color, |color, &(radius, paint)| {
            over(
                color,
                paint,
                crate::screens::chrome::disc(x, y, cx, cy, radius),
            )
        })
    };
    if let Some(at) = frame.junction_quarter_px {
        color = rings(color, at, &JUNCTION_RINGS_QUARTER_PX);
    }
    if let Some(Puck {
        centre_quarter_px: at,
        arrow: tint,
    }) = frame.puck
    {
        color = rings(color, at, &PUCK_RINGS_QUARTER_PX);
        let (left, top) = puck_top_left(at);
        color = PUCK.pixel(x, y, left, top, tint, color).unwrap_or(color);
    }
    color
}

pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let edge = map_left(view.radar_paired);
    let (width, cx) = (WIDTH - edge, (edge + WIDTH) / 2);
    let say = |canvas: &mut Canvas, text: &[u8]| {
        canvas.fit(&[&BODY_FACE], text, width, cx, MID_Y, Align::Center, WHITE);
    };
    let frame = match view.map() {
        Some(frame) if frame.show == Show::Map => frame,
        Some(frame) if frame.show == Show::NoGps => {
            say(&mut canvas, b"NO GPS YET");
            canvas.caption(b"MAP STARTS WITH GPS", width, cx, MID_Y + 24, Align::Center);
            return canvas;
        }
        _ => {
            say(&mut canvas, b"NO TRACK");
            return canvas;
        }
    };
    if !frame.has_track {
        let below = usize::try_from(frame.last_pose.rider.1).unwrap_or(MID_Y) - 36;
        canvas.caption(b"DRAWS AS YOU RIDE", width, cx, below, Align::Center);
    }
    let (bar, label) = frame.last_pose.reach.bar();
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: BAR_LEFT,
            top: BAR_TOP,
            right: BAR_LEFT + bar,
            bottom: BAR_TOP + BAR_THICKNESS,
        },
        radius: 1,
        color: SECONDARY,
    });
    let left = BAR_LEFT + bar + BAR_GAP;
    canvas.caption(label, DISTANCE_RIGHT - left, left, FOOT_BASE, Align::Left);
    let mut distance = Text::<12>::new();
    let mm = frame
        .left_m
        .map_or(view.totals.distance_mm(), |m| u64::from(m) * 1000);
    kilometres(&mut distance, mm);
    canvas.fit(
        &[&FOOT_20],
        distance.as_bytes(),
        DISTANCE_RIGHT - left,
        DISTANCE_RIGHT,
        FOOT_BASE,
        Align::Right,
        WHITE,
    );
    canvas.caption(
        b"KM",
        UNIT_RIGHT - DISTANCE_RIGHT,
        UNIT_RIGHT,
        FOOT_BASE,
        Align::Right,
    );
    canvas
}

#[cfg(test)]
mod tests;
