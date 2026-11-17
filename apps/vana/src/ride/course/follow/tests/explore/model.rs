use super::*;
use crate::ride::course::nav::{ARRIVED_MS, CONFIRM_MS, REMIND_MS};
use core::hash::{Hash, Hasher};

/// #399's `Lifecycle`, read from today's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Life {
    NoCourse,
    Unplaced,
    Joining,
    Following,
    Arrived,
    LetGo,
}

pub(super) const LIVES: [Life; 6] = [
    Life::NoCourse,
    Life::Unplaced,
    Life::Joining,
    Life::Following,
    Life::Arrived,
    Life::LetGo,
];

impl Life {
    fn of(follower: &Owned) -> Self {
        match follower {
            Owned {
                following: true, ..
            } => Self::Following,
            Owned { held: true, .. } if latches(&follower.nav).arrived_ms.is_some() => {
                Self::Arrived
            }
            Owned { held: true, .. } => Self::LetGo,
            Owned { unplaced: true, .. } => Self::Unplaced,
            Owned { len: 2.., .. } => Self::Joining,
            _ => Self::NoCourse,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::NoCourse => "NoCourse",
            Self::Unplaced => "Unplaced",
            Self::Joining => "Joining",
            Self::Following => "Following",
            Self::Arrived => "Arrived",
            Self::LetGo => "LetGo",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Move {
    Stay,
    Ahead,
    Back,
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Way {
    With,
    Against,
    Unknown,
}

/// #399's `Input`, owned so the search can copy it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Step {
    Observe {
        now: u64,
        fix: Option<(Move, Way)>,
        speed_mm_s: Option<u32>,
        paused: bool,
    },
    Course {
        full: bool,
    },
    NewRide,
    LetGo,
}

pub(super) struct Bounds {
    pub(super) size: Size,
    pub(super) times: Vec<u64>,
    speeds: &'static [Option<u32>],
    paused: &'static [bool],
    courses: u8,
    new_rides: u8,
    let_goes: u8,
    max_fixes: Option<u8>,
    pub(super) starts_following: bool,
}

const SPEEDS: [Option<u32>; 4] = [None, Some(1_000), Some(2_000), Some(8_000)];

impl Bounds {
    /// #399's configuration.
    pub(super) fn full() -> Self {
        Self {
            size: Size::Issue,
            times: (0..=ARRIVED_MS / 1_000 + 1).map(|s| s * 1_000).collect(),
            speeds: &SPEEDS,
            paused: &[false, true],
            courses: 2,
            new_rides: 1,
            let_goes: 1,
            max_fixes: None,
            starts_following: false,
        }
    }

    /// Shrunk until the search closes in seconds.
    pub(super) fn places() -> Self {
        Self {
            size: Size::Short,
            times: std::vec![0],
            speeds: &[None, Some(1_000)],
            paused: &[false, true],
            courses: 2,
            new_rides: 1,
            let_goes: 1,
            max_fixes: Some(8),
            starts_following: false,
        }
    }

    pub(super) fn times() -> Self {
        Self {
            times: std::vec![0, nav::LOST_MS.max(GAP_MS) + 1_000, ARRIVED_MS + 1_000],
            max_fixes: Some(6),
            ..Self::places()
        }
    }

    /// Only the straight is searched: beside the L and the hairpin, 60 m to
    /// the left is within `OFF_M` of their other leg.
    pub(super) fn off_course() -> Self {
        Self {
            times: std::vec![0, CONFIRM_MS, CONFIRM_MS + REMIND_MS],
            max_fixes: Some(10),
            starts_following: true,
            ..Self::places()
        }
    }

    pub(super) fn speeds() -> Self {
        Self {
            speeds: &[Some(2_000), Some(8_000)],
            ..Self::places()
        }
    }
}

#[derive(Clone, Copy, Hash)]
enum FixesLeft {
    Unlimited,
    Remaining(u8),
}

#[derive(Clone)]
pub(super) struct World {
    fixture: Option<&'static Fixture>,
    lattice_of: &'static Fixture,
    pub(super) follower: Owned,
    along: usize,
    offset: usize,
    pub(super) now: u64,
    courses: u8,
    new_rides: u8,
    let_goes: u8,
    fixes_left: FixesLeft,
    /// A bit per turn, by index.
    pub(super) turn_now_since_restart: u64,
    pub(super) edge: Edge,
}

#[derive(Clone, Copy)]
pub(super) struct Edge {
    pub(super) raised: Option<Event>,
    pub(super) shown: Sign,
}

impl Hash for World {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.fixture.map(|fixture| fixture.name).hash(state);
        self.lattice_of.name.hash(state);
        self.follower.hash(state);
        (self.along, self.offset, self.now).hash(state);
        (self.courses, self.new_rides, self.let_goes, self.fixes_left).hash(state);
        self.turn_now_since_restart.hash(state);
    }
}

impl World {
    pub(super) fn new(bounds: &Bounds, fixture: Option<&'static Fixture>, now: u64) -> Self {
        let room = fixture.map_or(0, |fixture| fixture.course.points().len());
        let (mut line, mut turns) = (vec![Crumb::default(); room], vec![Turn::default(); ROOM]);
        let mut follower = Follower::new(&mut line, &mut turns);
        if let Some(fixture) = fixture {
            place(&mut follower, fixture, true);
        }
        Self {
            fixture,
            lattice_of: fixture.unwrap_or(super::fixture(bounds.size, Shape::Straight)),
            follower: Owned::of(&follower),
            along: 0,
            offset: 0,
            now,
            courses: 0,
            new_rides: 0,
            let_goes: 0,
            fixes_left: bounds
                .max_fixes
                .map_or(FixesLeft::Unlimited, FixesLeft::Remaining),
            turn_now_since_restart: 0,
            edge: Edge {
                raised: None,
                shown: Sign::None,
            },
        }
    }

    pub(super) fn life(&self) -> Life {
        Life::of(&self.follower)
    }

    fn lattice(&self) -> &'static [[LatticeSpot; 3]] {
        &self.lattice_of.lattice
    }

    fn moved(&self, step: Move) -> Option<(usize, usize)> {
        let (along, offset) = (self.along, self.offset);
        let to = match step {
            Move::Stay => (along, offset),
            Move::Ahead => (along + 1, offset),
            Move::Back => (along.checked_sub(1)?, offset),
            Move::Out => (along, offset + 1),
            Move::In => (along, offset.checked_sub(1)?),
        };
        (to.0 < self.lattice().len() && to.1 < OFFSETS_M.len()).then_some(to)
    }
}

/// Mirrors the glue's `course_changed`.
pub(super) fn place(follower: &mut Follower, fixture: &Fixture, full: bool) {
    let points: &[course::Point] = match full {
        true => fixture.course.points(),
        false => &[],
    };
    follower.set_course(points, Some(frame()));
    follower.take_turns(points, |list| {
        list[..fixture.stored.len()].copy_from_slice(&fixture.stored);
        Some((fixture.stored.len(), false))
    });
}

pub(super) fn alphabet(bounds: &Bounds, world: &World) -> Vec<Step> {
    const MOVES: [Move; 5] = [Move::Stay, Move::Ahead, Move::Back, Move::Out, Move::In];
    const WAYS: [Way; 3] = [Way::With, Way::Against, Way::Unknown];
    let more = !matches!(world.fixes_left, FixesLeft::Remaining(0));
    let fixes: Vec<Option<(Move, Way)>> = core::iter::once(None)
        .chain(
            MOVES
                .into_iter()
                .filter(|&step| more && world.moved(step).is_some())
                .flat_map(|step| WAYS.map(|way| Some((step, way)))),
        )
        .collect();
    let mut steps = Vec::new();
    for &now in bounds.times.iter().filter(|&&now| now >= world.now) {
        for &fix in &fixes {
            for &speed_mm_s in bounds.speeds {
                for &paused in bounds.paused {
                    steps.push(Step::Observe {
                        now,
                        fix,
                        speed_mm_s,
                        paused,
                    });
                }
            }
        }
    }
    if world.courses < bounds.courses {
        steps.push(Step::Course { full: false });
        if world.fixture.is_some() {
            steps.push(Step::Course { full: true });
        }
    }
    if world.new_rides < bounds.new_rides {
        steps.push(Step::NewRide);
    }
    if world.let_goes < bounds.let_goes {
        steps.push(Step::LetGo);
    }
    steps
}

pub(super) fn next(world: &World, step: Step) -> World {
    let mut after = world.clone();
    after.edge.raised = None;
    match step {
        Step::Observe {
            now,
            fix,
            speed_mm_s,
            paused,
        } => {
            after.now = now;
            let fix = fix.map(|(step, way)| {
                if let FixesLeft::Remaining(left) = &mut after.fixes_left {
                    *left -= 1;
                }
                (after.along, after.offset) = world.moved(step).expect("offered on the lattice");
                let LatticeSpot { at, course_bearing } = after.lattice()[after.along][after.offset];
                Fix {
                    at,
                    heading: match way {
                        Way::With => Some(course_bearing),
                        Way::Against => Some((course_bearing + TURN / 2) % TURN),
                        Way::Unknown => None,
                    },
                    moved: step != Move::Stay,
                    taken_loop_ms: now,
                }
            });
            after.edge.raised = after
                .follower
                .with(|follower| follower.observe(fix, now, speed_mm_s, paused));
        }
        Step::Course { full } => {
            after.courses += 1;
            after.follower.with(|follower| match world.fixture {
                Some(fixture) => place(follower, fixture, full),
                None => follower.set_course(&[], Some(frame())),
            });
        }
        Step::NewRide => {
            after.new_rides += 1;
            after.follower.with(|follower| follower.reset());
        }
        Step::LetGo => {
            after.let_goes += 1;
            after.follower.with(|follower| {
                if follower.following() {
                    follower.stop();
                }
            });
        }
    }
    if started_over(world, step, &after) {
        after.turn_now_since_restart = 0;
    }
    if after.edge.raised == Some(Event::TurnNow)
        && let Some(k) = latches(&after.follower.nav).last_beeped
    {
        assert!(k < u64::BITS as usize, "turn {k} past the bitset");
        after.turn_now_since_restart |= 1 << k;
    }
    let now = after.now;
    after.edge.shown = after.follower.with(|follower| follower.sign(now));
    after
}

pub(super) fn started_over(before: &World, step: Step, after: &World) -> bool {
    let rewound =
        latches(&after.follower.nav).furthest_mm < latches(&before.follower.nav).furthest_mm;
    matches!(step, Step::Course { .. } | Step::NewRide) || rewound
}
