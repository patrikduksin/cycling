//! The climb page follows the road: it comes up by itself before a climb or
//! once one is under way, and when the road is clear again the ride screen
//! goes back to the page the rider was on. A rider who pages away during a
//! climb is left there until the climb is over.
//!
//! The road is one observation from both sources (`Road`): the climb the map
//! pinned (`climb_ahead::Pinned`), or else the barometer's climb under way.
//! Each pass, after the ride and the look-ahead, `follow_road` steps the
//! follower with the road before and now.
use super::Runtime;
use super::navigation::To;
use crate::screens::ride::RidePage;

impl Runtime {
    /// Look at the road and follow it; once per pass, from the ride's tick
    /// and after the look-ahead's step.
    pub(super) fn follow_road(&mut self) {
        let odometer = self.ride.totals().distance_mm();
        let road = match self.pinned() {
            Some(pin) if odometer < pin.foot_mm => Road::Ahead,
            Some(_) => Road::Climbing,
            None if self.ride.climbs().climbing().is_some() => Road::Climbing,
            None => Road::Clear,
        };
        let page_on = self.deck_with_course_map().shows(RidePage::Climb);
        self.navigation.go(To::Road { road, page_on });
    }
}

/// What the climb follower knows that no other owner does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Follow {
    /// Nothing shown by itself: a climb brings the page up.
    #[default]
    Idle,
    /// The climb brought the page up; a clear road goes back to `back_to`.
    Shown { back_to: RidePage },
    /// This climb is the rider's: they paged away, or were on the page
    /// already. Nothing changes until it is over.
    Done,
}

impl Follow {
    pub(super) fn workout_done(self) -> Self {
        match self {
            Self::Shown {
                back_to: RidePage::Workout,
            } => Self::Shown {
                back_to: RidePage::Main,
            },
            other => other,
        }
    }
}

/// The road, as the climb page sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Road {
    /// No climb under way, and none pinned ahead.
    #[default]
    Clear,
    /// The map has a climb starting within `climb_ahead::ANNOUNCE_MM`.
    Ahead,
    /// On a climb, by the map or the barometer.
    Climbing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// A pass looked at the road.
    Road(Seen),
    /// The rider picked a ride page.
    Paged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    pub(super) before: Road,
    pub(super) road: Road,
    /// The ride screen is up (not paused over, not another screen).
    pub(super) riding: bool,
    /// The rider kept the climb page on.
    pub(super) page_on: bool,
    pub(super) ride_page: RidePage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Step {
    pub(super) follow: Follow,
    /// The ride page to switch to.
    pub(super) show: Option<RidePage>,
}

pub(super) fn on_road(follow: Follow, seen: Seen) -> Step {
    step(follow, Event::Road(seen))
}

pub(super) fn on_paged(follow: Follow) -> Step {
    step(follow, Event::Paged)
}

const fn stay(follow: Follow) -> Step {
    Step { follow, show: None }
}

fn step(follow: Follow, event: Event) -> Step {
    let seen = match event {
        Event::Paged => {
            return stay(match follow {
                Follow::Shown { .. } => Follow::Done,
                other => other,
            });
        }
        Event::Road(seen) => seen,
    };
    // Off the ride screen nothing moves: the page waits for the rider.
    if !seen.riding {
        return stay(follow);
    }
    let climb = seen.road != Road::Clear;
    // Over a top straight into the next climb: a climb of its own.
    let next = seen.before == Road::Climbing && seen.road == Road::Ahead;
    match follow {
        Follow::Idle | Follow::Done if !climb => stay(Follow::Idle),
        Follow::Done if !next => stay(Follow::Done),
        Follow::Idle | Follow::Done => {
            if !seen.page_on {
                // CLIMB off: the page never comes up by itself.
                stay(Follow::Idle)
            } else if seen.ride_page == RidePage::Climb {
                stay(Follow::Done)
            } else {
                Step {
                    follow: Follow::Shown {
                        back_to: seen.ride_page,
                    },
                    show: Some(RidePage::Climb),
                }
            }
        }
        Follow::Shown { back_to } if !climb => Step {
            follow: Follow::Idle,
            show: Some(back_to),
        },
        Follow::Shown { .. } => stay(follow),
    }
}

/// A rider's mark undoes the MOVE press that began its hold: the ride page and
/// the follower go back as one, so a page is never put back under a follower
/// that has moved on. A climb page that came up during the hold stays, and
/// goes back to the page from before the press, not the one the press showed.
pub(super) fn press_undone(
    before: (RidePage, Follow),
    after: (RidePage, Follow),
    now: (RidePage, Follow),
) -> (RidePage, Follow) {
    if now == after {
        return before;
    }
    match (after.1, now.1) {
        (Follow::Idle | Follow::Done, Follow::Shown { back_to }) if back_to == after.0 => {
            let chosen = match before.1 {
                Follow::Shown { back_to } => back_to,
                Follow::Idle | Follow::Done => before.0,
            };
            (now.0, Follow::Shown { back_to: chosen })
        }
        _ => now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_puts_back_the_page_and_the_follower_as_one() {
        let shown = |back_to| Follow::Shown { back_to };
        let on_climb = (Climb, shown(RidePage::Map));
        let paged = (Main, Follow::Done);
        let rows = [
            (
                "nothing else moved",
                (Main, Follow::Idle),
                (Map, Follow::Idle),
                (Map, Follow::Idle),
                (Main, Follow::Idle),
            ),
            (
                "the climb page that came up by itself, and where it goes back",
                on_climb,
                paged,
                paged,
                on_climb,
            ),
            (
                "a climb that ended leaves the page the press showed",
                on_climb,
                paged,
                (Main, Follow::Idle),
                (Main, Follow::Idle),
            ),
            (
                "a climb page that came up stays and goes back to the page from before",
                (Main, Follow::Idle),
                (Map, Follow::Idle),
                (Climb, shown(Map)),
                (Climb, shown(Main)),
            ),
        ];
        for (what, before, after, now, want) in rows {
            assert_eq!(press_undone(before, after, now), want, "{what}");
        }
    }

    use RidePage::{Climb, Main, Map};
    use Road::{Ahead, Clear, Climbing};

    const SHOWN_MAIN: Follow = Follow::Shown { back_to: Main };

    fn road(before: Road, road: Road, ride_page: RidePage) -> Event {
        Event::Road(Seen {
            before,
            road,
            riding: true,
            page_on: true,
            ride_page,
        })
    }

    fn off(event: Event, change: impl Fn(&mut Seen)) -> Event {
        match event {
            Event::Road(mut seen) => {
                change(&mut seen);
                Event::Road(seen)
            }
            paged => paged,
        }
    }

    #[test]
    fn every_state_follows_the_road() {
        let show = |back_to| Step {
            follow: Follow::Shown { back_to },
            show: Some(Climb),
        };
        let rows: [(&str, Follow, Event, Step); 16] = [
            (
                "clear stays",
                Follow::Idle,
                road(Clear, Clear, Main),
                stay(Follow::Idle),
            ),
            (
                "a climb ahead opens",
                Follow::Idle,
                road(Clear, Ahead, Main),
                show(Main),
            ),
            (
                "a climb under way opens",
                Follow::Idle,
                road(Clear, Climbing, Map),
                show(Map),
            ),
            (
                "already there is the rider's",
                Follow::Idle,
                road(Clear, Ahead, Climb),
                stay(Follow::Done),
            ),
            (
                "CLIMB off never opens",
                Follow::Idle,
                off(road(Clear, Climbing, Main), |s| s.page_on = false),
                stay(Follow::Idle),
            ),
            (
                "not on the ride screen waits",
                Follow::Idle,
                off(road(Clear, Climbing, Main), |s| s.riding = false),
                stay(Follow::Idle),
            ),
            (
                "shown stays on the climb",
                SHOWN_MAIN,
                road(Ahead, Climbing, Climb),
                stay(SHOWN_MAIN),
            ),
            (
                "shown into the next climb stays",
                SHOWN_MAIN,
                road(Climbing, Ahead, Climb),
                stay(SHOWN_MAIN),
            ),
            (
                "a clear road goes back",
                SHOWN_MAIN,
                road(Climbing, Clear, Climb),
                Step {
                    follow: Follow::Idle,
                    show: Some(Main),
                },
            ),
            (
                "paused over, it waits",
                SHOWN_MAIN,
                off(road(Climbing, Clear, Climb), |s| s.riding = false),
                stay(SHOWN_MAIN),
            ),
            (
                "paging away is the rider's",
                SHOWN_MAIN,
                Event::Paged,
                stay(Follow::Done),
            ),
            (
                "paging while idle",
                Follow::Idle,
                Event::Paged,
                stay(Follow::Idle),
            ),
            (
                "done waits out the climb",
                Follow::Done,
                road(Climbing, Climbing, Main),
                stay(Follow::Done),
            ),
            (
                "done re-arms on a clear road",
                Follow::Done,
                road(Climbing, Clear, Main),
                stay(Follow::Idle),
            ),
            (
                "done opens for the next climb",
                Follow::Done,
                road(Climbing, Ahead, Main),
                show(Main),
            ),
            (
                "done ahead to climbing is the same climb",
                Follow::Done,
                road(Ahead, Climbing, Main),
                stay(Follow::Done),
            ),
        ];
        for (what, follow, event, want) in rows {
            assert_eq!(step(follow, event), want, "{what}");
        }
    }
}
