//! A line of the workout step: the target range, high over low with a rule
//! in the target's zone colour between them; the reading the target checks
//! as the page's hero, its unit over power's `3s` beside it; the time left
//! in the step; or the next step, its zone as a square before it.
use super::{px, specs};
use crate::compose::data::{Data, Reads};
use crate::compose::input::Action;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::ride::Training;
use crate::screens::ride::workout::{clock, km, unit};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;
use crate::workout::model::{Intensity, Length, Metric};
use crate::workout::player::{Left, TestReadout};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct StepTile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    Range,
    Reading,
    Left,
    Next,
}

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub shows: Shows,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A line of the workout step. Ids: Workout 64–127.
    pub enum Step: Spec {
        Range = 64 => Spec { name: "RANGE", shows: Shows::Range },
        Reading = 65 => Spec { name: "READING", shows: Shows::Reading },
        Left = 66 => Spec { name: "LEFT", shows: Shows::Left },
        Next = 67 => Spec { name: "NEXT", shows: Shows::Next },
    }
}

const RANGE_SLOT: &[u8] = b"888";
const HERO_SLOT: &[u8] = b"888";
const HERO_STEPPED: &[u8] = b"65535";
const LEFT_SLOT: &[u8] = b"88:88";
const NEXT_SLOT: &[u8] = b"88:88";
const HERO_FACES: [FaceRole; 3] = [
    LAYOUT.face.hero,
    LAYOUT.face.digits,
    LAYOUT.face.digits_small,
];
const POWER_NOTE: &[u8] = b"3s";
const FINISH: &[u8] = b"FINISH";

fn power_column(at: &Scaled) -> usize {
    at.face(LAYOUT.face.label)
        .fit_width(unit(Metric::Power))
        .max(at.face(LAYOUT.face.note).fit_width(POWER_NOTE))
}

impl Tile for StepTile {
    type Arg = Step;
    const NAME: &'static str = "step";

    fn reads(step: Step) -> Option<Reads> {
        (step.spec().shows == Shows::Reading).then_some(Reads::Target)
    }

    fn forms(step: Step, at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let gutter = px(at, space.gutter);
        let gap = px(at, space.gap);
        let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
        match step.spec().shows {
            Shows::Range => {
                let digits = at.face(LAYOUT.face.heading);
                forms(&[Form {
                    width: (gutter + digits.fit_width(RANGE_SLOT)) as u16,
                    height: (digits.fit_height()
                        + 2 * gap
                        + px(at, space.mark)
                        + digits.cap as usize) as u16,
                    shapes: 1,
                    runs: 2,
                }])
            }
            Shows::Reading => HERO_FACES
                .windows(2)
                .map(|faces| {
                    let (digits, stepped) = (at.face(faces[0]), at.face(faces[1]));
                    let number = digits
                        .fit_width(HERO_SLOT)
                        .max(stepped.fit_width(HERO_STEPPED));
                    Form {
                        width: (number + gap + power_column(at) + gutter) as u16,
                        height: digits.fit_height() as u16,
                        shapes: 0,
                        runs: 3,
                    }
                })
                .collect(),
            Shows::Left => {
                let digits = at.face(LAYOUT.face.digits);
                forms(&[Form {
                    width: (2 * gutter
                        + digits.fit_width(LEFT_SLOT)
                        + gap
                        + label.fit_width(b"LEFT")) as u16,
                    height: digits.fit_height() as u16,
                    shapes: 0,
                    runs: 2,
                }])
            }
            Shows::Next => {
                let digits = at.face(LAYOUT.face.line);
                let name = Intensity::ALL
                    .iter()
                    .map(|intensity| label.fit_width(intensity.short_label().as_bytes()))
                    .max()
                    .unwrap_or(0);
                forms(&[Form {
                    width: (2 * gutter
                        + note.fit_width(b"NEXT")
                        + 3 * gap
                        + px(at, space.dot)
                        + digits.fit_width(NEXT_SLOT)
                        + name) as u16,
                    height: digits.fit_height().max(label.fit_height()) as u16,
                    shapes: 1,
                    runs: 3,
                }])
            }
        }
    }

    fn action(step: Step) -> Option<Action> {
        (step.spec().shows == Shows::Left).then_some(Action::ExtendStep)
    }

    fn prepare(step: Step, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(training) = data.training() else {
            return;
        };
        let rect = placed.rect.rect();
        let theme = data.theme();
        match step.spec().shows {
            Shows::Range if !training.done => range(training, rect, at, theme, out),
            Shows::Reading => reading(training, placed, at, theme, out),
            Shows::Left => left(training, rect, at, theme, out),
            Shows::Next if !training.done => next(training, rect, at, theme, out),
            Shows::Range | Shows::Next => {}
        }
    }
}

fn range(training: &Training, rect: Rect, at: &Scaled, theme: &Theme, out: &mut Canvas) {
    let now = &training.now;
    let mut high = Text::<12>::new();
    let mut low = Text::<12>::new();
    let (rule, under): (u16, &[u8]) = match (now.ramp_watts, now.band, now.test) {
        (Some(watts), Some(_), _) => {
            let _ = write!(low, "{watts}");
            (theme.zone(training.zone), b"")
        }
        (None, Some(band), _) => {
            if band.low != band.high {
                let _ = write!(high, "{}", band.high);
            }
            let _ = write!(low, "{}", band.low);
            (theme.zone(training.zone), b"")
        }
        (_, None, Some(test)) => {
            let (watts, what): (u16, &[u8]) = match test {
                TestReadout::Average(watts) => (watts, b"AVG"),
                TestReadout::BestMinute(watts) => (watts, b"BEST"),
            };
            let _ = write!(high, "{watts}");
            (theme.off, what)
        }
        (_, None, None) => return,
    };
    let space = &LAYOUT.space;
    let digits = at.face(LAYOUT.face.heading);
    let left = rect.left + px(at, space.gutter);
    let width = rect.right.saturating_sub(left);
    let faces = [digits, at.face(LAYOUT.face.label)];
    let gap = px(at, space.gap);
    let rule_bottom = rect.bottom - digits.cap as usize - gap;
    let rule_top = rule_bottom - px(at, space.mark);
    out.shape(Shape::Bar {
        rect: Rect {
            left,
            right: left + digits.fit_width(RANGE_SLOT).min(width),
            top: rule_top,
            bottom: rule_bottom,
        },
        radius: px(at, LAYOUT.radius.bar),
        color: rule,
    });
    let hi_base = rule_top - gap;
    out.fit(
        &faces,
        high.as_bytes(),
        width,
        left,
        hi_base,
        Align::Left,
        theme.text,
    );
    if under.is_empty() {
        out.fit(
            &faces,
            low.as_bytes(),
            width,
            left,
            rect.bottom,
            Align::Left,
            theme.text,
        );
    } else {
        let label = at.face(LAYOUT.face.label);
        out.fit(
            &[label],
            under,
            width,
            left,
            rect.bottom,
            Align::Left,
            theme.sub,
        );
    }
}

fn reading(training: &Training, placed: &Placed, at: &Scaled, theme: &Theme, out: &mut Canvas) {
    let rect = placed.rect.rect();
    let now = &training.now;
    let metric = now.band.map_or(Metric::Power, |band| band.metric);
    let space = &LAYOUT.space;
    let right = rect.right - px(at, space.gutter);
    let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
    let power = metric == Metric::Power;
    let column =
        label
            .fit_width(unit(metric))
            .max(if power { note.fit_width(POWER_NOTE) } else { 0 });
    let column_left = right - column;
    let tone = |metric| match metric {
        Metric::Power => theme.power,
        Metric::Heart => theme.heart,
        Metric::Cadence => theme.cadence,
    };
    let base = rect.bottom;
    let mut unit_base = base;
    if power {
        out.fit(
            &[note],
            POWER_NOTE,
            column,
            column_left,
            base,
            Align::Left,
            theme.sub,
        );
        unit_base = base - note.fit_height() - px(at, space.tight);
    }
    out.fit(
        &[label],
        unit(metric),
        column,
        column_left,
        unit_base,
        Align::Left,
        tone(metric),
    );
    let mut value = Text::<12>::new();
    let _ = match now.reading {
        Some(reading) => write!(value, "{reading}"),
        None => value.push_str("--"),
    };
    let form = usize::from(placed.form).min(HERO_FACES.len() - 2);
    let faces = [HERO_FACES[form], HERO_FACES[form + 1]].map(|role| at.face(role));
    let hero_right = column_left - px(at, space.gap);
    out.fit(
        &faces,
        value.as_bytes(),
        hero_right.saturating_sub(rect.left),
        hero_right,
        base,
        Align::Right,
        tone(metric),
    );
}

fn left(training: &Training, rect: Rect, at: &Scaled, theme: &Theme, out: &mut Canvas) {
    let space = &LAYOUT.space;
    let left = rect.left + px(at, space.gutter);
    let right = rect.right - px(at, space.gutter);
    let label = at.face(LAYOUT.face.label);
    if training.done {
        out.fit(
            &[label],
            b"WORKOUT DONE",
            right - left,
            left,
            rect.bottom,
            Align::Left,
            theme.text,
        );
        return;
    }
    let now = &training.now;
    let mut value = Text::<12>::new();
    let words: &[u8] = match now.left {
        Left::Secs(secs) => {
            clock(&mut value, secs);
            b"LEFT"
        }
        Left::Metres(metres) => {
            km(&mut value, metres);
            b"KM LEFT"
        }
        Left::Open(secs) => {
            clock(&mut value, secs);
            let starts_test = now
                .next
                .is_some_and(|next| next.step.intensity == Intensity::Test);
            if starts_test {
                b"LAP TO START"
            } else {
                b"LAP TO END"
            }
        }
    };
    let words_width = label.fit_width(words);
    let digits = [
        at.face(LAYOUT.face.digits),
        at.face(LAYOUT.face.digits_small),
    ];
    let gap = px(at, space.gap);
    let run = out.fit(
        &digits,
        value.as_bytes(),
        (right - left).saturating_sub(gap + words_width),
        left,
        rect.bottom,
        Align::Left,
        theme.text,
    );
    let after = run.left + run.line.face().ink_width(run.line.text()) + gap;
    out.fit(
        &[label, at.face(LAYOUT.face.note)],
        words,
        right.saturating_sub(after),
        after,
        rect.bottom,
        Align::Left,
        theme.sub,
    );
}

fn next(training: &Training, rect: Rect, at: &Scaled, theme: &Theme, out: &mut Canvas) {
    let space = &LAYOUT.space;
    let gap = px(at, space.gap);
    let (left, right) = (
        rect.left + px(at, space.gutter),
        rect.right - px(at, space.gutter),
    );
    let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
    let base = rect.bottom;
    let said = out.fit(
        &[note],
        b"NEXT",
        right - left,
        left,
        base,
        Align::Left,
        theme.sub,
    );
    let mut x = said.left + note.ink_width(b"NEXT") + gap;
    let Some(played) = training.now.next else {
        out.fit(
            &[label],
            FINISH,
            right.saturating_sub(x),
            x,
            base,
            Align::Left,
            theme.text,
        );
        return;
    };
    let dot = px(at, space.dot);
    let middle = base - label.cap as usize / 2;
    out.shape(Shape::Bar {
        rect: Rect {
            left: x,
            right: x + dot,
            top: middle - dot / 2,
            bottom: middle - dot / 2 + dot,
        },
        radius: px(at, LAYOUT.radius.bar),
        color: theme.zone(training.next_zone),
    });
    x += dot + gap;
    let mut value = Text::<12>::new();
    let unit: &str = match played.step.length {
        Length::Time(secs) => {
            clock(&mut value, u32::from(secs));
            ""
        }
        Length::Distance(metres) => {
            let _ = write!(value, "{}.{}", metres / 1000, metres % 1000 / 100);
            "KM "
        }
        Length::Open => "",
    };
    if !value.as_bytes().is_empty() {
        let digits = at.face(LAYOUT.face.line);
        let run = out.fit(
            &[digits],
            value.as_bytes(),
            right.saturating_sub(x),
            x,
            base,
            Align::Left,
            theme.text,
        );
        x = run.left + digits.ink_width(run.line.text()) + gap;
    }
    let intensity = played.step.intensity;
    let room = right.saturating_sub(x);
    let mut name = Text::<24>::new();
    for words in [intensity.label(), intensity.short_label()] {
        name = Text::new();
        let _ = write!(name, "{unit}{words}");
        if label.fit_width(name.as_bytes()) <= room {
            break;
        }
    }
    out.fit(
        &[label, note],
        name.as_bytes(),
        room,
        x,
        base,
        Align::Left,
        theme.text,
    );
}
