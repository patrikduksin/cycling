//! The climb page's words about the road: its title with the climb's
//! category, and the two readings the road calls for, a caption over each
//! value. `screens::ride::climb` says what the road is.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{ACCENT, Align, GRADE, SECONDARY, WHITE};
use crate::screens::ride::climb::{Road as Shown, Situation, category, metres, tenths_km};
use crate::screens::ride::{grade, kilometres, whole};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole};

pub struct Road;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// CLIMB IN, CLIMBING or NO CLIMB, and the category or NO MAP.
    Title,
    /// The road's two readings.
    Facts,
}

impl Arg for Part {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Title),
            1 => Some(Self::Facts),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Title => "TITLE",
            Self::Facts => "FACTS",
        }
    }
}

/// Columns between the title and the category.
const TITLE_GAP_UNITS: usize = 8;
/// Columns between the readings' two columns.
const COLUMN_GAP_UNITS: usize = 4;
/// Rows from a reading's caption to its value, baseline to baseline.
const VALUE_UNDER_UNITS: usize = 29;
/// Rows from the right column's top caption to the average under it, when
/// two readings stack there.
const STACKED_UNDER_UNITS: usize = 4;
const VALUE_FACES: [FaceRole; 2] = [FaceRole::Value, FaceRole::Body];
/// Each column's captions.
const CAPTIONS: [&[&[u8]]; 2] = [
    &[b"LENGTH KM", b"TO TOP KM", b"SO FAR KM", b"ALTITUDE M"],
    &[b"UP TO GO M", b"GAIN M", b"ASCENT M"],
];
/// The widest a road's reading reads in its largest look: four digits of
/// metres. A fifth, past 9999 m of ascent, steps down a face.
const WIDEST_VALUE: &[u8] = b"9999";

/// The two columns' left and right edges in `left..right`: the halves, as
/// power and heart rate above them split their row.
fn columns(left: usize, right: usize, at: &Scaled) -> [(usize, usize); 2] {
    let middle = (left + right) / 2;
    [(left, middle - at.unit(COLUMN_GAP_UNITS)), (middle, right)]
}

fn widest(face: &Face, texts: &[&[u8]]) -> usize {
    texts
        .iter()
        .map(|text| face.fit_width(text))
        .max()
        .unwrap_or(0)
}

impl Tile for Road {
    type Arg = Part;
    const NAME: &'static str = "road";

    fn forms(part: Part, at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        match part {
            Part::Title => forms(&[Form {
                width: [
                    (&b"CLIMB IN 9.9 KM"[..], &b"CAT 4"[..]),
                    (b"CLIMBING", b"NO MAP"),
                ]
                .map(|(title, right)| {
                    caption.fit_width(title) + at.unit(TITLE_GAP_UNITS) + caption.fit_width(right)
                })
                .into_iter()
                .fold(0, usize::max) as u16,
                height: caption.fit_height() as u16,
                shapes: 0,
                runs: 2,
            }]),
            Part::Facts => VALUE_FACES
                .iter()
                .map(|&role| {
                    let value = at.face(role).fit_width(WIDEST_VALUE);
                    let [left, right] =
                        CAPTIONS.map(|captions| widest(caption, captions).max(value));
                    let half = (left + at.unit(COLUMN_GAP_UNITS)).max(right);
                    Form {
                        width: (2 * half) as u16,
                        height: (caption.fit_height() + at.unit(VALUE_UNDER_UNITS)) as u16,
                        shapes: 0,
                        runs: 4,
                    }
                })
                .collect(),
        }
    }

    fn prepare(part: Part, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(road) = data.road() else {
            return;
        };
        match part {
            Part::Title => title(&road, placed, at, out),
            Part::Facts => facts(&road, placed, at, out),
        }
    }
}

/// The title on the left and the category, or NO MAP, on the right.
fn title(road: &Shown, placed: &Placed, at: &Scaled, out: &mut Canvas) {
    let rect = placed.rect.rect();
    let mut title = Text::<24>::new();
    let (category, color) = match road.situation {
        Situation::Ahead(pin) => {
            let mut km = Text::<12>::new();
            tenths_km(&mut km, pin.foot_mm.saturating_sub(road.odometer_mm));
            let _ = write!(title, "CLIMB IN {} KM", km.as_str());
            category(pin.summary().gain_dm, pin.summary().grade_tenths())
        }
        Situation::OnMap(pin) => {
            let _ = title.push_str("CLIMBING");
            category(pin.summary().gain_dm, pin.summary().grade_tenths())
        }
        Situation::OnClimb { length_mm, gain_dm } => {
            let _ = title.push_str("CLIMBING");
            let grade = (length_mm > 0).then(|| (gain_dm as u64 * 100_000 / length_mm) as i32);
            category(gain_dm, grade)
        }
        Situation::Clear => {
            let _ = title.push_str("NO CLIMB");
            (None, SECONDARY)
        }
    };
    let title_color = match road.situation {
        Situation::Clear => SECONDARY,
        _ => ACCENT,
    };
    let caption = [at.face(FaceRole::Caption)];
    let title = out.fit(
        &caption,
        title.as_bytes(),
        rect.width(),
        rect.left,
        rect.bottom,
        Align::Left,
        title_color,
    );
    let right: Option<(&[u8], u16)> = match category {
        Some(name) => Some((name, color)),
        None if !road.terrain => Some((b"NO MAP", SECONDARY)),
        None => None,
    };
    if let Some((text, color)) = right {
        out.fit(
            &caption,
            text,
            rect.right
                .saturating_sub(title.right() + at.unit(TITLE_GAP_UNITS)),
            rect.right,
            rect.bottom,
            Align::Right,
            color,
        );
    }
}

/// The road's two readings, each a caption over its value.
fn facts(road: &Shown, placed: &Placed, at: &Scaled, out: &mut Canvas) {
    let rect = placed.rect.rect();
    let caption_base = rect.top + at.face(FaceRole::Caption).fit_height();
    let value = at.face(VALUE_FACES[usize::from(placed.form)]);
    let columns = columns(rect.left, rect.right, at);
    let cell = |out: &mut Canvas, column: usize, caption: &[u8], text: &Text<12>, color: u16| {
        let (left, right) = columns[column];
        out.caption(at, caption, right - left, left, caption_base, Align::Left);
        out.fit(
            &[value, at.face(FaceRole::Body)],
            text.as_bytes(),
            right - left,
            left,
            rect.bottom,
            Align::Left,
            color,
        );
    };
    let mut values: [Text<12>; 2] = core::array::from_fn(|_| Text::new());
    let labels: [(&[u8], u16); 2] = match road.situation {
        Situation::Ahead(pin) => {
            let climb = pin.summary();
            kilometres(&mut values[0], climb.length_mm);
            cell(out, 0, b"LENGTH KM", &values[0], WHITE);
            let average_base = caption_base + at.unit(STACKED_UNDER_UNITS);
            ahead(&climb, columns[1], (average_base, rect.bottom), at, out);
            return;
        }
        Situation::OnMap(pin) => {
            let odometer = road.odometer_mm;
            tenths_km(&mut values[0], pin.top_mm.saturating_sub(odometer));
            let left = pin.altitude_dm(pin.top_mm) - pin.altitude_dm(odometer);
            metres(&mut values[1], left.max(0));
            [(b"TO TOP KM", WHITE), (b"UP TO GO M", GRADE)]
        }
        Situation::OnClimb { length_mm, gain_dm } => {
            kilometres(&mut values[0], length_mm);
            metres(&mut values[1], gain_dm as i32);
            [(b"SO FAR KM", WHITE), (b"GAIN M", GRADE)]
        }
        Situation::Clear => {
            match road.altitude_dm {
                Some(dm) => metres(&mut values[0], dm),
                None => whole(&mut values[0], None::<u8>),
            }
            metres(&mut values[1], road.ascent_dm as i32);
            [(b"ALTITUDE M", GRADE), (b"ASCENT M", GRADE)]
        }
    };
    for (column, ((caption, color), value)) in labels.iter().zip(&values).enumerate() {
        cell(out, column, caption, value, *color);
    }
}

/// Ahead of a climb, its average grade and gain stack in the right column,
/// on the two baselines `bases`.
fn ahead(
    climb: &crate::ride::climb::Summary,
    column: (usize, usize),
    bases: (usize, usize),
    at: &Scaled,
    out: &mut Canvas,
) {
    let mut average = Text::<12>::new();
    grade(&mut average, climb.grade_tenths());
    let mut gain = Text::<12>::new();
    metres(&mut gain, climb.gain_dm as i32);
    stacked(out, column, b"AVG", &average, bases.0, at);
    stacked(out, column, b"GAIN M", &gain, bases.1, at);
}

/// A caption and its value on one line of the right column: two of them
/// stack where one reading would be.
fn stacked(
    out: &mut Canvas,
    (left, right): (usize, usize),
    caption: &[u8],
    value: &Text<12>,
    base: usize,
    at: &Scaled,
) {
    let run = out.fit(
        &[at.face(FaceRole::Body)],
        value.as_bytes(),
        right - left,
        right,
        base,
        Align::Right,
        GRADE,
    );
    out.caption(
        at,
        caption,
        run.left.saturating_sub(left + at.unit(4)),
        left,
        base,
        Align::Left,
    );
}
