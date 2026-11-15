//! Where the map stands each fix: its zoom step and the rider's place on
//! the screen, heading up.
//!
//! Riding a course, the rider sits low, at `RIDER_Y`. The step is the
//! closest whose screen holds the next turn, with the turn, the route up to
//! it and the rider clear of the badge, and the closest one within
//! `CLOSE_M` of the turn. A wider step is taken at once, a closer one only
//! once the turn fits it `SPARE_PX` farther from the rider. Only the badge
//! zooms the map out: the rider's column starts by the turn's side and
//! moves along a `GRID` away from the side the route ahead goes to, to the
//! first column whose route in view keeps `EDGE` from both edges, else to
//! the first that clears the badge; a column that still fits is kept. Off
//! course the rider and the missed junction share the screen, the lower of
//! them at `JUNCTION_Y`. With no turn to aim at, the map is `Step::Km1`
//! across with the rider mid-map.
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
const ROUTE_REACH: f32 = 9.0;
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
}

impl Keepout {
    fn blocks(&self, (x, y): Point, reach: f32) -> bool {
        let pad = CLEARANCE + reach;
        x > self.left - pad && x < self.right + pad && y > self.top - pad && y < self.bottom + pad
    }
}

/// What the map is aimed at this fix, in the ride's frame.
#[derive(Clone, Copy, Debug)]
pub struct Aim {
    pub here: Point,
    pub heading: u16,
    /// The next turn's start, and its glyph.
    pub turn: Option<(Point, Glyph)>,
    /// Off course: where the rider left the course.
    pub junction: Option<Point>,
}

#[derive(Clone, Copy)]
enum Target {
    /// Riding without a turn ahead.
    Free,
    Turn(Point),
    Junction(Point),
}

/// The columns the rider may take: `start`, then along the grid to one
/// side, or with `both` the other side too.
fn columns(start: i16, left: bool, both: bool) -> heapless::Vec<i16, 16> {
    let (low, high) = (EDGE as i16, WIDTH as i16 - EDGE as i16);
    let down = (1..).map(|k| start - k * GRID).take_while(|&x| x >= low);
    let up = (1..).map(|k| start + k * GRID).take_while(|&x| x <= high);
    let mut all = heapless::Vec::new();
    let _ = all.push(start);
    match (left, both) {
        (true, false) => all.extend(down),
        (false, false) => all.extend(up),
        _ => all.extend(down.chain(up)),
    }
    all
}

/// A step tried for the pose: the rider's row, and per column of
/// `columns(start, true, true)` whether the badge stays clear (`clear`) and the
/// route in view also keeps its margin (`roomy`).
struct Trial {
    y: i16,
    clear: u16,
    roomy: u16,
    left_first: bool,
}

/// The pose for this fix. `route` calls back with the route ahead in order,
/// east-north metres, each point with whether it is before the next turn;
/// `prev` is the last pose, for hysteresis.
pub fn choose_pose(
    prev: Option<Pose>,
    aim: &Aim,
    route: impl Fn(&mut dyn FnMut(Point, bool)),
    keepout: Option<Keepout>,
) -> Pose {
    let turn = rotation(aim.heading);
    let rel = |p: Point| axes(p, aim.here, turn);
    let target = match (aim.junction, aim.turn) {
        (Some(junction), _) => Target::Junction(rel(junction)),
        (None, Some((at, _))) => Target::Turn(rel(at)),
        (None, None) => Target::Free,
    };
    let start = match (target, aim.turn) {
        (Target::Turn(_), Some((_, Glyph::Left | Glyph::SharpLeft))) => 160,
        (Target::Turn(_), Some((_, Glyph::UTurn))) => 120,
        (Target::Free, _) => 120,
        _ => 80,
    };
    let close = matches!(target, Target::Turn((r, a)) if sqrtf(r * r + a * a) <= CLOSE_M);
    let steps: &[Step] = match target {
        Target::Free => &[Step::Km1],
        Target::Junction(_) => &Step::ALL[1..],
        Target::Turn(_) => &Step::ALL,
    };
    let canonical = columns(start, true, true);
    for &step in steps {
        let spare = !close && prev.is_some_and(|prev| prev.step > step);
        let Some(trial) = try_step(step, target, &canonical, spare, &rel, &route, keepout) else {
            continue;
        };
        let kept = prev
            .filter(|prev| prev.step == step)
            .map(|prev| prev.rider.0)
            .filter(|x| canonical.contains(x));
        let preferred = columns(start, trial.left_first, false);
        let ordered = kept
            .into_iter()
            .chain(preferred.into_iter().filter(|&x| Some(x) != kept));
        let mut clear = None;
        for x in ordered {
            let k = canonical.iter().position(|&c| c == x).unwrap_or(0);
            let pose = Pose {
                step,
                rider: (x, trial.y),
                heading: aim.heading,
            };
            if trial.roomy >> k & 1 == 1 {
                return pose;
            }
            if clear.is_none() && trial.clear >> k & 1 == 1 {
                clear = Some(pose);
            }
        }
        // The badge stays clear here; the route may run off the edge.
        if let Some(pose) = clear {
            return pose;
        }
    }
    let y = match target {
        Target::Free => FREE_Y,
        _ => RIDER_Y,
    };
    Pose {
        step: steps[steps.len() - 1],
        rider: (start, y),
        heading: aim.heading,
    }
}

/// Whether `step` holds the target at all and, if so, which columns clear
/// the badge and keep the margin.
fn try_step(
    step: Step,
    target: Target,
    columns: &[i16],
    spare: bool,
    rel: &impl Fn(Point) -> Point,
    route: &impl Fn(&mut dyn FnMut(Point, bool)),
    keepout: Option<Keepout>,
) -> Option<Trial> {
    let s = step.px_per_m();
    let (top, bottom) = (GROUND.start as f32 + EDGE, GROUND.end as f32);
    let y = match target {
        Target::Junction((_, ahead)) if ahead <= 0.0 => {
            let y = f32::from(JUNCTION_Y) + ahead * s;
            (y >= top).then_some(libm::roundf(y) as i16)?
        }
        Target::Junction((_, ahead)) => {
            (f32::from(JUNCTION_Y) - ahead * s >= top).then_some(JUNCTION_Y)?
        }
        Target::Free => FREE_Y,
        Target::Turn(_) => RIDER_Y,
    };
    let screen =
        |x: i16, (right, ahead): Point| (f32::from(x) + right * s, f32::from(y) - ahead * s);
    let forbid = |p: Point, reach: f32, on_screen: bool, clear: &mut u16| {
        for (k, &x) in columns.iter().enumerate() {
            let at = screen(x, p);
            let off = on_screen && !(EDGE..=WIDTH - EDGE).contains(&at.0);
            if off || keepout.is_some_and(|keep| keep.blocks(at, reach)) {
                *clear &= !(1 << k);
            }
        }
    };
    let mut clear = (1u16 << columns.len()) - 1;
    forbid((0.0, 0.0), PUCK_REACH, false, &mut clear);
    match target {
        Target::Free => {}
        Target::Turn(at) => {
            let at = match spare {
                true => {
                    let px = sqrtf(at.0 * at.0 + at.1 * at.1) * s;
                    let farther = (px + SPARE_PX) / px.max(1e-3);
                    (at.0 * farther, at.1 * farther)
                }
                false => at,
            };
            let row = f32::from(y) - at.1 * s;
            if !(top..bottom).contains(&row) {
                return None;
            }
            forbid(at, ROUTE_REACH, true, &mut clear);
        }
        Target::Junction(at) => forbid(at, JUNCTION_REACH, true, &mut clear),
    }
    let (mut least, mut most) = (0.0f32, 0.0f32);
    route(&mut |p, before| {
        let p = rel(p);
        let row = f32::from(y) - p.1 * s;
        if (GROUND.start as f32..bottom).contains(&row) {
            least = least.min(p.0);
            most = most.max(p.0);
        }
        if before {
            forbid(p, ROUTE_REACH, false, &mut clear);
        }
    });
    let mut roomy = clear;
    for (k, &x) in columns.iter().enumerate() {
        let x = f32::from(x);
        if x + least * s < EDGE || x + most * s > WIDTH - EDGE {
            roomy &= !(1 << k);
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
        })
    }

    /// The course from `from` metres north on the first leg, each point
    /// marked before or after the turn.
    fn ahead(from: f32) -> impl Fn(&mut dyn FnMut(Point, bool)) {
        move |visit| {
            visit((0.0, from), from < TURN_AT.1);
            for &p in COURSE.iter().filter(|p| p.0 > 0.0 || p.1 > from) {
                visit(p, p.0 == 0.0 && p.1 <= TURN_AT.1);
            }
        }
    }

    fn on_course(north: f32) -> Aim {
        Aim {
            here: (0.0, north),
            heading: 0,
            turn: Some((TURN_AT, Glyph::Right)),
            junction: None,
        }
    }

    #[test]
    fn the_poses_reproduce_the_three_map_mockups() {
        let approach = choose_pose(None, &on_course(0.0), ahead(0.0), badge(137.0));
        assert_eq!(
            (approach.step, approach.rider),
            (Step::M400, (80, 266)),
            "map-approach"
        );
        let close = choose_pose(
            Some(approach),
            &on_course(210.0),
            ahead(210.0),
            badge(118.0),
        );
        assert_eq!(
            (close.step, close.rider),
            (Step::M300, (64, 266)),
            "map-close"
        );
        let off = Aim {
            here: (0.0, 390.0),
            junction: Some(TURN_AT),
            ..on_course(390.0)
        };
        let off = choose_pose(Some(close), &off, ahead(250.0), badge(152.0));
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
        // Riding east, 100 m east is 60 px up and 100 m north is 60 px left.
        let east = at.at((200.0, 50.0));
        assert!((east.0 - 80.0).abs() < 1e-3 && (east.1 - 206.0).abs() < 1e-3);
        let north = at.at((100.0, 150.0));
        assert!((north.0 - 20.0).abs() < 1e-3 && (north.1 - 266.0).abs() < 1e-3);
    }

    #[test]
    fn the_turn_under_the_badge_zooms_out_and_zooming_in_waits_for_room() {
        // 250 m ahead, 300 m across would put the turn under the badge.
        let wide = choose_pose(None, &on_course(0.0), ahead(0.0), badge(137.0));
        assert_eq!(wide.step, Step::M400);
        let no_badge = choose_pose(None, &on_course(0.0), ahead(0.0), None);
        assert_eq!(no_badge.step, Step::M300, "only the badge pushed it out");
        // 195 m ahead clears the badge 300 m across, but not 6 px farther.
        let fresh = choose_pose(None, &on_course(55.0), ahead(55.0), badge(137.0));
        assert_eq!(fresh.step, Step::M300);
        let near = choose_pose(Some(wide), &on_course(55.0), ahead(55.0), badge(137.0));
        assert_eq!(near.step, Step::M400);
        let nearer = choose_pose(Some(near), &on_course(70.0), ahead(70.0), badge(137.0));
        assert_eq!(nearer.step, Step::M300);
        // Riding in from 3 km across, 250 m out is already 400 m across.
        let far = Pose {
            step: Step::Km3,
            ..wide
        };
        let arriving = choose_pose(Some(far), &on_course(0.0), ahead(0.0), badge(137.0));
        assert_eq!(arriving.step, Step::M400);
        // Within 100 m the closest step comes at once.
        let close = choose_pose(Some(wide), &on_course(160.0), ahead(160.0), badge(137.0));
        assert_eq!(close.step, Step::M300);
    }

    #[test]
    fn a_far_turn_takes_a_wide_step_and_a_left_turn_starts_right_of_centre() {
        let far = Aim {
            turn: Some(((0.0, 900.0), Glyph::Right)),
            ..on_course(0.0)
        };
        let straight = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 900.0), true);
            visit((0.0, 3_000.0), false);
        };
        assert_eq!(
            choose_pose(None, &far, straight, badge(137.0)).step,
            Step::Km3
        );
        let left = Aim {
            turn: Some(((0.0, 90.0), Glyph::Left)),
            ..on_course(0.0)
        };
        let to_the_left = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 90.0), true);
            visit((-60.0, 100.0), false);
        };
        let pose = choose_pose(None, &left, to_the_left, badge(137.0));
        assert_eq!((pose.step, pose.rider), (Step::M300, (160, 266)));
    }

    #[test]
    fn a_route_running_off_the_edge_pans_but_never_zooms_out() {
        let aim = Aim {
            turn: Some(((0.0, 200.0), Glyph::Right)),
            ..on_course(0.0)
        };
        let east = |visit: &mut dyn FnMut(Point, bool)| {
            visit((0.0, 0.0), true);
            visit((0.0, 200.0), true);
            visit((500.0, 200.0), false);
        };
        let pose = choose_pose(None, &aim, east, badge(137.0));
        assert_eq!((pose.step, pose.rider), (Step::M400, (80, 266)));
    }

    #[test]
    fn a_column_that_still_fits_is_kept() {
        let held = Pose {
            step: Step::M400,
            rider: (48, 266),
            heading: 0,
        };
        let pose = choose_pose(Some(held), &on_course(0.0), ahead(0.0), badge(137.0));
        assert_eq!(pose.rider, (48, 266));
    }

    #[test]
    fn without_a_turn_the_map_is_a_kilometre_across_with_the_rider_mid_map() {
        let free = Aim {
            here: (0.0, 0.0),
            heading: 0,
            turn: None,
            junction: None,
        };
        let pose = choose_pose(None, &free, |_: &mut dyn FnMut(Point, bool)| {}, None);
        assert_eq!((pose.step, pose.rider), (Step::Km1, (120, 156)));
    }
}
