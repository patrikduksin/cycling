//! Ready: the calm screen between Home and a recording ride.
//!
//! It shows what matters before a ride in plain words: whether GPS has a
//! position, which sensors are live, the battery and room to record. Each
//! line is a status dot, a short headline and, when something needs the
//! rider, what to do about it. ACT starts the ride (the green cue beside it),
//! BACK returns Home, and rolling off starts it too (auto-start).
use super::Layout;
use super::canvas::{Canvas, Shape};
use super::chrome::{AMBER, Align, GREEN, RED, Rect, SECONDARY, TERTIARY, WHITE};
use super::cue::{self, Corner, Cue, Icon};
use super::scaled::{Scaled, Track};
use super::workout::Text;
use firmware_shell::rendering::font::FaceRole;

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

/// Ready on one panel: the headline and what the ride will be, then a line
/// per check. Each text row's capitals end on its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub headline: Rect,
    pub riding: Rect,
    /// A tap here, round what the ride will be, changes the workout.
    pub riding_tap: Rect,
    pub checks: [CheckRegions; 4],
}

/// One check's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CheckRegions {
    /// A tap here opens what the line is about.
    pub tap: Rect,
    /// The status dot's columns, centred on the title's capitals.
    pub dot: Rect,
    pub title: Rect,
    pub hint: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Cap, Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, text, _] = content.cols([Unit(12), Fr(1), Unit(4)], Unit(0), at);
    let [_, dot, _] = content.cols([Unit(12), Unit(10), Fr(1)], Unit(0), at);
    let [_, line_text, _] = content.cols([Unit(30), Fr(1), Unit(4)], Unit(0), at);
    let [headline, _, riding, under, first, second, third, fourth, _] = content.rows(
        [
            Unit(42),
            Unit(4),
            Unit(16),
            Unit(6),
            Unit(47),
            Unit(47),
            Unit(47),
            Unit(47),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let taps = [first, second, third, fourth];
    let mut checks = [CheckRegions {
        tap: first,
        dot: first,
        title: first,
        hint: first,
    }; 4];
    let mut i = 0;
    while i < checks.len() {
        let tap = taps[i];
        let [_, title, hint, _] =
            tap.rows([Unit(6), Cap(FaceRole::Body), Unit(20), Fr(1)], Unit(0), at);
        let clear_of_cue = at.panel.width - cue::reach(at.panel.height - hint.bottom);
        let hint = hint.with_columns(line_text);
        checks[i] = CheckRegions {
            tap,
            dot: title.with_columns(dot),
            title: title.with_columns(line_text),
            hint: Rect {
                right: if clear_of_cue < hint.right {
                    clear_of_cue
                } else {
                    hint.right
                },
                ..hint
            },
        };
        i += 1;
    }
    Regions {
        headline: headline.with_columns(text),
        riding: riding.with_columns(text),
        riding_tap: Rect {
            bottom: under.bottom,
            ..riding
        },
        checks,
    }
}

impl Regions {
    /// A tap at panel point (`x`, `y`) on what the ride will be.
    pub fn riding_at(&self, x: usize, y: usize) -> bool {
        self.riding_tap.contains(x, y)
    }

    /// The line under a tap at panel point (`x`, `y`), if any.
    pub fn line_at(&self, x: usize, y: usize) -> Option<Line> {
        let index = self
            .checks
            .iter()
            .position(|check| check.tap.contains(x, y))?;
        LINES.get(index).copied()
    }
}

pub fn prepare(layout: &Layout, readiness: &Readiness) -> Canvas {
    let (at, regions) = (&layout.scaled, &layout.ready);
    let [title, body, caption] =
        [FaceRole::Title, FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let mut canvas = Canvas::default();
    let left = |canvas: &mut Canvas, faces: &[_], text: &[u8], row: Rect, color| {
        canvas.fit(
            faces,
            text,
            row.width(),
            row.left,
            row.bottom,
            Align::Left,
            color,
        );
    };
    let (headline, color): (&[u8], u16) = if !readiness.can_start() {
        (b"NOT READY", RED)
    } else if readiness.all_set() {
        (b"READY", WHITE)
    } else {
        (b"ALMOST READY", WHITE)
    };
    left(
        &mut canvas,
        &[title, body],
        headline,
        regions.headline,
        color,
    );
    let riding = match readiness.riding.as_bytes() {
        b"" => &b"FREE RIDE"[..],
        name => name,
    };
    left(
        &mut canvas,
        &[caption],
        riding,
        regions.riding,
        super::chrome::ACCENT,
    );
    for (check, rows) in readiness.checks.iter().zip(&regions.checks) {
        let (dot, title) = (rows.dot, rows.title);
        canvas.shape(Shape::Disc {
            cx: 2 * (dot.left + dot.right) as i32,
            cy: 4 * (title.top + title.height() / 2) as i32,
            radius: 2 * dot.width() as i32,
            color: check.level.color(),
        });
        left(
            &mut canvas,
            &[body, caption],
            check.title.as_bytes(),
            title,
            WHITE,
        );
        if !check.hint.as_bytes().is_empty() {
            let hint = check.hint.as_bytes();
            left(&mut canvas, &[caption], hint, rows.hint, SECONDARY);
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::{Page, View};
    use crate::screens::{BASE, panels};

    fn readiness(level: Level, title: &str, hint: &str) -> Readiness {
        Readiness {
            riding: Text::new(),
            checks: core::array::from_fn(|_| Check::new(level, title, hint)),
        }
    }

    fn under_the_cue(rect: Rect, layout: &Layout) -> bool {
        let panel = layout.scaled.panel;
        let dx = panel.width.saturating_sub(rect.right);
        let dy = panel.height.saturating_sub(rect.bottom);
        dx * dx + dy * dy < cue::RADIUS * cue::RADIUS
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
        for layout in panels::ALL {
            let at = &layout.scaled;
            for check in &layout.ready.checks {
                let what = std::format!("a hint's region on {:?}", at.panel);
                assert!(!under_the_cue(check.hint, layout), "{what} under the cue");
            }
            for (hint, stored) in hints.iter().zip(storage.iter().cycle()) {
                let mut ready = readiness(Level::Wait, "SENSORS WAKING UP", hint);
                ready.checks[Line::Storage as usize] =
                    Check::new(Level::Wait, "UNDER 1 MIN OF ROOM", stored);
                let _ = ready.riding.push_str("RECOVERY SPIN 30");
                let mut view = View::new(Page::Ready, 0, None);
                view.layout = layout;
                view.show_ready(ready);
                view.prepare().check_layout();
                let what = std::format!("{hint} on {:?}", at.panel);
                let canvas = view.canvas();
                assert_eq!(canvas.runs.len(), 10, "{what}");
                let mut want =
                    std::vec!["ALMOST READY", "SENSORS WAKING UP", "UNDER 1 MIN OF ROOM"];
                want.push(stored);
                want.push(hint);
                canvas.assert_laid_out(layout.chrome.content, &want, &what);
                for run in &canvas.runs {
                    let (name, rect) = crate::screens::chrome::layout::text(run).unwrap();
                    assert!(!under_the_cue(rect, layout), "{what}: {name} under the cue");
                }
            }
            // A journal kept in memory that has no room for another start.
            for title in ["SYNC RIDES FIRST", "CLEAR RIDES FIRST"] {
                let mut ready = readiness(Level::Good, "GPS LOCKED", "");
                ready.checks[Line::Storage as usize] =
                    Check::new(Level::Problem, title, "ON THE PHONE PAGE");
                let mut view = View::new(Page::Ready, 0, None);
                view.layout = layout;
                view.show_ready(ready);
                view.prepare().check_layout();
                let what = std::format!("{title} on {:?}", at.panel);
                let canvas = view.canvas();
                let want = ["NOT READY", title, "ON THE PHONE PAGE"];
                canvas.assert_laid_out(layout.chrome.content, &want, &what);
                for run in &canvas.runs {
                    let (name, rect) = crate::screens::chrome::layout::text(run).unwrap();
                    assert!(!under_the_cue(rect, layout), "{what}: {name} under the cue");
                }
            }
        }
    }

    #[test]
    fn headline_follows_the_checks_and_taps_find_lines() {
        let good = readiness(Level::Good, "GPS LOCKED", "");
        assert!(prepare(&BASE, &good).shows("READY"));
        let mut blocked = good.clone();
        blocked.checks[Line::Storage as usize] = Check::new(Level::Problem, "STORAGE FULL", "");
        assert!(!blocked.can_start());
        assert!(prepare(&BASE, &blocked).shows("NOT READY"));
        let ready = &BASE.ready;
        assert_eq!(ready.line_at(100, 92), Some(Line::Gps));
        assert_eq!(ready.line_at(100, 92 + 47 + 10), Some(Line::Sensors));
        assert_eq!(ready.line_at(100, 92 + 3 * 47 + 30), Some(Line::Storage));
        assert_eq!(ready.line_at(100, 92 + 4 * 47 + 10), None);
        assert_eq!(ready.line_at(100, 30), None);
        assert!(ready.riding_at(100, 70) && ready.riding_at(100, 91));
        assert!(!ready.riding_at(100, 69) && !ready.riding_at(100, 92));
        let small = &panels::SMALL.ready;
        for (index, check) in small.checks.iter().enumerate() {
            let y = check.title.top;
            assert_eq!(small.line_at(90, y), Some(LINES[index]));
        }
    }
}
