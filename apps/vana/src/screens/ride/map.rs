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
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BODY as BODY_FACE, CAPTION, GREEN, Rect, Row, SECONDARY, WHITE,
};
use crate::screens::workout::{Text, View};
use core::f32::consts::TAU;
use firmware_shell::rendering::font::blend;
use libm::{cosf, sinf, sqrtf};

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

/// The header baseline, as on the back-to-start page.
const HEAD: usize = 40;
/// Where the track is drawn: the rider's column, and rows from `TOP` to
/// `BOTTOM` with the rider low.
const CX: usize = (LEFT + RIGHT) / 2;
const TOP: usize = 56;
const BOTTOM: usize = 280;
const RIDER_Y: usize = 186;
/// The most the track reaches from the rider, in pixels, at the fit: what
/// is left below the rider, less a margin.
const FIT_PX: f32 = (BOTTOM - RIDER_Y - 8) as f32;
/// The width a zoom step spans, in pixels.
const WIDTH_PX: f32 = (RIGHT - LEFT - 10) as f32;
/// The closest zoom spans this many metres across, and each step out is
/// this much wider, up to `MAX_STEPS`.
const MIN_SPAN_M: f32 = 500.0;
const STEP: f32 = 1.25;
const MAX_STEPS: usize = 22;
/// The track is this wide, 3 px, and the rider's chevron 4.5 px.
const TRACK_HALF: f32 = 1.5;
const CHEVRON_HALF: f32 = 2.25;
/// The footer: scale and distance, on one baseline in the toast band.
const FOOT: usize = 304;
/// The start dot and the rider's disc, in pixels.
const START_RADIUS: i32 = 4;
const RIDER_RADIUS: i32 = 7;
/// The scale bar is at most this long, in pixels, and 3 px thick.
const BAR_MAX_PX: f32 = 70.0;
const BAR_THICKNESS: usize = 3;

/// Points kept after projection, and so segments one fewer.
const MAX_POINTS: usize = 161;
/// Points closer than this to the last kept are dropped.
const MIN_STEP: f32 = 2.0;
/// Rows per bucket, from the top of the body.
const BAND: usize = 16;
const BANDS: usize = (super::BODY.bottom - super::BODY.top).div_ceil(BAND);
/// Entries across all buckets: a segment sits in each band it reaches.
const ITEMS: usize = 1024;
/// Columns a row of the page covers.
const SPAN: usize = super::BODY.right - super::BODY.left;

const _: () = assert!(BOTTOM <= crate::screens::chrome::TOAST.top);
const _: () = assert!(HEAD + 12 < TOP);
const _: () = assert!(RIDER_Y > TOP && RIDER_Y < BOTTOM);
const _: () = assert!(MAX_POINTS - 1 < 256, "segments are bucketed as bytes");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    /// The device keeps no track.
    #[default]
    Unavailable,
    /// No position yet.
    NoGps,
    Track,
}

/// The track in screen space for one frame.
#[derive(Clone)]
pub struct Frame {
    show: Show,
    points: [Point; MAX_POINTS],
    count: usize,
    /// Segments from this one on are the far part of the track.
    far: usize,
    colors: [u16; 2],
    /// Where each band's segments start in `items`, and the segments.
    starts: [u16; BANDS + 1],
    items: [u8; ITEMS],
    /// The chevron: tip, left wing, right wing, when heading up.
    chevron: Option<[Point; 3]>,
    /// The rider is drawn as a disc, or not at all without a fix.
    rider: bool,
    start: Point,
    /// Pixels per metre.
    scale: f32,
    following: bool,
    heading_up: bool,
    /// No fresh fix: the track is where it was.
    lost: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            show: Show::Unavailable,
            points: [(0.0, 0.0); MAX_POINTS],
            count: 0,
            far: usize::MAX,
            colors: [WHITE; 2],
            starts: [0; BANDS + 1],
            items: [0; ITEMS],
            chevron: None,
            rider: false,
            start: (0.0, 0.0),
            scale: 1.0,
            following: false,
            heading_up: false,
            lost: false,
        }
    }
}

impl Frame {
    fn segments(&self) -> usize {
        self.count.saturating_sub(1)
    }

    fn segment(&self, i: usize) -> (Point, Point) {
        (self.points[i], self.points[i + 1])
    }

    fn color(&self, segment: usize) -> usize {
        usize::from(segment >= self.far)
    }
}

/// The way the track turns: sine and cosine of the heading, or north up.
fn rotation(heading: Option<u16>) -> (f32, f32) {
    let turns = f32::from(heading.unwrap_or(0) % TURN) / f32::from(TURN);
    (sinf(turns * TAU), cosf(turns * TAU))
}

/// Metres east and north of the rider, as pixels on screen: the heading
/// up, the rider at (`CX`, `RIDER_Y`).
fn project(p: Point, centre: Point, (sin, cos): (f32, f32), scale: f32) -> Point {
    let (east, north) = (p.0 - centre.0, p.1 - centre.1);
    let right = east * cos - north * sin;
    let ahead = east * sin + north * cos;
    (CX as f32 + right * scale, RIDER_Y as f32 - ahead * scale)
}

/// How many pixels a metre is, to fit points reaching `reach` metres from
/// the rider: the closest zoom that does, within 500 m across to the widest.
fn zoom(reach: f32) -> f32 {
    let needed = WIDTH_PX * reach / FIT_PX;
    let mut span = MIN_SPAN_M;
    for _ in 0..MAX_STEPS {
        if span >= needed {
            break;
        }
        span *= STEP;
    }
    WIDTH_PX / span
}

/// Project the track and thin it to what a frame can paint.
pub(super) fn fill(frame: &mut Frame, track: &Track) {
    frame.show = if !track.available {
        Show::Unavailable
    } else if track.centre().is_none() {
        Show::NoGps
    } else {
        Show::Track
    };
    frame.count = 0;
    frame.chevron = None;
    frame.rider = false;
    frame.far = usize::MAX;
    frame.colors = [WHITE; 2];
    frame.starts = [0; BANDS + 1];
    let Some(centre) = track.centre().filter(|_| frame.show == Show::Track) else {
        return;
    };
    let total = track.total();
    let heading = track.heading.filter(|_| track.here.is_some());
    let turn = rotation(heading);
    // Zoom to fit the farthest point from the rider, whichever way it faces,
    // so the zoom does not change as the rider turns.
    let mut reach: f32 = 0.0;
    let mut farthest = |p: Point| {
        let (east, north) = (p.0 - centre.0, p.1 - centre.1);
        reach = reach.max(sqrtf(east * east + north * north));
    };
    farthest((0.0, 0.0));
    (0..total).for_each(|i| farthest(track.at(i)));
    let scale = zoom(reach);
    frame.scale = scale;
    frame.start = project((0.0, 0.0), centre, turn, scale);
    frame.following = track.home.is_some();
    frame.heading_up = heading.is_some();
    frame.lost = track.here.is_none();
    frame.rider = track.here.is_some() && heading.is_none();
    if track.home.is_some() {
        frame.colors = [GREEN, SECONDARY];
    }
    if heading.is_some() {
        let (x, y) = (CX as f32, RIDER_Y as f32);
        frame.chevron = Some([(x, y - 9.0), (x - 8.0, y + 8.0), (x + 8.0, y + 8.0)]);
    }
    let home_end = track.home_end();
    let mut step = MIN_STEP;
    loop {
        let fits = thin(frame, track, centre, turn, step, home_end) && bucket(frame);
        if fits || step > 1_000.0 {
            break;
        }
        step *= 1.5;
    }
}

/// Keep the points at least `step` pixels apart, the first, the last and
/// the end of the way home always. False when they do not fit.
fn thin(
    frame: &mut Frame,
    track: &Track,
    centre: Point,
    turn: (f32, f32),
    step: f32,
    home_end: Option<usize>,
) -> bool {
    let total = track.total();
    let (mut count, mut last) = (0, (f32::MAX, f32::MAX));
    let mut forced = false;
    for i in 0..total {
        let p = project(track.at(i), centre, turn, frame.scale);
        let (dx, dy) = (p.0 - last.0, p.1 - last.1);
        let wanted =
            i == 0 || i + 1 == total || Some(i) == home_end || dx * dx + dy * dy >= step * step;
        if !wanted {
            continue;
        }
        // The last point stands in for a kept one it all but repeats, or is
        // dropped when that one has to stay.
        if i + 1 == total && dx * dx + dy * dy < step * step && Some(i) != home_end {
            if forced {
                continue;
            }
            count -= 1;
        }
        forced = i == 0 || Some(i) == home_end;
        if count == MAX_POINTS {
            return false;
        }
        if Some(i) == home_end {
            frame.far = count;
        }
        frame.points[count] = p;
        count += 1;
        last = p;
    }
    frame.count = count;
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
    if let Some([tip, left, right]) = frame.chevron {
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
    row.each_over(left + first..left + last, |x, under| {
        let held = track[x - left];
        let mut color = match held & 15 {
            0 => under,
            c => blend(frame.colors[usize::from(held >> 4)], under, c),
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
        c => blend(frame.colors[usize::from(held >> 4)], under, c),
    };
    if let Some([tip, left, right]) = frame.chevron {
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

/// The longest of 1, 2 and 5 times a power of ten metres that is no more
/// than `limit`.
fn nice(limit: f32) -> u32 {
    let mut best = 1;
    let mut decade = 1u32;
    while decade as f32 <= limit {
        for times in [1, 2, 5] {
            if (decade * times) as f32 <= limit {
                best = decade * times;
            }
        }
        decade *= 10;
    }
    best
}

/// "100 M" or "2 KM".
fn length(text: &mut Text<12>, metres: u32) {
    use core::fmt::Write;
    let _ = if metres >= 1000 {
        write!(text, "{} KM", metres / 1000)
    } else {
        write!(text, "{metres} M")
    };
}

/// The text, the scale and the marks that lie over the track.
pub(super) fn prepare(view: &View) -> Canvas {
    let frame = &view.map;
    let mut canvas = Canvas::default();
    let width = RIGHT - LEFT;
    canvas.caption(b"MAP", width, LEFT, HEAD, Align::Left);
    match frame.show {
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
    if frame.segments() == 0 {
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
    canvas.caption(label.as_bytes(), 60, LEFT + bar + 10, FOOT, Align::Left);
    let mut distance = Text::<12>::new();
    kilometres(&mut distance, view.totals.distance_mm());
    let _ = distance.push_str(" KM");
    canvas.fit(
        &[&CAPTION],
        distance.as_bytes(),
        width - bar - 80,
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
        assert_eq!(frame.count, 3);
        assert!(frame.rider && frame.chevron.is_none() && !frame.heading_up);
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
        assert!(frame.chevron.is_some() && !frame.rider && frame.heading_up);
        // Riding north, the track runs straight down from the rider.
        let (a, b) = frame.segment(0);
        assert!((a.0 - b.0).abs() < 0.01 && a.1 > b.1);
    }

    #[test]
    fn the_zoom_fits_the_whole_track_and_is_never_closer_than_500_m() {
        let near = zoom(40.0);
        assert!((near * 500.0 - WIDTH_PX).abs() < 0.01, "{near}");
        // A track reaching 3 km from the rider fits below them.
        let far = zoom(3_000.0);
        assert!(3_000.0 * far <= FIT_PX + 0.01, "{far}");
        assert!(3_000.0 * far > FIT_PX / STEP - 0.01, "closest step: {far}");
        // Farther is never closer; a huge track stops at the widest zoom.
        assert!(zoom(30_000.0) < far);
        assert_eq!(zoom(1e9), zoom(2e9));
        // Every point of a track is inside the page at the zoom.
        let c = crumbs(&[
            (0.0, 0.0),
            (1_500.0, 800.0),
            (-700.0, 2_300.0),
            (400.0, -900.0),
        ]);
        let frame = filled(&track(&c));
        for &(x, y) in &frame.points[..frame.count] {
            assert!((LEFT as f32..RIGHT as f32).contains(&x), "{x}");
            assert!((TOP as f32..BOTTOM as f32).contains(&y), "{y}");
        }
    }

    #[test]
    fn close_points_are_dropped_and_the_budget_holds() {
        // A thousand points half a metre apart collapse to a few.
        let line: std::vec::Vec<_> = (0..1000).map(|i| (i as f32 * 0.5, 0.0)).collect();
        let c = crumbs(&line);
        let frame = filled(&track(&c));
        assert!(frame.count < 100, "{}", frame.count);
        let first = frame.points[0];
        let last = frame.points[frame.count - 1];
        assert!(first.0 < last.0, "the ends stay");
        for pair in frame.points[..frame.count - 1].windows(2) {
            let (dx, dy) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
            assert!(dx * dx + dy * dy >= MIN_STEP * MIN_STEP - 0.01);
        }
        // A wandering 4,096-point track still fits points and buckets.
        let wander: std::vec::Vec<_> = (0..4096)
            .map(|i| {
                let a = i as f32 * 0.37;
                (sinf(a) * 400.0 + i as f32, cosf(a * 1.3) * 400.0)
            })
            .collect();
        let c = crumbs(&wander);
        let frame = filled(&track(&c));
        assert!(frame.count <= MAX_POINTS);
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
    fn following_paints_the_way_home_green_to_the_matched_point() {
        let c = crumbs(&[(0.0, 0.0), (0.0, 100.0), (0.0, 200.0), (100.0, 200.0)]);
        let mut t = track(&c);
        // The rider is on the second segment, 50 m along it.
        t.home = Some((2, Some((0.0, 150.0))));
        t.here = Some((5.0, 150.0));
        let frame = filled(&t);
        assert_eq!(frame.colors, [GREEN, SECONDARY]);
        // Points: start, first bend, the matched point, second bend, end.
        assert_eq!(frame.count, 5);
        assert_eq!(frame.far, 2);
        assert_eq!(frame.color(1), 0);
        assert_eq!(frame.color(2), 1);
        // Frozen: the rider's fix is not a point of the track.
        assert!(frame.following);
        // Without a match yet, the whole track is the way home.
        t.home = Some((3, None));
        let frame = filled(&t);
        assert_eq!(frame.count, 4);
        assert_eq!(frame.far, 3);
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

    fn page(track: Track<'_>, distance_mm: u64) -> View<'_> {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Map;
        view.radar_paired = true;
        view.track = track;
        view.totals.add(1000, 8333, &Default::default(), None);
        let _ = distance_mm;
        view.prepare();
        view
    }

    #[test]
    fn every_state_lays_out_its_text() {
        let none = Track::default();
        page(none, 0)
            .canvas
            .assert_laid_out(PAGE, &["MAP", "NO TRACK"], "unavailable");
        let waiting = Track {
            available: true,
            ..Track::default()
        };
        page(waiting, 0).canvas.assert_laid_out(
            PAGE,
            &["MAP", "NO GPS YET", "MAP STARTS WITH GPS"],
            "no gps",
        );
        let c = crumbs(&[(0.0, 0.0), (0.0, 800.0), (300.0, 900.0)]);
        let mut t = track(&c);
        page(t, 0)
            .canvas
            .assert_laid_out(PAGE, &["MAP", "N UP", "500 M", "0.01 KM"], "still");
        t.heading = Some(0);
        page(t, 0)
            .canvas
            .assert_laid_out(PAGE, &["MAP", "500 M", "0.01 KM"], "riding");
        t.home = Some((2, None));
        t.heading = None;
        page(t, 0)
            .canvas
            .assert_laid_out(PAGE, &["MAP", "FOLLOWING  N UP"], "following");
        t.here = None;
        page(t, 0)
            .canvas
            .assert_laid_out(PAGE, &["MAP", "NO GPS"], "fix lost");
        let start = crumbs(&[(0.0, 0.0)]);
        page(track(&start), 0).canvas.assert_laid_out(
            PAGE,
            &["MAP", "DRAWS AS YOU RIDE", "100 M"],
            "first fix",
        );
    }

    #[test]
    fn the_scale_bar_is_one_two_or_five() {
        assert_eq!(nice(190.0), 100);
        assert_eq!(nice(380.0), 200);
        assert_eq!(nice(999.0), 500);
        assert_eq!(nice(70.0 / 0.0184), 2000);
        assert_eq!(nice(0.5), 1);
        let mut text = Text::<12>::new();
        length(&mut text, 500);
        assert_eq!(text.as_bytes(), b"500 M");
        let mut text = Text::<12>::new();
        length(&mut text, 2000);
        assert_eq!(text.as_bytes(), b"2 KM");
    }
}
