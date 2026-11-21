//! FORM's tiles: a band of fitness, fatigue or form, and the trend of the
//! last weeks. Only the two ends of the range draw the eye: green for FRESH,
//! red for VERY TIRED.
use crate::compose::data::{Count, Data};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::ride::fitness::{DAY_MAX, TREND};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, POWER, Rect, TERTIARY};
use crate::screens::form::{signed, verdict};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

/// Rows from the caption's baseline down to the band's bottom, and from the
/// value's.
const CAPTION_UNITS: usize = 14;
const VALUE_UNITS: usize = 3;
/// The value's column on the right, and the word's start after the caption.
const VALUE_COLUMN_UNITS: usize = 100;
const WORD_UNITS: usize = 70;
const GAP_UNITS: usize = 4;
/// No day's stress passes `DAY_MAX`, so neither do fitness and fatigue;
/// as both average the same days, they never stand a whole `DAY_MAX`
/// apart, so form stays within three digits either way.
const END: i32 = DAY_MAX as i32;
const FORM_END: i32 = 999;
/// A form in each of `verdict`'s words.
const WORDS: [i16; 5] = [26, 5, 0, -11, -31];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    Fitness,
    Fatigue,
    Form,
}

impl Reads {
    const fn caption(self) -> &'static [u8] {
        match self {
            Self::Fitness => b"FITNESS",
            Self::Fatigue => b"FATIGUE",
            Self::Form => b"FORM",
        }
    }

    const fn count(self) -> Count {
        match self {
            Self::Fitness => Count::Fitness,
            Self::Fatigue => Count::Fatigue,
            Self::Form => Count::Form,
        }
    }

    const fn end(self) -> i32 {
        match self {
            Self::Form => FORM_END,
            _ => END,
        }
    }

    /// The value as it reads, up to its end either way.
    fn value(self, value: i32, out: &mut Text<12>) {
        let value = value.clamp(-self.end(), self.end());
        match self {
            Self::Form => signed(out, value as i16),
            _ => {
                let _ = write!(out, "{value}");
            }
        }
    }

    /// The widest value it can show in `face`.
    fn widest(self, face: &Face) -> usize {
        [-self.end(), self.end()]
            .iter()
            .map(|&end| {
                let mut text = Text::<12>::new();
                self.value(end, &mut text);
                face.fit_width(text.as_bytes())
            })
            .max()
            .unwrap_or(0)
    }

    /// Where form's word starts: at its column, or past a caption that
    /// reaches into it.
    fn word_at(self, at: &Scaled) -> usize {
        let caption = at.face(FaceRole::Caption);
        at.unit(WORD_UNITS)
            .max(caption.ink_width(self.caption()) + at.unit(GAP_UNITS))
    }
}

/// The widest word form can mean in `face`.
fn widest_word(face: &Face) -> usize {
    WORDS
        .iter()
        .map(|&form| face.ink_width(verdict(form).0.as_bytes()))
        .max()
        .unwrap_or(0)
}

impl Arg for Reads {
    fn decode(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Fitness,
            1 => Self::Fatigue,
            2 => Self::Form,
            _ => return None,
        })
    }

    fn encode(self) -> u8 {
        self as u8
    }
}

/// A caption over its value on the right; form adds what it means today.
pub struct Band;

impl Tile for Band {
    type Arg = Reads;
    const NAME: &'static str = "form band";

    /// The caption, then form's word, on the caption's line; the value on
    /// the right reaches up beside both.
    fn forms(reads: Reads, at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let label = Form::text(caption, reads.caption());
        let before_value = match reads {
            Reads::Form => reads.word_at(at) + widest_word(caption),
            _ => caption.ink_width(reads.caption()),
        };
        let value = reads.widest(at.face(FaceRole::Body));
        forms(&[Form {
            width: (before_value + at.unit(GAP_UNITS) + value) as u16,
            height: label.height + at.unit(CAPTION_UNITS) as u16,
            shapes: 0,
            runs: 3,
        }])
    }

    fn prepare(reads: Reads, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let width = right.saturating_sub(left);
        let base = usize::from(rect.bottom);
        let caption_base = base.saturating_sub(at.unit(CAPTION_UNITS));
        let caption = [at.face(FaceRole::Caption)];
        out.caption(at, reads.caption(), width, left, caption_base, Align::Left);
        let Some(value) = data.count(reads.count()) else {
            return;
        };
        let body = at.face(FaceRole::Body);
        let column = at
            .unit(VALUE_COLUMN_UNITS)
            .max(reads.widest(body))
            .min(width);
        let mut text = Text::<12>::new();
        reads.value(value, &mut text);
        let color = match reads {
            Reads::Form => {
                let (word, tone) = verdict(value.clamp(-reads.end(), reads.end()) as i16);
                let from = left + reads.word_at(at);
                let room = right.saturating_sub(from);
                let word = word.as_bytes();
                out.fit(&caption, word, room, from, caption_base, Align::Left, tone);
                tone
            }
            _ => POWER,
        };
        out.fit(
            &[body],
            text.as_bytes(),
            column,
            right.saturating_sub(SPILL),
            base.saturating_sub(at.unit(VALUE_UNITS)),
            Align::Right,
            color,
        );
    }
}

/// How form stood at the end of each of the last weeks: one bar a week, up
/// for rested and down for tired, on a baseline at zero. A form of `SCALE`
/// reaches across either half.
pub struct Trend;

const SCALE: i32 = 50;
const WEEK_UNITS: usize = 24;
const BETWEEN_UNITS: usize = 11;
/// The baseline's rows and the clear row under it.
const ZERO_UNITS: usize = 1;
const UNDER_ZERO_UNITS: usize = 1;
const RADIUS_UNITS: usize = 2;

impl Tile for Trend {
    type Arg = ();
    const NAME: &'static str = "form trend";

    fn forms((): (), at: &Scaled) -> Forms {
        let weeks = TREND * WEEK_UNITS + (TREND - 1) * BETWEEN_UNITS;
        forms(&[Form {
            width: at.unit(weeks) as u16,
            height: at.unit(2 * SCALE as usize / 5) as u16,
            shapes: 1 + TREND as u8,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let split = at.unit(ZERO_UNITS) + at.unit(UNDER_ZERO_UNITS);
        let half = rect.height().saturating_sub(split) / 2;
        let zero = Rect {
            top: rect.top + half,
            bottom: rect.top + half + at.unit(ZERO_UNITS),
            ..rect
        };
        out.shape(Shape::Bar {
            rect: zero,
            radius: 0,
            color: TERTIARY,
        });
        let (week, between) = (at.unit(WEEK_UNITS), at.unit(BETWEEN_UNITS));
        let weeks = TREND * week + (TREND - 1) * between;
        let first = rect.left + rect.width().saturating_sub(weeks) / 2;
        let tired = zero.top + split;
        for at_week in 0..TREND {
            let value = data.count(Count::FormWeek(at_week as u8)).unwrap_or(0);
            let reach = value.clamp(-SCALE, SCALE).unsigned_abs() as usize;
            let high = (reach * half / SCALE as usize).max(2).min(half);
            let (top, bottom) = if value >= 0 {
                (zero.top - high, zero.top)
            } else {
                (tired, tired + high)
            };
            let left = first + at_week * (week + between);
            out.shape(Shape::Bar {
                rect: Rect {
                    left,
                    right: left + week,
                    top,
                    bottom,
                },
                radius: at.unit(RADIUS_UNITS),
                color: verdict(value.clamp(-999, 999) as i16).1,
            });
        }
    }
}
