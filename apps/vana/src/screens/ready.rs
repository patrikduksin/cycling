//! Ready: the calm screen between Home and a recording ride.
//!
//! It shows what matters before a ride in plain words: whether GPS has a
//! position, which sensors are live, the battery and room to record. Each
//! line is a status dot, a short headline and, when something needs the
//! rider, what to do about it. ACT starts the ride (the green cue beside it),
//! BACK returns Home, and rolling off starts it too (auto-start). The Ready
//! screen's tree (`compose::builtin::ready`) lays it out.
use super::chrome::{AMBER, GREEN, RED, TERTIARY};
use super::workout::Text;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::builtin::ScreenId;
    use crate::compose::input::{Action, Go, hit};
    use crate::screens::cue::{Cue, Icon};
    use crate::screens::workout::{Page, View};
    use crate::screens::{Layout, panels};
    use device_api::input::Role;

    fn readiness(level: Level, title: &str, hint: &str) -> Readiness {
        Readiness {
            riding: Text::new(),
            checks: core::array::from_fn(|_| Check::new(level, title, hint)),
        }
    }

    fn shown(layout: &'static Layout, ready: Readiness) -> View<'static> {
        let mut view = View::new(Page::Ready, 0, None);
        view.layout = layout;
        view.compose_main();
        view.show_ready(ready);
        view.prepare().check_layout();
        view
    }

    #[test]
    fn longest_lines_lay_out_whole_clear_of_the_start_cue() {
        crate::screens::check::enable(true);
        let hints = [
            "RECORDS WITHOUT A MAP",
            "YOU CAN START ANYWAY",
            "TAP HERE TO PAIR ONE",
            "TURN VANA OFF AND ON",
            "CHARGE AFTER THE RIDE",
            "CHARGE BEFORE RIDING",
            "HR  PWR  SPD/C  RDR",
        ];
        let storage = [
            "EXPORT OLD RIDES",
            "ON INTERNAL MEMORY",
            "LAST RIDE RECOVERED",
        ];
        let play = [Cue::new(Role::Act, Icon::Play)];
        for layout in panels::ALL {
            for (hint, stored) in hints.iter().zip(storage.iter().cycle()) {
                let mut ready = readiness(Level::Wait, "SENSORS WAKING UP", hint);
                ready.checks[Line::Storage as usize] =
                    Check::new(Level::Wait, "UNDER 1 MIN OF ROOM", stored);
                let _ = ready.riding.push_str("RECOVERY SPIN 30");
                let view = shown(layout, ready);
                let what = std::format!("{hint} on {:?}", layout.scaled.panel);
                let canvas = view.canvas();
                assert_eq!(canvas.runs.len(), 10, "{what}");
                let want = [
                    "ALMOST READY",
                    "RECOVERY SPIN 30",
                    "SENSORS WAKING UP",
                    "UNDER 1 MIN OF ROOM",
                    stored,
                    hint,
                ];
                canvas.assert_laid_out(layout.chrome.content, &want, &what);
                for run in &canvas.runs {
                    let (name, rect) = crate::screens::chrome::layout::text(run).unwrap();
                    let under = play.iter().any(|cue| cue.covers(rect, layout));
                    assert!(!under, "{what}: {name} under the cue");
                }
            }
        }
    }

    #[test]
    fn the_headline_follows_the_checks_and_taps_open_the_workout_and_sensors_lines() {
        let good = readiness(Level::Good, "GPS LOCKED", "");
        for layout in panels::ALL {
            assert!(shown(layout, good.clone()).canvas().shows("READY"));
            let mut blocked = good.clone();
            blocked.checks[Line::Storage as usize] =
                Check::new(Level::Problem, "SYNC RIDES FIRST", "ON THE PHONE PAGE");
            assert!(!blocked.can_start());
            let view = shown(layout, blocked);
            assert!(view.canvas().shows("NOT READY"));
            let laid = view
                .laids
                .and_then(|laids| laids.get(ScreenId::Ready))
                .unwrap();
            let solved = laid.solved();
            let actions: std::vec::Vec<_> = solved
                .focus_order()
                .map(|placed| {
                    let rect = placed.rect;
                    let (x, y) = (rect.left + rect.width() / 2, rect.top + 2);
                    hit(laid.buttons(), &solved, x, y).map(|(_, action)| action)
                })
                .collect();
            let open = |page| Some(Action::Go(Go::Open(page)));
            assert_eq!(actions, [open(Page::Workouts), open(Page::Sensors)]);
            let headline = view.canvas().find("NOT READY").unwrap();
            let (x, y) = (headline.right() as u16 - 2, headline.top as u16 + 2);
            assert_eq!(hit(laid.buttons(), &solved, x, y), None);
        }
    }
}
