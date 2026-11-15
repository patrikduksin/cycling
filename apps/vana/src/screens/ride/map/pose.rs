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
const FREE_RIDER_Y: i16 = ((GROUND.start + GROUND.end) / 2) as i16;
const GRID: i16 = 16;
const EDGE: f32 = 24.0;
pub(super) const CLEARANCE: f32 = 8.0;
const SPARE_PX: f32 = 6.0;
const CLOSE_M: f32 = 100.0;
const ROUTE_CASING_HALF_PX: f32 = 9.0;
const ROUTE_LINE_HALF_PX: f32 = 6.0;
const PUCK_RADIUS_PX: f32 = 14.5;
const JUNCTION_RADIUS_PX: f32 = 12.0;
pub(super) const ARROW_SHAFT_PX: f32 = 26.0;
pub(super) const ARROW_BEFORE_TURN_PX: f32 = 10.0;
pub(super) const ARROW_HEAD_LENGTH_PX: f32 = 10.5;
pub(super) const ARROW_HEAD_HALF_WIDTH_PX: f32 = 7.9;
pub(super) const ARROW_OUTLINE_PX: f32 = 1.75;
const ARROW_HALF_PX: f32 = ARROW_HEAD_HALF_WIDTH_PX + ARROW_OUTLINE_PX;

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
        let (right, ahead) = right_and_ahead(p, self.here, (self.sin, self.cos));
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

fn right_and_ahead(p: Point, centre: Point, (sin, cos): (f32, f32)) -> Point {
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
    fn blocked_rider_columns(&self, a: Point, b: Point, pad: f32) -> Option<(f32, f32)> {
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
            .blocked_rider_columns(a, b, 0.0)
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

#[derive(Clone, Copy, Debug)]
pub enum Target {
    Free,
    Turn(Point, Glyph),
    Finish(Point),
    Junction(Point),
}

impl Aim {
    fn relative(&self, p: Point) -> Point {
        right_and_ahead(p, self.here, rotation(self.heading))
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Clear {
    #[default]
    Nothing,
    Rider,
    RiderAndRouteToOffscreenTarget,
    RiderAndTarget,
    RiderTargetAndRouteToIt,
}

const COLUMNS: usize = 16;

fn columns(target: Target, left: f32) -> (heapless::Vec<i16, COLUMNS>, usize) {
    let start = ((left + WIDTH) / 2.0) as i16
        + match target {
            Target::Turn(_, Glyph::Left | Glyph::SharpLeft) => 40,
            Target::Turn(_, Glyph::UTurn) | Target::Free | Target::Finish(_) => 0,
            Target::Turn(..) | Target::Junction(_) => -40,
        };
    let (low, high) = ((left + EDGE) as i16, WIDTH as i16 - EDGE as i16);
    let down = (1..).map(|k| start - k * GRID).take_while(|&x| x >= low);
    let up = (1..)
        .map(|k| start + k * GRID)
        .take_while(|&x| x < high)
        .chain([high]);
    let mut all = heapless::Vec::new();
    let _ = all.push(start);
    all.extend(down);
    let count = all.len() - 1;
    all.extend(up);
    (all, count)
}

struct Trial {
    y: i16,
    clear: [Clear; COLUMNS],
    past_clear: u16,
    roomy: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

fn route_side(target: Target, aim: &Aim, route: &impl Fn(&mut dyn FnMut(Point, bool))) -> Side {
    let ahead_not_beside = |(right, ahead): Point| right.abs() <= ahead;
    let at = match target {
        Target::Turn(at, Glyph::Left | Glyph::SharpLeft) if ahead_not_beside(at) => {
            return Side::Left;
        }
        Target::Turn(at, Glyph::Right | Glyph::SharpRight) if ahead_not_beside(at) => {
            return Side::Right;
        }
        Target::Free => None,
        Target::Turn(at, _) | Target::Finish(at) | Target::Junction(at) => Some(at),
    };
    let (mut furthest_left, mut furthest_right) = (0.0f32, 0.0f32);
    let mut reach = |right: f32| {
        furthest_left = furthest_left.min(right);
        furthest_right = furthest_right.max(right);
    };
    if let Some((right, _)) = at {
        reach(right);
    }
    route(&mut |p, before| {
        if before {
            reach(aim.relative(p).0);
        }
    });
    if furthest_right >= -furthest_left {
        Side::Right
    } else {
        Side::Left
    }
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
    let close = matches!(target, Target::Turn((r, a), _) if sqrtf(r * r + a * a) <= CLOSE_M);
    let steps: &[Step] = match target {
        Target::Free => &[Step::Km1],
        Target::Junction(_) => &Step::ALL[1..],
        Target::Turn(..) if close => &Step::ALL[..1],
        Target::Turn(..) | Target::Finish(_) => &Step::ALL,
    };
    let last = steps.len() - 1;
    let (columns, left_count) = columns(target, aim.left);
    let side = route_side(target, aim, &route);
    let trials: heapless::Vec<Option<Trial>, 4> = steps
        .iter()
        .enumerate()
        .map(|(i, &step)| {
            let spare = !close && prev.is_some_and(|prev| prev.step > step);
            let margin = if spare { SPARE_PX } else { 0.0 };
            try_step(step, target, &columns, margin, i == last, aim, &route)
        })
        .collect();
    let kept_at = |step: Step| {
        prev.filter(|prev| prev.step == step)
            .and_then(|prev| columns.iter().position(|&x| x == prev.rider.0))
    };
    let on_toward_side = |k: usize| k != 0 && (k <= left_count) == (side == Side::Left);
    let mut best: Option<(Clear, usize)> = None;
    'steps: for toward in [false, true] {
        for (i, trial) in trials.iter().enumerate() {
            let Some(trial) = trial else {
                continue;
            };
            let kept = kept_at(steps[i]);
            let side =
                (0..columns.len()).filter(|&k| (on_toward_side(k) && Some(k) != kept) == toward);
            for k in side {
                let clear = trial.clear[k];
                if best.is_none_or(|(most, _)| clear > most) {
                    best = Some((clear, i));
                }
                if clear == Clear::RiderTargetAndRouteToIt {
                    break 'steps;
                }
            }
        }
    }
    let chosen = best
        .filter(|&(clear, _)| clear > Clear::Nothing)
        .and_then(|(clear, i)| {
            let trial = trials[i].as_ref()?;
            let kept = kept_at(steps[i]);
            let bit = |mask: u16, k: usize| mask >> k & 1 == 1;
            let k = (0..columns.len())
                .filter(|&k| trial.clear[k] == clear)
                .min_by_key(|&k| {
                    (
                        !bit(trial.past_clear, k),
                        on_toward_side(k),
                        Some(k) != kept,
                        !bit(trial.roomy, k),
                        k,
                    )
                })?;
            Some(Pose {
                step: steps[i],
                rider: (columns[k], trial.y),
                heading: aim.heading,
            })
        });
    chosen.unwrap_or(Pose {
        step: steps[last],
        rider: (
            columns[0],
            if matches!(target, Target::Free) {
                FREE_RIDER_Y
            } else {
                RIDER_Y
            },
        ),
        heading: aim.heading,
    })
}

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
            (libm::roundf(y.max(top)) as i16, y >= top)
        }
        Target::Junction((_, ahead)) => {
            let holds = f32::from(JUNCTION_Y) - ahead * s >= top;
            (if holds { JUNCTION_Y } else { RIDER_Y }, holds)
        }
        Target::Free => (FREE_RIDER_Y, true),
        Target::Turn((_, ahead), _) | Target::Finish((_, ahead)) => (
            RIDER_Y,
            (top..bottom).contains(&(f32::from(RIDER_Y) - ahead * s)),
        ),
    };
    if !holds && !widest {
        return None;
    }
    let screen = |(right, ahead): Point| (right * s, f32::from(y) - ahead * s);
    let all = (1u16 << columns.len()) - 1;
    let block = |a: Point, b: Point, pad: f32, mask: &mut u16| {
        let Some((low, high)) = keepout.and_then(|k| k.blocked_rider_columns(a, b, pad)) else {
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
        let Some((low, high)) = keep.blocked_rider_columns(a, b, ROUTE_LINE_HALF_PX) else {
            return;
        };
        for (k, &x) in columns.iter().enumerate() {
            let x = f32::from(x);
            let shift = |(px, py): Point| (px + x, py);
            if low < x && x < high && keep.near(shift(a), shift(b), ROUTE_LINE_HALF_PX) {
                *mask &= !(1 << k);
            }
        }
    };
    let mut rider = all;
    block(
        (0.0, f32::from(y)),
        (0.0, f32::from(y)),
        CLEARANCE + PUCK_RADIUS_PX,
        &mut rider,
    );
    let goal = match target {
        Target::Free => None,
        Target::Turn(at, _) | Target::Finish(at) => {
            Some((screen(at), margin + CLEARANCE + ROUTE_CASING_HALF_PX))
        }
        Target::Junction(at) => Some((screen(at), margin + CLEARANCE + JUNCTION_RADIUS_PX)),
    };
    let arrow_past = match (target, goal) {
        (Target::Turn(..), Some((at, _))) => {
            let d = sqrtf(at.0 * at.0 + (at.1 - f32::from(y)) * (at.1 - f32::from(y)));
            ARROW_SHAFT_PX - ARROW_BEFORE_TURN_PX.min(d / 4.0) + ARROW_HEAD_LENGTH_PX
        }
        _ => 0.0,
    };
    let (mut arrow, mut arrow_from) = (all, None);
    let mut shown = if holds { all } else { 0 };
    if let Some((at, pad)) = goal {
        block(at, at, pad, &mut shown);
        for (k, &x) in columns.iter().enumerate() {
            if !(left + EDGE..=WIDTH - EDGE).contains(&(f32::from(x) + at.0)) {
                shown &= !(1 << k);
            }
        }
    }
    let (mut to_goal, mut past_clear) = (all, all);
    let (mut least, mut most) = (0.0f32, 0.0f32);
    let mut last: Option<(Point, bool)> = None;
    route(&mut |p, before| {
        let p = screen(aim.relative(p));
        if (GROUND.start as f32..bottom).contains(&p.1) {
            least = least.min(p.0);
            most = most.max(p.0);
        }
        if let Some((from, was_before)) = last {
            past_the_turn(from, p, &mut past_clear);
            if was_before {
                let to = match (before, goal) {
                    (false, Some((corner, _))) => corner,
                    _ => p,
                };
                block(
                    from,
                    to,
                    margin + CLEARANCE + ROUTE_CASING_HALF_PX,
                    &mut to_goal,
                );
            }
            if let (true, false, Some((corner, _))) = (was_before, before, goal) {
                arrow_from = Some(corner);
            }
            if let Some(corner) = arrow_from {
                let (dx, dy) = (p.0 - corner.0, p.1 - corner.1);
                let length = sqrtf(dx * dx + dy * dy);
                if length >= 1.0 {
                    let t = arrow_past / length;
                    let tip = (corner.0 + t * dx, corner.1 + t * dy);
                    block(corner, tip, CLEARANCE + ARROW_HALF_PX, &mut arrow);
                    arrow_from = None;
                }
            }
        }
        last = Some((p, before));
    });
    shown &= arrow;
    to_goal &= arrow;
    let bit = |mask: u16, k: usize| mask >> k & 1 == 1;
    let mut clear = [Clear::Nothing; COLUMNS];
    let mut roomy = 0;
    for (k, &x) in columns.iter().enumerate() {
        clear[k] = match (bit(rider, k), bit(shown, k), bit(to_goal, k)) {
            (false, ..) => Clear::Nothing,
            (true, true, true) => Clear::RiderTargetAndRouteToIt,
            (true, true, false) => Clear::RiderAndTarget,
            (true, false, true) => Clear::RiderAndRouteToOffscreenTarget,
            (true, false, false) => Clear::Rider,
        };
        let x = f32::from(x);
        if x + least >= left + EDGE && x + most <= WIDTH - EDGE {
            roomy |= 1 << k;
        }
    }
    Some(Trial {
        y,
        clear,
        past_clear,
        roomy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCKUP_COURSE: [Point; 9] = [
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

    fn mockup_route(before: f32) -> impl Fn(&mut dyn FnMut(Point, bool)) {
        let from = TURN_AT.1 - before;
        move |visit| {
            visit((0.0, from), from < TURN_AT.1);
            for &p in MOCKUP_COURSE.iter().filter(|p| p.0 > 0.0 || p.1 > from) {
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
        let approach = posed(None, on_course(250.0), mockup_route(250.0), badge(137.0));
        assert_eq!(
            (approach.step, approach.rider),
            (Step::M400, (80, 266)),
            "map-approach"
        );
        let close = posed(
            Some(approach),
            on_course(40.0),
            mockup_route(40.0),
            badge(118.0),
        );
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
        let off = posed(Some(close), off, mockup_route(0.0), badge(152.0));
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
        let wide = posed(None, on_course(250.0), mockup_route(250.0), badge(137.0));
        assert_eq!(wide.step, Step::M400);
        let no_badge = posed(None, on_course(250.0), mockup_route(250.0), None);
        assert_eq!(no_badge.step, Step::M300, "only the badge pushed it out");
        let fresh = posed(None, on_course(195.0), mockup_route(195.0), badge(137.0));
        assert_eq!(fresh.step, Step::M300);
        let near = posed(
            Some(wide),
            on_course(195.0),
            mockup_route(195.0),
            badge(137.0),
        );
        assert_eq!(near.step, Step::M400);
        let nearer = posed(
            Some(near),
            on_course(180.0),
            mockup_route(180.0),
            badge(137.0),
        );
        assert_eq!(nearer.step, Step::M300);
        let far = Pose {
            step: Step::Km3,
            ..wide
        };
        let arriving = posed(
            Some(far),
            on_course(250.0),
            mockup_route(250.0),
            badge(137.0),
        );
        assert_eq!(arriving.step, Step::M400);
        let close = posed(
            Some(wide),
            on_course(90.0),
            mockup_route(90.0),
            badge(137.0),
        );
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
            pose.step,
            Step::Km3,
            "the turn alone sets the step, from the side away from its route"
        );
        assert_eq!(
            pose.rider,
            (160, 266),
            "the route past the turn pans the rider out from under the badge"
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
    fn a_column_that_still_fits_is_kept_on_its_side_of_the_route() {
        let far = Aim {
            target: Target::Turn((0.0, 900.0), Glyph::Right),
            ..on_course(250.0)
        };
        let straight = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 900.0), true);
            visit((0.0, 3_000.0), false);
        };
        let held = Pose {
            step: Step::Km3,
            rider: (176, 266),
            heading: 0,
        };
        let pose = posed(Some(held), far, straight, badge(137.0));
        assert_eq!(pose.rider, (176, 266), "not 160, as on a first fix");
        let towards = Pose {
            step: Step::M400,
            rider: (96, 266),
            heading: 0,
        };
        let pose = posed(
            Some(towards),
            on_course(250.0),
            mockup_route(250.0),
            badge(137.0),
        );
        assert_eq!(
            pose.rider,
            (80, 266),
            "it moves back away from the route's side once that keeps as much clear"
        );
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

    fn route(points: &[(Point, bool)]) -> impl Fn(&mut dyn FnMut(Point, bool)) + '_ {
        move |visit| points.iter().for_each(|&(p, before)| visit(p, before))
    }

    fn casing_gap_to_badge(pose: Pose, points: &[Point], badge: Keepout) -> f32 {
        let at = pose.projection((0.0, 0.0));
        let gap = |(x, y): Point| {
            let dx = (badge.left - x).max(x - badge.right).max(0.0);
            let dy = (badge.top - y).max(y - badge.bottom).max(0.0);
            sqrtf(dx * dx + dy * dy) - ROUTE_CASING_HALF_PX
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
            casing_gap_to_badge(pose, &points[..3], BADGE) >= CLEARANCE,
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
                casing_gap_to_badge(pose, &points, BADGE) >= CLEARANCE,
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
            casing_gap_to_badge(pose, &[(0.0, 0.0), corner], BADGE) >= CLEARANCE,
            "{pose:?} covers the turn or the way to it"
        );
    }

    #[test]
    fn more_points_on_the_same_route_past_the_turn_change_nothing() {
        let sparse = [
            ((0.0, 0.0), true),
            ((0.0, 220.0), false),
            ((60.0, 220.0), false),
            ((-240.0, 350.0), false),
            ((-240.0, 1_000.0), false),
        ];
        let mut sampled = sparse.to_vec();
        sampled.insert(3, ((-120.0, 298.0), false));
        let aim = toward((0.0, 220.0), Glyph::Right);
        let held = |step| Pose {
            step,
            rider: (80, 266),
            heading: 0,
        };
        for prev in [None, Some(held(Step::Km3)), Some(held(Step::M400))] {
            assert_eq!(
                posed(prev, aim, route(&sampled), Some(BADGE)),
                posed(prev, aim, route(&sparse), Some(BADGE)),
                "after {prev:?}"
            );
        }
    }

    #[test]
    fn a_turn_beside_the_rider_just_after_the_last_one_keeps_the_closest_step() {
        let turn = (120.0, 1_560.0);
        let here = (17.9, 1_508.9);
        let aim = Aim {
            here,
            heading: 0,
            target: Target::Turn(turn, Glyph::Left),
            left: 0.0,
            badge: badge(118.0),
        };
        let points = [
            (here, true),
            (turn, false),
            ((120.0, 1_700.0), false),
            ((0.0, 1_760.0), false),
        ];
        let held = Pose {
            step: Step::M300,
            rider: (160, 266),
            heading: 0,
        };
        let pose = choose_pose(Some(held), &aim, route(&points));
        assert_eq!(
            pose.step,
            Step::M300,
            "{pose:?}: the left turn lies to the right while the heading catches up"
        );
    }

    const LEFT_COURSE: [Point; 5] = [
        (0.0, 0.0),
        (0.0, 2_000.0),
        (-300.0, 2_000.0),
        (-300.0, 2_400.0),
        (200.0, 3_000.0),
    ];

    fn ahead_of(course: &[Point], turn: usize, before: f32) -> std::vec::Vec<(Point, bool)> {
        let here = (0.0, course[turn].1 - before);
        core::iter::once((here, true))
            .chain(
                course
                    .iter()
                    .enumerate()
                    .filter(|&(i, p)| i > turn || p.1 > here.1)
                    .map(|(i, &p)| (p, i <= turn)),
            )
            .collect()
    }

    fn steps_on_the_way(
        course: &[Point],
        turn: usize,
        glyph: Glyph,
        badge: Keepout,
    ) -> std::vec::Vec<Step> {
        let mut prev = None;
        (1..=200)
            .rev()
            .map(|k| {
                let before = 10.0 * k as f32;
                let aim = Aim {
                    here: (0.0, course[turn].1 - before),
                    heading: 0,
                    target: Target::Turn(course[turn], glyph),
                    left: 0.0,
                    badge: Some(badge),
                };
                let points = ahead_of(course, turn, before);
                let pose = choose_pose(prev, &aim, route(&points));
                prev = Some(pose);
                pose.step
            })
            .collect()
    }

    #[test]
    fn the_route_past_the_turn_never_changes_the_step() {
        let mockup: std::vec::Vec<Point> = MOCKUP_COURSE.to_vec();
        let courses = [
            (mockup, 1, Glyph::Right, badge(137.0).unwrap()),
            (LEFT_COURSE.to_vec(), 1, Glyph::Left, BADGE),
        ];
        for (course, turn, glyph, badge) in courses {
            let along = steps_on_the_way(&course, turn, glyph, badge);
            let tx = course[turn].0;
            let mirrored: std::vec::Vec<Point> = course
                .iter()
                .enumerate()
                .map(|(i, &(x, y))| {
                    if i > turn + 1 {
                        (2.0 * tx - x, y)
                    } else {
                        (x, y)
                    }
                })
                .collect();
            let truncated = &course[..turn + 2];
            for (tail, other) in [("mirrored", &mirrored[..]), ("truncated", truncated)] {
                let steps = steps_on_the_way(other, turn, glyph, badge);
                let differ: std::vec::Vec<usize> = (0..steps.len())
                    .filter(|&k| steps[k] != along[k])
                    .map(|k| 10 * (200 - k))
                    .collect();
                assert!(
                    differ.is_empty(),
                    "{glyph:?} turn, {tail} tail: another step at {differ:?} m out"
                );
            }
        }
    }

    #[test]
    fn the_route_past_the_turn_is_kept_clear_whenever_the_chosen_step_and_rank_can() {
        let mockup: std::vec::Vec<Point> = MOCKUP_COURSE.to_vec();
        let mirrored_left: std::vec::Vec<Point> =
            LEFT_COURSE.iter().map(|&(x, y)| (-x, y)).collect();
        let courses = [
            (mockup, Glyph::Right, badge(137.0).unwrap()),
            (LEFT_COURSE.to_vec(), Glyph::Left, BADGE),
            (mirrored_left, Glyph::Right, BADGE),
        ];
        for (course, glyph, badge) in courses {
            let mut prev: Option<Pose> = None;
            for k in (1..=200).rev() {
                let before = 10.0 * k as f32;
                let aim = Aim {
                    here: (0.0, course[1].1 - before),
                    heading: 0,
                    target: Target::Turn(course[1], glyph),
                    left: 0.0,
                    badge: Some(badge),
                };
                let points = ahead_of(&course, 1, before);
                let pose = choose_pose(prev, &aim, route(&points));
                let target = aim.target.moved(|p| aim.relative(p));
                let close = before <= CLOSE_M;
                let spare = !close && prev.is_some_and(|prev| prev.step > pose.step);
                let margin = if spare { SPARE_PX } else { 0.0 };
                let (columns, _) = columns(target, aim.left);
                let trial = try_step(
                    pose.step,
                    target,
                    &columns,
                    margin,
                    true,
                    &aim,
                    &route(&points),
                )
                .unwrap();
                let chosen = columns.iter().position(|&x| x == pose.rider.0).unwrap();
                let clear_past = |k: usize| trial.past_clear >> k & 1 == 1;
                let could = (0..columns.len())
                    .filter(|&k| trial.clear[k] == trial.clear[chosen])
                    .find(|&k| clear_past(k));
                assert!(
                    could.is_none() || clear_past(chosen),
                    "{glyph:?} turn {before} m out: {pose:?} runs the route past the turn under the badge, x {} would not",
                    columns[could.unwrap()]
                );
                prev = Some(pose);
            }
        }
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
