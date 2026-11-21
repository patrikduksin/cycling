//! One line of the workout page: the workout's name and the step count, the
//! step and its target, the reading the target checks, the verdict, or the
//! next step.
use crate::compose::data::{Data, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{AMBER, Align, GREEN, POWER, SECONDARY, WHITE};
use crate::screens::ride::Training;
use crate::screens::ride::workout::{
    metric_color, next_step, step_name, target_text, unit, whole_words,
};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use crate::workout::model::Metric;
use crate::workout::player::{Compliance, TestReadout};
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct WorkoutLine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// The workout's name and the step count.
    Title,
    /// The step and its target.
    Step,
    /// The reading the target checks, as the hero.
    Reading,
    /// UNDER, ON TARGET or OVER, and a test's running number.
    Verdict,
    /// The step after this one.
    Next,
}

impl Line {
    const ALL: [Self; 5] = [
        Self::Title,
        Self::Step,
        Self::Reading,
        Self::Verdict,
        Self::Next,
    ];
}

impl Arg for Line {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Title => "TITLE",
            Self::Step => "STEP",
            Self::Reading => "READING",
            Self::Verdict => "VERDICT",
            Self::Next => "NEXT",
        }
    }
}

/// The hero's looks, and under each the face a reading past its end takes.
const HERO_FACES: [FaceRole; 3] = [FaceRole::Hero, FaceRole::Large, FaceRole::Value];
/// Columns between the hero and its unit, and between words on a line.
const UNIT_GAP_UNITS: usize = 6;
const WORD_GAP_UNITS: usize = 8;

/// What sits beside the hero: its unit, and power's smoothing caption.
fn beside_hero(at: &Scaled) -> usize {
    let caption = at.face(FaceRole::Caption);
    let power = caption.fit_width(b"W") + at.unit(WORD_GAP_UNITS) + caption.fit_width(b"3 S");
    at.unit(UNIT_GAP_UNITS) + power.max(caption.fit_width(b"BPM"))
}

/// The hero's widest reading in its own face. Past it, a sprint's power
/// steps down exactly one face, which holds any reading whole.
const WIDEST_READING: &[u8] = b"999";
const WIDEST_STEPPED: &[u8] = b"65535";
/// What the step line says without a workout.
const NO_WORKOUT: &[u8] = b"NO WORKOUT";

fn text(face: &Face, widest: &[u8], runs: u8) -> Form {
    Form {
        runs,
        ..Form::text(face, widest)
    }
}

impl Tile for WorkoutLine {
    type Arg = Line;
    const NAME: &'static str = "workout line";

    fn forms(line: Line, at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let body = at.face(FaceRole::Body);
        match line {
            Line::Title => forms(&[text(caption, b"STEP 99/99", 2)]),
            Line::Step => {
                let widest = [b"FREE RIDE".as_slice(), NO_WORKOUT]
                    .into_iter()
                    .max_by_key(|text| body.fit_width(text))
                    .unwrap_or_default();
                forms(&[text(body, widest, 2)])
            }
            Line::Reading => HERO_FACES
                .windows(2)
                .map(|faces| {
                    let (face, stepped) = (at.face(faces[0]), at.face(faces[1]));
                    let widest = face
                        .fit_width(WIDEST_READING)
                        .max(stepped.fit_width(WIDEST_STEPPED));
                    Form {
                        width: (widest + beside_hero(at)) as u16,
                        ..text(face, WIDEST_READING, 3)
                    }
                })
                .collect(),
            Line::Verdict => forms(&[text(caption, b"ON TARGET", 2)]),
            Line::Next => forms(&[text(body, b"NEXT FINISH", 2)]),
        }
    }

    fn reads(line: Line) -> Option<Reads> {
        (line == Line::Reading).then_some(Reads::Target)
    }

    fn prepare(line: Line, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let Some(training) = data.training() else {
            if line == Line::Step {
                out.fit(
                    &[at.face(FaceRole::Body)],
                    NO_WORKOUT,
                    rect.width(),
                    rect.left,
                    rect.bottom,
                    Align::Left,
                    SECONDARY,
                );
            }
            return;
        };
        let edges = Edges {
            left: rect.left,
            right: rect.right,
            base: rect.bottom,
        };
        match line {
            Line::Title => title(training, edges, at, out),
            Line::Step => step(training, edges, at, out),
            Line::Reading => {
                let form = usize::from(placed.form);
                let faces = [HERO_FACES[form], HERO_FACES[form + 1]].map(|role| at.face(role));
                reading(training, faces, edges, at, out);
            }
            Line::Verdict => verdict(training, edges, at, out),
            Line::Next => next(training, edges, at, out),
        }
    }
}

/// A line's left and right edges and its baseline.
#[derive(Clone, Copy)]
struct Edges {
    left: usize,
    right: usize,
    base: usize,
}

impl Edges {
    fn width(self) -> usize {
        self.right - self.left
    }
}

fn title(training: &Training, line: Edges, at: &Scaled, out: &mut Canvas) {
    let caption = at.face(FaceRole::Caption);
    let now = &training.now;
    let width = line.width();
    let mut widest = Text::<24>::new();
    let _ = write!(widest, "STEP {}/{}", now.steps, now.steps);
    let spelled_out_fits = caption.width(training.name.as_bytes())
        + caption.width(widest.as_bytes())
        + at.unit(WORD_GAP_UNITS)
        + 2 * SPILL
        <= width;
    let count = |step| {
        let mut count = Text::<24>::new();
        let _ = if training.done {
            count.push_str("DONE")
        } else if spelled_out_fits {
            write!(count, "STEP {}/{}", step, now.steps)
        } else {
            write!(count, "{}/{}", step, now.steps)
        };
        count
    };
    out.caption(
        at,
        count(now.position + 1).as_bytes(),
        width / 2,
        line.right - SPILL,
        line.base,
        Align::Right,
    );
    let widest_count_width = caption.width(count(now.steps).as_bytes());
    let room = width.saturating_sub(SPILL + widest_count_width + at.unit(WORD_GAP_UNITS));
    let full_name = training.name.as_bytes();
    let name =
        whole_words(full_name, |text| caption.width(text) + SPILL <= room).unwrap_or(full_name);
    out.caption(at, name, room, line.left, line.base, Align::Left);
}

fn step(training: &Training, line: Edges, at: &Scaled, out: &mut Canvas) {
    let (caption, body) = (at.face(FaceRole::Caption), at.face(FaceRole::Body));
    let now = &training.now;
    let width = line.width();
    let metric = now.band.map_or(Metric::Power, |band| band.metric);
    let mut target = Text::<24>::new();
    let mut name = Text::<24>::new();
    if training.done {
        let _ = name.push_str("WORKOUT DONE");
    } else {
        step_name(&mut name, now.played, false);
        target_text(&mut target, now.played, now, training.test);
    }
    let target_color = if now.band.is_some() {
        metric_color(metric)
    } else {
        SECONDARY
    };
    let name_room = caption.width(name.as_bytes()) + at.unit(WORD_GAP_UNITS) + 2 * SPILL;
    // The target is never cut: it keeps at least its smallest size.
    let target_floor = caption.width(target.as_bytes()) + SPILL;
    let name_right = if training.done {
        width
    } else {
        let target = out.fit(
            &[body, caption],
            target.as_bytes(),
            (width * 3 / 5)
                .min(width.saturating_sub(name_room))
                .max(target_floor),
            line.right - SPILL,
            line.base,
            Align::Right,
            target_color,
        );
        target
            .left
            .saturating_sub(line.left + at.unit(WORD_GAP_UNITS))
    };
    let name_color = if training.done { GREEN } else { WHITE };
    if !training.done && caption.width(name.as_bytes()) + SPILL > name_right {
        name = Text::new();
        step_name(&mut name, now.played, true);
    }
    out.fit(
        &[body, caption],
        name.as_bytes(),
        name_right,
        line.left,
        line.base,
        Align::Left,
        name_color,
    );
}

/// The reading the band checks, as the hero, its unit beside it.
fn reading(
    training: &Training,
    faces: [&'static Face; 2],
    line: Edges,
    at: &Scaled,
    out: &mut Canvas,
) {
    let caption = at.face(FaceRole::Caption);
    let now = &training.now;
    let metric = now.band.map_or(Metric::Power, |band| band.metric);
    let mut reading = Text::<12>::new();
    let _ = match now.reading {
        Some(value) => write!(reading, "{value}"),
        None => reading.push_str("--"),
    };
    let unit_width = caption.width(unit(metric)) + SPILL;
    let hero = out.fit(
        &faces,
        reading.as_bytes(),
        line.width().saturating_sub(beside_hero(at)),
        line.left,
        line.base,
        Align::Left,
        metric_color(metric),
    );
    out.fit(
        &[caption],
        unit(metric),
        unit_width,
        hero.right() + at.unit(UNIT_GAP_UNITS),
        line.base,
        Align::Left,
        SECONDARY,
    );
    if metric == Metric::Power && now.reading.is_some() {
        out.caption(
            at,
            b"3 S",
            at.unit(40),
            line.right - SPILL,
            line.base,
            Align::Right,
        );
    }
}

fn verdict(training: &Training, line: Edges, at: &Scaled, out: &mut Canvas) {
    let caption = at.face(FaceRole::Caption);
    let now = &training.now;
    let mut free_from = line.left;
    if let Some(compliance) = now.compliance {
        let (text, color): (&[u8], u16) = match compliance {
            Compliance::Under => (b"UNDER", AMBER),
            Compliance::In => (b"ON TARGET", GREEN),
            Compliance::Over => (b"OVER", AMBER),
        };
        let run = out.fit(
            &[caption],
            text,
            line.width() / 2,
            line.left,
            line.base,
            Align::Left,
            color,
        );
        free_from = run.right() + at.unit(10);
    }
    if let Some(test) = now.test {
        let mut text = Text::<24>::new();
        let _ = match test {
            TestReadout::Average(w) => write!(text, "AVG {w} W"),
            TestReadout::BestMinute(w) => write!(text, "BEST {w} W"),
        };
        out.fit(
            &[caption],
            text.as_bytes(),
            (line.right - SPILL).saturating_sub(free_from),
            line.right - SPILL,
            line.base,
            Align::Right,
            POWER,
        );
    }
}

fn next(training: &Training, line: Edges, at: &Scaled, out: &mut Canvas) {
    if training.done {
        return;
    }
    let (caption, body) = (at.face(FaceRole::Caption), at.face(FaceRole::Body));
    let label = out.caption(at, b"NEXT", at.unit(50), line.left, line.base, Align::Left);
    let room = line.right.saturating_sub(label.right() + at.unit(14));
    let next = next_step(training.now.next, |text| {
        caption.width(text) + SPILL <= room
    });
    out.fit(
        &[body, caption],
        next.as_bytes(),
        room,
        label.right() + at.unit(WORD_GAP_UNITS),
        line.base,
        Align::Left,
        WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::data::{Effort, ListKind, Metric as DataMetric};
    use crate::compose::solve::Rect16;
    use crate::ride::zones::TrainingProfile;
    use crate::workout::library::VO2_MAX;
    use crate::workout::player::Player;

    struct Shows(Training);

    impl Data for Shows {
        fn metric(&self, _: DataMetric) -> Option<i32> {
            None
        }
        fn zone(&self, _: Effort) -> Option<u8> {
            None
        }
        fn list(&self, _: ListKind) -> u16 {
            0
        }
        fn training(&self) -> Option<&Training> {
            Some(&self.0)
        }
    }

    /// The hero drawn in look `form` at that look's own width, showing
    /// `watts`: the face it took and the text it drew.
    fn hero(at: &Scaled, form: u8, watts: u16) -> (&'static Face, Text<12>) {
        let look = WorkoutLine::forms(Line::Reading, at)[usize::from(form)];
        let mut training = Training::of(&Player::new(VO2_MAX), &TrainingProfile::default());
        training.now.band = None;
        training.now.reading = Some(watts);
        let placed = Placed {
            rect: Rect16 {
                left: 0,
                top: 0,
                right: look.width,
                bottom: look.height,
            },
            node: 0,
            form,
        };
        let mut out = Canvas::default();
        WorkoutLine::prepare(Line::Reading, &placed, at, &Shows(training), &mut out);
        let run = &out.runs[0];
        let mut text = Text::new();
        let _ = text.push_str(core::str::from_utf8(run.line.text()).unwrap());
        (run.line.face(), text)
    }

    /// Fable's rule: a reading is measured on its usual end, 999, and past
    /// it drops exactly one face, whole, never clamped or cut.
    #[test]
    fn power_past_999_drops_exactly_one_face_whole() {
        for &(width, height) in crate::screens::limits::PANELS {
            let at = Scaled::fitting(width, height).unwrap();
            for form in 0..2u8 {
                let [own, under] = [
                    HERO_FACES[usize::from(form)],
                    HERO_FACES[usize::from(form) + 1],
                ]
                .map(|role| at.face(role));
                for watts in [0, 7, 999] {
                    let (face, text) = hero(&at, form, watts);
                    assert!(core::ptr::eq(face, own), "{watts} W in look {form}");
                    assert_eq!(text.as_str(), std::format!("{watts}"));
                }
                for watts in [1000, 1999, 9999, u16::MAX] {
                    let (face, text) = hero(&at, form, watts);
                    assert!(core::ptr::eq(face, under), "{watts} W in look {form}");
                    assert_eq!(text.as_str(), std::format!("{watts}"));
                }
            }
        }
    }
}
