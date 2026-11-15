//! Where the map stands each fix: its zoom step and the rider's place on
//! the screen, heading up.
//!
//! Riding a course, the rider sits low, at `RIDER_Y`, and the step holds
//! the target: the next turn, else the finish. Each step and column is
//! ranked by what it keeps clear of the badge (`Clear`), and the closest
//! step with the best rank wins; within `CLOSE_M` of the turn it is
//! `Step::M300`. A wider step is taken at once, a closer one only once the
//! target and the route up to it fit `SPARE_PX` further from the badge.
//! The rider's column starts by the turn's side on a `GRID`; a column that
//! still fits is kept, then columns away from the side the route goes to
//! are tried at every step before those towards it, and those whose route
//! in view keeps `EDGE` from both edges first. Off course the rider and the
//! missed junction share the screen, the lower of them at `JUNCTION_Y`.
//! With no course, the map is `Step::Km1` across with the rider mid-map.
use crate::ride::course::nav::Glyph;
use crate::ride::homing::TURN;
use core::f32::consts::TAU;
use core::ops::Range;
use libm::{cosf, sinf, sqrtf};

pub type Point = (f32, f32);

pub const WIDTH: f32 = crate::screens::chrome::WIDTH as f32;
/// The map's rows; the footer is below.
pub const GROUND: Range<usize> = 22..290;
pub const RIDER_Y: i16 = 266;
pub const JUNCTION_Y: i16 = 242;
/// With no turn to aim at, the rider is mid-map, the track behind in view.
const FREE_Y: i16 = ((GROUND.start + GROUND.end) / 2) as i16;
const GRID: i16 = 16;
const EDGE: f32 = 24.0;
const CLEARANCE: f32 = 8.0;
const SPARE_PX: f32 = 6.0;
const CLOSE_M: f32 = 100.0;
/// What must clear the badge: the route's casing, and the rider's puck.
/// Past the turn only the route's blue line must, since map-approach
/// draws its casing under the badge's corner.
const ROUTE_REACH: f32 = 9.0;
const LINE_REACH: f32 = 6.0;
const PUCK_REACH: f32 = 14.5;
const JUNCTION_REACH: f32 = 12.0;

/// The zoom steps, by metres across the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step {
    #[default]
    M300,
    M400,
    Km1,
    Km3,
}

impl Step {
    pub const ALL: [Self; 4] = [Self::M300, Self::M400, Self::Km1, Self::Km3];

    const fn span_m(self) -> f32 {
        match self {
            Self::M300 => 300.0,
            Self::M400 => 400.0,
            Self::Km1 => 1_000.0,
            Self::Km3 => 3_000.0,
        }
    }

    pub fn px_per_m(self) -> f32 {
        WIDTH / self.span_m()
    }

    /// The scale bar's length in pixels, and what it reads.
    pub const fn bar(self) -> (usize, &'static [u8]) {
        match self {
            Self::M300 => (80, b"100 M"),
            Self::M400 => (60, b"100 M"),
            Self::Km1 => (60, b"250 M"),
            Self::Km3 => (80, b"1 KM"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pose {
    pub step: Step,
    /// The rider's screen position.
    pub rider: (i16, i16),
    /// The heading that is up, in `TURN`ths clockwise from north.
    pub heading: u16,
}

/// A pose about one point of the ride: east-north metres to screen pixels.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    here: Point,
    sin: f32,
    cos: f32,
    scale: f32,
    rider: Point,
}

impl Pose {
    pub fn projection(&self, here: Point) -> Projection {
        let (sin, cos) = rotation(self.heading);
        Projection {
            here,
            sin,
            cos,
            scale: self.step.px_per_m(),
            rider: (f32::from(self.rider.0), f32::from(self.rider.1)),
        }
    }
}

impl Projection {
    pub fn at(&self, p: Point) -> Point {
        let (right, ahead) = axes(p, self.here, (self.sin, self.cos));
        (
            self.rider.0 + right * self.scale,
            self.rider.1 - ahead * self.scale,
        )
    }
}

fn rotation(heading: u16) -> (f32, f32) {
    let turns = f32::from(heading % TURN) / f32::from(TURN);
    (sinf(turns * TAU), cosf(turns * TAU))
}

/// Metres to the right of and ahead of `centre`, facing the heading.
fn axes(p: Point, centre: Point, (sin, cos): (f32, f32)) -> Point {
    let (east, north) = (p.0 - centre.0, p.1 - centre.1);
    (east * cos - north * sin, east * sin + north * cos)
}

/// The badge's box, which the route, the turn and the rider keep clear of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keepout {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub radius: f32,
}

impl Keepout {
    /// The rider's columns, an open range, at which the segment from `a`
    /// to `b` comes within `pad` of the box. `a` and `b` are pixels right
    /// of the rider's column, and screen rows.
    fn blocked(&self, a: Point, b: Point, pad: f32) -> Option<(f32, f32)> {
        let (top, bottom) = (self.top - pad, self.bottom + pad);
        let rise = b.1 - a.1;
        let (from, to) = if rise.abs() < 1e-6 {
            ((top < a.1 && a.1 < bottom).then_some((0.0, 1.0)))?
        } else {
            let (p, q) = ((top - a.1) / rise, (bottom - a.1) / rise);
            (p.min(q).max(0.0), p.max(q).min(1.0))
        };
        if from > to {
            return None;
        }
        let x = |t: f32| a.0 + t * (b.0 - a.0);
        let (low, high) = (x(from).min(x(to)), x(from).max(x(to)));
        Some((self.left - pad - high, self.right + pad - low))
    }

    /// Whether the segment from `a` to `b` comes within `pad` of the
    /// badge's rounded box.
    fn near(&self, a: Point, b: Point, pad: f32) -> bool {
        let r = self.radius;
        let inner = Keepout {
            left: self.left + r,
            top: self.top + r,
            right: self.right - r,
            bottom: self.bottom - r,
            radius: 0.0,
        };
        if inner
            .blocked(a, b, 0.0)
            .is_some_and(|(low, high)| low <= 0.0 && 0.0 <= high)
        {
            return true;
        }
        let corners = [
            (inner.left, inner.top),
            (inner.right, inner.top),
            (inner.left, inner.bottom),
            (inner.right, inner.bottom),
        ];
        let reach = r + pad;
        corners
            .iter()
            .any(|&c| distance_to_segment(c, a, b) < reach)
            || [a, b].iter().any(|&p| inner.distance_to(p) < reach)
    }

    fn distance_to(&self, (x, y): Point) -> f32 {
        let dx = (self.left - x).max(x - self.right).max(0.0);
        let dy = (self.top - y).max(y - self.bottom).max(0.0);
        sqrtf(dx * dx + dy * dy)
    }
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = match length > 0.0 {
        true => (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0),
        false => 0.0,
    };
    let (x, y) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    sqrtf(x * x + y * y)
}

/// What the map is aimed at this fix, in the ride's frame.
#[derive(Clone, Copy, Debug)]
pub struct Aim {
    pub here: Point,
    pub heading: u16,
    pub target: Target,
    /// The map's left edge: 0, or the right of a radar's strip.
    pub left: f32,
    pub badge: Option<Keepout>,
}

/// What the map holds in view besides the rider.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// Riding without a course.
    Free,
    /// The next turn's corner, and its glyph.
    Turn(Point, Glyph),
    /// The course's end, with no turn left before it.
    Finish(Point),
    /// Off course: where the rider left the course.
    Junction(Point),
}

impl Aim {
    /// Metres right of and ahead of the rider.
    fn relative(&self, p: Point) -> Point {
        axes(p, self.here, rotation(self.heading))
    }
}

impl Target {
    fn moved(self, by: impl Fn(Point) -> Point) -> Self {
        match self {
            Self::Free => Self::Free,
            Self::Turn(at, glyph) => Self::Turn(by(at), glyph),
            Self::Finish(at) => Self::Finish(by(at)),
            Self::Junction(at) => Self::Junction(by(at)),
        }
    }
}

/// How much of the map a pose keeps clear of the badge, least first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Clear {
    #[default]
    Nothing,
    Rider,
    /// The rider and the route in view, with the target off the screen.
    RiderAndRoute,
    /// The rider and the target, on the screen.
    Target,
    /// Those and the route up to the target.
    UpToTarget,
    /// Those and the route past the target, by its blue line.
    Everything,
}

const COLUMNS: usize = 16;

/// The columns the rider may take: `start`, those left of it nearest
/// first, then those right of it; and how many are left of it.
fn columns(start: i16, left: f32) -> (heapless::Vec<i16, COLUMNS>, usize) {
    let (low, high) = ((left + EDGE) as i16, WIDTH as i16 - EDGE as i16);
    let down = (1..).map(|k| start - k * GRID).take_while(|&x| x >= low);
    let up = (1..).map(|k| start + k * GRID).take_while(|&x| x <= high);
    let mut all = heapless::Vec::new();
    let _ = all.push(start);
    all.extend(down);
    let count = all.len() - 1;
    all.extend(up);
    (all, count)
}

/// A step tried for the pose: the rider's row, and per column of
/// `columns(start)` what stays clear and whether the route in view also
/// keeps its margin (`roomy`).
struct Trial {
    y: i16,
    clear: [Clear; COLUMNS],
    roomy: u16,
    left_first: bool,
}

/// The pose for this fix. `route` calls back with the route ahead in order,
/// east-north metres, each point with whether it is before the target;
/// `prev` is the last pose, for hysteresis.
pub fn choose_pose(
    prev: Option<Pose>,
    aim: &Aim,
    route: impl Fn(&mut dyn FnMut(Point, bool)),
) -> Pose {
    let target = aim.target.moved(|p| aim.relative(p));
    let centre = ((aim.left + WIDTH) / 2.0) as i16;
    let start = centre
        + match target {
            Target::Turn(_, Glyph::Left | Glyph::SharpLeft) => 40,
            Target::Turn(_, Glyph::UTurn) | Target::Free | Target::Finish(_) => 0,
            Target::Turn(..) | Target::Junction(_) => -40,
        };
    let close = matches!(target, Target::Turn((r, a), _) if sqrtf(r * r + a * a) <= CLOSE_M);
    let steps: &[Step] = match target {
        Target::Free => &[Step::Km1],
        Target::Junction(_) => &Step::ALL[1..],
        Target::Turn(..) if close => &Step::ALL[..1],
        Target::Turn(..) | Target::Finish(_) => &Step::ALL,
    };
    let last = steps.len() - 1;
    let (columns, left_count) = columns(start, aim.left);
    let trials: heapless::Vec<Option<Trial>, 4> = steps
        .iter()
        .enumerate()
        .map(|(i, &step)| {
            let spare = !close && prev.is_some_and(|prev| prev.step > step);
            let margin = if spare { SPARE_PX } else { 0.0 };
            try_step(step, target, &columns, margin, i == last, aim, &route)
        })
        .collect();
    let mut best: Option<(Clear, Pose)> = None;
    for toward in [false, true] {
        for (trial, &step) in trials.iter().zip(steps) {
            let Some(trial) = trial else {
                continue;
            };
            let kept = prev
                .filter(|prev| prev.step == step && !toward)
                .and_then(|prev| columns.iter().position(|&x| x == prev.rider.0));
            let on_toward_side = |k: usize| k != 0 && (k <= left_count) != trial.left_first;
            let order: heapless::Vec<usize, COLUMNS> = kept
                .into_iter()
                .chain(
                    (0..columns.len()).filter(|&k| on_toward_side(k) == toward && Some(k) != kept),
                )
                .collect();
            for roomy in [true, false] {
                for &k in &order {
                    if roomy && trial.roomy >> k & 1 == 0 {
                        continue;
                    }
                    let clear = trial.clear[k];
                    let pose = Pose {
                        step,
                        rider: (columns[k], trial.y),
                        heading: aim.heading,
                    };
                    if clear == Clear::Everything {
                        return pose;
                    }
                    if best.is_none_or(|(most, _)| clear > most) {
                        best = Some((clear, pose));
                    }
                }
            }
        }
    }
    match best {
        Some((clear, pose)) if clear > Clear::Nothing => pose,
        _ => Pose {
            step: steps[last],
            rider: (
                start,
                if matches!(target, Target::Free) {
                    FREE_Y
                } else {
                    RIDER_Y
                },
            ),
            heading: aim.heading,
        },
    }
}

/// What `step` keeps clear at each column, with the target and the route
/// up to it held `margin` further from the badge. `None` when the step
/// does not hold the target and is not the `widest`.
fn try_step(
    step: Step,
    target: Target,
    columns: &[i16],
    margin: f32,
    widest: bool,
    aim: &Aim,
    route: &impl Fn(&mut dyn FnMut(Point, bool)),
) -> Option<Trial> {
    let (left, keepout) = (aim.left, aim.badge);
    let s = step.px_per_m();
    let (top, bottom) = (GROUND.start as f32 + EDGE + margin, GROUND.end as f32);
    let (y, holds) = match target {
        Target::Junction((_, ahead)) if ahead <= 0.0 => {
            let y = f32::from(JUNCTION_Y) + ahead * s;
            (libm::roundf(y) as i16, y >= top)
        }
        Target::Junction((_, ahead)) => (JUNCTION_Y, f32::from(JUNCTION_Y) - ahead * s >= top),
        Target::Free => (FREE_Y, true),
        Target::Turn((_, ahead), _) | Target::Finish((_, ahead)) => (
            RIDER_Y,
            (top..bottom).contains(&(f32::from(RIDER_Y) - ahead * s)),
        ),
    };
    if !holds && !widest {
        return None;
    }
    let y = if holds { y } else { RIDER_Y };
    let screen = |(right, ahead): Point| (right * s, f32::from(y) - ahead * s);
    let all = (1u16 << columns.len()) - 1;
    let block = |a: Point, b: Point, pad: f32, mask: &mut u16| {
        let Some((low, high)) = keepout.and_then(|k| k.blocked(a, b, pad)) else {
            return;
        };
        for (k, &x) in columns.iter().enumerate() {
            if low < f32::from(x) && f32::from(x) < high {
                *mask &= !(1 << k);
            }
        }
    };
    let past_the_turn = |a: Point, b: Point, mask: &mut u16| {
        let Some(keep) = keepout else {
            return;
        };
        let Some((low, high)) = keep.blocked(a, b, LINE_REACH) else {
            return;
        };
        for (k, &x) in columns.iter().enumerate() {
            let x = f32::from(x);
            let shift = |(px, py): Point| (px + x, py);
            if low < x && x < high && keep.near(shift(a), shift(b), LINE_REACH) {
                *mask &= !(1 << k);
            }
        }
    };
    let mut rider = all;
    block(
        (0.0, f32::from(y)),
        (0.0, f32::from(y)),
        CLEARANCE + PUCK_REACH,
        &mut rider,
    );
    let goal = match target {
        Target::Free => None,
        Target::Turn(at, _) | Target::Finish(at) => {
            Some((screen(at), margin + CLEARANCE + ROUTE_REACH))
        }
        Target::Junction(at) => Some((screen(at), margin + CLEARANCE + JUNCTION_REACH)),
    };
    let mut shown = if holds { all } else { 0 };
    if let Some((at, pad)) = goal {
        block(at, at, pad, &mut shown);
        for (k, &x) in columns.iter().enumerate() {
            if !(left + EDGE..=WIDTH - EDGE).contains(&(f32::from(x) + at.0)) {
                shown &= !(1 << k);
            }
        }
    }
    let (mut to_goal, mut whole) = (all, all);
    let (mut least, mut most) = (0.0f32, 0.0f32);
    let mut last: Option<(Point, bool)> = None;
    route(&mut |p, before| {
        let p = screen(aim.relative(p));
        if (GROUND.start as f32..bottom).contains(&p.1) {
            least = least.min(p.0);
            most = most.max(p.0);
        }
        if let Some((from, was_before)) = last {
            past_the_turn(from, p, &mut whole);
            if was_before {
                let to = match (before, goal) {
                    (false, Some((corner, _))) => corner,
                    _ => p,
                };
                block(from, to, margin + CLEARANCE + ROUTE_REACH, &mut to_goal);
            }
        }
        last = Some((p, before));
    });
    let bit = |mask: u16, k: usize| mask >> k & 1 == 1;
    let mut clear = [Clear::Nothing; COLUMNS];
    let mut roomy = 0;
    for (k, &x) in columns.iter().enumerate() {
        let route = bit(to_goal, k) && bit(whole, k);
        clear[k] = match (bit(rider, k), bit(shown, k), bit(to_goal, k), route) {
            (false, ..) => Clear::Nothing,
            (true, true, _, true) => Clear::Everything,
            (true, true, true, false) => Clear::UpToTarget,
            (true, true, false, false) => Clear::Target,
            (true, false, _, true) => Clear::RiderAndRoute,
            (true, false, _, false) => Clear::Rider,
        };
        let x = f32::from(x);
        if x + least >= left + EDGE && x + most <= WIDTH - EDGE {
            roomy |= 1 << k;
        }
    }
    Some(Trial {
        y,
        clear,
        roomy,
        left_first: most >= -least,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The course the three map mockups draw, in metres from where
    /// map-approach's rider is, heading north: 250 m on, a right turn that
    /// bends 183 m east and 66 m north, then north again.
    const COURSE: [Point; 9] = [
        (0.0, 0.0),
        (0.0, 250.0),
        (25.0, 248.0),
        (60.0, 260.0),
        (95.0, 280.0),
        (150.0, 310.0),
        (183.0, 316.0),
        (184.0, 360.0),
        (180.0, 700.0),
    ];
    const TURN_AT: Point = (0.0, 250.0);

    fn badge(width: f32) -> Option<Keepout> {
        Some(Keepout {
            left: 8.0,
            top: 30.0,
            right: 8.0 + width,
            bottom: 90.0,
            radius: 12.0,
        })
    }

    /// The course from `from` metres north on the first leg, each point
    /// marked before or after the turn.
    fn ahead(before: f32) -> impl Fn(&mut dyn FnMut(Point, bool)) {
        let from = TURN_AT.1 - before;
        move |visit| {
            visit((0.0, from), from < TURN_AT.1);
            for &p in COURSE.iter().filter(|p| p.0 > 0.0 || p.1 > from) {
                visit(p, p.0 == 0.0 && p.1 <= TURN_AT.1);
            }
        }
    }

    fn on_course(before: f32) -> Aim {
        Aim {
            here: (0.0, TURN_AT.1 - before),
            heading: 0,
            target: Target::Turn(TURN_AT, Glyph::Right),
            left: 0.0,
            badge: None,
        }
    }

    fn posed(
        prev: Option<Pose>,
        aim: Aim,
        route: impl Fn(&mut dyn FnMut(Point, bool)),
        badge: Option<Keepout>,
    ) -> Pose {
        choose_pose(prev, &Aim { badge, ..aim }, route)
    }

    #[test]
    fn the_poses_reproduce_the_three_map_mockups() {
        let approach = posed(None, on_course(250.0), ahead(250.0), badge(137.0));
        assert_eq!(
            (approach.step, approach.rider),
            (Step::M400, (80, 266)),
            "map-approach"
        );
        let close = posed(Some(approach), on_course(40.0), ahead(40.0), badge(118.0));
        assert_eq!(
            (close.step, close.rider),
            (Step::M300, (64, 266)),
            "map-close"
        );
        let off = Aim {
            here: (0.0, 390.0),
            target: Target::Junction(TURN_AT),
            ..on_course(-140.0)
        };
        let off = posed(Some(close), off, ahead(0.0), badge(152.0));
        assert_eq!(
            (off.step, off.rider),
            (Step::M400, (80, 158)),
            "map-offcourse"
        );
    }

    #[test]
    fn a_projection_puts_the_heading_up_and_the_rider_where_posed() {
        let pose = Pose {
            step: Step::M400,
            rider: (80, 266),
            heading: TURN / 4,
        };
        let at = pose.projection((100.0, 50.0));
        assert_eq!(at.at((100.0, 50.0)), (80.0, 266.0));
        let east = at.at((200.0, 50.0));
        assert!((east.0 - 80.0).abs() < 1e-3 && (east.1 - 206.0).abs() < 1e-3);
        let north = at.at((100.0, 150.0));
        assert!((north.0 - 20.0).abs() < 1e-3 && (north.1 - 266.0).abs() < 1e-3);
    }

    #[test]
    fn the_turn_under_the_badge_zooms_out_and_zooming_in_waits_for_room() {
        let wide = posed(None, on_course(250.0), ahead(250.0), badge(137.0));
        assert_eq!(wide.step, Step::M400);
        let no_badge = posed(None, on_course(250.0), ahead(250.0), None);
        assert_eq!(no_badge.step, Step::M300, "only the badge pushed it out");
        let fresh = posed(None, on_course(195.0), ahead(195.0), badge(137.0));
        assert_eq!(fresh.step, Step::M300);
        let near = posed(Some(wide), on_course(195.0), ahead(195.0), badge(137.0));
        assert_eq!(near.step, Step::M400);
        let nearer = posed(Some(near), on_course(180.0), ahead(180.0), badge(137.0));
        assert_eq!(nearer.step, Step::M300);
        let far = Pose {
            step: Step::Km3,
            ..wide
        };
        let arriving = posed(Some(far), on_course(250.0), ahead(250.0), badge(137.0));
        assert_eq!(arriving.step, Step::M400);
        let close = posed(Some(wide), on_course(90.0), ahead(90.0), badge(137.0));
        assert_eq!(close.step, Step::M300);
    }

    #[test]
    fn a_far_turn_pans_rather_than_run_the_route_past_it_under_the_badge() {
        let far = Aim {
            target: Target::Turn((0.0, 900.0), Glyph::Right),
            ..on_course(250.0)
        };
        let straight = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 900.0), true);
            visit((0.0, 3_000.0), false);
        };
        let pose = posed(None, far, straight, badge(137.0));
        assert_eq!(
            (pose.step, pose.rider),
            (Step::Km1, (176, 266)),
            "3 km across runs the route past the turn up under the badge"
        );
        let left = Aim {
            target: Target::Turn((0.0, 90.0), Glyph::Left),
            ..on_course(250.0)
        };
        let to_the_left = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 90.0), true);
            visit((-60.0, 100.0), false);
        };
        let pose = posed(None, left, to_the_left, badge(137.0));
        assert_eq!((pose.step, pose.rider), (Step::M300, (160, 266)));
    }

    #[test]
    fn a_route_running_off_the_edge_pans_but_never_zooms_out() {
        let aim = Aim {
            target: Target::Turn((0.0, 200.0), Glyph::Right),
            ..on_course(250.0)
        };
        let east = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 200.0), true);
            visit((500.0, 200.0), false);
        };
        let pose = posed(None, aim, east, badge(137.0));
        assert_eq!((pose.step, pose.rider), (Step::M400, (80, 266)));
    }

    #[test]
    fn a_column_that_still_fits_is_kept() {
        let held = Pose {
            step: Step::M400,
            rider: (96, 266),
            heading: 0,
        };
        let pose = posed(Some(held), on_course(250.0), ahead(250.0), badge(137.0));
        assert_eq!(pose.rider, (96, 266));
    }

    const BADGE: Keepout = Keepout {
        left: 8.0,
        top: 30.0,
        right: 145.0,
        bottom: 90.0,
        radius: 12.0,
    };

    fn toward(turn: Point, glyph: Glyph) -> Aim {
        Aim {
            here: (0.0, 0.0),
            heading: 0,
            target: Target::Turn(turn, glyph),
            left: 0.0,
            badge: None,
        }
    }

    /// Calls back with `points`, each marked before the turn until the
    /// first that is not.
    fn route(points: &[(Point, bool)]) -> impl Fn(&mut dyn FnMut(Point, bool)) + '_ {
        move |visit| points.iter().for_each(|&(p, before)| visit(p, before))
    }

    /// How near the badge the route's casing comes, in pixels past its
    /// edge, sampled every quarter pixel along each segment on the ground.
    fn nearest_to_badge(pose: Pose, points: &[Point], badge: Keepout) -> f32 {
        let at = pose.projection((0.0, 0.0));
        let gap = |(x, y): Point| {
            let dx = (badge.left - x).max(x - badge.right).max(0.0);
            let dy = (badge.top - y).max(y - badge.bottom).max(0.0);
            sqrtf(dx * dx + dy * dy) - ROUTE_REACH
        };
        let mut nearest = f32::INFINITY;
        for pair in points.windows(2) {
            let (a, b) = (at.at(pair[0]), at.at(pair[1]));
            let length = sqrtf((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1));
            let samples = (length * 4.0) as usize + 1;
            for k in 0..=samples {
                let t = k as f32 / samples as f32;
                let p = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
                if (GROUND.start as f32..GROUND.end as f32).contains(&p.1) {
                    nearest = nearest.min(gap(p));
                }
            }
        }
        nearest
    }

    #[test]
    fn a_pose_taken_on_zooming_in_holds_on_the_next_fix() {
        let aim = toward((101.25, 220.0), Glyph::Right);
        let points = [
            ((0.0, 0.0), true),
            ((0.0, 5.0), true),
            ((101.25, 220.0), false),
            ((400.0, 220.0), false),
        ];
        let wide = Pose {
            step: Step::M400,
            rider: (80, 266),
            heading: 0,
        };
        let first = posed(Some(wide), aim, route(&points), Some(BADGE));
        let next = posed(Some(first), aim, route(&points), Some(BADGE));
        assert_eq!(next, first, "the same fix keeps its pose");
    }

    #[test]
    fn the_route_between_its_points_clears_the_badge() {
        let corner = (103.75, 270.0);
        let points = [(0.0, 0.0), (0.0, 5.0), corner, (400.0, 270.0)];
        let marked = [
            (points[0], true),
            (points[1], true),
            (corner, false),
            (points[3], false),
        ];
        let aim = toward(corner, Glyph::Right);
        let pose = posed(None, aim, route(&marked), Some(BADGE));
        assert!(
            nearest_to_badge(pose, &points[..3], BADGE) >= CLEARANCE,
            "{pose:?} runs the route to the turn under the badge"
        );
    }

    #[test]
    fn a_far_right_turn_pans_the_route_and_the_turn_clear_of_the_badge() {
        for ahead in [2_500.0, 3_400.0] {
            let points = [
                (0.0, 0.0),
                (0.0, ahead),
                (700.0, ahead),
                (700.0, ahead + 2_500.0),
            ];
            let marked = [
                (points[0], true),
                (points[1], false),
                (points[2], false),
                (points[3], false),
            ];
            let aim = toward(points[1], Glyph::Right);
            let pose = posed(None, aim, route(&marked), Some(BADGE));
            assert!(
                nearest_to_badge(pose, &points, BADGE) >= CLEARANCE,
                "{ahead} m: {pose:?} runs the route under the badge"
            );
        }
    }

    #[test]
    fn when_no_pose_clears_the_route_past_the_turn_the_turn_and_the_way_to_it_stay_clear() {
        let corner = (0.0, 100.0);
        let mut marked = std::vec![((0.0, 0.0), true), (corner, false)];
        for k in 0..40 {
            let (from, to) = if k % 2 == 0 {
                (-3_000.0, 3_000.0)
            } else {
                (3_000.0, -3_000.0)
            };
            let north = 150.0 + 100.0 * k as f32;
            marked.push(((from, north), false));
            marked.push(((to, north), false));
        }
        let aim = toward(corner, Glyph::Right);
        let pose = posed(None, aim, route(&marked), Some(BADGE));
        assert_eq!(pose.step, Step::M300, "zooming out clears nothing more");
        assert!(
            nearest_to_badge(pose, &[(0.0, 0.0), corner], BADGE) >= CLEARANCE,
            "{pose:?} covers the turn or the way to it"
        );
    }

    #[test]
    fn without_a_turn_the_map_is_a_kilometre_across_with_the_rider_mid_map() {
        let free = Aim {
            here: (0.0, 0.0),
            heading: 0,
            target: Target::Free,
            left: 0.0,
            badge: None,
        };
        let pose = posed(None, free, |_: &mut dyn FnMut(Point, bool)| {}, None);
        assert_eq!((pose.step, pose.rider), (Step::Km1, (120, 156)));
    }
}
