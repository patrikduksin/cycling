//! The map page: the ride's own track drawn as a breadcrumb, with the rider
//! on it, heading up, and a scale.
//!
//! The track is the ride's GPS breadcrumb (`ride::homing`), thinned to its
//! bends. Riding, the track is rotated so the way the rider is going is up;
//! standing still it is north up and says so. The rider sits low, so more of
//! the road behind shows than ahead. The zoom steps out as the ride grows,
//! to fit the whole track, and never closer than 500 m across. Following the
//! track back, the way home is green, as on the back-to-start page.
//!
//! Painting a row may not walk thousands of points. `fill` projects the
//! track into screen space once per frame, drops points closer than
//! `MIN_STEP` pixels (wider when the budget is full), and buckets the
//! segments by `BAND` rows, so a row tests only the segments that reach it.
//! The track is the page's only per-row work; `prepare` lays out the text
//! and the marks as on other canvas pages, and the page paints the track
//! first, so the start and the rider lie over it.
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
}

type Point = (f32, f32);

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
/// Where the track is drawn: the rider's column, and rows from `TOP` to
/// `BOTTOM` with the rider low.
const CX: usize = (LEFT + RIGHT) / 2;
const TOP: usize = 56;
const BOTTOM: usize = 280;
const RIDER_Y: usize = 196;
/// How far the track may reach from the rider at the fit, in pixels: to
/// each side, ahead, and behind, each less a margin for the line and dots.
const SIDE_PX: f32 = ((RIGHT - LEFT) / 2 - 4) as f32;
const AHEAD_PX: f32 = (RIDER_Y - TOP - 6) as f32;
const BEHIND_PX: f32 = (BOTTOM - RIDER_Y - 8) as f32;
/// The width a zoom step spans, in pixels.
const WIDTH_PX: f32 = (RIGHT - LEFT - 10) as f32;
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

/// Segments kept after projection.
const MAX_SEGMENTS: usize = 255;
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
/// Entries across all buckets: a segment sits in each band it reaches.
const ITEMS: usize = 1536;
/// Columns a row of the page covers.
const SPAN: usize = super::BODY.right - super::BODY.left;

const _: () = assert!(BOTTOM <= crate::screens::chrome::TOAST.top);
const _: () = assert!(HEAD + 12 < TOP);
const _: () = assert!(RIDER_Y > TOP && RIDER_Y < BOTTOM);
const _: () = assert!(MAX_SEGMENTS < 256, "segments are bucketed as bytes");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    /// The device keeps no track.
    #[default]
    Unavailable,
    /// No position yet.
    NoGps,
    Track,
}

/// One drawn stretch of the track, in pixels.
#[derive(Clone, Copy, Default)]
struct Segment {
    a: Point,
    b: Point,
    /// On the far side of the way home.
    far: bool,
}

/// The track in screen space: refilled when the rider or the breadcrumb
/// moves, painted every frame until then. Composition places it in PSRAM,
/// zeroed, so all zero bytes are a valid `Frame`: nothing shown yet.
#[derive(Clone)]
pub struct Frame {
    show: Show,
    segments: [Segment; MAX_SEGMENTS],
    count: usize,
    /// Two or more points to draw: the page never says it has no track.
    has_track: bool,
    /// Where each band's segments start in `items`, and the segments.
    starts: [u16; BANDS + 1],
    items: [u8; ITEMS],
    /// The chevron: tip, left wing, right wing; drawn when heading up.
    chevron: [Point; 3],
    /// The rider is drawn as a disc, or not at all without a fix.
    rider: bool,
    start: Point,
    /// Pixels per metre, and the zoom step it is.
    scale: f32,
    /// The zoom step plus one; zero before the first fill.
    zoom_held: usize,
    following: bool,
    heading_up: bool,
    /// No fresh fix: the track is where it was.
    lost: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unavailable,
            segments: [Segment::default(); MAX_SEGMENTS],
            count: 0,
            has_track: false,
            starts: [0; BANDS + 1],
            items: [0; ITEMS],
            chevron: [(0.0, 0.0); 3],
            rider: false,
            start: (0.0, 0.0),
            scale: 0.0,
            zoom_held: 0,
            following: false,
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

/// Metres east and north of the rider, as pixels on screen: the heading
/// up, the rider at (`CX`, `RIDER_Y`).
fn project(p: Point, centre: Point, turn: (f32, f32), scale: f32) -> Point {
    let (right, ahead) = axes(p, centre, turn);
    (CX as f32 + right * scale, RIDER_Y as f32 - ahead * scale)
}

/// Metres to the right of and ahead of the rider.
fn axes(p: Point, centre: Point, (sin, cos): (f32, f32)) -> Point {
    let (east, north) = (p.0 - centre.0, p.1 - centre.1);
    (east * cos - north * sin, east * sin + north * cos)
}

/// How far the track reaches from the rider, in metres, on the page's axes.
#[derive(Clone, Copy, Default)]
struct Reach {
    side: f32,
    ahead: f32,
    behind: f32,
}

impl Reach {
    fn include(&mut self, (right, ahead): Point) {
        self.side = self.side.max(right.abs());
        self.ahead = self.ahead.max(ahead);
        self.behind = self.behind.max(-ahead);
    }
}

/// The span in metres across the page at zoom `step` of the ladder.
fn span(step: usize) -> f32 {
    MIN_SPAN_M * powf(STEP, step as f32)
}

/// The step of the zoom ladder that keeps `reach` on the page, from 500 m
/// across to the widest. Steps are 20% apart, and a zoom that has stepped
/// out steps back in only once the track fits 90% of the closer step.
fn zoom(reach: Reach, held: Option<usize>) -> usize {
    let needed = [
        (reach.side, SIDE_PX),
        (reach.ahead, AHEAD_PX),
        (reach.behind, BEHIND_PX),
    ]
    .into_iter()
    .map(|(metres, room)| WIDTH_PX * metres / room)
    .fold(0.0, f32::max);
    let fitting = (0..MAX_STEPS).find(|&step| span(step) >= needed);
    let fitting = fitting.unwrap_or(MAX_STEPS);
    match held {
        Some(held) if fitting < held && needed > 0.9 * span(held - 1) => held,
        _ => fitting,
    }
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
    frame.has_track = false;
    frame.rider = false;
    frame.heading_up = false;
    frame.following = false;
    frame.starts = [0; BANDS + 1];
    let Some(centre) = track.centre().filter(|_| frame.show == Show::Track) else {
        frame.zoom_held = 0;
        return;
    };
    let total = track.total();
    let heading = track.heading.filter(|_| track.here.is_some());
    let turn = rotation(heading);
    let mut reach = Reach::default();
    let mut include = |p: Point| {
        let (right, ahead) = axes(p, centre, turn);
        if right * right + ahead * ahead < GLITCH_M * GLITCH_M {
            reach.include((right, ahead));
        }
    };
    include((0.0, 0.0));
    (0..total).for_each(|i| include(track.at(i)));
    let step = zoom(reach, frame.zoom_held.checked_sub(1));
    frame.zoom_held = step + 1;
    let scale = WIDTH_PX / span(step);
    frame.scale = scale;
    frame.start = project((0.0, 0.0), centre, turn, scale);
    frame.following = track.home.is_some();
    frame.heading_up = heading.is_some();
    frame.lost = track.here.is_none();
    frame.rider = track.here.is_some() && heading.is_none();
    let (x, y) = (CX as f32, RIDER_Y as f32);
    frame.chevron = [(x, y - 9.0), (x - 8.0, y + 8.0), (x + 8.0, y + 8.0)];
    let home_end = track.home_end();
    let mut step = MIN_STEP;
    loop {
        // At the widest step what does not fit is the oldest of the track.
        let last_try = step >= MAX_STEP;
        let fits = thin(frame, track, (centre, turn), step, home_end, last_try) && bucket(frame);
        if fits || last_try {
            break;
        }
        step = (step * 1.5).min(MAX_STEP);
    }
    frame.has_track = frame.count > 0 || track.crumbs.len() >= 2;
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
    (centre, turn): (Point, (f32, f32)),
    step: f32,
    home_end: Option<usize>,
    newest_first: bool,
) -> bool {
    let total = track.total();
    frame.count = 0;
    if total < 2 {
        return true;
    }
    let project_at = |i: usize| project(track.at(i), centre, turn, frame.scale);
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
            if frame.count == MAX_SEGMENTS {
                if newest_first {
                    break;
                }
                return false;
            }
            frame.segments[frame.count] = Segment {
                a: anchor,
                b: p,
                far,
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
/// they would not fit.
fn bucket(frame: &mut Frame) -> bool {
    let mut counts = [0u16; BANDS];
    let mut total = 0;
    for i in 0..frame.segments() {
        let (a, b) = frame.segment(i);
        if let Some((first, last)) = bands(a, b, TRACK_HALF) {
            for count in &mut counts[first..=last] {
                *count += 1;
                total += 1;
            }
        }
    }
    if total > ITEMS {
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
                frame.items[usize::from(*at)] = i as u8;
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

/// A pixel's track coverage in the low bits and which colour in the fifth.
fn keep_track(held: &mut u8, coverage: u8, color: usize) {
    if coverage > *held & 15 {
        *held = coverage | (color as u8) << 4;
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
            keep_track(held, coverage(a, b, TRACK_HALF, x, y), frame.color(segment));
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
        let held = track[x - left];
        let mut color = match held & 15 {
            0 => under,
            c => blend(colors[usize::from(held >> 4)], under, c),
        };
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
        keep_track(&mut held, coverage(a, b, TRACK_HALF, x, y), frame.color(i));
    }
    let mut color = match held & 15 {
        0 => under,
        c => blend(frame.colors()[usize::from(held >> 4)], under, c),
    };
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
    let show = view.map.map_or(Show::Unavailable, |frame| frame.show);
    let Some(frame) = view.map else {
        canvas.fit(
            &[&BODY_FACE],
            b"NO TRACK",
            width,
            CX,
            RIDER_Y,
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
                RIDER_Y,
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
                RIDER_Y,
                Align::Center,
                WHITE,
            );
            canvas.caption(
                b"MAP STARTS WITH GPS",
                width,
                CX,
                RIDER_Y + 24,
                Align::Center,
            );
            return canvas;
        }
        Show::Track => {}
    }
    // What the page says of itself, over the track's corner.
    let mut state = Text::<24>::new();
    let color = if frame.lost {
        let _ = state.push_str("NO GPS");
        AMBER
    } else if frame.following {
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
            cx: 4 * CX as i32 + 2,
            cy: 4 * RIDER_Y as i32 + 2,
            radius: 4 * RIDER_RADIUS,
            color: ACCENT,
        });
    }
    if !frame.has_track {
        canvas.caption(b"DRAWS AS YOU RIDE", width, CX, RIDER_Y + 36, Align::Center);
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
        }
    }

    fn filled(track: &Track) -> Frame {
        let mut frame = Frame::default();
        fill(&mut frame, track);
        frame
    }

    #[test]
    fn north_up_puts_north_up_and_the_rider_in_place() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 100.0), (100.0, 100.0)]);
        let frame = filled(&track(&c));
        let s = frame.scale;
        let rider = project((100.0, 100.0), (100.0, 100.0), (0.0, 1.0), s);
        assert_eq!(rider, (CX as f32, RIDER_Y as f32));
        // The start is south west of the rider: left, and down.
        assert!(frame.start.0 < CX as f32 && frame.start.1 > RIDER_Y as f32);
        assert!((frame.start.0 - (CX as f32 - 100.0 * s)).abs() < 0.01);
        assert!((frame.start.1 - (RIDER_Y as f32 + 100.0 * s)).abs() < 0.01);
        assert_eq!(frame.count, 2);
        assert!(frame.rider && !frame.heading_up);
    }

    #[test]
    fn heading_up_rotates_the_way_ridden_to_the_top() {
        let sin_cos = |turns| rotation(Some(turns));
        let centre = (0.0, 0.0);
        // Riding east: a point to the east is ahead, one to the north is left.
        let east = project((10.0, 0.0), centre, sin_cos(TURN / 4), 1.0);
        assert!((east.0 - CX as f32).abs() < 1e-3);
        assert!((east.1 - (RIDER_Y as f32 - 10.0)).abs() < 1e-3);
        let north = project((0.0, 10.0), centre, sin_cos(TURN / 4), 1.0);
        assert!((north.0 - (CX as f32 - 10.0)).abs() < 1e-3);
        assert!((north.1 - RIDER_Y as f32).abs() < 1e-3);
        // Riding south: north is behind.
        let behind = project((0.0, 10.0), centre, sin_cos(TURN / 2), 1.0);
        assert!((behind.1 - (RIDER_Y as f32 + 10.0)).abs() < 1e-3);
        // North up: unchanged.
        let still = project((3.0, 4.0), centre, rotation(None), 2.0);
        assert!((still.0 - (CX as f32 + 6.0)).abs() < 1e-3);
        assert!((still.1 - (RIDER_Y as f32 - 8.0)).abs() < 1e-3);
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
        let ahead = |metres| Reach {
            side: 0.0,
            ahead: metres,
            behind: 0.0,
        };
        let scale = |step| WIDTH_PX / span(step);
        assert_eq!(zoom(ahead(40.0), None), 0);
        assert!((scale(0) * 500.0 - WIDTH_PX).abs() < 0.01);
        // A track reaching 3 km ahead fits, within one 20% step of filling.
        let far = zoom(ahead(3_000.0), None);
        assert!(3_000.0 * scale(far) <= AHEAD_PX + 0.01);
        assert!(3_000.0 * scale(far) > AHEAD_PX / STEP - 0.01);
        // Behind has less room than ahead, and each axis counts.
        let behind = zoom(
            Reach {
                behind: 3_000.0,
                ..Reach::default()
            },
            None,
        );
        assert!(behind > far);
        assert!(zoom(ahead(30_000.0), None) > far);
        assert_eq!(zoom(ahead(1e9), None), zoom(ahead(2e9), None));
        // Every point of a track is inside the page at the zoom.
        let c = crumbs(&[
            (0.0, 0.0),
            (1_500.0, 800.0),
            (-700.0, 2_300.0),
            (400.0, -900.0),
        ]);
        let frame = filled(&track(&c));
        for i in 0..frame.segments() {
            let (a, b) = frame.segment(i);
            for (x, y) in [a, b] {
                assert!((LEFT as f32..RIGHT as f32).contains(&x), "{x}");
                assert!((TOP as f32..BOTTOM as f32).contains(&y), "{y}");
            }
        }
    }

    #[test]
    fn the_zoom_steps_back_in_only_once_the_track_fits_the_closer_step() {
        let ahead = |metres| Reach {
            side: 0.0,
            ahead: metres,
            behind: 0.0,
        };
        let metres_at = |step: usize, share: f32| share * span(step) * AHEAD_PX / WIDTH_PX;
        // Stepped out to 5, the track now needs step 4 but only just fits it.
        assert_eq!(zoom(ahead(metres_at(4, 0.98)), Some(5)), 5);
        // Well inside step 4: step in.
        assert_eq!(zoom(ahead(metres_at(4, 0.85)), Some(5)), 4);
        // Outgrowing a step always steps out at once.
        assert_eq!(zoom(ahead(metres_at(5, 0.99)), Some(4)), 5);
        // A change of several steps is followed at once.
        assert_eq!(zoom(ahead(metres_at(2, 0.95)), Some(6)), 2);
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
        view.map = frame;
        view.totals.add(1000, 8333, &Default::default(), None);
        view.prepare();
        view
    }

    #[test]
    fn every_state_lays_out_its_text() {
        let check = |track: &Track, want: &[&str], what: &str| {
            let frame = filled(track);
            page(Some(&frame)).canvas.assert_laid_out(PAGE, want, what);
        };
        page(None)
            .canvas
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
        check(&t, &["MAP", "N UP", "600 M", "0.01 KM"], "still");
        t.heading = Some(0);
        check(&t, &["MAP", "600 M", "0.01 KM"], "riding");
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
}
