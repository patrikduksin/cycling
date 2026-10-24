//! Turn back by: the last moment to start home and still be there by sunset.
//!
//! Pure policy. The way home is the guide's TRACK KM (or TO END KM when the
//! course is followed) over the ride's moving average speed; the latest turn
//! is sunset less that time. The alert is due once less than `WARN_S` is
//! left to it. It is given once per ride, and again only when the margin has
//! stayed at or above `CLEAR_S` for `CLEAR_MS`: riding home keeps the margin
//! where it was, so a margin that recovers means the plan changed (a shorter
//! course, a faster pace), not that the rider ignored the first warning.
use crate::daylight::Sun;
use crate::ride::homing::{Guide, Mode, TURN};

/// The alert is due when this little time is left before the latest turn.
pub const WARN_S: i64 = 15 * 60;
/// It re-arms after the margin has been at least this for `CLEAR_MS`.
pub const CLEAR_S: i64 = 30 * 60;
pub const CLEAR_MS: u64 = 10 * 60_000;
/// Slower than this (about 5 km/h) the average says nothing about the way
/// home: a rider walking a hill or waiting at a stop.
pub const MIN_SPEED_MM_S: u32 = 1_400;
/// The average counts once the ride has moved this long.
pub const MIN_MOVING_MS: u64 = 5 * 60_000;
/// A way home longer than this is no estimate. It also keeps the sums in
/// range for any input.
const MAX_HOME_S: u64 = 24 * 3600;
/// Riding within this of straight to the start is riding home already.
const HOME_ARC: u16 = TURN / 4;

/// The way home, as the ride knows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Leg {
    pub home_m: u32,
    /// Moving average speed of the ride.
    pub speed_mm_s: u32,
    pub moving_ms: u64,
}

impl Leg {
    /// The leg from `guide`, or `None` when it is not one to warn about: at
    /// the start, or riding towards it already, by following the track back
    /// or by heading within `HOME_ARC` of it. Following a course is riding to
    /// its end, which is the way home then.
    pub fn of(guide: &Guide, speed_mm_s: Option<u32>, moving_ms: u64) -> Option<Self> {
        let outbound = guide.course || (!guide.following && !towards_start(guide));
        (outbound && guide.mode != Mode::Arrived).then_some(Self {
            home_m: guide.track_m,
            speed_mm_s: speed_mm_s?,
            moving_ms,
        })
    }

    /// Seconds to ride home at the average speed, or `None` while the speed
    /// is too low or too new to say.
    pub fn home_s(&self) -> Option<u64> {
        if self.home_m == 0 || self.speed_mm_s < MIN_SPEED_MM_S || self.moving_ms < MIN_MOVING_MS {
            return None;
        }
        let seconds = u64::from(self.home_m) * 1000 / u64::from(self.speed_mm_s);
        Some(seconds.min(MAX_HOME_S))
    }
}

fn towards_start(guide: &Guide) -> bool {
    guide.heading.is_some_and(|heading| {
        let apart = (guide.bearing + TURN - heading) % TURN;
        apart <= HOME_ARC || apart >= TURN - HOME_ARC
    })
}

/// The latest turn: when, and how long from now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Turn {
    /// Unix time of the latest turn.
    pub by: i64,
    /// Seconds from now to `by`; negative when it has passed.
    pub left_s: i64,
}

impl Turn {
    /// The latest turn from the sun, the Unix time now and the way home.
    /// Nothing without a sunset ahead (down, or polar day) or a fair estimate.
    pub fn new(sun: Sun, now_unix: i64, leg: Leg) -> Option<Self> {
        let Sun::Up { sets_at } = sun else {
            return None;
        };
        let by = sets_at.saturating_sub(leg.home_s()? as i64);
        Some(Self {
            by,
            left_s: by.saturating_sub(now_unix),
        })
    }
}

/// Whether this ride has been told, and since when the margin has been clear.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Watch {
    #[default]
    Armed,
    Told {
        clear_since: Option<u64>,
    },
}

impl Watch {
    /// Whether to warn now. `turn` is `None` when nothing can be said, which
    /// holds the state and breaks any clear run; `waiting` holds the warning
    /// back unspent (not on the ride page, or too soon after another alert).
    pub fn due(&mut self, now: u64, turn: Option<Turn>, waiting: bool) -> bool {
        match (*self, turn) {
            (Self::Armed, Some(turn)) if turn.left_s < WARN_S && !waiting => {
                *self = Self::Told { clear_since: None };
                true
            }
            (Self::Told { clear_since }, Some(turn)) if turn.left_s >= CLEAR_S => {
                let since = clear_since.unwrap_or(now);
                *self = if now.saturating_sub(since) >= CLEAR_MS {
                    Self::Armed
                } else {
                    Self::Told {
                        clear_since: Some(since),
                    }
                };
                false
            }
            (Self::Told { .. }, _) => {
                *self = Self::Told { clear_since: None };
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETS: i64 = 1_000_000;
    const FAST: u32 = 6_944;
    const HOUR_MS: u64 = 3_600_000;

    fn leg(home_m: u32) -> Leg {
        Leg {
            home_m,
            speed_mm_s: FAST,
            moving_ms: HOUR_MS,
        }
    }

    /// Heading straight for the start.
    fn homeward() -> Guide {
        Guide {
            mode: Mode::Direct,
            following: false,
            course: false,
            course_near: false,
            direct_m: 5_000,
            track_m: 5_000,
            off_m: 0,
            bearing: 0,
            heading: Some(0),
        }
    }

    fn outbound() -> Guide {
        Guide {
            bearing: TURN / 2,
            ..homeward()
        }
    }

    #[test]
    fn the_latest_turn_is_sunset_less_the_ride_home() {
        let turn = Turn::new(Sun::Up { sets_at: SETS }, SETS - 5400, leg(25_000)).unwrap();
        assert_eq!(turn.by, SETS - 3600);
        assert_eq!(turn.left_s, 1800);
    }

    #[test]
    fn nothing_without_a_sunset_ahead_or_a_fair_speed() {
        for sun in [Sun::Down, Sun::Midnight] {
            assert_eq!(Turn::new(sun, 0, leg(25_000)), None);
        }
        let up = Sun::Up { sets_at: SETS };
        for speed_mm_s in [0, 1, MIN_SPEED_MM_S - 1] {
            let slow = Leg {
                speed_mm_s,
                ..leg(25_000)
            };
            assert_eq!(Turn::new(up, 0, slow), None, "{speed_mm_s}");
        }
        let young = Leg {
            moving_ms: MIN_MOVING_MS - 1,
            ..leg(25_000)
        };
        assert_eq!(Turn::new(up, 0, young), None);
        assert_eq!(Turn::new(up, 0, leg(0)), None);
    }

    #[test]
    fn hostile_inputs_do_not_overflow() {
        let huge = Leg {
            home_m: u32::MAX,
            speed_mm_s: MIN_SPEED_MM_S,
            moving_ms: u64::MAX,
        };
        for sets_at in [i64::MIN, -1, 0, i64::MAX] {
            for now in [i64::MIN, 0, i64::MAX] {
                assert!(Turn::new(Sun::Up { sets_at }, now, huge).is_some());
            }
        }
        let fast = Leg {
            speed_mm_s: u32::MAX,
            ..huge
        };
        assert_eq!(fast.home_s(), Some(1000));
    }

    #[test]
    fn a_ride_out_from_the_start_has_a_leg() {
        let leg = Leg::of(&outbound(), Some(FAST), HOUR_MS).unwrap();
        assert_eq!(leg.home_m, 5_000);
    }

    #[test]
    fn riding_towards_the_start_has_no_leg() {
        assert_eq!(Leg::of(&homeward(), Some(FAST), HOUR_MS), None);
        let following = Guide {
            following: true,
            ..outbound()
        };
        assert_eq!(Leg::of(&following, Some(FAST), HOUR_MS), None);
        let arrived = Guide {
            mode: Mode::Arrived,
            ..outbound()
        };
        assert_eq!(Leg::of(&arrived, Some(FAST), HOUR_MS), None);
        assert_eq!(Leg::of(&outbound(), None, HOUR_MS), None);
    }

    #[test]
    fn a_heading_at_the_edge_of_the_arc_is_still_towards_the_start() {
        for heading in [TURN - HOME_ARC, HOME_ARC] {
            let guide = Guide {
                heading: Some(heading),
                ..homeward()
            };
            assert_eq!(Leg::of(&guide, Some(FAST), HOUR_MS), None, "{heading}");
        }
        let across = Guide {
            heading: Some(HOME_ARC + 1),
            ..homeward()
        };
        assert!(Leg::of(&across, Some(FAST), HOUR_MS).is_some());
    }

    #[test]
    fn a_course_followed_is_a_leg_even_heading_for_its_end() {
        let course = Guide {
            following: true,
            course: true,
            ..homeward()
        };
        assert!(Leg::of(&course, Some(FAST), HOUR_MS).is_some());
    }

    fn turn(left_s: i64) -> Option<Turn> {
        Some(Turn { by: 0, left_s })
    }

    #[test]
    fn it_warns_once_when_less_than_the_margin_is_left() {
        let mut watch = Watch::default();
        assert!(!watch.due(0, turn(WARN_S), false));
        assert!(watch.due(1_000, turn(WARN_S - 1), false));
        assert!(!watch.due(2_000, turn(0), false));
        assert!(!watch.due(3_000, turn(-600), false));
    }

    #[test]
    fn a_warning_held_back_stays_due() {
        let mut watch = Watch::default();
        assert!(!watch.due(0, turn(10), true));
        assert!(watch.due(1_000, turn(10), false));
    }

    #[test]
    fn it_warns_again_only_after_a_long_clear_margin() {
        let mut watch = Watch::default();
        assert!(watch.due(0, turn(0), false));
        assert!(!watch.due(1_000, turn(CLEAR_S), false));
        assert!(!watch.due(1_000 + CLEAR_MS - 1, turn(CLEAR_S), false));
        assert!(!watch.due(1_000 + CLEAR_MS, turn(CLEAR_S), false));
        assert!(watch.due(2_000 + CLEAR_MS, turn(0), false));
    }

    #[test]
    fn a_dip_or_a_gap_restarts_the_clear_run() {
        let mut watch = Watch::default();
        assert!(watch.due(0, turn(0), false));
        assert!(!watch.due(1_000, turn(CLEAR_S), false));
        assert!(!watch.due(2_000, turn(CLEAR_S - 1), false));
        assert!(!watch.due(3_000, turn(CLEAR_S), false));
        assert!(!watch.due(3_000 + CLEAR_MS - 1, turn(CLEAR_S), false));
        assert!(!watch.due(3_000 + CLEAR_MS, turn(0), false));
        assert!(!watch.due(4_000 + CLEAR_MS, None, false));
        assert!(!watch.due(5_000 + CLEAR_MS, turn(CLEAR_S), false));
        assert!(!watch.due(6_000 + CLEAR_MS, turn(0), false));
    }
}
