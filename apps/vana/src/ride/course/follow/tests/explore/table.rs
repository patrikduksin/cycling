use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Against {
    Zero,
    Under,
    TurnAround,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OldFields {
    placed: bool,
    unplaced: bool,
    held: bool,
    following: bool,
    join_fixes: bool,
    against: Against,
    matched: bool,
    matched_fix: bool,
    arrived: bool,
}

const NONE: OldFields = OldFields {
    placed: false,
    unplaced: false,
    held: false,
    following: false,
    join_fixes: false,
    against: Against::Zero,
    matched: false,
    matched_fix: false,
    arrived: false,
};

impl OldFields {
    fn of(follower: &Owned) -> Self {
        let following = match follower.life {
            Lifecycle::Following(following) => Some(following),
            _ => None,
        };
        let joining = match follower.life {
            Lifecycle::Joining(joining) => joining,
            _ => Default::default(),
        };
        Self {
            placed: follower.len >= 2,
            unplaced: follower.life.unplaced(),
            held: matches!(
                follower.life,
                Lifecycle::Arrived { .. } | Lifecycle::LetGo { .. }
            ),
            following: following.is_some(),
            join_fixes: joining.fixes > 0,
            against: match joining.against_fixes {
                0 => Against::Zero,
                TURN_AROUND_FIXES.. => Against::TurnAround,
                _ => Against::Under,
            },
            matched: following.is_some_and(|following| following.matched.is_some()),
            matched_fix: following.is_some_and(|following| following.matched_fix.is_some()),
            arrived: latches(&follower.nav()).arrived_ms.is_some(),
        }
    }

    fn with_matched_fix_only_while_following(self) -> Self {
        Self {
            matched_fix: self.following && self.matched_fix,
            ..self
        }
    }

    const fn placed(self) -> Self {
        Self {
            placed: true,
            ..self
        }
    }

    const fn unplaced(self) -> Self {
        Self {
            unplaced: true,
            ..self
        }
    }

    const fn held(self) -> Self {
        Self { held: true, ..self }
    }

    const fn following(self) -> Self {
        Self {
            following: true,
            ..self
        }
    }

    const fn join_fixes(self) -> Self {
        Self {
            join_fixes: true,
            ..self
        }
    }

    const fn against(self, against: Against) -> Self {
        Self { against, ..self }
    }

    const fn matched(self) -> Self {
        Self {
            matched: true,
            ..self
        }
    }

    const fn matched_fix(self) -> Self {
        Self {
            matched_fix: true,
            ..self
        }
    }

    const fn arrived(self) -> Self {
        Self {
            arrived: true,
            ..self
        }
    }
}

const fn observe(fix: (Move, Way)) -> Step {
    Step::Observe {
        now: 0,
        fix: Some(fix),
        speed_mm_s: None,
        paused: false,
    }
}

const AHEAD: Step = observe((Move::Ahead, Way::With));
const AGAINST: Step = observe((Move::Ahead, Way::Against));
const STAND: Step = observe((Move::Stay, Way::With));
const EMPTY: Step = Step::Course { full: false };
const FULL: Step = Step::Course { full: true };
const NEW: Step = Step::NewRide;
const JOIN: [Step; 5] = [AHEAD; 5];
const AGAIN: [Step; 3] = [AGAINST; 3];

#[derive(Clone, Copy)]
enum Path {
    FromPlacedStraight(&'static [&'static [Step]]),
    UnframedWhileFollowing,
}

struct Row {
    old: OldFields,
    kind: LifecycleKind,
    path: Path,
}

const fn row(old: OldFields, kind: LifecycleKind, path: Path) -> Row {
    Row { old, kind, path }
}

use Against::{TurnAround, Under};
use LifecycleKind::{Arrived, Following, Joining, LetGo, NoCourse, Unplaced};
use Path::{FromPlacedStraight, UnframedWhileFollowing};

#[rustfmt::skip]
const ROWS: &[Row] = &[
    row(NONE,                                         NoCourse,  FromPlacedStraight(&[&[EMPTY]])),
    row(NONE.matched_fix(),                           NoCourse,  FromPlacedStraight(&[&JOIN, &[EMPTY, NEW]])),
    row(NONE.unplaced(),                              Unplaced,  FromPlacedStraight(&[&[NEW]])),
    row(NONE.unplaced().matched_fix(),                Unplaced,  FromPlacedStraight(&[&JOIN, &[NEW]])),
    row(NONE.placed(),                                Joining,   FromPlacedStraight(&[])),
    row(NONE.placed().matched_fix(),                  Joining,   FromPlacedStraight(&[&JOIN, &[NEW, FULL]])),
    row(NONE.placed().join_fixes(),                   Joining,   FromPlacedStraight(&[&[AHEAD]])),
    row(NONE.placed().join_fixes().matched_fix(),     Joining,   FromPlacedStraight(&[&JOIN, &[NEW, FULL, AHEAD]])),
    row(NONE.placed().against(Under),                 Joining,   FromPlacedStraight(&[&[STAND, AGAINST]])),
    row(NONE.placed().against(Under).matched_fix(),   Joining,   FromPlacedStraight(&[&JOIN, &[NEW, STAND, FULL, AGAINST]])),
    row(NONE.placed().against(TurnAround),            Joining,   FromPlacedStraight(&[&[STAND], &AGAIN])),
    row(NONE.placed().against(TurnAround).matched_fix(), Joining, FromPlacedStraight(&[&JOIN, &[NEW, STAND, FULL], &AGAIN])),
    row(NONE.placed().following().matched().matched_fix(), Following, FromPlacedStraight(&[&JOIN])),
    row(NONE.placed().held().arrived(),               Arrived,   FromPlacedStraight(&[&[AGAINST], &JOIN])),
    row(NONE.placed().held().matched_fix().arrived(), Arrived,   FromPlacedStraight(&[&JOIN, &[AHEAD]])),
    row(NONE.held(),                                  LetGo,     FromPlacedStraight(&[&[AGAINST], &JOIN, &[EMPTY]])),
    row(NONE.held().matched_fix(),                    LetGo,     FromPlacedStraight(&[&JOIN, &[EMPTY]])),
    row(NONE.placed().held(),                         LetGo,     FromPlacedStraight(&[&[AGAINST], &JOIN, &[FULL]])),
    row(NONE.placed().held().matched_fix(),           LetGo,     FromPlacedStraight(&[&JOIN, &[FULL]])),
    row(NONE.unplaced().held().matched_fix(),         LetGo,     UnframedWhileFollowing),
];

pub(super) fn is_a_row_of_the_old_to_new_table(world: &World) -> bool {
    let old = OldFields::of(&world.follower);
    ROWS.iter().any(|row| {
        row.old.with_matched_fix_only_while_following() == old && row.kind == world.kind()
    })
}

#[test]
fn each_row_of_the_old_to_new_table_is_reached_by_its_path() {
    let mut bounds = Bounds::places();
    bounds.max_fixes = None;
    let straight = fixture(bounds.size, Shape::Straight);
    let walk = |path: &[&[Step]]| {
        path.iter()
            .copied()
            .flatten()
            .fold(World::new(&bounds, Some(straight), 0), |world, &step| {
                next(&world, step)
            })
    };
    for (k, row) in ROWS.iter().enumerate() {
        let world = match row.path {
            FromPlacedStraight(path) => walk(path),
            UnframedWhileFollowing => {
                let mut world = walk(&[&JOIN]);
                let points = straight.course.points();
                world.follower.with(|follower| {
                    follower.handle(Input::Course {
                        points,
                        frame: None,
                        read: &mut |_| None,
                    })
                });
                world
            }
        };
        assert_eq!(
            OldFields::of(&world.follower),
            row.old.with_matched_fix_only_while_following(),
            "row {k}"
        );
        assert_eq!(world.kind(), row.kind, "row {k}");
    }
}
