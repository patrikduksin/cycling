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
//! went. The way back leads to where they left the course (`Junction`),
//! which never moves. Each step returns at most one `Event`. A second one
//! due on the same fix waits one fix, and is dropped if it no longer holds.
use crate::ride::course::turns::{HALF_CHORD_MM, Kind, Side, Turn};
use crate::ride::homing::distance;

type Point = (f32, f32);

/// Off course beyond this from the course, for `CONFIRM_MS`.
pub const OFF_M: f32 = 40.0;
/// Back on course within this, on any stretch, riding its way.
pub const ON_M: f32 = 25.0;
const CONFIRM_MS: u64 = 5_000;
/// No fix for this long: the sign shows nothing.
pub const LOST_MS: u64 = 5_000;
/// The off-course tones play again once, this long after the first.
const REMIND_MS: u64 = 15_000;
const WINDOW_S: u32 = 30;
const WINDOW_MIN_MM: u32 = 150_000;
const WINDOW_MAX_MM: u32 = 400_000;
const GRACE_MM: u32 = 40_000;
const DENSE_MM: u32 = 300_000;
const DENSE_S: u32 = 25;
const BEEP_MM: u32 = 30_000;
/// The rider this far back from the furthest they were has turned round:
/// what was shown and sounded is forgotten.
const REWIND_MM: u32 = 30_000;
/// Left the course within this along of a turn: they missed that turn.
const MISSED_MM: u32 = 60_000;
const ARRIVED_MS: u64 = 60_000;

/// What the follower saw on one fix.
#[derive(Clone, Copy, Debug)]
pub enum Observed {
    /// Riding along the course: following started by itself on this fix.
    Joined(Fixed),
    Fix(Fixed),
    /// At the course's end.
    Arrived { now_ms: u64 },
    /// No fix for `LOST_MS`.
    Lost,
}

/// The rider matched to the course on a fix.
#[derive(Clone, Copy, Debug)]
pub struct Fixed {
    pub now_ms: u64,
    /// The matched point, millimetres along from the course's start.
    pub along_mm: u32,
    /// Millimetres from the matched point to the course's end.
    pub to_end_mm: u32,
    /// Metres from the rider to the matched point.
    pub off_m: f32,
    /// The rider, metres east and north in the ride's frame.
    pub here: Point,
    /// Heading the course's way at the matched stretch.
    pub forward: bool,
    /// The ride's live speed; without one the banner opens at its least.
    pub speed_mm_s: Option<u32>,
}

/// What a step asks the app to sound or show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Following started by itself.
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

impl Event {
    /// The higher plays first when two are due on one fix.
    const fn rank(self) -> u8 {
        match self {
            Self::Arrived => 5,
            Self::OffCourse => 4,
            Self::Following | Self::OnCourse => 3,
            Self::StillOff => 2,
            Self::TurnNow => 1,
            Self::TurnSoon => 0,
        }
    }
}

/// What the sign draws. One byte, so `Sign` fits in four.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Left,
    Right,
    SharpLeft,
    SharpRight,
    UTurn,
    /// The course's end, past its last turn.
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

/// The glyph a turn shows. Round a city block (`twice`) it is one turn,
/// the way each corner goes.
fn glyph(turn: &Turn) -> Option<Glyph> {
    turn.kind.map(Glyph::from)
}

/// A distance as the sign draws it: the nearest 10 m below 1 km, half down,
/// then the nearest 0.1 km below 10 km, then whole km. Two distances that
/// draw the same are equal.
///
/// Encoded as tens of metres below 100, `100 +` tenths of a km below 200,
/// and `200 +` km, saturating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Distance(u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Metres,
    Kilometres,
}

impl Distance {
    pub fn from_mm(mm: u32) -> Self {
        let tens = mm.saturating_add(4_999) / 10_000;
        if tens < 100 {
            return Self(tens as u16);
        }
        let tenths = mm.saturating_add(49_999) / 100_000;
        if tenths < 100 {
            return Self(100 + tenths as u16);
        }
        let km = mm.saturating_add(499_999) / 1_000_000;
        Self(u16::try_from(km + 200).unwrap_or(u16::MAX))
    }

    fn from_m(metres: f32) -> Self {
        // A float cast saturates, so no distance overflows.
        Self::from_mm((metres * 1000.0) as u32)
    }

    pub fn unit(self) -> Unit {
        match self.0 {
            0..100 => Unit::Metres,
            _ => Unit::Kilometres,
        }
    }
}

/// The digits: `120`, `6.4`, `12`.
impl core::fmt::Display for Distance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            tens @ 0..100 => write!(f, "{}", u32::from(tens) * 10),
            tenths @ 100..200 => write!(f, "{}.{}", (tenths - 100) / 10, tenths % 10),
            km => write!(f, "{}", km - 200),
        }
    }
}

/// What the rider sees now: one sign in one of three fills, or none.
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
    /// The next turn: where it starts and its glyph.
    pub turn: Option<(Point, Glyph)>,
    /// The matched point: the course ahead starts here.
    pub matched: Point,
    /// Off course: where the rider left it, which the way back leads to.
    pub junction: Option<Point>,
}

/// Where the rider left the course.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Junction {
    along_mm: u32,
    at: Point,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Off {
    #[default]
    On,
    /// Over `OFF_M` since the fix at `since_ms`.
    Leaving { since_ms: u64, junction: Junction },
    Off {
        since_ms: u64,
        junction: Junction,
        reminded: bool,
    },
}

/// The next named turn from the cursor: `clear` unless an unnamed corner
/// comes first, which holds its banner and beep back until the rider is
/// past it.
#[derive(Clone, Copy)]
struct Next<'t> {
    index: usize,
    turn: &'t Turn,
    glyph: Glyph,
    clear: bool,
}

/// The two highest-ranked events due on a step.
#[derive(Default)]
struct Due {
    first: Option<Event>,
    second: Option<Event>,
}

impl Due {
    fn raise(&mut self, event: Event) {
        match self.first {
            Some(first) if first.rank() >= event.rank() => {
                if self.second.is_none_or(|second| second.rank() < event.rank()) {
                    self.second = Some(event);
                }
            }
            first => (self.first, self.second) = (Some(event), first),
        }
    }
}

/// The rider against the course's turns, fix by fix.
///
/// Turn indexes are into the follower's list, which drops the turns up to
/// `passed` when it fills (`forget`). `sounded` and `beeped` name the last
/// turn whose banner sound and beep played: none plays twice for a turn
/// unless the rider turns round (`REWIND_MM`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Navigator {
    /// The first turn the rider is not past.
    cursor: usize,
    /// The furthest along the rider has been: their place for the turns,
    /// so jitter never moves it back.
    furthest_mm: u32,
    /// The banner is up for this turn: the next one, or one just passed.
    banner: Option<usize>,
    sounded: Option<usize>,
    beeped: Option<usize>,
    off: Off,
    last: Option<Fixed>,
    arrived_ms: Option<u64>,
    lost: bool,
    pending: Option<Event>,
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
                if let Off::Leaving { .. } = self.off {
                    self.off = Off::On;
                }
                return None;
            }
            Observed::Joined(fix) => {
                *self = Self::default();
                due.raise(Event::Following);
                self.fix(fix, turns, &locate, &mut due);
            }
            Observed::Fix(fix) => self.fix(fix, turns, &locate, &mut due),
        }
        if let Some(parked) = parked.filter(|&event| self.holds(event, turns)) {
            due.raise(parked);
        }
        self.pending = due.second;
        due.first
    }

    fn fix(
        &mut self,
        fix: Fixed,
        turns: &[Turn],
        locate: &impl Fn(u32) -> Point,
        due: &mut Due,
    ) {
        self.lost = false;
        let gap = self
            .last
            .is_some_and(|last| fix.now_ms.saturating_sub(last.now_ms) > LOST_MS);
        let now = fix.now_ms;
        self.off = match self.off {
            Off::On if fix.off_m > OFF_M => Off::Leaving {
                since_ms: now,
                junction: self.junction(turns, fix.along_mm, locate),
            },
            Off::Leaving { .. } if fix.off_m <= OFF_M => Off::On,
            Off::Leaving { junction, .. } if gap => Off::Leaving {
                since_ms: now,
                junction,
            },
            Off::Leaving { since_ms, junction } if now.saturating_sub(since_ms) >= CONFIRM_MS => {
                due.raise(Event::OffCourse);
                Off::Off {
                    since_ms: now,
                    junction,
                    reminded: false,
                }
            }
            Off::Off { .. } if fix.off_m <= ON_M && fix.forward => {
                due.raise(Event::OnCourse);
                Off::On
            }
            Off::Off {
                since_ms,
                junction,
                reminded: false,
            } if now.saturating_sub(since_ms) >= REMIND_MS => {
                due.raise(Event::StillOff);
                Off::Off {
                    since_ms,
                    junction,
                    reminded: true,
                }
            }
            off => off,
        };
        if self.off == Off::On {
            self.follow_turns(&fix, turns, due);
        }
        self.last = Some(fix);
    }

    /// Where the rider, `along_mm` now, left the course: the start of the
    /// turn they were at, if they left within `MISSED_MM` of one, else the
    /// last point matched on it.
    fn junction(&self, turns: &[Turn], along_mm: u32, locate: &impl Fn(u32) -> Point) -> Junction {
        let left_mm = self.last.map_or(along_mm, |last| last.along_mm);
        let just_passed = self.cursor.checked_sub(1).and_then(|k| turns.get(k));
        let shown = self.next(turns).map(|next| next.turn);
        let missed = [just_passed, shown]
            .into_iter()
            .flatten()
            .filter(|turn| turn.start_mm.abs_diff(left_mm) <= MISSED_MM)
            .min_by_key(|turn| turn.start_mm.abs_diff(left_mm));
        let along_mm = missed.map_or(left_mm, |turn| turn.start_mm);
        Junction {
            along_mm,
            at: locate(along_mm),
        }
    }

    fn follow_turns(&mut self, fix: &Fixed, turns: &[Turn], due: &mut Due) {
        if !fix.forward {
            self.banner = None;
            return;
        }
        if fix.along_mm.saturating_add(REWIND_MM) < self.furthest_mm {
            (self.cursor, self.banner, self.sounded, self.beeped) = (0, None, None, None);
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
        // None once the rider is in the turn: too late to say.
        let ahead = |next: &Next| next.turn.start_mm.checked_sub(here);
        let opens = |next: &Next| {
            next.clear && ahead(next).is_some_and(|mm| mm <= window_mm(speed))
        };
        let was_open = self.banner.is_some();
        self.banner = match (self.banner, next) {
            (Some(k), Some(next)) if k == next.index => Some(k),
            (Some(k), next) if k < self.cursor => {
                let passed = &turns[k];
                match next {
                    Some(next) if next.clear && dense(passed, next.turn, speed) => Some(next.index),
                    _ if here < passed.end_mm.saturating_add(GRACE_MM) => Some(k),
                    Some(next) if opens(&next) => Some(next.index),
                    _ => None,
                }
            }
            _ => next.filter(opens).map(|next| next.index),
        };
        if let Some(k) = self.banner {
            if !was_open && self.sounded < Some(k) {
                due.raise(Event::TurnSoon);
            }
            self.sounded = self.sounded.max(Some(k));
        }
        let beep = next.filter(|next| {
            self.banner.is_some()
                && next.clear
                && self.beeped < Some(next.index)
                && ahead(next).is_some_and(|mm| mm <= BEEP_MM)
        });
        if let Some(next) = beep {
            self.beeped = Some(next.index);
            due.raise(Event::TurnNow);
        }
    }

    fn next<'t>(&self, turns: &'t [Turn]) -> Option<Next<'t>> {
        let mut clear = true;
        for (index, turn) in turns.iter().enumerate().skip(self.cursor) {
            match glyph(turn) {
                Some(glyph) => {
                    return Some(Next {
                        index,
                        turn,
                        glyph,
                        clear,
                    });
                }
                None if turn.cornered => clear = false,
                None => {}
            }
        }
        None
    }

    /// Whether an event parked a fix ago still says what is so.
    fn holds(&self, event: Event, turns: &[Turn]) -> bool {
        let next = self.next(turns).map(|next| next.index);
        match event {
            Event::Following | Event::OnCourse => self.off == Off::On,
            Event::OffCourse | Event::StillOff => matches!(self.off, Off::Off { .. }),
            Event::TurnSoon => self.banner.is_some() && self.beeped < next,
            Event::TurnNow => self.banner.is_some() && next.is_some() && self.beeped == next,
            Event::Arrived => true,
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
        if let Off::Off { junction, .. } = self.off {
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
        let last = self.last.filter(|_| !self.lost)?;
        let junction = match self.off {
            Off::Off { junction, .. } => Some(junction.at),
            Off::On | Off::Leaving { .. } => None,
        };
        Some(Guidance {
            sign: self.sign(turns, now_ms),
            turn: self
                .next(turns)
                .map(|next| (locate(next.turn.start_mm), next.glyph)),
            matched: locate(last.along_mm),
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
        self.sounded = shift(self.sounded);
        self.beeped = shift(self.beeped);
    }
}

/// How far before a turn the banner opens at `speed_mm_s`.
fn window_mm(speed_mm_s: Option<u32>) -> u32 {
    speed_mm_s
        .map_or(WINDOW_MIN_MM, |v| v.saturating_mul(WINDOW_S))
        .clamp(WINDOW_MIN_MM, WINDOW_MAX_MM)
}

/// Whether `next` follows `turn` closely enough to keep the sign up.
fn dense(turn: &Turn, next: &Turn, speed_mm_s: Option<u32>) -> bool {
    let gap = next.start_mm.saturating_sub(turn.end_mm);
    gap < DENSE_MM || speed_mm_s.is_some_and(|v| gap < v.saturating_mul(DENSE_S))
}

#[cfg(test)]
pub(crate) mod tests;
