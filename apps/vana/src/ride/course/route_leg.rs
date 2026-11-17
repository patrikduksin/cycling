use super::follow::{self, MORE_THAN_135_DEG_OFF, MOVING_M, Placement, TURN_AROUND_FIXES};
use super::nav::Spot;
use crate::ride::breadcrumb::Crumb;
use crate::ride::geo::{TURN, bearing, distance, rotation};

const JOIN_WITHIN_M: f32 = 25.0;
const JOIN_SWITCH_M: f32 = 10.0;
const TOO_FAR_FIXES: u8 = 2;

type Point = (f32, f32);

#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    /// The placed course, end first, each point's `along` its metres to the
    /// end; empty without one.
    pub course: &'a [Crumb],
    pub placement: Placement,
    /// The rider, while the fix is fresh.
    pub here: Option<Point>,
    /// When the receiver took `here`.
    pub here_taken_ms: Option<u64>,
    /// In `TURN`ths clockwise from north.
    pub heading: Option<u16>,
    pub speed_mm_s: Option<u32>,
    /// While following: the leg the follower is on, from [`split`].
    pub split: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RouteLeg {
    placement: Placement,
    end: Option<usize>,
    last_fix: Option<Fixed>,
    counted_from: Option<Point>,
    against_fixes: u8,
    too_far_fixes: u8,
}

impl RouteLeg {
    /// The held leg, as the index of its end in the course.
    pub fn end(&self) -> Option<usize> {
        self.end
    }

    pub fn observe(&mut self, input: &Input) {
        if self.placement != input.placement {
            *self = Self {
                placement: input.placement,
                ..Self::default()
            };
        }
        let fix = input.here.map(|at| Fixed {
            at,
            taken_ms: input.here_taken_ms,
        });
        let motion = fix.and_then(|fix| self.motion(fix, input.speed_mm_s));
        if let Some(end) = input.split {
            self.hold(end);
            return;
        }
        if let (Some(fix), Some(motion)) = (fix, motion)
            && let Some(joining) = joining(input.course, input.heading, fix.at, self.end)
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
            Some(v) if !follow::moving(v) => Motion::Stopped,
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

/// The leg to draw from `centre` before following, and the point on it
/// nearest `centre`: the `held` leg while it is one of the course's.
pub fn at(
    course: &[Crumb],
    heading: Option<u16>,
    centre: Point,
    held: Option<usize>,
) -> Option<(usize, Point)> {
    let joining = joining(course, heading, centre, held)?;
    let leg = joining.held.unwrap_or(joining.best);
    Some((leg.end, leg.at))
}

/// While following, where the course ahead starts: at the missed
/// `junction` while there is one, or else on the follower's segment.
pub fn split(
    course: &[Crumb],
    ahead: Option<(usize, Option<Point>)>,
    junction: Option<Spot>,
) -> Option<(usize, Point)> {
    let (segment, matched) = ahead.filter(|_| course.len() >= 2)?;
    if let Some(junction) = junction {
        let to_end = metres_to_end(course, junction);
        let end = course
            .partition_point(|c| c.along < to_end)
            .clamp(1, course.len() - 1);
        return Some((end, junction.at));
    }
    let end = segment.clamp(1, course.len() - 1);
    let c = course[end];
    Some((end, matched.unwrap_or((c.east, c.north))))
}

/// Metres from `spot` to the course's end.
pub fn metres_to_end(course: &[Crumb], spot: Spot) -> f32 {
    let total = course.last().map_or(0.0, |c| c.along);
    (total - spot.along_mm as f32 / 1000.0).max(0.0)
}

fn joining(
    course: &[Crumb],
    heading: Option<u16>,
    centre: Point,
    held: Option<usize>,
) -> Option<Joining> {
    let legs = 1..course.len();
    let leg = |end: usize| {
        let (a, b) = (course[end - 1], course[end]);
        let at = follow::project(centre, (a.east, a.north), (b.east, b.north)).0;
        Leg {
            end,
            at,
            gap_m: distance(at, centre),
        }
    };
    let held = held.filter(|end| legs.contains(end)).map(leg);
    let Some(heading) = heading else {
        let best = held.or_else(|| closest(legs.map(leg)))?;
        return Some(Joining {
            held,
            best,
            heading_against_held: false,
            held_leg_too_far: false,
        });
    };
    let (sin, cos) = rotation(heading);
    let our_way = |&end: &usize| {
        let (a, b) = (course[end - 1], course[end]);
        (a.east - b.east) * sin + (a.north - b.north) * cos > 0.0
    };
    let ours = closest(legs.clone().filter(our_way).map(leg)).filter(|l| l.gap_m <= JOIN_WITHIN_M);
    let best = ours.or_else(|| closest(legs.map(leg)))?;
    let heading_against_held = held.is_some_and(|held| {
        let (a, b) = (course[held.end - 1], course[held.end]);
        let off_its_way = (heading + TURN - bearing((b.east, b.north), (a.east, a.north))) % TURN;
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

fn closest(legs: impl Iterator<Item = Leg>) -> Option<Leg> {
    legs.min_by(|a, b| a.gap_m.total_cmp(&b.gap_m))
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
