//! The map page: the course ahead heading up, with the rider low, the turn
//! to make and, off course, the way back to where the rider left it.
//!
//! The map fills the screen between the status bar and a black footer.
//! `pose` chooses the zoom step and the rider's place each fix. `fill`
//! projects through it, once per fix, the ridden line (the course behind
//! the rider, or the ride's own track when no course is followed) and the
//! course ahead, keeping only what reaches the screen, nearest the rider
//! first, and buckets the strokes by `BAND` rows. A row then paints the
//! ground, the ridden line, the route's casing and the route, the turn
//! arrow, the chevrons back, the missed junction and the rider's puck, each
//! colour as row spans (`raster`). The footer holds the scale and how far
//! is left; `prepare` lays it out as on other canvas pages.
mod pose;
mod raster;

use super::kilometres;
use crate::ride::course::nav::{Glyph, Guidance, Sign, Spot};
use crate::ride::homing::{Crumb, distance};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BLACK, BODY as BODY_FACE, HEIGHT, Rect, Row, SECONDARY, WHITE, WIDTH,
    blend, paint_disc_over, rgb,
};
use crate::screens::icons::PUCK;
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::FOOT_20;
pub use pose::GROUND;
use pose::{Aim, Keepout, Pose, Projection, choose_pose};
use raster::{Cover, Stroke, Triangle};

type Point = (f32, f32);

/// What the page draws: the ride, the course it follows and the rider.
#[derive(Clone, Copy, Debug, Default)]
pub struct Track<'a> {
    /// The device has memory set aside for the breadcrumb.
    pub available: bool,
    /// Kept points, metres east and north of the start, oldest first.
    pub crumbs: &'a [Crumb],
    /// The rider, while the fix is fresh.
    pub here: Option<Point>,
    /// The way the rider is riding, in `TURN`ths clockwise from north.
    pub heading: Option<u16>,
    /// The loaded course, end first, each point's `along` its metres to the
    /// end; empty without one.
    pub course: &'a [Crumb],
    /// Following the course: what is ahead is its first points up to this
    /// index, from its end, and then the matched point if any.
    pub ahead: Option<(usize, Option<Point>)>,
    pub guidance: Option<Guidance>,
    /// Metres left to the course's end, while following it.
    pub left_m: Option<u32>,
    /// The badge's box, which the route, the turn and the rider keep clear of.
    pub badge: Option<Rect>,
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
    course: usize,
    course_m: u32,
    ahead: Option<(usize, Option<(u32, u32)>)>,
    sign: Option<Sign>,
    turn: Option<u32>,
    junction: Option<u32>,
    left_m: Option<u32>,
    badge: Option<Rect>,
}

impl Track<'_> {
    fn key(&self) -> Key {
        let bits = |(x, y): Point| (x.to_bits(), y.to_bits());
        Key {
            available: self.available,
            crumbs: self.crumbs.len(),
            newest: self.crumbs.last().map_or(0, |c| c.along.to_bits()),
            here: self.here.map(bits),
            heading: self.heading,
            course: self.course.len(),
            course_m: self.course.last().map_or(0, |c| c.along.to_bits()),
            ahead: self
                .ahead
                .map(|(segment, matched)| (segment, matched.map(bits))),
            sign: self.guidance.map(|g| g.sign),
            turn: self.turn().map(|(spot, _)| spot.along_mm),
            junction: self.junction().map(|j| j.along_mm),
            left_m: self.left_m,
            badge: self.badge,
        }
    }

    /// Where the map centres: the rider, else where the track last was.
    fn centre(&self) -> Option<Point> {
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

    /// The next turn to show: none while off course.
    fn turn(&self) -> Option<(Spot, Glyph)> {
        self.guidance
            .and_then(|g| g.turn)
            .filter(|_| self.following() && self.junction().is_none())
    }

    /// Where the route ahead starts: the index of the first course point
    /// behind it (end first) and the point itself; the junction while off
    /// course.
    fn split(&self) -> Option<(usize, Point)> {
        let (segment, matched) = self.ahead.filter(|_| self.following())?;
        if let Some(junction) = self.junction() {
            return Some((self.index_of(junction), junction.at));
        }
        let end = segment.clamp(1, self.course.len() - 1);
        let c = self.course[end];
        Some((end, matched.unwrap_or((c.east, c.north))))
    }

    /// A spot's metres to the course's end.
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

    /// The course ahead from `split`, in order, each point with whether it
    /// comes before the next turn.
    fn ahead_points(&self, (end, at): (usize, Point)) -> impl Iterator<Item = (Point, bool)> + '_ {
        let turn = self.turn().map(|(spot, _)| self.metres_to_end(spot));
        let near = self.course[end - 1].along;
        core::iter::once((at, turn.is_some())).chain(
            self.course[..end]
                .iter()
                .rev()
                .take_while(move |c| near - c.along <= AHEAD_M)
                .map(move |c| ((c.east, c.north), turn.is_some_and(|t| c.along > t))),
        )
    }

    /// The course behind `split`, nearest first.
    fn behind_points(&self, (end, at): (usize, Point)) -> impl Iterator<Item = Point> + '_ {
        core::iter::once(at).chain(self.course[end..].iter().map(|c| (c.east, c.north)))
    }

    /// The ride's own track, newest first from the rider.
    fn crumbs_back(&self) -> impl Iterator<Item = Point> + '_ {
        self.here
            .into_iter()
            .chain(self.crumbs.iter().rev().map(|c| (c.east, c.north)))
    }

    /// The course one way from its point `nearest`: towards its end, or
    /// with `up` towards its start.
    fn course_from(&self, nearest: usize, up: bool) -> impl Iterator<Item = Point> + '_ {
        let (down, rest) = self.course.split_at(nearest + 1);
        let to_end = down.iter().rev().filter(move |_| !up);
        let to_start = down[nearest..].iter().chain(rest).filter(move |_| up);
        to_end.chain(to_start).map(|c| (c.east, c.north))
    }

    fn nearest(&self, centre: Point) -> usize {
        let gap = |c: &Crumb| distance((c.east, c.north), centre);
        (0..self.course.len())
            .min_by(|&a, &b| gap(&self.course[a]).total_cmp(&gap(&self.course[b])))
            .unwrap_or(0)
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

    pub fn update(&mut self, track: &Track) {
        let key = track.key();
        if self.held != Some(key) {
            fill(self.frame, track);
            self.held = Some(key);
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
        self.frame.pose.hash(state);
    }
}

/// The map's ground and the course's colours (docs/design/course-navigation).
const GROUND_COLOR: u16 = rgb(0x10, 0x10, 0x12);
const RIDDEN: u16 = rgb(0x1f, 0x4a, 0x78);
const CASING: u16 = rgb(0x04, 0x26, 0x4c);
/// Rows per bucket, from the top of the ground.
const BAND: usize = 16;
const BANDS: usize = (GROUND.end - GROUND.start).div_ceil(BAND);
/// Points closer than this on screen to the last kept are dropped.
const MIN_STEP: f32 = 2.0;
/// Segments of each line kept after projection, and band entries in all.
const RIDDEN_SEGMENTS: usize = 255;
const ROUTE_SEGMENTS: usize = 160;
const STROKES: usize = RIDDEN_SEGMENTS + ROUTE_SEGMENTS;
const ITEMS: usize = 2048;
/// The route ahead is walked this far along, past what the widest step shows.
const AHEAD_M: f32 = 4_000.0;
/// The turn arrow: `SHAFT` px along the route from `BEFORE` px before the
/// turn (or a quarter of the way back to the rider, when that is less),
/// then a head `HEAD_LENGTH` long and twice `HEAD_HALF` wide.
const SHAFT: f32 = 26.0;
const BEFORE: f32 = 10.0;
const HEAD_LENGTH: f32 = 10.5;
const HEAD_HALF: f32 = 7.9;
const ARROW_STROKES: usize = 24;
/// The arrow's casing, then its white: shaft half width and head outline.
const ARROW_LAYERS: [(f32, f32, u16); 2] = [(4.25, 1.75, CASING), (2.5, 0.5, WHITE)];
/// Chevrons back to the junction: pitch, depth and half span, in px.
const CHEVRONS: usize = 3;
const CHEVRON_PITCH: f32 = 15.0;
const CHEVRON_DEPTH: f32 = 6.0;
const CHEVRON_HALF: f32 = 5.0;
const CHEVRON_STROKE: f32 = 1.75;
/// The puck's black ring and white disc, and the junction's rings, in
/// quarter pixels.
const PUCK_RINGS: [(i32, u16); 2] = [(58, BLACK), (50, WHITE)];
const PUCK_CLEAR: f32 = 14.5;
const JUNCTION_CLEAR: f32 = 12.0;
const JUNCTION_RINGS: [(i32, u16); 4] = [(48, GROUND_COLOR), (40, AMBER), (22, BLACK), (14, WHITE)];
const _: () = assert!(ITEMS <= u16::MAX as usize, "strokes are bucketed as u16");

/// The footer: the scale bar and its length, and how far is left.
const FOOT_BASE: usize = 309;
const BAR_LEFT: usize = 12;
const BAR_TOP: usize = 303;
const BAR_THICKNESS: usize = 3;
const BAR_GAP: usize = 6;
const DISTANCE_RIGHT: usize = 203;
const UNIT_RIGHT: usize = 228;
const CX: usize = WIDTH / 2;
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

/// The lines bucketed by band, in paint order, each with the widths and
/// colours it paints in turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    /// The course behind the rider, or the ride's own track.
    Ridden,
    /// The course ahead: a casing, then the route over it.
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

/// The turn arrow on the route: its shaft and its head.
#[derive(Clone, Copy, Debug, Default)]
struct Arrow {
    shaft: [Stroke; ARROW_STROKES],
    strokes: usize,
    head: Triangle,
}

/// The map in screen space: refilled when the track's key changes,
/// painted every frame until then. Composition places it in PSRAM,
/// zeroed, so all zero bytes are a valid `Frame`: nothing shown yet.
#[derive(Clone)]
pub struct Frame {
    show: Show,
    /// The pose of the last fill, once `posed`.
    pose: Pose,
    posed: bool,
    strokes: [Stroke; STROKES],
    counts: [usize; 2],
    /// Per line, where each band's strokes start in `items`.
    starts: [[u16; BANDS + 1]; 2],
    items: [u16; ITEMS],
    arrow: Arrow,
    chevrons: [Stroke; 2 * CHEVRONS],
    chevron_count: usize,
    /// The junction's centre while off course, in quarter pixels.
    junction: Option<(i32, i32)>,
    /// The rider's puck, in quarter pixels, and its arrow's colour.
    puck: Option<((i32, i32), u16)>,
    has_track: bool,
    left_m: Option<u32>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unavailable,
            pose: Pose::default(),
            posed: false,
            strokes: [Stroke::default(); STROKES],
            counts: [0; 2],
            starts: [[0; BANDS + 1]; 2],
            items: [0; ITEMS],
            arrow: Arrow::default(),
            chevrons: [Stroke::default(); 2 * CHEVRONS],
            chevron_count: 0,
            junction: None,
            puck: None,
            has_track: false,
            left_m: None,
        }
    }
}

impl Frame {
    fn line(&self, line: Line) -> &[Stroke] {
        let room = line.room();
        &self.strokes[room.start..room.start + self.counts[line as usize]]
    }

    fn band(&self, line: Line, band: usize) -> &[u16] {
        let starts = &self.starts[line as usize];
        &self.items[usize::from(starts[band])..usize::from(starts[band + 1])]
    }

    /// Project `points` as one polyline of `line`, dropping points closer
    /// than `MIN_STEP` and segments off the screen, until its room is full.
    fn add(&mut self, line: Line, at: &Projection, points: impl Iterator<Item = Point>) {
        let room = line.room();
        let reach = line.reach() + 1.0;
        let mut points = points.map(|p| at.at(p)).peekable();
        let Some(mut anchor) = points.next() else {
            return;
        };
        while let Some(p) = points.next() {
            let (dx, dy) = (p.0 - anchor.0, p.1 - anchor.1);
            if dx * dx + dy * dy < MIN_STEP * MIN_STEP && points.peek().is_some() {
                continue;
            }
            if visible(anchor, p, reach) {
                let count = &mut self.counts[line as usize];
                if *count == room.len() {
                    return;
                }
                self.strokes[room.start + *count] = Stroke::new(anchor, p);
                *count += 1;
            }
            anchor = p;
        }
    }
}

/// Whether a segment can show on the ground at all.
fn visible(a: Point, b: Point, reach: f32) -> bool {
    let (left, right) = (-reach, WIDTH as f32 + reach);
    let (top, bottom) = (GROUND.start as f32 - reach, GROUND.end as f32 + reach);
    a.0.max(b.0) >= left && a.0.min(b.0) <= right && a.1.max(b.1) >= top && a.1.min(b.1) <= bottom
}

fn quarter((x, y): Point) -> (i32, i32) {
    (libm::roundf(4.0 * x) as i32, libm::roundf(4.0 * y) as i32)
}

/// Project the track through this fix's pose and bucket it.
fn fill(frame: &mut Frame, track: &Track) {
    frame.show = if !track.available {
        Show::Unavailable
    } else if track.centre().is_none() {
        Show::NoGps
    } else {
        Show::Map
    };
    frame.counts = [0; 2];
    frame.arrow.strokes = 0;
    frame.chevron_count = 0;
    frame.junction = None;
    frame.puck = None;
    frame.left_m = track.left_m.filter(|_| track.following());
    let Some(centre) = track.centre().filter(|_| frame.show == Show::Map) else {
        frame.posed = false;
        frame.starts = [[0; BANDS + 1]; 2];
        return;
    };
    let split = track.split();
    let aim = Aim {
        here: centre,
        heading: track.heading.unwrap_or(frame.pose.heading),
        turn: track.turn().map(|(spot, glyph)| (spot.at, glyph)),
        junction: track.junction().map(|j| j.at),
    };
    let keepout = track.badge.map(|rect| Keepout {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    });
    let route = |visit: &mut dyn FnMut(Point, bool)| {
        if let Some(split) = split {
            track
                .ahead_points(split)
                .for_each(|(p, before)| visit(p, before));
        }
    };
    let pose = choose_pose(frame.posed.then_some(frame.pose), &aim, route, keepout);
    frame.pose = pose;
    frame.posed = true;
    let at = pose.projection(centre);
    match split {
        Some(split) => {
            frame.add(Line::Ridden, &at, track.behind_points(split));
            frame.add(Line::Route, &at, track.ahead_points(split).map(|(p, _)| p));
        }
        None => {
            frame.add(Line::Ridden, &at, track.crumbs_back());
            if track.course.len() >= 2 {
                let nearest = track.nearest(centre);
                frame.add(Line::Route, &at, track.course_from(nearest, false));
                frame.add(Line::Route, &at, track.course_from(nearest, true));
            }
        }
    }
    frame.has_track = track.crumbs.len() >= 2 || split.is_some();
    if let (Some(split), Some((turn, _))) = (split, track.turn()) {
        arrow(frame, &at, || track.ahead_points(split), at.at(turn.at));
    }
    let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
    if let Some(junction) = track.junction() {
        let junction = at.at(junction.at);
        chevrons(frame, rider, junction);
        frame.junction = Some(quarter(junction));
    }
    let tint = match (track.here, track.junction()) {
        (None, _) => SECONDARY,
        (Some(_), Some(_)) => AMBER,
        (Some(_), None) => ACCENT,
    };
    frame.puck = Some((quarter(rider), tint));
    bucket(frame);
}

/// The turn arrow: the route from just before the turn at `turn`, along it,
/// and a head. `points` walks the route ahead from the rider, each point
/// with whether it is before the turn.
fn arrow<I: Iterator<Item = (Point, bool)>>(
    frame: &mut Frame,
    at: &Projection,
    points: impl Fn() -> I,
    turn: Point,
) {
    let screen = || points().map(|(p, before)| (at.at(p), before));
    // How far along the route the turn is.
    let (mut walked, mut last, mut base) = (0.0, None, None);
    for (p, before) in screen() {
        if let Some(last) = last {
            if !before {
                base = Some(walked + distance(last, turn));
                break;
            }
            walked += distance(last, p);
        }
        last = Some(p);
    }
    let Some(((rider, _), base)) = screen().next().zip(base) else {
        return;
    };
    let start = (base - BEFORE.min(distance(rider, turn) / 4.0)).max(0.0);
    let end = start + SHAFT;
    let mut shaft: heapless::Vec<Point, { ARROW_STROKES + 1 }> = heapless::Vec::new();
    let (mut walked, mut last) = (0.0, None::<Point>);
    for (p, _) in screen() {
        let Some(from) = last.replace(p) else {
            continue;
        };
        let length = distance(from, p);
        let point = |s: f32| {
            let t = ((s - walked) / length.max(1e-3)).clamp(0.0, 1.0);
            (from.0 + t * (p.0 - from.0), from.1 + t * (p.1 - from.1))
        };
        if shaft.is_empty() && walked + length >= start {
            let _ = shaft.push(point(start));
        }
        if walked + length >= end {
            let _ = shaft.push(point(end));
            break;
        }
        if !shaft.is_empty() && shaft.push(p).is_err() {
            break;
        }
        walked += length;
    }
    // The head points the way the shaft's last stretch goes.
    let Some(&tip) = shaft.last() else {
        return;
    };
    let Some((ux, uy)) = shaft.windows(2).rev().find_map(|pair| {
        let length = distance(pair[0], pair[1]);
        (length >= 0.5).then(|| {
            (
                (pair[1].0 - pair[0].0) / length,
                (pair[1].1 - pair[0].1) / length,
            )
        })
    }) else {
        return;
    };
    frame.arrow.head = Triangle([
        (tip.0 + ux * HEAD_LENGTH, tip.1 + uy * HEAD_LENGTH),
        (tip.0 - uy * HEAD_HALF, tip.1 + ux * HEAD_HALF),
        (tip.0 + uy * HEAD_HALF, tip.1 - ux * HEAD_HALF),
    ]);
    frame.arrow.strokes = 0;
    for pair in shaft.windows(2) {
        frame.arrow.shaft[frame.arrow.strokes] = Stroke::new(pair[0], pair[1]);
        frame.arrow.strokes += 1;
    }
}

/// Three chevrons between the rider and the junction, pointing to it.
fn chevrons(frame: &mut Frame, rider: Point, junction: Point) {
    let (dx, dy) = (junction.0 - rider.0, junction.1 - rider.1);
    let length = libm::sqrtf(dx * dx + dy * dy);
    if length < 1.0 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let middle = ((rider.0 + junction.0) / 2.0, (rider.1 + junction.1) / 2.0);
    frame.chevron_count = 0;
    let half = length / 2.0;
    for k in 0..CHEVRONS {
        let offset = (k as f32 - 1.0) * CHEVRON_PITCH;
        // Only chevrons clear of the puck and the junction's ring.
        let (tail, tip) = (half + offset - CHEVRON_DEPTH, half + offset);
        if tail - CHEVRON_STROKE < PUCK_CLEAR || length - tip - CHEVRON_STROKE < JUNCTION_CLEAR {
            continue;
        }
        let tip = (middle.0 + ux * offset, middle.1 + uy * offset);
        for side in [-1.0, 1.0] {
            let wing = (
                tip.0 - ux * CHEVRON_DEPTH - uy * CHEVRON_HALF * side,
                tip.1 - uy * CHEVRON_DEPTH + ux * CHEVRON_HALF * side,
            );
            frame.chevrons[frame.chevron_count] = Stroke::new(wing, tip);
            frame.chevron_count += 1;
        }
    }
}

/// The bands a stroke of `half` reaches, if it reaches the ground at all.
fn bands(stroke: &Stroke, half: f32) -> Option<(usize, usize)> {
    let (top, bottom) = stroke.rows(half);
    let low = (top - 1.0).max(GROUND.start as f32);
    let high = (bottom + 1.0).min((GROUND.end - 1) as f32);
    if low > high {
        return None;
    }
    let band = |row: f32| (row as usize - GROUND.start) / BAND;
    Some((band(low), band(high)))
}

/// Sort each line's strokes into the bands their rows reach, in order,
/// dropping the farthest of a line when the entries run out.
fn bucket(frame: &mut Frame) {
    let mut at = 0usize;
    for line in Line::ALL {
        let room = line.room();
        let mut counts = [0usize; BANDS];
        let mut entries = 0;
        let mut kept = 0;
        for stroke in frame.line(line) {
            if let Some((first, last)) = bands(stroke, line.reach()) {
                if at + entries + last - first + 1 > ITEMS {
                    break;
                }
                entries += last - first + 1;
                counts[first..=last].iter_mut().for_each(|c| *c += 1);
            }
            kept += 1;
        }
        frame.counts[line as usize] = kept;
        let starts = &mut frame.starts[line as usize];
        for (band, count) in counts.iter().enumerate() {
            starts[band] = at as u16;
            at += count;
        }
        starts[BANDS] = at as u16;
        let mut next = *starts;
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

/// The map's ground and everything on it.
pub(super) fn paint(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if frame.show != Show::Map || !GROUND.contains(&y) {
        return;
    }
    row.fill(GROUND_COLOR);
    let mut cover = Cover::new(y);
    let band = (y - GROUND.start) / BAND;
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
    if arrow.strokes > 0 {
        for (half, outline, color) in ARROW_LAYERS {
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
    if let Some(at) = frame.junction {
        for (radius, color) in JUNCTION_RINGS {
            disc(row, at, radius, color);
        }
    }
    if let Some((at, tint)) = frame.puck {
        for (radius, color) in PUCK_RINGS {
            disc(row, at, radius, color);
        }
        let (left, top) = puck_corner(at);
        PUCK.paint(row, left, top, tint);
    }
}

/// The puck icon's square, centred on the puck.
fn puck_corner((cx, cy): (i32, i32)) -> (usize, usize) {
    let half = i32::from(PUCK.size) * 2;
    (
        ((cx - half) / 4).max(0) as usize,
        ((cy - half) / 4).max(0) as usize,
    )
}

/// The per-pixel original of `paint`: every stroke, no buckets.
#[cfg(test)]
pub(super) fn pixel(frame: &Frame, x: usize, y: usize, under: u16) -> u16 {
    if frame.show != Show::Map || !GROUND.contains(&y) {
        return under;
    }
    let over = |color: u16, paint: u16, coverage: u8| match coverage {
        0 => color,
        c => blend(paint, color, c),
    };
    let mut color = GROUND_COLOR;
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
        for (half, outline, paint) in ARROW_LAYERS {
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
    if let Some(at) = frame.junction {
        color = rings(color, at, &JUNCTION_RINGS);
    }
    if let Some((at, tint)) = frame.puck {
        color = rings(color, at, &PUCK_RINGS);
        let (left, top) = puck_corner(at);
        color = PUCK.pixel(x, y, left, top, tint, color).unwrap_or(color);
    }
    color
}

/// The footer, and what the page says when it has no map to draw.
pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let say = |canvas: &mut Canvas, text: &[u8]| {
        canvas.fit(&[&BODY_FACE], text, WIDTH, CX, MID_Y, Align::Center, WHITE);
    };
    let frame = match view.map() {
        Some(frame) if frame.show == Show::Map => frame,
        Some(frame) if frame.show == Show::NoGps => {
            say(&mut canvas, b"NO GPS YET");
            canvas.caption(b"MAP STARTS WITH GPS", WIDTH, CX, MID_Y + 24, Align::Center);
            return canvas;
        }
        _ => {
            say(&mut canvas, b"NO TRACK");
            return canvas;
        }
    };
    if !frame.has_track {
        let below = usize::try_from(frame.pose.rider.1).unwrap_or(MID_Y) - 36;
        canvas.caption(b"DRAWS AS YOU RIDE", WIDTH, CX, below, Align::Center);
    }
    let (bar, label) = frame.pose.step.bar();
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
