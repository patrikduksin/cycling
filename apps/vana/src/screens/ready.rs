//! Ready: the calm screen between Home and a recording ride.
//!
//! It shows what matters before a ride in plain words: whether GPS has a
//! position, which sensors are live, the battery and room to record. Each
//! line is a status dot, a short headline and, when something needs the
//! rider, what to do about it. ACT starts the ride (the green cue beside it),
//! BACK returns Home, and rolling off starts it too (auto-start).
use super::canvas::{Canvas, Shape};
use super::chrome::{
    AMBER, Align, BODY, CAPTION, CONTENT, GREEN, RED, SECONDARY, TERTIARY, TITLE, WHITE,
};
use super::cue::{Corner, Cue, Icon};
use super::workout::Text;

/// ACT starts the ride.
pub const CUES: [Cue; 1] = [Cue::new(Corner::BottomRight, Icon::Play)];

/// How a check stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    /// Ready: green.
    Good,
    /// Still coming, or worth knowing: amber.
    #[default]
    Wait,
    /// Stops a ride from recording, or will soon: red.
    Problem,
    /// Not used on this ride: grey.
    Off,
}

impl Level {
    pub const fn color(self) -> u16 {
        match self {
            Self::Good => GREEN,
            Self::Wait => AMBER,
            Self::Problem => RED,
            Self::Off => TERTIARY,
        }
    }
}

/// One line of the Ready screen.
#[derive(Clone, Default)]
pub struct Check {
    pub level: Level,
    pub title: Text<24>,
    /// What to do about it; empty when nothing is needed.
    pub hint: Text<28>,
}

impl Check {
    const EMPTY: Self = Self {
        level: Level::Wait,
        title: Text::new(),
        hint: Text::new(),
    };

    pub fn new(level: Level, title: &str, hint: &str) -> Self {
        let mut check = Self {
            level,
            ..Self::default()
        };
        let _ = check.title.push_str(title);
        let _ = check.hint.push_str(hint);
        check
    }
}

/// The checks, in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Gps,
    Sensors,
    Battery,
    Storage,
}

pub const LINES: [Line; 4] = [Line::Gps, Line::Sensors, Line::Battery, Line::Storage];

#[derive(Clone, Default)]
pub struct Readiness {
    /// What the ride will be: FREE RIDE, or the chosen workout's name.
    pub riding: Text<24>,
    pub checks: [Check; 4],
}

impl Readiness {
    pub const EMPTY: Self = Self {
        riding: Text::new(),
        checks: [Check::EMPTY; 4],
    };

    /// A ride can start: no check is a problem that stops recording.
    pub fn can_start(&self) -> bool {
        self.checks[Line::Storage as usize].level != Level::Problem
    }

    /// Every check is good or not in use.
    pub fn all_set(&self) -> bool {
        self.checks
            .iter()
            .all(|check| matches!(check.level, Level::Good | Level::Off))
    }
}

const LEFT: usize = 12;
const TEXT_LEFT: usize = LEFT + 18;
const RIGHT: usize = CONTENT.right - 4;
const HEADLINE: usize = CONTENT.top + 42;
/// What the ride will be, under the headline.
const RIDING: usize = HEADLINE + 20;
/// The first line's top and the pitch between lines.
const FIRST: usize = CONTENT.top + 74;
const PITCH: usize = 47;

/// A tap on what the ride will be, under the headline, changes the workout.
pub fn riding_at(y: usize) -> bool {
    (HEADLINE + 4..FIRST - 6).contains(&y)
}

/// The line under a tap, if any.
pub fn line_at(y: usize) -> Option<Line> {
    let index = y.checked_sub(FIRST - 6)? / PITCH;
    LINES.get(index).copied()
}

pub fn prepare(readiness: &Readiness) -> Canvas {
    let mut canvas = Canvas::default();
    let (headline, color): (&[u8], u16) = if !readiness.can_start() {
        (b"NOT READY", RED)
    } else if readiness.all_set() {
        (b"READY", WHITE)
    } else {
        (b"ALMOST READY", WHITE)
    };
    canvas.fit(
        &[&TITLE, &BODY],
        headline,
        RIGHT - LEFT,
        LEFT,
        HEADLINE,
        Align::Left,
        color,
    );
    let riding = match readiness.riding.as_bytes() {
        b"" => &b"FREE RIDE"[..],
        name => name,
    };
    canvas.fit(
        &[&CAPTION],
        riding,
        RIGHT - LEFT,
        LEFT,
        RIDING,
        Align::Left,
        super::chrome::ACCENT,
    );
    for (index, check) in readiness.checks.iter().enumerate() {
        let top = FIRST + index * PITCH;
        // The dot centres on the headline's capitals.
        let cy = top + BODY.cap as usize / 2;
        canvas.shape(Shape::Disc {
            cx: 4 * (LEFT + 5) as i32,
            cy: 4 * cy as i32,
            radius: 4 * 5,
            color: check.level.color(),
        });
        canvas.fit(
            &[&BODY, &CAPTION],
            check.title.as_bytes(),
            RIGHT - TEXT_LEFT,
            TEXT_LEFT,
            top + BODY.cap as usize,
            Align::Left,
            WHITE,
        );
        if !check.hint.as_bytes().is_empty() {
            canvas.fit(
                &[&CAPTION],
                check.hint.as_bytes(),
                RIGHT - TEXT_LEFT,
                TEXT_LEFT,
                top + BODY.cap as usize + 20,
                Align::Left,
                SECONDARY,
            );
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{Rect, WIDTH};
    use crate::screens::cue;

    fn readiness(level: Level, title: &str, hint: &str) -> Readiness {
        Readiness {
            riding: Text::new(),
            checks: core::array::from_fn(|_| Check::new(level, title, hint)),
        }
    }

    #[test]
    fn longest_lines_lay_out_whole_clear_of_the_start_cue() {
        let ready = readiness(Level::Wait, "SENSORS WAKING UP", "RECORDS WITHOUT A MAP");
        let canvas = prepare(&ready);
        // The ACT cue owns the bottom-right corner.
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: 320 - cue::RADIUS,
        };
        let want = [
            "ALMOST READY",
            "FREE RIDE",
            "SENSORS WAKING UP",
            "RECORDS WITHOUT A MAP",
        ];
        canvas.assert_laid_out(bounds, &want, "ready");
        // Every hint the runtime writes fits whole.
        for hint in [
            "YOU CAN START ANYWAY",
            "TAP HERE TO PAIR ONE",
            "TURN VANA OFF AND ON",
            "CHARGE AFTER THE RIDE",
            "CHARGE BEFORE RIDING",
            "LAST RIDE RECOVERED",
            "HR  PWR  SPD/C  RDR",
        ] {
            let canvas = prepare(&readiness(Level::Wait, "ROOM FOR 6419 H", hint));
            canvas.assert_laid_out(bounds, &[hint, "ROOM FOR 6419 H"], hint);
        }
        let low = readiness(Level::Wait, "UNDER 1 MIN OF ROOM", "EXPORT OLD RIDES");
        prepare(&low).assert_laid_out(bounds, &["UNDER 1 MIN OF ROOM"], "low");
        assert_eq!(canvas.runs.len(), 10);
    }

    #[test]
    fn headline_follows_the_checks_and_taps_find_lines() {
        let good = readiness(Level::Good, "GPS LOCKED", "");
        assert!(prepare(&good).shows("READY"));
        let mut blocked = good.clone();
        blocked.checks[Line::Storage as usize] = Check::new(Level::Problem, "STORAGE FULL", "");
        assert!(!blocked.can_start());
        assert!(prepare(&blocked).shows("NOT READY"));
        assert_eq!(line_at(FIRST), Some(Line::Gps));
        assert_eq!(line_at(FIRST + PITCH + 10), Some(Line::Sensors));
        assert_eq!(line_at(FIRST + 3 * PITCH + 30), Some(Line::Storage));
        assert_eq!(line_at(FIRST + 4 * PITCH + 10), None);
        assert_eq!(line_at(30), None);
    }
}
