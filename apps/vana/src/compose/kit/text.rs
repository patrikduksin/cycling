//! One line of words or a number, in sentence case where its face has
//! lowercase: a fixed string, a metric as `Metric::write` writes it, words
//! before a metric, or one of two lines by whether the ride paused itself.
use super::specs;
use crate::clock::Day;
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::done::{Kept, Picture};
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
    /// The words, then the metric: `for 4:12`.
    Before(&'static str, Metric),
    /// The first while the ride is auto-paused, else the second.
    Paused(&'static Words, &'static Words),
    /// Ride done's day, or "Indoor" beside its power bars.
    Day,
    /// Ride done's title: whether the ride is saved.
    Kept,
}

const INDOOR: &str = "Indoor";

fn number(metric: Metric, data: &dyn Data, out: &mut Written<24>) {
    let mut number = Written::<12>::new();
    metric.write(data.metric(metric), &mut number);
    let _ = out.push_str(number.as_str());
}

impl Words {
    fn write(&self, data: &dyn Data, out: &mut Written<24>) {
        match *self {
            Self::Fixed(words) => {
                let _ = out.push_str(words);
            }
            Self::Metric(metric) => number(metric, data, out),
            Self::Before(words, metric) => {
                let _ = out.push_str(words);
                number(metric, data, out);
            }
            Self::Paused(auto, manual) => {
                let words = if data.auto_paused() { auto } else { manual };
                words.write(data, out);
            }
            Self::Day => match data.done() {
                Some(done) if done.picture() == Picture::Power => {
                    let _ = out.push_str(INDOOR);
                }
                Some(done) => {
                    if let Some(day) = done.day() {
                        let _ = day.write(out);
                    }
                }
                None => {}
            },
            Self::Kept => {
                let kept = data.done().map(|done| done.kept()).unwrap_or_default();
                let _ = out.push_str(kept.title());
            }
        }
    }

    fn widest(&self, face: &Face) -> usize {
        let numbers = |metric: Metric| {
            metric
                .extremes()
                .iter()
                .map(|text| face.fit_width(text.as_bytes()))
                .max()
                .unwrap_or_default()
        };
        match *self {
            Self::Fixed(words) => face.fit_width(words.as_bytes()),
            Self::Metric(metric) => numbers(metric),
            Self::Before(words, metric) => face.width(words.as_bytes()) + numbers(metric),
            Self::Paused(auto, manual) => auto.widest(face).max(manual.widest(face)),
            Self::Day => {
                let widest = |names: &[&str]| {
                    names
                        .iter()
                        .map(|name| face.width(name.as_bytes()))
                        .max()
                        .unwrap_or(0)
                };
                let day = widest(&Day::WEEKDAYS) + face.width(b" 28 ") + widest(&Day::MONTHS);
                day.max(face.fit_width(INDOOR.as_bytes()))
            }
            Self::Kept => Kept::TITLES
                .iter()
                .map(|title| face.fit_width(title.as_bytes()))
                .max()
                .unwrap_or(0),
        }
    }

    const fn metric(&self) -> Option<Metric> {
        match *self {
            Self::Metric(metric) | Self::Before(_, metric) => Some(metric),
            Self::Paused(_, manual) => manual.metric(),
            Self::Fixed(_) | Self::Day | Self::Kept => None,
        }
    }
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
    pub name: &'static str,
    pub words: Words,
    pub style: Style,
    pub tone: Tone,
    pub align: Align,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A line of text. Ids: home and sheet 0–63, pause, end and done
    /// 64–127, sensors 128–191, phone 192–255.
    pub enum Say: Spec {
        Paused = 64 => Spec {
            name: "PAUSED",
            words: Words::Paused(&Words::Fixed("Auto paused"), &Words::Fixed("Paused")),
            style: Style::Heading,
            tone: Tone::Warn,
            align: Align::Center,
        },
        PausedFor = 65 => Spec {
            name: "PAUSED FOR",
            words: Words::Paused(
                &Words::Fixed("Ride on to resume"),
                &Words::Before("for ", Metric::Paused),
            ),
            style: Style::Note,
            tone: Tone::Sub,
            align: Align::Center,
        },
        EndRide = 66 => Spec {
            name: "END RIDE",
            words: Words::Fixed("End ride?"),
            style: Style::Heading,
            tone: Tone::Text,
            align: Align::Center,
        },
        WillSave = 67 => Spec {
            name: "WILL SAVE",
            words: Words::Fixed("It will be saved"),
            style: Style::Note,
            tone: Tone::Sub,
            align: Align::Center,
        },
        RideSaved = 68 => Spec {
            name: "RIDE SAVED",
            words: Words::Kept,
            style: Style::Label,
            tone: Tone::Text,
            align: Align::Left,
        },
        SavedOn = 69 => Spec {
            name: "SAVED ON",
            words: Words::Day,
            style: Style::Note,
            tone: Tone::Sub,
            align: Align::Right,
        },
        ScanWith = 192 => Spec {
            name: "SCAN WITH",
            words: Words::Fixed("Scan with the "),
            style: Style::Label,
            tone: Tone::Text,
            align: Align::Left,
        },
        IphoneCamera = 196 => Spec {
            name: "IPHONE CAMERA",
            words: Words::Fixed("iPhone Camera"),
            style: Style::Label,
            tone: Tone::Text,
            align: Align::Left,
        },
        PhonePaired = 193 => Spec {
            name: "PHONE PAIRED",
            words: Words::Fixed("iPhone paired"),
            style: Style::Heading,
            tone: Tone::Text,
            align: Align::Center,
        },
        ForgetPhone = 194 => Spec {
            name: "FORGET PHONE",
            words: Words::Fixed("Forget iPhone?"),
            style: Style::Heading,
            tone: Tone::Text,
            align: Align::Center,
        },
        StopsConnecting = 195 => Spec {
            name: "STOPS CONNECTING",
            words: Words::Fixed("It stops connecting"),
            style: Style::Note,
            tone: Tone::Sub,
            align: Align::Center,
        },
    }
}

impl Tile for Text {
    type Arg = Say;
    const NAME: &'static str = "text";

    fn reads(text: Say) -> Option<Reads> {
        text.spec().words.metric().map(Reads::Metric)
    }

    fn forms(text: Say, at: &Scaled) -> Forms {
        let spec = text.spec();
        let face = at.face(spec.style.role());
        forms(&[Form {
            width: spec.words.widest(face) as u16,
            height: face.fit_height() as u16,
            shapes: 0,
            runs: 1,
        }])
    }

    fn prepare(text: Say, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let spec = text.spec();
        let face = at.face(spec.style.role());
        let rect = placed.rect.rect();
        let mut words = Written::<24>::new();
        spec.words.write(data, &mut words);
        let advance = rect.width().saturating_sub(SPILL);
        let x = match spec.align {
            Align::Left => rect.left,
            Align::Center => rect.left + advance / 2,
            Align::Right => rect.left + advance,
        };
        let color = data.theme().tone(spec.tone);
        out.fit(
            &[face],
            words.as_bytes(),
            rect.width(),
            x,
            rect.bottom,
            spec.align,
            color,
        );
    }
}
