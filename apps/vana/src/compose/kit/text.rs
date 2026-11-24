//! One line of words or a number, in sentence case where its face has
//! lowercase: a fixed string or a metric as `Metric::write` writes it.
use super::specs;
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Tone};
use crate::screens::workout::Text as Written;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct Text;

/// What a text says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    Fixed(&'static str),
    Metric(Metric),
}

/// Which of the layout's faces a text takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Heading,
    Label,
    Note,
    Tag,
}

impl Style {
    pub const fn role(self) -> FaceRole {
        let face = &LAYOUT.face;
        match self {
            Self::Heading => face.heading,
            Self::Label => face.label,
            Self::Note => face.note,
            Self::Tag => face.tag,
        }
    }
}

#[derive(Debug)]
pub struct Spec {
    pub words: Words,
    pub style: Style,
    pub tone: Tone,
    pub align: Align,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        match self.words {
            Words::Fixed(words) => words,
            Words::Metric(metric) => metric.name(),
        }
    }
}

specs! {
    /// A line of text. Ids: home and sheet 0–63, pause, end and done
    /// 64–127, sensors 128–191.
    pub enum Say: Spec {
        RideSaved = 64 => Spec {
            words: Words::Fixed("Ride saved"),
            style: Style::Heading,
            tone: Tone::Text,
            align: Align::Center,
        },
        RideTime = 65 => Spec {
            words: Words::Metric(Metric::Elapsed),
            style: Style::Label,
            tone: Tone::Sub,
            align: Align::Center,
        },
    }
}

/// The widest text the spec may show in `face`.
fn widest(spec: &Spec, face: &Face) -> usize {
    match spec.words {
        Words::Fixed(words) => face.fit_width(words.as_bytes()),
        Words::Metric(metric) => metric
            .extremes()
            .iter()
            .map(|text| face.fit_width(text.as_bytes()))
            .max()
            .unwrap_or_default(),
    }
}

impl Tile for Text {
    type Arg = Say;
    const NAME: &'static str = "text";

    fn reads(text: Say) -> Option<Reads> {
        match text.spec().words {
            Words::Metric(metric) => Some(Reads::Metric(metric)),
            Words::Fixed(_) => None,
        }
    }

    fn forms(text: Say, at: &Scaled) -> Forms {
        let spec = text.spec();
        let face = at.face(spec.style.role());
        forms(&[Form {
            width: widest(spec, face) as u16,
            height: face.fit_height() as u16,
            shapes: 0,
            runs: 1,
        }])
    }

    fn prepare(text: Say, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let spec = text.spec();
        let face = at.face(spec.style.role());
        let rect = placed.rect.rect();
        let mut number = Written::<12>::new();
        let words = match spec.words {
            Words::Fixed(words) => words.as_bytes(),
            Words::Metric(metric) => {
                metric.write(data.metric(metric), &mut number);
                number.as_bytes()
            }
        };
        // The box holds the words' advance and their spill past it.
        let advance = rect.width().saturating_sub(SPILL);
        let x = match spec.align {
            Align::Left => rect.left,
            Align::Center => rect.left + advance / 2,
            Align::Right => rect.left + advance,
        };
        let color = data.theme().tone(spec.tone);
        out.fit(
            &[face],
            words,
            rect.width(),
            x,
            rect.bottom,
            spec.align,
            color,
        );
    }
}
