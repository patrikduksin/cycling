mod arrow;
mod pose;
mod raster;
mod roads;

use super::kilometres;
use crate::ride::breadcrumb::Crumb;
use crate::ride::course::follow::Placement;
use crate::ride::course::nav::{Glyph, Guidance, Sign, Spot};
use crate::ride::course::route_leg;
use crate::ride::geo::{Frame as RideFrame, distance};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    self, ACCENT, AMBER, Align, BLACK, CASING, Look, RIDDEN, Rect, Row, SECONDARY, Shown, WHITE,
    paint_disc_over, rgb,
};
use crate::screens::icons::StaticIcon;
use crate::screens::limits;
use crate::screens::look;
use crate::screens::scaled::{Scaled, Track as Split};
use crate::screens::workout::{Text, View};
use arrow::{ARROW_OUTLINE_PX, ARROW_SHAFT_HALF_PX, ARROW_STROKES, arrow_shape};
use firmware_shell::rendering::font::{FaceRole, SPILL};
use pose::{Pose, Projection};
pub use pose::{Reach, Zoom};
use raster::{Bar, Class, Cover, Layer, Stroke, Triangle};
use roads::{Roads, Source};

type Point = (f32, f32);

/// The map page on one panel: the ground the map covers, without a radar
/// and beside one's strip, and the foot below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub free: Ground,
    pub beside_radar: Ground,
    pub foot: Foot,
}

impl Regions {
    pub const fn ground(&self, radar: bool) -> &Ground {
        if radar {
            &self.beside_radar
        } else {
            &self.free
        }
    }
}

/// Where the map draws in a frame, with or without a radar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ground {
    /// The rows the map covers, across the panel: lines and roads are kept
    /// over all of it, under a radar's strip too.
    pub drawn: Rect,
    /// What of it shows: right of a radar's strip, or all of it.
    pub shown: Rect,
    /// The rider's arrow, inside its rings.
    pub puck: StaticIcon,
    /// Ring radii in quarter pixels, outermost first: the rider's, then a
    /// junction's.
    pub puck_rings: [i32; 2],
    pub junction_rings: [i32; 4],
}

impl Ground {
    fn puck_clear(&self) -> f32 {
        self.puck_rings[0] as f32 / 4.0
    }

    fn junction_clear(&self) -> f32 {
        self.junction_rings[0] as f32 / 4.0
    }
}

/// The foot under the map: the scale bar and its label at the left, the
/// distance and its unit at the right, on one baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Foot {
    /// Across the panel: beside a radar it runs under the strip, which
    /// ends above it.
    pub rect: Rect,
    /// The longest scale bar.
    pub bar: Rect,
    /// Between the bar and its label.
    pub gap: usize,
    pub baseline: usize,
    pub distance_right: usize,
    pub unit_right: usize,
}

const FOOT_ROWS: u16 = 30;
const GROUND_ROWS: usize = ground_rows(limits::PANELS);
const BAR_MAX_PX: u16 = 90;
const PUCK_RINGS_QUARTER_PX: [usize; 2] = [58, 50];
const PUCK_RING_COLOURS: [u16; 2] = [BLACK, WHITE];
const JUNCTION_RINGS_QUARTER_PX: [usize; 4] = [48, 40, 22, 14];
const JUNCTION_RING_COLOURS: [u16; 4] = [GROUND_COLOR, AMBER, BLACK, WHITE];

/// The most rows the map's ground has on any of `panels`.
const fn ground_rows(panels: &[(usize, usize)]) -> usize {
    crate::screens::scaled::largest!(panels, |at| ground_and_foot(&at)[0].height())
}

const fn ground_and_foot(at: &Scaled) -> [Rect; 2] {
    use Split::{Fr, Unit};
    let below_bar = Rect {
        top: chrome::regions(at).status_bar.bottom,
        ..at.screen()
    };
    below_bar.rows([Fr(1), Unit(FOOT_ROWS)], Unit(0), at)
}

pub const fn regions(at: &Scaled) -> Regions {
    use Split::{Fr, Unit};
    let [drawn, foot] = ground_and_foot(at);
    assert!(
        drawn.height() <= GROUND_ROWS,
        "the map's ground has more rows than its frame holds"
    );
    let [_, inner, _] = foot.cols([Unit(12), Fr(1), Unit(12)], Unit(0), at);
    let [_, bar, _] = foot.rows([Unit(13), Unit(3), Fr(1)], Unit(0), at);
    let bar = Rect {
        right: inner.left + at.unit(BAR_MAX_PX as usize),
        ..bar.with_columns(inner)
    };
    let body = super::frame(at).body;
    Regions {
        free: ground(at, drawn, drawn),
        beside_radar: ground(at, drawn, drawn.with_columns(body)),
        foot: Foot {
            rect: foot,
            bar,
            gap: at.unit(6),
            baseline: foot.top + at.unit(19),
            distance_right: inner.right - at.unit(KM_ROOM),
            unit_right: inner.right,
        },
    }
}

/// The room for "KM" and the gap before it.
const KM_ROOM: usize = 25;

const fn ground(at: &Scaled, drawn: Rect, shown: Rect) -> Ground {
    Ground {
        drawn,
        shown,
        puck: StaticIcon(at.art.puck),
        puck_rings: rings(at, PUCK_RINGS_QUARTER_PX),
        junction_rings: rings(at, JUNCTION_RINGS_QUARTER_PX),
    }
}

const fn rings<const N: usize>(at: &Scaled, quarter_px: [usize; N]) -> [i32; N] {
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = at.unit(quarter_px[i]) as i32;
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Track<'a> {
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
    /// The course is placed and following may still start: marks on its
    /// line show which way it runs.
    pub joinable: bool,
    /// The leg of the course held before following, by its end's index.
    pub leg: Option<usize>,
    pub guidance: Option<Guidance>,
    /// Metres left to the course's end, while following it.
    pub left_m: Option<u32>,
    pub reach: Reach,
    /// A radar's strip takes the screen's left edge.
    pub radar: bool,
    /// The light look is in force: the roads' colours are mapped once, as
    /// the frame is filled.
    pub light: bool,
}

/// What a frame was filled from: refill it only when this changes, about
/// once a second, not on every radar redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    crumbs: usize,
    newest: u32,
    here: Option<(u32, u32)>,
    heading: Option<u16>,
    placement: Placement,
    ahead: Option<(usize, Option<(u32, u32)>)>,
    leg: Option<usize>,
    joinable: bool,
    sign: Option<Sign>,
    turn: Option<u32>,
    junction: Option<u32>,
    left_m: Option<u32>,
    reach: Reach,
    radar: bool,
    light: bool,
    roads: u32,
}

impl Track<'_> {
    fn key(&self) -> Key {
        let bits = |(x, y): Point| (x.to_bits(), y.to_bits());
        Key {
            crumbs: self.crumbs.len(),
            newest: self.crumbs.last().map_or(0, |c| c.along.to_bits()),
            here: self.here.map(bits),
            heading: self.heading,
            placement: self.placement,
            ahead: self
                .ahead
                .map(|(segment, matched)| (segment, matched.map(bits))),
            leg: self.leg,
            joinable: self.joinable,
            sign: self.guidance.map(|g| g.sign),
            turn: self.turn_on_course().map(|(spot, _)| spot.along_mm),
            junction: self.junction().map(|j| j.along_mm),
            left_m: self.left_m,
            reach: self.reach,
            radar: self.radar,
            light: self.light,
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
        route_leg::split(self.course, self.ahead, self.junction())
    }

    fn metres_to_end(&self, spot: Spot) -> f32 {
        route_leg::metres_to_end(self.course, spot)
    }

    fn place(&self, centre: Point) -> Option<(usize, Point)> {
        self.split()
            .or_else(|| route_leg::at(self.course, self.heading, centre, self.leg))
    }

    fn metres_to_end_at(&self, (end, at): (usize, Point)) -> f32 {
        let next = self.course[end - 1];
        next.along + distance(at, (next.east, next.north))
    }

    fn ahead_points(
        &self,
        place: (usize, Point),
        reach_m: f32,
    ) -> impl Iterator<Item = (Point, bool)> + '_ {
        let (end, at) = place;
        let turn = self
            .turn_on_course()
            .map(|(spot, _)| self.metres_to_end(spot));
        let before = move |along: f32| turn.is_none_or(|t| along > t);
        let start = (at, self.metres_to_end_at(place));
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
}

pub struct Cache<'a> {
    frame: &'a mut Frame,
    held: Option<Key>,
}

impl<'a> Cache<'a> {
    pub fn new(frame: &'a mut Frame) -> Self {
        Self { frame, held: None }
    }

    /// Fill the frame again, laid out in `map`, if `track` or the roads
    /// changed; whether it did. A cache serves one panel, so `map` is the
    /// same every time.
    pub fn update(&mut self, track: &Track, map: &'static Regions) -> bool {
        let key = Key {
            roads: self.frame.roads.generation(),
            ..track.key()
        };
        let stale = self.held != Some(key);
        if stale {
            fill(self.frame, track, map);
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
        let detail = self.frame.last_pose.reach.detail();
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
const BAND_ROWS: usize = 16;
const BAND_COUNT: usize = GROUND_ROWS.div_ceil(BAND_ROWS);
const STRAY_PX: f32 = 0.35;
const STRAIGHT_RUN: usize = 16;
const RIDDEN_SEGMENTS: usize = 255;
const ROUTE_SEGMENTS: usize = 160;
const STROKES: usize = RIDDEN_SEGMENTS + ROUTE_SEGMENTS;
const BAND_ENTRIES: usize = 2048;
const LOOKAHEAD_M: f32 = 4_000.0;
const ROUTE_REACH: f32 = 1.5;
const ROAD_SEGMENTS: usize = 2_048;
const MINOR_ROAD_HALF_PX: f32 = 2.5;
const MAJOR_ROAD_HALF_PX: f32 = 4.5;
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
const MARKS: usize = 8;
const MARK_STEP_M: f32 = 25.0;
const MARK_PITCH_PX: f32 = 40.0;
const MARK_REACH_PX: f32 = 320.0;
const MARK_DEPTH: f32 = 3.5;
const MARK_HALF: f32 = 3.5;
const MARK_STROKE: f32 = 1.0;
const _: () = assert!(
    BAND_ENTRIES <= u16::MAX as usize,
    "strokes are bucketed as u16"
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    #[default]
    Unfilled,
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
    ground: Option<&'static Ground>,
    last_pose: Pose,
    last_centre: Point,
    layer: Layer<GROUND_ROWS>,
    pairs: [[Shown; 2]; 256],
    roads_drawn: [usize; 2],
    strokes: [Stroke; STROKES],
    counts: [usize; 2],
    band_offsets: [[u16; BAND_COUNT + 1]; 2],
    items: [u16; BAND_ENTRIES],
    arrow: Arrow,
    chevrons: [Stroke; 2 * CHEVRONS],
    chevron_count: usize,
    marks: [Stroke; 2 * MARKS],
    mark_count: usize,
    junction_quarter_px: Option<(i32, i32)>,
    puck: Option<Puck>,
    has_track: bool,
    left_m: Option<u32>,
    roads: Roads,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unfilled,
            ground: None,
            last_pose: Pose::default(),
            last_centre: (0.0, 0.0),
            layer: Layer::new(0),
            pairs: [[Shown(0); 2]; 256],
            roads_drawn: [0; 2],
            strokes: [Stroke::default(); STROKES],
            counts: [0; 2],
            band_offsets: [[0; BAND_COUNT + 1]; 2],
            items: [0; BAND_ENTRIES],
            arrow: Arrow::default(),
            chevrons: [Stroke::default(); 2 * CHEVRONS],
            chevron_count: 0,
            marks: [Stroke::default(); 2 * MARKS],
            mark_count: 0,
            junction_quarter_px: None,
            puck: None,
            has_track: false,
            left_m: None,
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

    fn add(
        &mut self,
        line: Line,
        at: &Projection,
        drawn: &Rect,
        points: impl Iterator<Item = Point>,
    ) {
        let room = line.room();
        let (strokes, count) = (&mut self.strokes, &mut self.counts[line as usize]);
        project(at, drawn, line.reach(), points, |a, b| {
            if *count == room.len() {
                return false;
            }
            strokes[room.start + *count] = Stroke::new(a, b);
            *count += 1;
            true
        });
    }

    fn view(&self, margin: f32) -> Option<[Point; 4]> {
        let drawn = self.ground.filter(|_| self.show == Show::Map)?.drawn;
        let at = self.last_pose.projection(self.last_centre);
        let (wide, high) = (drawn.width() as f32, drawn.height() as f32);
        let (dx, dy) = (wide * margin, high * margin);
        let (left, right) = (drawn.left as f32 - dx, drawn.right as f32 + dx);
        let (top, bottom) = (drawn.top as f32 - dy, drawn.bottom as f32 + dy);
        Some([(left, top), (right, top), (right, bottom), (left, bottom)].map(|p| at.back(p)))
    }

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

fn project(
    at: &Projection,
    drawn: &Rect,
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
    let mut draw = |a: Point, b: Point| !visible(drawn, a, b, reach) || put(a, b);
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

fn visible(drawn: &Rect, a: Point, b: Point, reach: f32) -> bool {
    let (left, right) = (drawn.left as f32 - reach, drawn.right as f32 + reach);
    let (top, bottom) = (drawn.top as f32 - reach, drawn.bottom as f32 + reach);
    a.0.max(b.0) >= left && a.0.min(b.0) <= right && a.1.max(b.1) >= top && a.1.min(b.1) <= bottom
}

fn quarter((x, y): Point) -> (i32, i32) {
    (libm::roundf(4.0 * x) as i32, libm::roundf(4.0 * y) as i32)
}

fn draw_roads(frame: &mut Frame, at: &Projection, drawn: &Rect, centre: Point, look: Look) {
    let Frame {
        roads,
        layer,
        pairs,
        roads_drawn,
        last_pose,
        ..
    } = frame;
    layer.clear(drawn.top);
    *pairs = raster::pairs(
        look.ink(GROUND_COLOR),
        look.ink(MINOR_ROAD),
        look.ink(MAJOR_ROAD),
    );
    *roads_drawn = [0; 2];
    let room = |drawn: &[usize; 2]| drawn[0] + drawn[1] < ROAD_SEGMENTS;
    for road in roads.each(last_pose.reach.detail(), centre) {
        if !room(roads_drawn) {
            break;
        }
        let (class, half) = if road.class.major() {
            (Class::Major, MAJOR_ROAD_HALF_PX)
        } else {
            (Class::Minor, MINOR_ROAD_HALF_PX)
        };
        project(at, drawn, half, road.points(), |a, b| {
            layer.add(&Bar::new(a, b, half), class);
            roads_drawn[class as usize] += 1;
            room(roads_drawn)
        });
    }
}

fn fill(frame: &mut Frame, track: &Track, map: &'static Regions) {
    let ground = map.ground(track.radar);
    frame.ground = Some(ground);
    frame.show = if track.rider_or_last_crumb().is_none() {
        Show::NoGps
    } else {
        Show::Map
    };
    frame.counts = [0; 2];
    frame.arrow.strokes = 0;
    frame.chevron_count = 0;
    frame.mark_count = 0;
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
    let pose = Pose::new(track.reach, ground, heading);
    draw(frame, track, ground, centre, pose);
}

fn draw(frame: &mut Frame, track: &Track, ground: &Ground, centre: Point, pose: Pose) {
    frame.last_pose = pose;
    frame.last_centre = centre;
    let place = track.place(centre);
    let at = pose.projection(centre);
    let drawn = &ground.drawn;
    draw_roads(frame, &at, drawn, centre, look::look(track.light));
    match place {
        Some(place) => {
            let to_junction = track.junction().map_or(0.0, |j| distance(centre, j.at));
            let reach_m = ROUTE_REACH * pose.reach.ahead_m() + to_junction;
            let ahead = track.ahead_points(place, reach_m);
            frame.add(Line::Route, &at, drawn, ahead.map(|(p, _)| p));
            if track.joinable {
                let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
                let from_end_m = track.metres_to_end_at(place);
                let ahead_m = (from_end_m - reach_m)..from_end_m;
                marks(
                    frame,
                    &at,
                    ground,
                    track.course,
                    ahead_m,
                    pose.px_per_m(),
                    rider,
                );
            }
        }
        None => frame.add(Line::Ridden, &at, drawn, track.crumbs_back()),
    }
    frame.has_track = track.crumbs.len() >= 2 || place.is_some();
    if let (Some(place), Some((turn, _))) = (place, track.turn_on_course()) {
        arrow(frame, &at, track, place, at.at(turn.at));
    }
    let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
    if let Some(junction) = track.junction() {
        let junction = at.at(junction.at);
        let reach = ground.junction_clear() + 1.0;
        frame.junction_quarter_px =
            visible(drawn, junction, junction, reach).then(|| quarter(junction));
        let ring = frame.junction_quarter_px.is_some();
        chevrons(
            frame,
            ground,
            rider,
            clipped(rider, junction, &ground.shown),
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
    bucket(frame, drawn);
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
    frame.arrow.head = Triangle::new(shape.head);
    frame.arrow.strokes = 0;
    for pair in shape.shaft.windows(2) {
        frame.arrow.shaft[frame.arrow.strokes] = Stroke::new(pair[0], pair[1]);
        frame.arrow.strokes += 1;
    }
    let reach = ARROW_LAYERS[0].shaft_half + 1.0;
    frame.arrow.rows = shape
        .shaft
        .iter()
        .chain(frame.arrow.head.corners())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(top, bottom), p| {
            (top.min(p.1 - reach), bottom.max(p.1 + reach))
        });
}

fn clipped(from: Point, to: Point, shown: &Rect) -> Point {
    let leaves = |from: f32, to: f32, low: usize, high: usize| match to - from {
        d if d > 0.0 => (high as f32 - from) / d,
        d if d < 0.0 => (low as f32 - from) / d,
        _ => f32::INFINITY,
    };
    let t = leaves(from.0, to.0, shown.left, shown.right)
        .min(leaves(from.1, to.1, shown.top, shown.bottom))
        .min(1.0);
    (from.0 + t * (to.0 - from.0), from.1 + t * (to.1 - from.1))
}

fn chevrons(frame: &mut Frame, ground: &Ground, rider: Point, junction: Point, ring: bool) {
    frame.chevron_count = 0;
    for (wing, tip) in chevron_wings(ground, rider, junction, ring) {
        frame.chevrons[frame.chevron_count] = Stroke::new(wing, tip);
        frame.chevron_count += 1;
    }
}

fn chevron_wings(
    ground: &Ground,
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
    let nearest =
        (length / 2.0 - CHEVRON_PITCH).max(ground.puck_clear() + CHEVRON_STROKE + CHEVRON_DEPTH);
    let furthest = length - CHEVRON_STROKE - if ring { ground.junction_clear() } else { 0.0 };
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

fn marks(
    frame: &mut Frame,
    at: &Projection,
    ground: &Ground,
    course: &[Crumb],
    ahead_m: core::ops::Range<f32>,
    px_per_m: f32,
    rider: Point,
) {
    let total = course.last().map_or(0.0, |c| c.along);
    let (pitch, nudge) = (mark_pitch_m(px_per_m), 1.0 / px_per_m);
    let furthest = ahead_m.start.max(ahead_m.end - MARK_REACH_PX / px_per_m);
    let first = libm::ceilf(furthest / pitch).max(0.0) as u32;
    let last = libm::floorf(ahead_m.end.min(total - nudge) / pitch);
    for k in first..=last.max(0.0) as u32 {
        let along = k as f32 * pitch;
        let tip = at.at(point_at_metres_to_end(course, along));
        let behind = at.at(point_at_metres_to_end(course, along + nudge));
        let (dx, dy) = (tip.0 - behind.0, tip.1 - behind.1);
        let length = libm::sqrtf(dx * dx + dy * dy);
        let clear = distance(tip, rider) > ground.puck_clear() + MARK_HALF;
        if length <= 0.0 || !clear || !visible(&ground.drawn, tip, tip, MARK_HALF) {
            continue;
        }
        if frame.mark_count + 2 > frame.marks.len() {
            return;
        }
        let (ux, uy) = (dx / length, dy / length);
        let tip = (tip.0 + ux * MARK_DEPTH / 2.0, tip.1 + uy * MARK_DEPTH / 2.0);
        for side in [-1.0, 1.0] {
            let wing = (
                tip.0 - ux * MARK_DEPTH - uy * MARK_HALF * side,
                tip.1 - uy * MARK_DEPTH + ux * MARK_HALF * side,
            );
            frame.marks[frame.mark_count] = Stroke::new(wing, tip);
            frame.mark_count += 1;
        }
    }
}

/// Marks sit every `MARK_STEP_M` along the course, or every second, fourth
/// and so on of those, the fewest at least `MARK_PITCH_PX` apart: as the
/// scale eases they stay put, and thin out or fill in only where it doubles.
fn mark_pitch_m(px_per_m: f32) -> f32 {
    let mut pitch = MARK_STEP_M;
    while pitch * px_per_m < MARK_PITCH_PX {
        pitch *= 2.0;
    }
    pitch
}

fn point_at_metres_to_end(course: &[Crumb], along: f32) -> Point {
    let end = course
        .partition_point(|c| c.along < along)
        .clamp(1, course.len() - 1);
    let (a, b) = (course[end - 1], course[end]);
    let t = match b.along > a.along {
        true => ((along - a.along) / (b.along - a.along)).clamp(0.0, 1.0),
        false => 0.0,
    };
    (
        a.east + t * (b.east - a.east),
        a.north + t * (b.north - a.north),
    )
}

fn bands(drawn: &Rect, stroke: &Stroke, half: f32) -> Option<(usize, usize)> {
    let (top, bottom) = stroke.rows(half);
    let low = (top - 1.0).max(drawn.top as f32);
    let high = (bottom + 1.0).min((drawn.bottom - 1) as f32);
    if low > high {
        return None;
    }
    let band = |row: f32| (row as usize - drawn.top) / BAND_ROWS;
    Some((band(low), band(high)))
}

fn bucket(frame: &mut Frame, drawn: &Rect) {
    let mut at = 0usize;
    for line in [Line::Route, Line::Ridden] {
        let room = line.room();
        let mut counts = [0usize; BAND_COUNT];
        let mut entries = 0;
        let mut kept = 0;
        for stroke in frame.line(line) {
            if let Some((first, last)) = bands(drawn, stroke, line.reach()) {
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
            if let Some((first, last)) = bands(drawn, &frame.strokes[i], line.reach()) {
                for slot in &mut next[first..=last] {
                    frame.items[usize::from(*slot)] = i as u16;
                    *slot += 1;
                }
            }
        }
    }
}

fn ground_on(frame: &Frame, y: usize) -> Option<&'static Ground> {
    frame.ground.filter(|ground| {
        frame.show == Show::Map && (ground.drawn.top..ground.drawn.bottom).contains(&y)
    })
}

pub(super) fn paint(frame: &Frame, row: &mut Row) {
    let y = row.y();
    let Some(ground) = ground_on(frame, y) else {
        return;
    };
    frame.layer.paint(row, &frame.pairs);
    let mut cover = Cover::new(y);
    let band = (y - ground.drawn.top) / BAND_ROWS;
    for line in Line::ALL {
        let items = frame.band(line, band);
        for &(half, color) in line.layers() {
            for &item in items {
                cover.stroke(&frame.strokes[usize::from(item)], half);
            }
            cover.paint(row, color);
        }
    }
    for stroke in &frame.marks[..frame.mark_count] {
        cover.stroke(stroke, MARK_STROKE);
    }
    cover.paint(row, CASING);
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
        for (radius, color) in ground.junction_rings.into_iter().zip(JUNCTION_RING_COLOURS) {
            paint_disc_over(row, (at.0, at.1, radius), color);
        }
    }
    if let Some(Puck {
        centre_quarter_px: at,
        arrow: tint,
    }) = frame.puck
    {
        for (radius, color) in ground.puck_rings.into_iter().zip(PUCK_RING_COLOURS) {
            paint_disc_over(row, (at.0, at.1, radius), color);
        }
        let (left, top) = puck_top_left(ground, at);
        ground.puck.paint(row, left, top, tint);
    }
}

fn puck_top_left(ground: &Ground, (cx, cy): (i32, i32)) -> (usize, usize) {
    let half = i32::from(ground.puck.size) * 2;
    (((cx - half) / 4) as usize, ((cy - half) / 4) as usize)
}

#[cfg(test)]
pub(super) fn pixel(frame: &Frame, look: Look, x: usize, y: usize, under: Shown) -> Shown {
    use crate::screens::chrome::blend;
    let Some(ground) = ground_on(frame, y) else {
        return under;
    };
    let over = |color: Shown, paint: u16, coverage: u8| match coverage {
        0 => color,
        c => blend(look.ink(paint), color, c),
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
    let c = frame.marks[..frame.mark_count]
        .iter()
        .map(|s| raster::coverage(s, MARK_STROKE, x, y))
        .max()
        .unwrap_or(0);
    color = over(color, CASING, c);
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
    let rings = |color: Shown, (cx, cy): (i32, i32), radii: &[i32], paints: &[u16]| {
        radii
            .iter()
            .zip(paints)
            .fold(color, |color, (&radius, &paint)| {
                over(
                    color,
                    paint,
                    crate::screens::chrome::disc(x, y, cx, cy, radius),
                )
            })
    };
    if let Some(at) = frame.junction_quarter_px {
        color = rings(color, at, &ground.junction_rings, &JUNCTION_RING_COLOURS);
    }
    if let Some(Puck {
        centre_quarter_px: at,
        arrow: tint,
    }) = frame.puck
    {
        color = rings(color, at, &ground.puck_rings, &PUCK_RING_COLOURS);
        let (left, top) = puck_top_left(ground, at);
        color = ground
            .puck
            .pixel(x, y, left, top, look.ink(tint), color)
            .unwrap_or(color);
    }
    color
}

pub(super) fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, map) = (&layout.scaled, &layout.map);
    let shown = map.ground(view.radar_paired).shown;
    let (width, cx) = (shown.width(), (shown.left + shown.right) / 2);
    let middle = (shown.top + shown.bottom) / 2;
    let mut canvas = Canvas::default();
    let say = |canvas: &mut Canvas, text: &[u8]| {
        let face = at.face(FaceRole::Body);
        canvas.fit(&[face], text, width, cx, middle, Align::Center, WHITE);
    };
    let frame = match view.map() {
        Some(frame) if frame.show == Show::Map => frame,
        Some(frame) if frame.show == Show::NoGps => {
            say(&mut canvas, b"NO GPS YET");
            let below = middle + at.unit(24);
            let words = b"MAP STARTS WITH GPS";
            canvas.caption(at, words, width, cx, below, Align::Center);
            return canvas;
        }
        _ => {
            say(&mut canvas, b"NO TRACK");
            return canvas;
        }
    };
    if !frame.has_track {
        let rider = usize::try_from(frame.last_pose.rider.1).unwrap_or(middle);
        let above = rider - at.unit(36);
        let words = b"DRAWS AS YOU RIDE";
        canvas.caption(at, words, width, cx, above, Align::Center);
    }
    let foot = &map.foot;
    let (baseline, right) = (foot.baseline, foot.distance_right);
    let mut distance = Text::<12>::new();
    let mm = frame
        .left_m
        .map_or(view.totals.distance_mm(), |m| u64::from(m) * 1000);
    kilometres(&mut distance, mm);
    let figures = at.face(FaceRole::Foot20);
    let across = right - foot.bar.left;
    let distance_left = right - figures.width(distance.as_bytes()).min(across);
    let caption = at.face(FaceRole::Caption);
    let fits = |bar, label: &[u8]| {
        let label_right = foot.bar.left + bar + foot.gap + caption.width(label) + SPILL;
        bar <= foot.bar.width() && label_right + foot.gap <= distance_left
    };
    if let Some((bar, label)) = frame.last_pose.bar(fits) {
        canvas.shape(Shape::Bar {
            rect: Rect {
                right: foot.bar.left + bar,
                ..foot.bar
            },
            radius: at.unit(1),
            color: SECONDARY,
        });
        let left = foot.bar.left + bar + foot.gap;
        let width = distance_left - foot.gap - left;
        canvas.caption(at, label, width, left, baseline, Align::Left);
    }
    canvas.fit(
        &[figures],
        distance.as_bytes(),
        across,
        right,
        baseline,
        Align::Right,
        WHITE,
    );
    let unit = foot.unit_right;
    canvas.caption(at, b"KM", unit - right, unit, baseline, Align::Right);
    canvas
}

#[cfg(test)]
mod tests;
