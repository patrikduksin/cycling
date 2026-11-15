//! Navigating a followed course: the one sign the rider reads and each of
//! its sounds, once, at its moment.
//!
//! `Navigator` sees one `Observed` per fix and the course's turns, in
//! millimetres along from its start, and keeps the latches the sign needs.
//! The banner opens `window_mm` before the next turn, and stays open once
//! open. After a turn it shows the next one at once, and stays up for
//! `GRACE_MM` past the turn, or longer while the turns are `dense`. Its
//! sound plays only when it opens, and the beep `BEEP_MM` before each
//! turn plays once. Off course is more than `OFF_M` off on fixes at least
//! `CONFIRM_MS` apart, with none nearer between. It holds until the rider
//! is back within `ON_M`, riding the course's way, however far off they
//! went. The way back leads to where they left the course (`Spot`),
//! which never moves. Each step returns at most one `Event`. A second one
//! due on the same fix waits one fix, and is dropped if it no longer holds.
use crate::ride::course::turns::{HALF_CHORD_MM, Kind, Side, Turn};
use crate::ride::homing::distance;

type Point = (f32, f32);

pub const OFF_M: f32 = 40.0;
pub const ON_M: f32 = 25.0;
const CONFIRM_MS: u64 = 5_000;
pub const LOST_MS: u64 = 5_000;
const REMIND_MS: u64 = 15_000;
const WINDOW_S: u32 = 30;
const WINDOW_MIN_MM: u32 = 150_000;
const WINDOW_MAX_MM: u32 = 400_000;
const GRACE_MM: u32 = 40_000;
const DENSE_MM: u32 = 300_000;
const DENSE_S: u32 = 25;
const BEEP_MM: u32 = 30_000;
const REWIND_MM: u32 = 30_000;
const MISSED_MM: u32 = 60_000;
const ARRIVED_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug)]
pub enum Observed {
    /// Riding along the course: following started by itself on this fix.
    Joined(MatchedFix),
    Fix(MatchedFix),
    Arrived {
        now_ms: u64,
    },
    /// No fix for `LOST_MS`.
    Lost,
}

#[derive(Clone, Copy, Debug)]
pub struct MatchedFix {
    pub taken_loop_ms: u64,
    /// The matched point, millimetres along from the course's start.
    pub along_mm: u32,
    pub to_end_mm: u32,
    pub off_m: f32,
    /// The rider, metres east and north in the ride's frame.
    pub here: Point,
    /// Heading the course's way at the matched stretch.
    pub forward: bool,
    /// The ride's live speed; without one the banner opens at its least.
    pub speed_mm_s: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Following,
    /// The banner opened.
    TurnSoon,
    /// `BEEP_MM` before the turn the sign shows.
    TurnNow,
    OffCourse,
    /// Still off course `REMIND_MS` after `OffCourse`, once.
    StillOff,
    OnCourse,
    Arrived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Left,
    Right,
    SharpLeft,
    SharpRight,
    UTurn,
    Finish,
}

impl From<Kind> for Glyph {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Turn(Side::Left) => Self::Left,
            Kind::Turn(Side::Right) => Self::Right,
            Kind::Sharp(Side::Left) => Self::SharpLeft,
            Kind::Sharp(Side::Right) => Self::SharpRight,
            Kind::UTurn => Self::UTurn,
        }
    }
}

fn glyph(turn: &Turn) -> Option<Glyph> {
    turn.kind.map(Glyph::from)
}

/// A distance as the sign draws it: the nearest 10 m below 1 km, half down,
/// then the nearest 0.1 km below 10 km, then whole km. Two distances that
/// draw the same are equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Distance(u16);

const KM_TENTHS: u16 = 100;
const KM: u16 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Metres,
    Kilometres,
}

impl Distance {
    pub fn from_mm(mm: u32) -> Self {
        let tens = mm.saturating_add(4_999) / 10_000;
        if tens < u32::from(KM_TENTHS) {
            return Self(tens as u16);
        }
        let tenths = mm.saturating_add(49_999) / 100_000;
        if tenths < u32::from(KM - KM_TENTHS) {
            return Self(KM_TENTHS + tenths as u16);
        }
        let km = mm.saturating_add(499_999) / 1_000_000;
        Self(u16::try_from(km + u32::from(KM)).unwrap_or(u16::MAX))
    }

    fn from_m(metres: f32) -> Self {
        // A float cast saturates, so no distance overflows.
        Self::from_mm((metres * 1000.0) as u32)
    }

    pub fn unit(self) -> Unit {
        match self.0 {
            0..KM_TENTHS => Unit::Metres,
            _ => Unit::Kilometres,
        }
    }
}

impl core::fmt::Display for Distance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            tens @ 0..KM_TENTHS => write!(f, "{}", u32::from(tens) * 10),
            tenths @ KM_TENTHS..KM => {
                let tenths = tenths - KM_TENTHS;
                write!(f, "{}.{}", tenths / 10, tenths % 10)
            }
            km => write!(f, "{}", km - KM),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sign {
    /// Not following, or no fix.
    #[default]
    None,
    /// At rest: the next turn, or the course's end, and how far.
    Chip { glyph: Glyph, distance: Distance },
    /// The banner (blue).
    Turn { glyph: Glyph, distance: Distance },
    /// Off course (amber): how far back to where the rider left it.
    OffCourse { back: Distance },
    /// The `ARRIVED_MS` after the end (green).
    Arrived,
}

const _: () = assert!(core::mem::size_of::<Sign>() == 4, "View has no spare bytes");

/// What the map needs besides the sign, in the ride's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guidance {
    pub sign: Sign,
    /// The next turn: its corner and its glyph.
    pub turn: Option<(Spot, Glyph)>,
    /// Off course: where the rider left it, which the way back leads to;
    /// a missed turn's corner.
    pub junction: Option<Spot>,
}

/// A point of the course, and how far along it is from the course's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub along_mm: u32,
    pub at: Point,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Place {
    #[default]
    On,
    Leaving {
        since_ms: u64,
        junction: Spot,
    },
    Off {
        since_ms: u64,
        junction: Spot,
        reminded: bool,
    },
}

#[derive(Clone, Copy)]
struct Next<'t> {
    index: usize,
    turn: &'t Turn,
    glyph: Glyph,
    unblocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TurnId(u32);

impl Turn {
    const fn id(&self) -> TurnId {
        TurnId(self.start_mm)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Raised {
    Following,
    TurnSoon { turn: TurnId },
    TurnNow { turn: TurnId },
    OffCourse,
    StillOff,
    OnCourse,
}

impl Raised {
    const fn event(self) -> Event {
        match self {
            Self::Following => Event::Following,
            Self::TurnSoon { .. } => Event::TurnSoon,
            Self::TurnNow { .. } => Event::TurnNow,
            Self::OffCourse => Event::OffCourse,
            Self::StillOff => Event::StillOff,
            Self::OnCourse => Event::OnCourse,
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::OffCourse => 4,
            Self::Following | Self::OnCourse => 3,
            Self::StillOff => 2,
            Self::TurnNow { .. } => 1,
            Self::TurnSoon { .. } => 0,
        }
    }
}

#[derive(Default)]
struct Due {
    first: Option<Raised>,
    second: Option<Raised>,
}

impl Due {
    fn raise(&mut self, raised: Raised) {
        match self.first {
            Some(first) if first.rank() >= raised.rank() => {
                if self
                    .second
                    .is_none_or(|second| second.rank() < raised.rank())
                {
                    self.second = Some(raised);
                }
            }
            first => (self.first, self.second) = (Some(raised), first),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Navigator {
    cursor: usize,
    furthest_mm: u32,
    banner: Option<usize>,
    last_sounded: Option<usize>,
    last_beeped: Option<usize>,
    place: Place,
    last: Option<MatchedFix>,
    arrived_ms: Option<u64>,
    lost: bool,
    pending: Option<Raised>,
}

impl Navigator {
    /// One observation against the course's `turns`; `locate` gives the
    /// course's point that many millimetres along from its start.
    pub fn step(
        &mut self,
        observed: Observed,
        turns: &[Turn],
        locate: impl Fn(u32) -> Point,
    ) -> Option<Event> {
        let parked = self.pending.take();
        let mut due = Due::default();
        match observed {
            Observed::Arrived { now_ms } => {
                if self.arrived_ms.is_none() {
                    self.arrived_ms = Some(now_ms);
                    return Some(Event::Arrived);
                }
                return None;
            }
            Observed::Lost => {
                self.lost = true;
                if let Place::Leaving { .. } = self.place {
                    self.place = Place::On;
                }
                return None;
            }
            Observed::Joined(fix) => {
                *self = Self::default();
                due.raise(Raised::Following);
                self.fix(fix, turns, &locate, &mut due);
            }
            Observed::Fix(fix) => self.fix(fix, turns, &locate, &mut due),
        }
        if let Some(parked) = parked.filter(|&raised| self.holds(raised, turns)) {
            due.raise(parked);
        }
        self.pending = due.second;
        due.first.map(Raised::event)
    }

    fn fix(
        &mut self,
        fix: MatchedFix,
        turns: &[Turn],
        locate: &impl Fn(u32) -> Point,
        due: &mut Due,
    ) {
        self.lost = false;
        let gap = self
            .last
            .is_some_and(|last| fix.taken_loop_ms.saturating_sub(last.taken_loop_ms) > LOST_MS);
        let now = fix.taken_loop_ms;
        self.place = match self.place {
            Place::On if fix.off_m > OFF_M => Place::Leaving {
                since_ms: now,
                junction: self.junction(turns, fix.along_mm, locate),
            },
            Place::Leaving { .. } if fix.off_m <= OFF_M => Place::On,
            Place::Leaving { junction, .. } if gap => Place::Leaving {
                since_ms: now,
                junction,
            },
            Place::Leaving { since_ms, junction } if now.saturating_sub(since_ms) >= CONFIRM_MS => {
                due.raise(Raised::OffCourse);
                Place::Off {
                    since_ms: now,
                    junction,
                    reminded: false,
                }
            }
            Place::Off { .. } if fix.off_m <= ON_M && fix.forward => {
                due.raise(Raised::OnCourse);
                Place::On
            }
            Place::Off {
                since_ms,
                junction,
                reminded: false,
            } if now.saturating_sub(since_ms) >= REMIND_MS => {
                due.raise(Raised::StillOff);
                Place::Off {
                    since_ms,
                    junction,
                    reminded: true,
                }
            }
            off => off,
        };
        if self.place == Place::On {
            self.follow_turns(&fix, turns, due);
        }
        self.last = Some(fix);
    }

    fn junction(&self, turns: &[Turn], along_mm: u32, locate: &impl Fn(u32) -> Point) -> Spot {
        let left_mm = self.last.map_or(along_mm, |last| last.along_mm);
        let just_passed = self.cursor.checked_sub(1).and_then(|k| turns.get(k));
        let shown = self.next(turns).map(|next| next.turn);
        let missed = [just_passed, shown]
            .into_iter()
            .flatten()
            .filter(|turn| turn.start_mm.abs_diff(left_mm) <= MISSED_MM)
            .min_by_key(|turn| turn.start_mm.abs_diff(left_mm));
        let along_mm = missed.map_or(left_mm, corner);
        Spot {
            along_mm,
            at: locate(along_mm),
        }
    }

    fn follow_turns(&mut self, fix: &MatchedFix, turns: &[Turn], due: &mut Due) {
        if !fix.forward {
            self.banner = None;
            return;
        }
        if fix.along_mm.saturating_add(REWIND_MM) < self.furthest_mm {
            (
                self.cursor,
                self.banner,
                self.last_sounded,
                self.last_beeped,
            ) = (0, None, None, None);
            self.furthest_mm = fix.along_mm;
        }
        self.furthest_mm = self.furthest_mm.max(fix.along_mm);
        let here = self.furthest_mm;
        let past = |turn: &Turn| turn.end_mm <= here.saturating_add(HALF_CHORD_MM);
        while turns.get(self.cursor).is_some_and(past) {
            self.cursor += 1;
        }
        let next = self.next(turns);
        let speed = fix.speed_mm_s;
        let ahead = |next: &Next| next.turn.start_mm.checked_sub(here);
        let opens =
            |next: &Next| next.unblocked && ahead(next).is_some_and(|mm| mm <= window_mm(speed));
        let was_open = self.banner.is_some();
        self.banner = match (self.banner, next) {
            (Some(k), Some(next)) if k == next.index => Some(k),
            (Some(k), next) if k < self.cursor => {
                let passed = &turns[k];
                match next {
                    Some(next) if next.unblocked && dense(passed, next.turn, speed) => {
                        Some(next.index)
                    }
                    _ if here < passed.end_mm.saturating_add(GRACE_MM) => Some(k),
                    Some(next) if opens(&next) => Some(next.index),
                    _ => None,
                }
            }
            _ => next.filter(opens).map(|next| next.index),
        };
        if let Some(k) = self.banner {
            if !was_open && self.last_sounded < Some(k) {
                due.raise(Raised::TurnSoon {
                    turn: turns[k].id(),
                });
            }
            self.last_sounded = self.last_sounded.max(Some(k));
        }
        let beep = next.filter(|next| {
            self.banner.is_some()
                && next.unblocked
                && self.last_beeped < Some(next.index)
                && ahead(next).is_some_and(|mm| mm <= BEEP_MM)
        });
        if let Some(next) = beep {
            self.last_beeped = Some(next.index);
            due.raise(Raised::TurnNow {
                turn: next.turn.id(),
            });
        }
    }

    fn next<'t>(&self, turns: &'t [Turn]) -> Option<Next<'t>> {
        let mut unblocked = true;
        for (index, turn) in turns.iter().enumerate().skip(self.cursor) {
            match glyph(turn) {
                Some(glyph) => {
                    return Some(Next {
                        index,
                        turn,
                        glyph,
                        unblocked,
                    });
                }
                None if turn.cornered => unblocked = false,
                None => {}
            }
        }
        None
    }

    fn holds(&self, raised: Raised, turns: &[Turn]) -> bool {
        let is = |k: usize, turn: TurnId| turns.get(k).is_some_and(|t| t.id() == turn);
        match raised {
            Raised::Following | Raised::OnCourse => self.place == Place::On,
            Raised::OffCourse | Raised::StillOff => matches!(self.place, Place::Off { .. }),
            Raised::TurnSoon { turn } => self
                .banner
                .is_some_and(|k| is(k, turn) && self.last_beeped < Some(k)),
            Raised::TurnNow { turn } => {
                let next = self.next(turns).map(|next| next.index);
                self.banner.is_some()
                    && next.is_some_and(|k| is(k, turn))
                    && self.last_beeped == next
            }
        }
    }

    pub fn sign(&self, turns: &[Turn], now_ms: u64) -> Sign {
        if let Some(at) = self.arrived_ms {
            return match now_ms < at.saturating_add(ARRIVED_MS) {
                true => Sign::Arrived,
                false => Sign::None,
            };
        }
        let Some(last) = self.last.filter(|_| !self.lost) else {
            return Sign::None;
        };
        if let Place::Off { junction, .. } = self.place {
            return Sign::OffCourse {
                back: Distance::from_m(distance(last.here, junction.at)),
            };
        }
        let next = self.next(turns).map(|next| {
            let ahead = next.turn.start_mm.saturating_sub(self.furthest_mm);
            (next.glyph, Distance::from_mm(ahead))
        });
        let passed = self.banner.and_then(|k| turns.get(k)).and_then(glyph);
        match (self.banner, next, passed) {
            (Some(_), Some((glyph, distance)), _) => Sign::Turn { glyph, distance },
            (Some(_), None, Some(glyph)) => Sign::Turn {
                glyph,
                distance: Distance::from_mm(0),
            },
            (_, Some((glyph, distance)), _) => Sign::Chip { glyph, distance },
            _ => Sign::Chip {
                glyph: Glyph::Finish,
                distance: Distance::from_mm(last.to_end_mm),
            },
        }
    }

    pub fn guidance(
        &self,
        turns: &[Turn],
        now_ms: u64,
        locate: impl Fn(u32) -> Point,
    ) -> Option<Guidance> {
        self.last.filter(|_| !self.lost)?;
        let junction = match self.place {
            Place::Off { junction, .. } => Some(junction),
            Place::On | Place::Leaving { .. } => None,
        };
        Some(Guidance {
            sign: self.sign(turns, now_ms),
            turn: self.next(turns).map(|next| {
                let along_mm = corner(next.turn);
                let at = locate(along_mm);
                (Spot { along_mm, at }, next.glyph)
            }),
            junction,
        })
    }

    /// Turns from the list's start it no longer needs: those passed, but
    /// the last, whose start may be where the rider leaves the course.
    pub fn passed(&self) -> usize {
        let keep = self.cursor.saturating_sub(1);
        self.banner.map_or(keep, |k| k.min(keep))
    }

    /// The first `count` turns of the list, up to `passed`, are gone.
    pub fn forget(&mut self, count: usize) {
        let count = count.min(self.passed());
        let shift = |k: Option<usize>| k.and_then(|k| k.checked_sub(count));
        self.cursor -= count;
        self.banner = shift(self.banner);
        self.last_sounded = shift(self.last_sounded);
        self.last_beeped = shift(self.last_beeped);
    }
}

/// Where a turn bends: midway from where it starts to where it ends.
fn corner(turn: &Turn) -> u32 {
    turn.start_mm / 2 + turn.end_mm / 2
}

fn window_mm(speed_mm_s: Option<u32>) -> u32 {
    speed_mm_s
        .map_or(WINDOW_MIN_MM, |v| v.saturating_mul(WINDOW_S))
        .clamp(WINDOW_MIN_MM, WINDOW_MAX_MM)
}

fn dense(turn: &Turn, next: &Turn, speed_mm_s: Option<u32>) -> bool {
    let gap = next.start_mm.saturating_sub(turn.end_mm);
    gap < DENSE_MM || speed_mm_s.is_some_and(|v| gap < v.saturating_mul(DENSE_S))
}

#[cfg(test)]
pub(crate) mod tests;
