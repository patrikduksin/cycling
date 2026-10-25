//! The map page: the ride's own track drawn as a breadcrumb, with the rider
//! on it, heading up, and a scale.
//!
//! The track is the ride's GPS breadcrumb (`ride::homing`), thinned to its
//! bends. Riding, the track is rotated so the way the rider is going is up;
//! standing still it is north up and says so. The rider sits low, so more of
//! the road behind shows than ahead. The zoom steps out as the ride grows,
//! to fit the whole track, and never closer than 500 m across. Following the
//! track back, the way home is green, as on the back-to-start page. A loaded
//! course is drawn grey under the track: while following it only what is
//! ahead, else both ways from its point nearest the rider. It does not change
//! the zoom, so a course far from the ride stays off the page.
//!
//! Painting a row may not walk thousands of points. `fill` projects the
//! track into screen space once per frame, drops points closer than
//! `MIN_STEP` pixels (wider when the budget is full), and buckets the
//! segments by `BAND` rows, so a row tests only the segments that reach it.
//! The course gets its own budget of segments, nearest the rider first. The
//! lines are the page's only per-row work; `prepare` lays out the text and
//! the marks as on other canvas pages, and the page paints the lines first,
//! so the start and the rider lie over them.
use super::{LEFT, RIGHT, kilometres};
use crate::ride::homing::{Crumb, TURN};
use crate::screens::canvas::{Canvas, GAP, Shape};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BODY as BODY_FACE, CAPTION, GREEN, Rect, Row, SECONDARY, WHITE,
};

use crate::screens::workout::{Text, View};
use core::f32::consts::TAU;
use firmware_shell::rendering::font::blend;
use libm::{cosf, powf, sinf, sqrtf};

/// What the page draws: the breadcrumb and where the rider is on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Track<'a> {
    /// The device has memory set aside for the breadcrumb.
    pub available: bool,
    /// Kept points, metres east and north of the start, oldest first.
    pub crumbs: &'a [Crumb],
    /// The rider, while the fix is fresh.
    pub here: Option<(f32, f32)>,
    /// The way the rider is riding, in `TURN`ths clockwise from north.
    pub heading: Option<u16>,
    /// Following the track back: the first points up to this index, then
    /// the matched point if any, are the way home.
    pub home: Option<(usize, Option<(f32, f32)>)>,
    /// The loaded course, end first; empty without one.
    pub course: &'a [Crumb],
    /// Following the course: what is ahead is its first points up to this
    /// index, from its end, and then the matched point if any.
    pub ahead: Option<(usize, Option<(f32, f32)>)>,
}

type Point = (f32, f32);

/// `count` points of the course from index `from`, down towards its end or
/// up towards its start; the first is `matched` when given.
#[derive(Clone, Copy, Default)]
struct Chain {
    from: usize,
    down: bool,
    count: usize,
    matched: Option<Point>,
}

impl Track<'_> {
    /// Where the matched point sits among the points drawn.
    fn inserted(&self) -> Option<usize> {
        let (segment, matched) = self.home?;
        matched.map(|_| segment.min(self.crumbs.len()))
    }

    /// The rider's fix as the track's last point: only while not following,
    /// when the breadcrumb is still growing.
    fn tip(&self) -> Option<Point> {
        self.home.is_none().then_some(self.here).flatten()
    }

    /// Points drawn: the start when nothing is kept yet, the breadcrumb
    /// with the matched point in it, and the tip.
    fn total(&self) -> usize {
        self.crumbs.len()
            + usize::from(self.inserted().is_some())
            + usize::from(self.tip().is_some())
    }

    fn at(&self, i: usize) -> Point {
        let insert = self.inserted();
        if insert == Some(i) {
            return self
                .home
                .and_then(|(_, matched)| matched)
                .unwrap_or_default();
        }
        let j = if insert.is_some_and(|at| i > at) {
            i - 1
        } else {
            i
        };
        match self.crumbs.get(j) {
            Some(c) => (c.east, c.north),
            None => self.tip().unwrap_or_default(),
        }
    }

    /// The last point of the way home, while following.
    fn home_end(&self) -> Option<usize> {
        let (_, matched) = self.home?;
        Some(match self.inserted() {
            Some(at) if matched.is_some() => at,
            _ => self.crumbs.len().saturating_sub(1),
        })
    }

    /// Where the ride began: its first kept point.
    fn start(&self) -> Option<Point> {
        self.crumbs.first().map(|c| (c.east, c.north))
    }

    /// The stretches of course to draw, nearest the rider first: while
    /// following, what is ahead; otherwise both ways from its point nearest
    /// `centre`.
    fn course_chains(&self, centre: Point) -> [Chain; 2] {
        let len = self.course.len();
        let none = Chain::default();
        if len < 2 {
            return [none; 2];
        }
        if let Some((end, matched)) = self.ahead {
            let from = end.min(len - 1);
            let ahead = Chain {
                from,
                down: true,
                count: from + 1,
                matched,
            };
            return [ahead, none];
        }
        let gap = |c: &Crumb| {
            let (east, north) = (c.east - centre.0, c.north - centre.1);
            east * east + north * north
        };
        let nearest = (0..len)
            .min_by(|&a, &b| gap(&self.course[a]).total_cmp(&gap(&self.course[b])))
            .unwrap_or(0);
        [
            Chain {
                from: nearest,
                down: true,
                count: nearest + 1,
                matched: None,
            },
            Chain {
                from: nearest,
                down: false,
                count: len - nearest,
                matched: None,
            },
        ]
    }

    fn chain_at(&self, chain: &Chain, k: usize) -> Point {
        match chain.matched {
            Some(matched) if k == 0 => matched,
            _ => {
                let c = self.course[if chain.down {
                    chain.from - k
                } else {
                    chain.from + k
                }];
                (c.east, c.north)
            }
        }
    }

    /// Where the page centres: the rider, else where the track last was.
    fn centre(&self) -> Option<Point> {
        self.here
            .or_else(|| self.home.and_then(|(_, matched)| matched))
            .or_else(|| self.crumbs.last().map(|c| (c.east, c.north)))
    }
}

/// What a frame was filled from: refill it only when this changes, about
/// once a second, not on every radar redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    available: bool,
    crumbs: usize,
    newest: u32,
    here: Option<(u32, u32)>,
    heading: Option<u16>,
    home: Option<(usize, Option<(u32, u32)>)>,
    course: usize,
    course_m: u32,
    ahead: Option<(usize, Option<(u32, u32)>)>,
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
            home: self
                .home
                .map(|(segment, matched)| (segment, matched.map(bits))),
            course: self.course.len(),
            course_m: self.course.last().map_or(0, |c| c.along.to_bits()),
            ahead: self
                .ahead
                .map(|(segment, matched)| (segment, matched.map(bits))),
        }
    }
}

/// Refill `frame` from `track` unless `held` says it already shows it.
pub fn update(frame: &mut Frame, held: &mut Option<Key>, track: &Track) {
    let key = track.key();
    if *held != Some(key) {
        fill(frame, track);
        *held = Some(key);
    }
}

/// The header baseline, as on the back-to-start page.
const HEAD: usize = 40;
/// Where the track is drawn: the middle column, and rows from `TOP` to
/// `BOTTOM`. The track and the rider are fitted inside a margin of `MARGIN`
/// for the line and the marks.
const CX: usize = (LEFT + RIGHT) / 2;
const TOP: usize = 56;
const BOTTOM: usize = 280;
const MID_Y: usize = (TOP + BOTTOM) / 2;
const MARGIN: f32 = 10.0;
const AVAIL_W: f32 = (RIGHT - LEFT) as f32 - 2.0 * MARGIN;
const AVAIL_H: f32 = (BOTTOM - TOP) as f32 - 2.0 * MARGIN;
/// The track stays where it is until it would come within this of the edge.
const HOLD_MARGIN: f32 = MARGIN / 2.0;
/// The width a zoom step spans, in pixels.
const WIDTH_PX: f32 = AVAIL_W;
/// The closest zoom spans this many metres across, and each step out is
/// this much wider, up to `MAX_STEPS`.
const MIN_SPAN_M: f32 = 500.0;
const STEP: f32 = 1.2;
const MAX_STEPS: usize = 28;
/// The track is this wide, 3 px, and the rider's chevron 4.5 px.
const TRACK_HALF: f32 = 1.5;
const CHEVRON_HALF: f32 = 2.25;
/// The footer: scale and distance, on one baseline in the toast band.
const FOOT: usize = 304;
/// The start dot and the rider's disc, in pixels.
const START_RADIUS: i32 = 4;
const RIDER_RADIUS: i32 = 7;
/// The scale bar is at most this long, in pixels, and 3 px thick.
const BAR_MAX_PX: f32 = 56.0;
const BAR_THICKNESS: usize = 3;

/// The course, under the track.
const COURSE: u16 = SECONDARY;
/// Segments of the track kept after projection, and of the course.
const MAX_SEGMENTS: usize = 255;
const COURSE_SEGMENTS: usize = 96;
const SEGMENTS: usize = MAX_SEGMENTS + COURSE_SEGMENTS;
/// Points closer than this to the last kept are dropped, widening to
/// `MAX_STEP` when the segments do not fit.
const MIN_STEP: f32 = 2.0;
const MAX_STEP: f32 = 24.0;
/// Laps over the same road are drawn once: the page is divided into cells
/// this many pixels wide, and a segment over cells already drawn is skipped.
const CELL: usize = 4;
const CELLS_ACROSS: usize = super::BODY
    .right
    .saturating_sub(super::BODY.left)
    .div_ceil(CELL);
const CELLS_DOWN: usize = super::BODY
    .bottom
    .saturating_sub(super::BODY.top)
    .div_ceil(CELL);
const GRID_BYTES: usize = (CELLS_ACROSS * CELLS_DOWN).div_ceil(8);
/// A point farther than this from the rider is a glitch, not part of the fit.
const GLITCH_M: f32 = 100_000.0;
/// Rows per bucket, from the top of the body.
const BAND: usize = 16;
const BANDS: usize = (super::BODY.bottom - super::BODY.top).div_ceil(BAND);
/// Entries across all buckets: a segment sits in each band it reaches. The
/// course takes at most `COURSE_ITEMS` of them.
const TRACK_ITEMS: usize = 1536;
const COURSE_ITEMS: usize = 512;
const ITEMS: usize = TRACK_ITEMS + COURSE_ITEMS;
/// Columns a row of the page covers.
const SPAN: usize = super::BODY.right - super::BODY.left;

const _: () = assert!(BOTTOM <= crate::screens::chrome::TOAST.top);
const _: () = assert!(HEAD + 12 < TOP);
const _: () = assert!(ITEMS <= u16::MAX as usize, "segments are bucketed as u16");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    /// The device keeps no track.
    #[default]
    Unavailable,
    /// No position yet.
    NoGps,
    Track,
}

/// One drawn stretch of the track or the course, in pixels.
#[derive(Clone, Copy, Default)]
struct Segment {
    a: Point,
    b: Point,
    /// On the far side of the way home.
    far: bool,
    course: bool,
}

/// The track in screen space: refilled when the rider or the breadcrumb
/// moves, painted every frame until then. Composition places it in PSRAM,
/// zeroed, so all zero bytes are a valid `Frame`: nothing shown yet.
#[derive(Clone)]
pub struct Frame {
    show: Show,
    /// The course's segments first, then the track's.
    segments: [Segment; SEGMENTS],
    count: usize,
    course_count: usize,
    /// Two or more points to draw: the page never says it has no track.
    has_track: bool,
    /// Where each band's segments start in `items`, and the segments.
    starts: [u16; BANDS + 1],
    items: [u16; ITEMS],
    /// The chevron: tip, left wing, right wing; drawn when heading up.
    chevron: [Point; 3],
    /// The rider is drawn as a disc, or not at all without a fix.
    rider: bool,
    start: Point,
    /// Where the rider is on the page.
    rider_at: Point,
    /// Pixels per metre, and the zoom step it is.
    scale: f32,
    /// The zoom step plus one; zero before the first fill.
    zoom_held: usize,
    following: bool,
    on_course: bool,
    heading_up: bool,
    /// No fresh fix: the track is where it was.
    lost: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unavailable,
            segments: [Segment::default(); SEGMENTS],
            count: 0,
            course_count: 0,
            has_track: false,
            starts: [0; BANDS + 1],
            items: [0; ITEMS],
            chevron: [(0.0, 0.0); 3],
            rider: false,
            start: (0.0, 0.0),
            rider_at: (0.0, 0.0),
            scale: 0.0,
            zoom_held: 0,
            following: false,
            on_course: false,
            heading_up: false,
            lost: false,
        }
    }
}

impl Frame {
    fn segments(&self) -> usize {
        self.count
    }

    fn segment(&self, i: usize) -> (Point, Point) {
        (self.segments[i].a, self.segments[i].b)
    }

    fn color(&self, segment: usize) -> usize {
        usize::from(self.segments[segment].far)
    }

    /// The near and far parts of the track: the way home is green while
    /// following, and what is left behind grey.
    fn colors(&self) -> [u16; 2] {
        if self.following {
            [GREEN, SECONDARY]
        } else {
            [WHITE; 2]
        }
    }
}

/// The way the track turns: sine and cosine of the heading, or north up.
fn rotation(heading: Option<u16>) -> (f32, f32) {
    let turns = f32::from(heading.unwrap_or(0) % TURN) / f32::from(TURN);
    (sinf(turns * TAU), cosf(turns * TAU))
}

/// How the track maps to the page: metres from `centre` (the rider, or the
/// last of the track), turned by `turn`, as `scale` pixels a metre around the
/// rider's place on the page, `anchor`.
#[derive(Clone, Copy)]
struct Place {
    centre: Point,
    turn: (f32, f32),
    anchor: Point,
    scale: f32,
}

/// Metres east and north, as pixels on screen: the heading up.
fn project(p: Point, place: &Place) -> Point {
    let (right, ahead) = axes(p, place.centre, place.turn);
    (
        place.anchor.0 + right * place.scale,
        place.anchor.1 - ahead * place.scale,
    )
}

/// Metres to the right of and ahead of the rider.
fn axes(p: Point, centre: Point, (sin, cos): (f32, f32)) -> Point {
    let (east, north) = (p.0 - centre.0, p.1 - centre.1);
    (east * cos - north * sin, east * sin + north * cos)
}

/// The box around the rider and the track, in metres right and ahead.
#[derive(Clone, Copy, Default)]
struct Bounds {
    left: f32,
    right: f32,
    behind: f32,
    ahead: f32,
}

impl Bounds {
    fn include(&mut self, (right, ahead): Point) {
        self.left = self.left.min(right);
        self.right = self.right.max(right);
        self.behind = self.behind.min(ahead);
        self.ahead = self.ahead.max(ahead);
    }

    fn width(&self) -> f32 {
        self.right - self.left
    }

    fn height(&self) -> f32 {
        self.ahead - self.behind
    }
}

/// The span in metres across the page at zoom `step` of the ladder.
fn span(step: usize) -> f32 {
    MIN_SPAN_M * powf(STEP, step as f32)
}

/// The step of the zoom ladder that fits `bounds` on the page, from 500 m
/// across to the widest. Steps are 20% apart, and a zoom that has stepped
/// out steps back in only once the track fits 90% of the closer step.
fn zoom(bounds: Bounds, held: Option<usize>) -> usize {
    let needed = bounds.width().max(bounds.height() * AVAIL_W / AVAIL_H);
    let fitting = (0..MAX_STEPS).find(|&step| span(step) >= needed);
    let fitting = fitting.unwrap_or(MAX_STEPS);
    match held {
        Some(held) if fitting < held && needed > 0.9 * span(held - 1) => held,
        _ => fitting,
    }
}

/// Whether `bounds` at `scale`, with the rider at `anchor`, lies within the
/// page less `margin`.
fn fits(bounds: Bounds, scale: f32, anchor: Point, margin: f32) -> bool {
    let (left, right) = (
        anchor.0 + bounds.left * scale,
        anchor.0 + bounds.right * scale,
    );
    let (top, bottom) = (
        anchor.1 - bounds.ahead * scale,
        anchor.1 - bounds.behind * scale,
    );
    left >= LEFT as f32 + margin
        && right <= RIGHT as f32 - margin
        && top >= TOP as f32 + margin
        && bottom <= BOTTOM as f32 - margin
}

/// Where the rider goes on the page: where it was if the track still fits
/// inside the hold margin, else on the rider's own column if the track fits
/// around them there (a short track stays centred on the rider), else the
/// middle of the box around the track.
fn anchor(bounds: Bounds, scale: f32, held: Option<Point>) -> Point {
    if let Some(held) = held.filter(|&held| fits(bounds, scale, held, HOLD_MARGIN)) {
        return held;
    }
    let centred = (CX as f32, MID_Y as f32);
    if fits(bounds, scale, centred, MARGIN) {
        return centred;
    }
    let (mid_right, mid_ahead) = (
        (bounds.left + bounds.right) / 2.0,
        (bounds.behind + bounds.ahead) / 2.0,
    );
    (
        CX as f32 - mid_right * scale,
        MID_Y as f32 + mid_ahead * scale,
    )
}

/// Project the track and thin it to what a frame can paint.
fn fill(frame: &mut Frame, track: &Track) {
    frame.show = if !track.available {
        Show::Unavailable
    } else if track.centre().is_none() {
        Show::NoGps
    } else {
        Show::Track
    };
    frame.count = 0;
    frame.course_count = 0;
    frame.has_track = false;
    frame.rider = false;
    frame.heading_up = false;
    frame.following = false;
    frame.on_course = false;
    frame.starts = [0; BANDS + 1];
    let Some(centre) = track.centre().filter(|_| frame.show == Show::Track) else {
        frame.zoom_held = 0;
        return;
    };
    let total = track.total();
    let heading = track.heading.filter(|_| track.here.is_some());
    let turn = rotation(heading);
    let mut bounds = Bounds::default();
    let mut include = |p: Point| {
        let (right, ahead) = axes(p, centre, turn);
        if right * right + ahead * ahead < GLITCH_M * GLITCH_M {
            bounds.include((right, ahead));
        }
    };
    let start = track.start().unwrap_or(centre);
    include(start);
    (0..total).for_each(|i| include(track.at(i)));
    let step = zoom(bounds, frame.zoom_held.checked_sub(1));
    let scale = WIDTH_PX / span(step);
    let held = (frame.zoom_held > 0).then_some(frame.rider_at);
    frame.zoom_held = step + 1;
    frame.rider_at = anchor(bounds, scale, held);
    let place = Place {
        centre,
        turn,
        anchor: frame.rider_at,
        scale,
    };
    frame.scale = scale;
    frame.start = project(start, &place);
    frame.following = track.home.is_some();
    frame.on_course = track.ahead.is_some();
    frame.heading_up = heading.is_some();
    frame.lost = track.here.is_none();
    frame.rider = track.here.is_some() && heading.is_none();
    let (x, y) = place.anchor;
    frame.chevron = [(x, y - 9.0), (x - 8.0, y + 8.0), (x + 8.0, y + 8.0)];
    thin_course(frame, track, place);
    let home_end = track.home_end();
    let mut step = MIN_STEP;
    loop {
        // At the widest step what does not fit is the oldest of the track.
        let last_try = step >= MAX_STEP;
        let fits = thin(frame, track, place, step, home_end, last_try) && bucket(frame);
        if fits || last_try {
            break;
        }
        step = (step * 1.5).min(MAX_STEP);
    }
    frame.has_track = frame.count > frame.course_count || track.crumbs.len() >= 2;
}

/// Whether a segment can show on the page at all.
fn visible(a: Point, b: Point) -> bool {
    let reach = TRACK_HALF + 1.0;
    let (left, right) = (
        super::BODY.left as f32 - reach,
        super::BODY.right as f32 + reach,
    );
    let (top, bottom) = (
        super::BODY.top as f32 - reach,
        super::BODY.bottom as f32 + reach,
    );
    a.0.max(b.0) >= left && a.0.min(b.0) <= right && a.1.max(b.1) >= top && a.1.min(b.1) <= bottom
}

/// Project the course to draw into the first segments, nearest the rider
/// first, each at least a step long, widening the step until it fits
/// `COURSE_SEGMENTS`; at the widest step what does not fit is the farthest.
fn thin_course(frame: &mut Frame, track: &Track, place: Place) {
    let chains = track.course_chains(place.centre);
    frame.course_count = 0;
    frame.count = 0;
    let mut step = MIN_STEP;
    loop {
        let last_try = step >= MAX_STEP;
        frame.course_count = 0;
        let mut items = 0;
        let mut fits = true;
        'chains: for chain in chains.iter().filter(|chain| chain.count >= 2) {
            let mut anchor = project(track.chain_at(chain, 0), &place);
            for k in 1..chain.count {
                let p = project(track.chain_at(chain, k), &place);
                let (dx, dy) = (p.0 - anchor.0, p.1 - anchor.1);
                if k + 1 < chain.count && dx * dx + dy * dy < step * step {
                    continue;
                }
                if visible(anchor, p) {
                    let reach =
                        bands(anchor, p, TRACK_HALF).map_or(0, |(first, last)| last - first + 1);
                    if frame.course_count == COURSE_SEGMENTS || items + reach > COURSE_ITEMS {
                        fits = false;
                        break 'chains;
                    }
                    items += reach;
                    frame.segments[frame.course_count] = Segment {
                        a: anchor,
                        b: p,
                        far: false,
                        course: true,
                    };
                    frame.course_count += 1;
                }
                anchor = p;
            }
        }
        if fits || last_try {
            break;
        }
        step = (step * 1.5).min(MAX_STEP);
    }
    frame.count = frame.course_count;
}

/// The cells a segment crosses, as (column, row) while on the page.
fn cells(a: Point, b: Point, mut visit: impl FnMut(Option<usize>)) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let samples = ((sqrtf(dx * dx + dy * dy) * 2.0 / CELL as f32) as usize).clamp(1, 400);
    for k in 0..=samples {
        let t = k as f32 / samples as f32;
        let (x, y) = (
            a.0 + t * dx - super::BODY.left as f32,
            a.1 + t * dy - super::BODY.top as f32,
        );
        let inside = x >= 0.0 && y >= 0.0;
        let (column, row) = ((x / CELL as f32) as usize, (y / CELL as f32) as usize);
        visit(
            (inside && column < CELLS_ACROSS && row < CELLS_DOWN)
                .then_some(row * CELLS_ACROSS + column),
        );
    }
}

/// Project the track into segments at least `step` pixels long, each end
/// kept where the road bends, skipping what an earlier lap already drew.
/// The track's first and last points and the end of the way home stay.
/// False when the segments do not fit; with `newest_first` the newest
/// segments are kept and it does not fail.
fn thin(
    frame: &mut Frame,
    track: &Track,
    place: Place,
    step: f32,
    home_end: Option<usize>,
    newest_first: bool,
) -> bool {
    let total = track.total();
    frame.count = frame.course_count;
    if total < 2 {
        return true;
    }
    let project_at = |i: usize| project(track.at(i), &place);
    let order = |k: usize| if newest_first { total - 1 - k } else { k };
    let mut grid = [0u8; GRID_BYTES];
    let mut anchor = project_at(order(0));
    for k in 1..total {
        let i = order(k);
        let p = project_at(i);
        let (dx, dy) = (p.0 - anchor.0, p.1 - anchor.1);
        if dx * dx + dy * dy < 0.01 {
            continue;
        }
        let wanted = k + 1 == total || Some(i) == home_end || dx * dx + dy * dy >= step * step;
        if !wanted {
            continue;
        }
        let drawn = {
            let mut seen = 0;
            let mut total_cells = 0;
            cells(anchor, p, |cell| {
                total_cells += 1;
                seen += usize::from(cell.is_some_and(|c| grid[c / 8] >> (c % 8) & 1 == 1));
            });
            seen == total_cells
        };
        let far = home_end.is_some_and(|end| i.max(order(k - 1)) > end);
        if !drawn {
            if frame.count == frame.course_count + MAX_SEGMENTS {
                if newest_first {
                    break;
                }
                return false;
            }
            frame.segments[frame.count] = Segment {
                a: anchor,
                b: p,
                far,
                course: false,
            };
            frame.count += 1;
            cells(anchor, p, |cell| {
                if let Some(c) = cell {
                    grid[c / 8] |= 1 << (c % 8);
                }
            });
        }
        anchor = p;
    }
    true
}

/// The bands a segment's rows reach, if it is on the page at all.
fn bands(a: Point, b: Point, half: f32) -> Option<(usize, usize)> {
    let reach = half + 0.5;
    let low = (a.1.min(b.1) - reach - 1.0).max(super::BODY.top as f32);
    let high = (a.1.max(b.1) + reach + 1.0).min((super::BODY.bottom - 1) as f32);
    if low > high {
        return None;
    }
    let band = |row: f32| (row as usize - super::BODY.top) / BAND;
    Some((band(low), band(high)))
}

/// Sort the segments into the bands their rows reach, in order. False when
/// the track's would not fit; the course's always do.
fn bucket(frame: &mut Frame) -> bool {
    let mut counts = [0u16; BANDS];
    let mut track = 0;
    for i in 0..frame.segments() {
        let (a, b) = frame.segment(i);
        if let Some((first, last)) = bands(a, b, TRACK_HALF) {
            for count in &mut counts[first..=last] {
                *count += 1;
            }
            if i >= frame.course_count {
                track += last - first + 1;
            }
        }
    }
    if track > TRACK_ITEMS {
        return false;
    }
    let mut at = 0;
    for (band, count) in counts.iter().enumerate() {
        frame.starts[band] = at;
        at += count;
    }
    frame.starts[BANDS] = at;
    let mut next = frame.starts;
    for i in 0..frame.segments() {
        let (a, b) = frame.segment(i);
        if let Some((first, last)) = bands(a, b, TRACK_HALF) {
            for at in &mut next[first..=last] {
                frame.items[usize::from(*at)] = i as u16;
                *at += 1;
            }
        }
    }
    true
}

/// How much of pixel (`x`, `y`) a stroke of `half` width from `a` to `b`
/// covers, 0 to 15.
fn coverage(a: Point, b: Point, half: f32, x: usize, y: usize) -> u8 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((px - a.0) * dx + (py - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
    let reach = half + 0.5;
    let near = qx * qx + qy * qy;
    if near >= reach * reach {
        return 0;
    }
    ((reach - sqrtf(near)).clamp(0.0, 1.0) * 15.0 + 0.5) as u8
}

/// The columns a stroke can touch on row `y`.
fn columns(a: Point, b: Point, half: f32, y: usize) -> Option<core::ops::Range<usize>> {
    let reach = half + 0.5;
    let row = y as f32 + 0.5;
    if row < a.1.min(b.1) - reach || row > a.1.max(b.1) + reach {
        return None;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut left, mut right) = (a.0.min(b.0) - reach, a.0.max(b.0) + reach);
    if dy.abs() > 1e-3 {
        let x = a.0 + (row - a.1) * dx / dy;
        let wide = reach * sqrtf(dx * dx + dy * dy) / dy.abs();
        left = left.max(x - wide);
        right = right.min(x + wide);
    }
    let first = (left - 1.0).max(super::BODY.left as f32) as usize;
    let last = (right + 1.0).min(super::BODY.right as f32) as usize;
    (first < last).then_some(first..last)
}

/// A pixel's track coverage in the low four bits, which colour in the fifth,
/// and the course's coverage, in eighths, in the top three.
fn keep_track(held: &mut u8, coverage: u8, color: usize) {
    if coverage > *held & 15 {
        *held = *held & 0xe0 | coverage | (color as u8) << 4;
    }
}

fn keep_course(held: &mut u8, coverage: u8) {
    let eighths = (coverage * 7 + 7) / 15;
    if eighths > *held >> 5 {
        *held = *held & 0x1f | eighths << 5;
    }
}

/// What a pixel shows: the course over what was there, the track over both.
fn shade(held: u8, colors: [u16; 2], under: u16) -> u16 {
    let color = match held >> 5 {
        0 => under,
        eighths => blend(COURSE, under, (eighths * 15 + 3) / 7),
    };
    match held & 15 {
        0 => color,
        c => blend(colors[usize::from(held >> 4 & 1)], color, c),
    }
}

/// The track and the rider's chevron over what the row holds.
pub(super) fn paint(frame: &Frame, row: &mut Row) {
    let y = row.y();
    if frame.show != Show::Track || !(super::BODY.top..super::BODY.bottom).contains(&y) {
        return;
    }
    let mut track = [0u8; SPAN];
    let mut rider = [0u8; SPAN];
    let (mut first, mut last) = (SPAN, 0);
    let mut touch = |range: core::ops::Range<usize>| {
        first = first.min(range.start - super::BODY.left);
        last = last.max(range.end - super::BODY.left);
    };
    let band = (y - super::BODY.top) / BAND;
    let items = &frame.items[usize::from(frame.starts[band])..usize::from(frame.starts[band + 1])];
    for &item in items {
        let segment = usize::from(item);
        let (a, b) = frame.segment(segment);
        let Some(range) = columns(a, b, TRACK_HALF, y) else {
            continue;
        };
        for x in range.clone() {
            let held = &mut track[x - super::BODY.left];
            let covered = coverage(a, b, TRACK_HALF, x, y);
            if frame.segments[segment].course {
                keep_course(held, covered);
            } else {
                keep_track(held, covered, frame.color(segment));
            }
        }
        touch(range);
    }
    if frame.heading_up {
        let [tip, left, right] = frame.chevron;
        for (a, b) in [(left, tip), (tip, right)] {
            let Some(range) = columns(a, b, CHEVRON_HALF, y) else {
                continue;
            };
            for x in range.clone() {
                let held = &mut rider[x - super::BODY.left];
                *held = (*held).max(coverage(a, b, CHEVRON_HALF, x, y));
            }
            touch(range);
        }
    }
    if first >= last {
        return;
    }
    let left = super::BODY.left;
    let colors = frame.colors();
    row.each_over(left + first..left + last, |x, under| {
        let mut color = shade(track[x - left], colors, under);
        if rider[x - left] > 0 {
            color = blend(ACCENT, color, rider[x - left]);
        }
        Some(color)
    });
}

/// The per-pixel original of `paint`: every segment, no buckets.
#[cfg(test)]
pub(super) fn pixel(frame: &Frame, x: usize, y: usize, under: u16) -> u16 {
    if frame.show != Show::Track || !super::BODY.contains(x, y) {
        return under;
    }
    let mut held = 0u8;
    for i in 0..frame.segments() {
        let (a, b) = frame.segment(i);
        let covered = coverage(a, b, TRACK_HALF, x, y);
        if frame.segments[i].course {
            keep_course(&mut held, covered);
        } else {
            keep_track(&mut held, covered, frame.color(i));
        }
    }
    let mut color = shade(held, frame.colors(), under);
    if frame.heading_up {
        let [tip, left, right] = frame.chevron;
        let c = [(left, tip), (tip, right)]
            .into_iter()
            .map(|(a, b)| coverage(a, b, CHEVRON_HALF, x, y))
            .max()
            .unwrap_or(0);
        if c > 0 {
            color = blend(ACCENT, color, c);
        }
    }
    color
}

/// The longest round length in metres, from 100 m, that is no more than
/// `limit`: 1, 1.5, 2, 3, 5, 6 or 8 times a power of ten.
fn nice(limit: f32) -> u32 {
    let mut best = 100;
    let mut decade = 100u32;
    while decade as f32 <= limit {
        for tenths in [10, 15, 20, 30, 50, 60, 80] {
            let metres = decade * tenths / 10;
            if metres as f32 <= limit {
                best = metres;
            }
        }
        decade *= 10;
    }
    best
}

/// "600 M", "1.5 KM" or "2 KM".
fn length(text: &mut Text<12>, metres: u32) {
    use core::fmt::Write;
    let _ = match (metres >= 1000, metres % 1000) {
        (false, _) => write!(text, "{metres} M"),
        (true, 0) => write!(text, "{} KM", metres / 1000),
        (true, rest) => write!(text, "{}.{} KM", metres / 1000, rest / 100),
    };
}

/// The text, the scale and the marks that lie over the track.
pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let width = RIGHT - LEFT;
    canvas.caption(b"MAP", width, LEFT, HEAD, Align::Left);
    let show = view.map().map_or(Show::Unavailable, |frame| frame.show);
    let Some(frame) = view.map() else {
        canvas.fit(
            &[&BODY_FACE],
            b"NO TRACK",
            width,
            CX,
            MID_Y,
            Align::Center,
            WHITE,
        );
        return canvas;
    };
    match show {
        Show::Unavailable => {
            canvas.fit(
                &[&BODY_FACE],
                b"NO TRACK",
                width,
                CX,
                MID_Y,
                Align::Center,
                WHITE,
            );
            return canvas;
        }
        Show::NoGps => {
            canvas.fit(
                &[&BODY_FACE],
                b"NO GPS YET",
                width,
                CX,
                MID_Y,
                Align::Center,
                WHITE,
            );
            canvas.caption(b"MAP STARTS WITH GPS", width, CX, MID_Y + 24, Align::Center);
            return canvas;
        }
        Show::Track => {}
    }
    // What the page says of itself, over the track's corner.
    let mut state = Text::<24>::new();
    let color = if frame.lost {
        let _ = state.push_str("NO GPS");
        AMBER
    } else if frame.following || frame.on_course {
        let _ = state.push_str("FOLLOWING");
        GREEN
    } else {
        SECONDARY
    };
    if !frame.lost && !frame.heading_up {
        if !state.as_bytes().is_empty() {
            let _ = state.push_str("  ");
        }
        let _ = state.push_str("N UP");
    }
    canvas.fit(
        &[&CAPTION],
        state.as_bytes(),
        width - 40,
        RIGHT,
        HEAD,
        Align::Right,
        color,
    );
    // Marks over the track: the start, then the rider.
    let quarter = |p: Point| ((4.0 * p.0) as i32, (4.0 * p.1) as i32);
    let on_page = |p: Point| {
        (LEFT as f32..RIGHT as f32).contains(&p.0) && (TOP as f32..BOTTOM as f32).contains(&p.1)
    };
    if on_page(frame.start) {
        let (cx, cy) = quarter(frame.start);
        canvas.shape(Shape::Disc {
            cx,
            cy,
            radius: 4 * START_RADIUS,
            color: GREEN,
        });
    }
    if frame.rider {
        canvas.shape(Shape::Disc {
            cx: (4.0 * frame.rider_at.0) as i32,
            cy: (4.0 * frame.rider_at.1) as i32,
            radius: 4 * RIDER_RADIUS,
            color: ACCENT,
        });
    }
    if !frame.has_track {
        let below = frame.rider_at.1 as usize + 36;
        canvas.caption(b"DRAWS AS YOU RIDE", width, CX, below, Align::Center);
    }
    // The footer: the scale bar and its length, and the ride's distance.
    let metres = nice(BAR_MAX_PX / frame.scale);
    let bar = ((metres as f32 * frame.scale + 0.5) as usize).max(2);
    let cap = usize::from(CAPTION.cap);
    let top = FOOT - cap / 2 - BAR_THICKNESS / 2 - 1;
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: LEFT,
            top,
            right: LEFT + bar,
            bottom: top + BAR_THICKNESS,
        },
        radius: 1,
        color: SECONDARY,
    });
    let mut label = Text::<12>::new();
    length(&mut label, metres);
    let label = canvas.caption(label.as_bytes(), 60, LEFT + bar + 10, FOOT, Align::Left);
    let mut distance = Text::<12>::new();
    kilometres(&mut distance, view.totals.distance_mm());
    let _ = distance.push_str(" KM");
    canvas.fit(
        &[&CAPTION],
        distance.as_bytes(),
        RIGHT - label.right() - GAP,
        RIGHT,
        FOOT,
        Align::Right,
        WHITE,
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::ride::{BODY as PAGE, RidePage};
    use crate::screens::workout::Page;

    fn crumbs(points: &[(f32, f32)]) -> std::vec::Vec<Crumb> {
        let mut along = 0.0;
        let mut last = (0.0, 0.0);
        points
            .iter()
            .map(|&(east, north)| {
                along +=
                    sqrtf((east - last.0) * (east - last.0) + (north - last.1) * (north - last.1));
                last = (east, north);
                Crumb { east, north, along }
            })
            .collect()
    }

    fn track(crumbs: &[Crumb]) -> Track<'_> {
        Track {
            available: true,
            crumbs,
            here: crumbs.last().map(|c| (c.east, c.north)),
            heading: None,
            home: None,
            course: &[],
            ahead: None,
        }
    }

    fn filled(track: &Track) -> Frame {
        let mut frame = Frame::default();
        fill(&mut frame, track);
        frame
    }

    fn place(turn: (f32, f32), scale: f32) -> Place {
        Place {
            centre: (0.0, 0.0),
            turn,
            anchor: (CX as f32, MID_Y as f32),
            scale,
        }
    }

    #[test]
    fn north_up_puts_north_up_and_the_rider_on_the_page() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 100.0), (100.0, 100.0)]);
        let frame = filled(&track(&c));
        let s = frame.scale;
        // The start is south west of the rider: left, and down.
        let rider = frame.rider_at;
        assert!((frame.start.0 - (rider.0 - 100.0 * s)).abs() < 0.01);
        assert!((frame.start.1 - (rider.1 + 100.0 * s)).abs() < 0.01);
        assert_eq!(frame.count, 2);
        assert!(frame.rider && !frame.heading_up);
    }

    #[test]
    fn heading_up_rotates_the_way_ridden_to_the_top() {
        let at = |p, turns| project(p, &place(rotation(Some(turns)), 1.0));
        let (cx, cy) = (CX as f32, MID_Y as f32);
        // Riding east: a point to the east is ahead, one to the north is left.
        let east = at((10.0, 0.0), TURN / 4);
        assert!((east.0 - cx).abs() < 1e-3 && (east.1 - (cy - 10.0)).abs() < 1e-3);
        let north = at((0.0, 10.0), TURN / 4);
        assert!((north.0 - (cx - 10.0)).abs() < 1e-3 && (north.1 - cy).abs() < 1e-3);
        // Riding south: north is behind.
        let behind = at((0.0, 10.0), TURN / 2);
        assert!((behind.1 - (cy + 10.0)).abs() < 1e-3);
        // North up: unchanged.
        let still = project((3.0, 4.0), &place(rotation(None), 2.0));
        assert!((still.0 - (cx + 6.0)).abs() < 1e-3 && (still.1 - (cy - 8.0)).abs() < 1e-3);
    }

    #[test]
    fn a_heading_draws_a_chevron_not_a_disc() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 100.0)]);
        let mut t = track(&c);
        t.heading = Some(0);
        let frame = filled(&t);
        assert!(!frame.rider && frame.heading_up);
        // Riding north, the track runs straight down from the rider.
        let (a, b) = frame.segment(0);
        assert!((a.0 - b.0).abs() < 0.01 && a.1 > b.1);
    }

    #[test]
    fn the_zoom_fits_the_whole_track_and_is_never_closer_than_500_m() {
        let wide = |metres: f32| Bounds {
            left: -metres / 2.0,
            right: metres / 2.0,
            ..Bounds::default()
        };
        assert_eq!(zoom(wide(40.0), None), 0);
        let step = zoom(wide(3_000.0), None);
        assert!(span(step) >= 3_000.0 && span(step) < 3_000.0 * STEP);
        // Height counts at the page's own proportions.
        let tall = zoom(
            Bounds {
                ahead: 3_000.0,
                ..Bounds::default()
            },
            None,
        );
        assert!(span(tall) >= 3_000.0 * AVAIL_W / AVAIL_H);
        assert_eq!(zoom(wide(1e9), None), zoom(wide(2e9), None));
        // The track and the rider are inside the page, and fill most of it.
        for heading in [None, Some(0), Some(700), Some(2500)] {
            let c = crumbs(&[
                (0.0, 0.0),
                (1_500.0, 800.0),
                (-700.0, 2_300.0),
                (400.0, -900.0),
                (200.0, 100.0),
            ]);
            let mut t = track(&c);
            t.heading = heading;
            let frame = filled(&t);
            let (mut left, mut right) = (f32::MAX, f32::MIN);
            let (mut top, mut bottom) = (f32::MAX, f32::MIN);
            for i in 0..frame.segments() {
                let (a, b) = frame.segment(i);
                for (x, y) in [a, b] {
                    left = left.min(x);
                    right = right.max(x);
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
            }
            assert!(left >= LEFT as f32 && right <= RIGHT as f32, "{heading:?}");
            assert!(top >= TOP as f32 && bottom <= BOTTOM as f32, "{heading:?}");
            let (width, height) = ((right - left) / AVAIL_W, (bottom - top) / AVAIL_H);
            assert!(width.max(height) > 0.8, "{heading:?}: {width} {height}");
        }
    }

    #[test]
    fn the_track_stays_put_until_it_would_leave_the_page() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 1_000.0), (600.0, 1_400.0)]);
        let mut t = track(&c);
        let mut frame = filled(&t);
        let first = frame.rider_at;
        // A rider a few metres on moves the box a little: nothing swims.
        let more = crumbs(&[
            (0.0, 0.0),
            (0.0, 1_000.0),
            (600.0, 1_400.0),
            (610.0, 1_405.0),
        ]);
        t = track(&more);
        fill(&mut frame, &t);
        assert_eq!(frame.rider_at, first);
        // Far enough to leave the margin: it moves.
        let far = crumbs(&[
            (0.0, 0.0),
            (0.0, 1_000.0),
            (600.0, 1_400.0),
            (-1_500.0, -1_500.0),
        ]);
        t = track(&far);
        fill(&mut frame, &t);
        assert_ne!(frame.rider_at, first);
    }

    #[test]
    fn the_route_home_is_inside_the_box_while_following() {
        let c = crumbs(&[
            (0.0, 0.0),
            (0.0, 800.0),
            (1_200.0, 800.0),
            (1_200.0, 1_500.0),
        ]);
        let mut t = track(&c);
        t.here = Some((1_200.0, 1_000.0));
        t.home = Some((3, Some((1_200.0, 1_000.0))));
        let frame = filled(&t);
        let (a, _) = frame.segment(0);
        assert!(a.0 >= LEFT as f32 && a.1 <= BOTTOM as f32);
        assert!(frame.following && frame.segments() >= 3);
    }

    #[test]
    fn the_zoom_steps_back_in_only_once_the_track_fits_the_closer_step() {
        let wide = |step: usize, share: f32| {
            let metres = share * span(step);
            Bounds {
                left: -metres / 2.0,
                right: metres / 2.0,
                ..Bounds::default()
            }
        };
        // Stepped out to 5, the track now needs step 4 but only just fits it.
        assert_eq!(zoom(wide(4, 0.98), Some(5)), 5);
        // Well inside step 4: step in.
        assert_eq!(zoom(wide(4, 0.85), Some(5)), 4);
        // Outgrowing a step always steps out at once.
        assert_eq!(zoom(wide(5, 0.99), Some(4)), 5);
        // A change of several steps is followed at once.
        assert_eq!(zoom(wide(2, 0.95), Some(6)), 2);
    }

    #[test]
    fn a_stray_fix_far_away_does_not_shrink_the_ride() {
        let ride = [(0.0, 0.0), (0.0, 400.0), (300.0, 600.0)];
        let c = crumbs(&ride);
        let normal = filled(&track(&c)).scale;
        let mut with_stray = ride.to_vec();
        with_stray.insert(2, (3.6e6, 1.0e6));
        let c = crumbs(&with_stray);
        let frame = filled(&track(&c));
        assert_eq!(frame.scale, normal);
        assert!(frame.has_track && frame.segments() >= 2);
    }

    #[test]
    fn close_points_are_dropped_and_the_budget_holds() {
        // A thousand points half a metre apart collapse to a few.
        let line: std::vec::Vec<_> = (0..1000).map(|i| (i as f32 * 0.5, 0.0)).collect();
        let c = crumbs(&line);
        let frame = filled(&track(&c));
        assert!(frame.count < 100, "{}", frame.count);
        assert!(frame.segment(0).0.0 < frame.segment(frame.count - 1).1.0);
        for i in 0..frame.count - 1 {
            let (a, b) = frame.segment(i);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            assert!(dx * dx + dy * dy >= MIN_STEP * MIN_STEP - 0.01);
        }
        // A wandering 4,096-point track still fits segments and buckets.
        let wander: std::vec::Vec<_> = (0..4096)
            .map(|i| {
                let a = i as f32 * 0.37;
                (sinf(a) * 400.0 + i as f32, cosf(a * 1.3) * 400.0)
            })
            .collect();
        let c = crumbs(&wander);
        let frame = filled(&track(&c));
        assert!(frame.count <= MAX_SEGMENTS);
        assert!(usize::from(frame.starts[BANDS]) <= ITEMS);
        // Each band holds only segments that reach it.
        for band in 0..BANDS {
            let items =
                &frame.items[usize::from(frame.starts[band])..usize::from(frame.starts[band + 1])];
            for &i in items {
                let (a, b) = frame.segment(usize::from(i));
                let (first, last) = bands(a, b, TRACK_HALF).unwrap();
                assert!((first..=last).contains(&band));
            }
        }
    }

    #[test]
    fn many_laps_of_a_short_loop_draw_the_loop_once() {
        let lap = |noise: f32| {
            (0..4096).map(move |i| {
                let a = (i % 120) as f32 * TAU / 120.0;
                let jitter = noise * sinf(i as f32 * 12.9898);
                (sinf(a) * 175.0 + jitter, (cosf(a) - 1.0) * 175.0 + jitter)
            })
        };
        let exact: std::vec::Vec<_> = lap(0.0).collect();
        let c = crumbs(&exact);
        let one_lap = filled(&track(&c[..120]));
        let frame = filled(&track(&c));
        assert!(frame.has_track && frame.segments() >= one_lap.segments() / 2);
        assert!(
            frame.segments() <= one_lap.segments() + 2,
            "{}",
            frame.segments()
        );
        // Wobble between laps costs more segments, never the whole track.
        let noisy: std::vec::Vec<_> = lap(4.0).collect();
        let c = crumbs(&noisy);
        let frame = filled(&track(&c));
        assert!(
            frame.has_track && frame.segments() >= 10,
            "{}",
            frame.segments()
        );
        assert!(frame.segments() <= MAX_SEGMENTS);
        assert!(usize::from(frame.starts[BANDS]) > 0);
    }

    #[test]
    fn the_frame_is_refilled_only_when_the_track_changes() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 300.0), (200.0, 500.0)]);
        let mut frame = Frame::default();
        let mut held = None;
        let t = track(&c);
        update(&mut frame, &mut held, &t);
        let drawn = frame.count;
        assert!(drawn > 0);
        // The same track again is left alone, even if the frame was spoiled.
        frame.count = 0;
        update(&mut frame, &mut held, &t);
        assert_eq!(frame.count, 0);
        let mut moved = t;
        moved.heading = Some(100);
        update(&mut frame, &mut held, &moved);
        assert_eq!(frame.count, drawn);
    }

    #[test]
    fn following_paints_the_way_home_green_to_the_matched_point() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 100.0), (0.0, 200.0), (100.0, 200.0)]);
        let mut t = track(&c);
        // The rider is on the second segment, 50 m along it.
        t.home = Some((2, Some((0.0, 150.0))));
        t.here = Some((5.0, 150.0));
        let frame = filled(&t);
        assert_eq!(frame.colors(), [GREEN, SECONDARY]);
        // Points: start, first bend, the matched point, second bend, end.
        assert_eq!(frame.count, 4);
        assert_eq!(frame.color(1), 0);
        assert_eq!(frame.color(2), 1);
        // Frozen: the rider's fix is not a point of the track.
        assert!(frame.following);
        // Without a match yet, the whole track is the way home.
        t.home = Some((3, None));
        let frame = filled(&t);
        assert_eq!(frame.count, 3);
        assert_eq!(frame.color(2), 0);
    }

    #[test]
    fn rows_paint_what_the_per_pixel_original_says() {
        let loop_track: std::vec::Vec<_> = (0..120)
            .map(|i| {
                let a = i as f32 * TAU / 100.0;
                (sinf(a) * 300.0, (cosf(a) - 1.0) * 300.0)
            })
            .collect();
        let c = crumbs(&loop_track);
        let mut cases = std::vec![track(&c)];
        let mut heading = track(&c);
        heading.heading = Some(900);
        cases.push(heading);
        let mut following = track(&c);
        following.home = Some((60, Some((10.0, -200.0))));
        following.heading = Some(3000);
        cases.push(following);
        for (n, t) in cases.iter().enumerate() {
            let frame = filled(t);
            assert!(frame.segments() > 10);
            for y in 0..320 {
                let mut pixels = [0x1234u16; 240];
                paint(
                    &frame,
                    &mut Row::new(y, &mut pixels).within(PAGE.left..PAGE.right),
                );
                for (x, &got) in pixels.iter().enumerate() {
                    let want = if (PAGE.left..PAGE.right).contains(&x) {
                        pixel(&frame, x, y, 0x1234)
                    } else {
                        0x1234
                    };
                    assert_eq!(got, want, "case {n} at {x},{y}");
                }
            }
        }
    }

    fn page<'a>(frame: Option<&'a Frame>) -> View<'a> {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Map;
        view.radar_paired = true;
        view.show_map(frame);
        view.totals.add(1000, 8333, &Default::default(), None);
        view.prepare();
        view
    }

    #[test]
    fn every_state_lays_out_its_text() {
        let check = |track: &Track, want: &[&str], what: &str| {
            let frame = filled(track);
            page(Some(&frame))
                .canvas()
                .assert_laid_out(PAGE, want, what);
        };
        page(None)
            .canvas()
            .assert_laid_out(PAGE, &["MAP", "NO TRACK"], "unavailable");
        check(&Track::default(), &["MAP", "NO TRACK"], "no memory");
        let waiting = Track {
            available: true,
            ..Track::default()
        };
        check(
            &waiting,
            &["MAP", "NO GPS YET", "MAP STARTS WITH GPS"],
            "no gps",
        );
        let c = crumbs(&[(0.0, 0.0), (0.0, 800.0), (300.0, 900.0)]);
        let mut t = track(&c);
        check(&t, &["MAP", "N UP", "200 M", "0.01 KM"], "still");
        t.heading = Some(0);
        check(&t, &["MAP", "200 M", "0.01 KM"], "riding");
        t.home = Some((2, None));
        t.heading = None;
        check(&t, &["MAP", "FOLLOWING  N UP"], "following");
        t.here = None;
        check(&t, &["MAP", "NO GPS"], "fix lost");
        let start = crumbs(&[(0.0, 0.0)]);
        check(
            &track(&start),
            &["MAP", "DRAWS AS YOU RIDE", "150 M"],
            "first fix",
        );
    }

    #[test]
    fn the_scale_bar_is_a_round_length() {
        assert_eq!(nice(190.0), 150);
        assert_eq!(nice(380.0), 300);
        assert_eq!(nice(999.0), 800);
        assert_eq!(nice(70.0 / 0.0184), 3000);
        assert_eq!(nice(0.5), 100);
        for (metres, want) in [
            (600, "600 M"),
            (1500, "1.5 KM"),
            (2000, "2 KM"),
            (10_000, "10 KM"),
        ] {
            let mut text = Text::<12>::new();
            length(&mut text, metres);
            assert_eq!(text.as_str(), want);
        }
    }

    #[test]
    fn the_course_ahead_is_grey_under_the_track() {
        use crate::screens::chrome::BLACK;
        // The ride went 300 m north; the course goes on north, then east.
        let c = crumbs(&[(0.0, 0.0), (0.0, 300.0)]);
        // End first, as the guide keeps it.
        let course = crumbs(&[(400.0, 900.0), (0.0, 900.0), (0.0, 0.0)]);
        let mut t = track(&c);
        t.course = &course;
        t.ahead = Some((2, Some((0.0, 300.0))));
        let frame = filled(&t);
        assert!(frame.on_course && !frame.following);
        assert_eq!(frame.course_count, 1, "only what is ahead and on the page");
        let s = frame.scale;
        let at = |north: f32| {
            let (x, y) = frame.rider_at;
            (x as usize, (y - (north - 300.0) * s) as usize)
        };
        let (x, y) = at(380.0);
        assert_eq!(pixel(&frame, x, y, BLACK), COURSE, "ahead is grey");
        let (x, y) = at(150.0);
        assert_eq!(pixel(&frame, x, y, BLACK), WHITE, "the ride is white");
        // Not following, the whole course is drawn.
        t.ahead = None;
        let frame = filled(&t);
        assert_eq!(frame.course_count, 1);
        let (x, y) = at(150.0);
        assert_eq!(pixel(&frame, x, y, BLACK), WHITE, "the track lies over it");
    }

    #[test]
    fn a_long_course_keeps_to_its_budget_nearest_the_rider() {
        // A course zigzagging 40 km north from the rider, every 10 m.
        let points: std::vec::Vec<(f32, f32)> = (0..4_000)
            .rev()
            .map(|i| (if i % 2 == 0 { 0.0 } else { 30.0 }, i as f32 * 10.0))
            .collect();
        let course = crumbs(&points);
        let c = crumbs(&[(0.0, -100.0), (0.0, 0.0)]);
        let mut t = track(&c);
        t.course = &course;
        let frame = filled(&t);
        assert!(frame.course_count > 0 && frame.course_count <= COURSE_SEGMENTS);
        let items: usize = (0..frame.course_count)
            .filter_map(|i| {
                let (a, b) = frame.segment(i);
                bands(a, b, TRACK_HALF).map(|(first, last)| last - first + 1)
            })
            .sum();
        assert!(items <= COURSE_ITEMS);
        assert!(frame.has_track, "the track is drawn beside the course");
    }
}
