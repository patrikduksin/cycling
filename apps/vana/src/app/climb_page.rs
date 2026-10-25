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
use super::presentation::Navigation;
use crate::screens::ride::{PageChoice, RidePage};
use crate::screens::workout::Page;

impl Runtime {
    /// Look at the road and follow it; once per pass, from the ride's tick
    /// and after the look-ahead's step.
    pub(super) fn follow_road(&mut self) {
        let odometer = self.ride.totals.distance_mm();
        let road = match self.pinned() {
            Some(pin) if odometer < pin.foot_mm => Road::Ahead,
            Some(_) => Road::Climbing,
            None if self.ride.climbs.climbing().is_some() => Road::Climbing,
            None => Road::Clear,
        };
        let choice = self.preferences.options.ride_pages;
        let navigation = &mut self.navigation;
        let before = core::mem::replace(&mut navigation.road, road);
        let seen = Seen::of(navigation, choice, before);
        let step = step(navigation.climb, Event::Road(seen));
        apply(navigation, step);
    }
}

impl Navigation {
    /// The rider picked a ride page: a climb no longer takes them back.
    pub(super) fn ride_page_chosen(&mut self, page: RidePage) {
        self.ride_page = page;
        let step = step(self.climb, Event::Paged);
        apply(self, step);
    }

    /// The ride screen opened on its first page: the climb page is the
    /// road's to bring up again.
    pub(super) fn ride_page_reset(&mut self, page: RidePage) {
        self.ride_page = page;
        self.climb = Follow::Idle;
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
struct Seen {
    before: Road,
    road: Road,
    /// The ride screen is up (not paused over, not another screen).
    riding: bool,
    /// The rider kept the climb page on.
    page_on: bool,
    ride_page: RidePage,
}

impl Seen {
    fn of(navigation: &Navigation, choice: PageChoice, before: Road) -> Self {
        Self {
            before,
            road: navigation.road,
            riding: navigation.page == Page::Ride,
            page_on: choice.shows(RidePage::Climb),
            ride_page: navigation.ride_page,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    follow: Follow,
    /// The ride page to switch to.
    show: Option<RidePage>,
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

/// The only writer of `Navigation::climb`, and of a ride page the road chose.
fn apply(navigation: &mut Navigation, step: Step) {
    navigation.climb = step.follow;
    if let Some(page) = step.show {
        navigation.ride_page = page;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RidePage::{Climb, Main, Stats};
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
                road(Clear, Climbing, Stats),
                show(Stats),
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
