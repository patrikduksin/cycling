//! Ready's lines: what the ride will be, then each check as a status dot,
//! a short headline and, when something needs the rider, what to do about
//! it. The workout line opens Workouts and the sensors line opens the
//! sensor menu; either shows its focus as an accent ring round the line.
use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, Go};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, SECONDARY, WHITE};
use crate::screens::ready::{LINES, Line};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Page;
use crate::workout::model::WIDEST_NAME;
use firmware_shell::rendering::font::FaceRole;

/// Every line keeps this far inside its box, where a focused line's ring
/// and raised fill go.
const ACROSS_UNITS: usize = 6;
const DOWN_UNITS: usize = 3;
const RADIUS_UNITS: usize = 10;
const RING_UNITS: usize = 2;
/// The dot's columns, and the text's start after the dot's.
const DOT_UNITS: usize = 10;
const TEXT_UNITS: usize = 16;
/// The hint's baseline under the title's.
const HINT_UNITS: usize = 20;

/// The longest texts a check shows: its titles, then its hints. Storage,
/// the last line, sits beside the start cue and says less.
const fn longest(line: Line) -> (&'static [&'static str], &'static [&'static str]) {
    match line {
        Line::Storage => (
            &[
                "STORAGE NOT FORMATTED",
                "UNDER 1 MIN OF ROOM",
                "CLEAR RIDES FIRST",
            ],
            &[
                "LAST RIDE RECOVERED",
                "OFF AND ON TO FORMAT",
                "TURN VANA OFF AND ON",
            ],
        ),
        Line::Gps | Line::Sensors | Line::Battery => (
            &["SENSORS UNAVAILABLE", "SENSORS WAKING UP"],
            &[
                "RECORDS WITHOUT A MAP",
                "CHARGE AFTER THE RIDE",
                "CHARGE BEFORE RIDING",
                "HR  PWR  SPD/C  RDR",
            ],
        ),
    }
}
/// What the line names: the widest workout name, or a ride with none.
const RIDING: [&str; 2] = [WIDEST_NAME, FREE_RIDE];
const FREE_RIDE: &str = "FREE RIDE";

fn widest(at: &Scaled, role: FaceRole, texts: &[&str]) -> u16 {
    let face = at.face(role);
    texts
        .iter()
        .map(|text| Form::text(face, text.as_bytes()).width)
        .max()
        .unwrap_or_default()
}

/// A box `width` and `height` wide and tall inside, with room for the ring.
fn form(at: &Scaled, width: u16, height: usize, shapes: u8, runs: u8) -> Form {
    Form {
        width: width + 2 * at.unit(ACROSS_UNITS) as u16,
        height: (height + 2 * at.unit(DOWN_UNITS)) as u16,
        shapes,
        runs,
    }
}

pub struct Check;

impl Arg for Line {
    fn decode(raw: u8) -> Option<Self> {
        LINES.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }
}

fn check(
    line: Line,
    placed: &Placed,
    at: &Scaled,
    data: &dyn Data,
    out: &mut Canvas,
    focused: bool,
) {
    let PageData::Ready(readiness) = data.page() else {
        return;
    };
    let check = &readiness.checks[line as usize];
    let rect = placed.rect.rect();
    let corners = (at.unit(RADIUS_UNITS), at.unit(RING_UNITS));
    super::choice::card(out, rect, corners, focused, None);
    let [body, caption] = [FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let left = rect.left + at.unit(ACROSS_UNITS);
    let right = rect.right - at.unit(ACROSS_UNITS);
    let base = rect.top + at.unit(DOWN_UNITS) + body.fit_height();
    let dot = at.unit(DOT_UNITS);
    out.shape(Shape::Disc {
        cx: (4 * left + 2 * dot) as i32,
        cy: (4 * base - 2 * body.cap as usize) as i32,
        radius: 2 * dot as i32,
        color: check.level.color(),
    });
    let text = left + at.unit(TEXT_UNITS);
    let title = check.title.as_bytes();
    out.fit(
        &[body, caption],
        title,
        right - text,
        text,
        base,
        Align::Left,
        WHITE,
    );
    let hint = check.hint.as_bytes();
    if !hint.is_empty() {
        let base = base + at.unit(HINT_UNITS);
        out.fit(
            &[caption],
            hint,
            right - text,
            text,
            base,
            Align::Left,
            SECONDARY,
        );
    }
}

impl Tile for Check {
    type Arg = Line;
    const NAME: &'static str = "check";

    /// The title in the body face, then in the caption face; the hint's
    /// rows under it either way.
    fn forms(line: Line, at: &Scaled) -> Forms {
        let body = at.face(FaceRole::Body);
        let text = at.unit(TEXT_UNITS) as u16;
        let (titles, hints) = longest(line);
        let hint = widest(at, FaceRole::Caption, hints);
        let height = body.fit_height() + at.unit(HINT_UNITS);
        [FaceRole::Body, FaceRole::Caption]
            .into_iter()
            .map(|role| {
                let width = text + widest(at, role, titles).max(hint);
                form(at, width, height, 3, 2)
            })
            .collect()
    }

    fn prepare(line: Line, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        check(line, placed, at, data, out, false);
    }

    fn focused(line: Line, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        check(line, placed, at, data, out, true);
    }

    fn action(line: Line) -> Option<Action> {
        (line == Line::Sensors).then_some(Action::Go(Go::Open(Page::Sensors)))
    }
}

/// What the ride will be: FREE RIDE, or the chosen workout's name.
pub struct Riding;

fn riding(placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas, focused: bool) {
    let PageData::Ready(readiness) = data.page() else {
        return;
    };
    let rect = placed.rect.rect();
    let corners = (at.unit(RADIUS_UNITS), at.unit(RING_UNITS));
    super::choice::card(out, rect, corners, focused, None);
    let name = match readiness.riding.as_bytes() {
        b"" => FREE_RIDE.as_bytes(),
        name => name,
    };
    let (left, right) = (
        rect.left + at.unit(ACROSS_UNITS),
        rect.right - at.unit(ACROSS_UNITS),
    );
    let base = rect.bottom - at.unit(DOWN_UNITS);
    let caption = at.face(FaceRole::Caption);
    out.fit(
        &[caption],
        name,
        right - left,
        left,
        base,
        Align::Left,
        ACCENT,
    );
}

impl Tile for Riding {
    type Arg = ();
    const NAME: &'static str = "riding";

    fn forms((): (), at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let width = widest(at, FaceRole::Caption, &RIDING);
        [form(at, width, caption.fit_height(), 2, 1)]
            .into_iter()
            .collect()
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        riding(placed, at, data, out, false);
    }

    fn focused((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        riding(placed, at, data, out, true);
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Go(Go::Open(Page::Workouts)))
    }
}
