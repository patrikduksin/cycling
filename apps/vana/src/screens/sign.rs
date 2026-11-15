//! The course sign: one block in three forms, as the mockups in
//! `docs/design/course-navigation` draw it. The banner spans a data page's
//! rows 22 to 104, the badge floats on the map and the chip rests on the
//! main page. Every form is laid out from the mockup's own fractional
//! positions and paints white ink on its fill after the light look, so it
//! reads the same in both looks.
use super::chrome::{self, MUTED, RAISED, Rect, Row, WHITE, blend, paint_rounded_over, rounded};
use super::icons::{SignForm, SignGlyph, StaticIcon};
use super::ride::RidePage;
use crate::ride::course::nav::{Distance, Glyph, Sign, Unit};
use core::fmt::Write;
use firmware_shell::rendering::font::{
    Align, Face, Label, Line, SIGN_13, SIGN_14, SIGN_18, SIGN_20, SIGN_27, SIGN_32, SIGN_50,
};

pub const TURN_FILL: u16 = chrome::rgb(0x0a, 0x6f, 0xe0);
pub const OFF_FILL: u16 = chrome::rgb(0xd6, 0x74, 0x00);
pub const ARRIVED_FILL: u16 = chrome::rgb(0x1b, 0x9a, 0x45);

/// From the digits' ink to their unit's pen, and from the furthest ink to
/// a fitted fill's right edge. Pens keep the mockups' fractions until a
/// label is placed; tests lock both against the mockups' badge and chip
/// widths.
const UNIT_GAP: f32 = 4.0;
const PAD: f32 = 8.25;
/// With a radar paired the radar strip keeps its columns: the banner starts
/// here and every form's content moves right by half of it, as the page
/// body does.
const RADAR: usize = 28;
const RADAR_SHIFT: usize = RADAR / 2;

/// A form as its mockup draws it without a radar, in screen pixels.
struct Geometry {
    left: usize,
    top: usize,
    height: usize,
    radius: usize,
    text_left: f32,
    baseline: f32,
    digits: &'static Face,
    unit: (&'static Face, u16),
    notices: Option<Notices>,
}

/// Off course and arrived, which the chip never shows.
struct Notices {
    /// The distance back and TURN BACK under it.
    baselines: (f32, f32),
    turn_back: &'static Face,
    arrived: (&'static Face, f32),
}

const BANNER: Geometry = Geometry {
    left: 0,
    top: 22,
    height: 82,
    radius: 0,
    text_left: 89.75,
    baseline: 80.625,
    digits: &SIGN_50,
    unit: (&SIGN_20, WHITE),
    notices: Some(Notices {
        baselines: (68.25, 93.0),
        turn_back: &SIGN_18,
        arrived: (&SIGN_27, 72.5175),
    }),
};

/// The banner at 60 / 82 of its size. No mockup draws an arrived badge: its
/// baseline is the banner's, scaled.
const BADGE: Geometry = Geometry {
    left: 8,
    top: 30,
    height: 60,
    radius: 12,
    text_left: 63.4167,
    baseline: 71.28,
    digits: &SIGN_32,
    unit: (&SIGN_14, WHITE),
    notices: Some(Notices {
        baselines: (60.56, 82.0),
        turn_back: &SIGN_14,
        arrived: (&SIGN_20, 66.97),
    }),
};

const CHIP: Geometry = Geometry {
    left: 24,
    top: 72,
    height: 28,
    radius: 8,
    text_left: 60.0,
    baseline: 93.05,
    digits: &SIGN_20,
    unit: (&SIGN_13, MUTED),
    notices: None,
};

/// The chip's turn-fill square.
const CHIP_SQUARE: usize = 28;

/// The form `page` shows `sign` in: a banner on the data pages, the chip
/// only on main.
pub fn form(page: RidePage, sign: Sign) -> Option<SignForm> {
    match (page, sign) {
        (_, Sign::None) => None,
        (RidePage::Main, Sign::Chip { .. }) => Some(SignForm::Chip),
        (RidePage::Stats | RidePage::Climb, Sign::Chip { .. }) => None,
        (RidePage::Main | RidePage::Stats | RidePage::Climb, _) => Some(SignForm::Banner),
        (RidePage::Lap | RidePage::Workout | RidePage::Map, _) => None,
    }
}

/// A sign laid out for one frame.
#[derive(Clone, Hash)]
pub struct Placed {
    form: SignForm,
    /// The whole sign: the banner or badge fill, or the chip's plate.
    bounds: Rect,
    fill: u16,
    glyph: StaticIcon,
    /// The glyph icon square's top-left corner.
    glyph_at: (usize, usize),
    lines: heapless::Vec<Label<9>, 3>,
}

/// Lay `sign` out in `form`; none for a sign that form does not draw.
pub fn place(sign: Sign, form: SignForm, radar: bool) -> Option<Placed> {
    let geometry = match form {
        SignForm::Banner => &BANNER,
        SignForm::Badge => &BADGE,
        SignForm::Chip => &CHIP,
    };
    let shift = if radar { RADAR_SHIFT } else { 0 };
    let text_left = geometry.text_left + shift as f32;
    let mut lines = heapless::Vec::new();
    let (fill, glyph, ink) = match (sign, &geometry.notices) {
        (Sign::Chip { glyph, distance }, _) | (Sign::Turn { glyph, distance }, Some(_)) => {
            let ink = distance_line(&mut lines, geometry, distance, text_left, geometry.baseline);
            (TURN_FILL, art(glyph), ink)
        }
        (Sign::OffCourse { back }, Some(notices)) => {
            let (first, second) = notices.baselines;
            let ink = distance_line(&mut lines, geometry, back, text_left, first);
            let words = push(
                &mut lines,
                notices.turn_back,
                b"TURN BACK",
                text_left,
                second,
                WHITE,
            );
            (OFF_FILL, SignGlyph::Back, ink.max(words))
        }
        (Sign::Arrived, Some(notices)) => {
            let (face, base) = notices.arrived;
            let ink = push(&mut lines, face, b"ARRIVED", text_left, base, WHITE);
            (ARRIVED_FILL, SignGlyph::Flag, ink)
        }
        (Sign::None, _) | (_, None) => return None,
    };
    let left = geometry.left + shift;
    let right = match form {
        SignForm::Banner => chrome::WIDTH,
        SignForm::Badge | SignForm::Chip => round(ink + PAD),
    };
    let art = glyph.art(form);
    Some(Placed {
        form,
        bounds: Rect {
            left: if radar && form == SignForm::Banner {
                RADAR
            } else {
                left
            },
            top: geometry.top,
            right,
            bottom: geometry.top + geometry.height,
        },
        fill,
        glyph: StaticIcon(&art.icon),
        glyph_at: (
            left + usize::from(art.left),
            geometry.top + usize::from(art.top),
        ),
        lines,
    })
}

/// A turn's glyph; left turns have their own art, drawn mirrored.
fn art(glyph: Glyph) -> SignGlyph {
    match glyph {
        Glyph::Left => SignGlyph::Left,
        Glyph::Right => SignGlyph::Right,
        Glyph::SharpLeft => SignGlyph::SharpLeft,
        Glyph::SharpRight => SignGlyph::SharpRight,
        Glyph::UTurn => SignGlyph::UTurnLeft,
        Glyph::Finish => SignGlyph::Flag,
    }
}

/// The distance in the form's digits, then its unit after their ink; where
/// the unit's ink ends.
fn distance_line(
    lines: &mut heapless::Vec<Label<9>, 3>,
    geometry: &Geometry,
    distance: Distance,
    left: f32,
    base: f32,
) -> f32 {
    let mut digits = heapless::String::<9>::new();
    let _ = write!(digits, "{distance}");
    let at = push(lines, geometry.digits, digits.as_bytes(), left, base, WHITE) + UNIT_GAP;
    let unit: &[u8] = match distance.unit() {
        Unit::Metres => b"M",
        Unit::Kilometres => b"KM",
    };
    let (face, color) = geometry.unit;
    push(lines, face, unit, at, base, color)
}

/// A line from pen `left` with its capitals on `base`; where its ink ends.
fn push(
    lines: &mut heapless::Vec<Label<9>, 3>,
    face: &'static Face,
    text: &[u8],
    left: f32,
    base: f32,
    color: u16,
) -> f32 {
    let top = round(base) - usize::from(face.top) - usize::from(face.cap);
    let label = Label::new(Line::new(face, text), round(left), top, Align::Left, color);
    let _ = lines.push(label);
    left + face.ink_width(text) as f32
}

fn round(value: f32) -> usize {
    (value + 0.5) as usize
}

impl Placed {
    /// Paint the sign over the row: its fills, the glyph, then the text.
    /// Writes every pixel of `bounds` it covers.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        if y < self.bounds.top || y >= self.bounds.bottom {
            return;
        }
        let (fill, radius) = self.fill_rect();
        if self.form == SignForm::Chip {
            paint_block(row, self.bounds, radius, RAISED);
        }
        paint_block(row, fill, radius, self.fill);
        self.glyph
            .paint(row, self.glyph_at.0, self.glyph_at.1, WHITE);
        for label in &self.lines {
            label.paint(row);
        }
    }

    /// The per-pixel original of `paint` over `under`; none outside the sign.
    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize, under: u16) -> Option<u16> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        let (fill, radius) = self.fill_rect();
        let mut color = under;
        if self.form == SignForm::Chip {
            color = blend(RAISED, color, rounded(self.bounds, radius, x, y));
        }
        if fill.contains(x, y) {
            color = blend(self.fill, color, rounded(fill, radius, x, y));
        }
        let (left, top) = self.glyph_at;
        color = self
            .glyph
            .pixel(x, y, left, top, WHITE, color)
            .unwrap_or(color);
        for label in &self.lines {
            color = label.pixel(x, y, color).unwrap_or(color);
        }
        Some(color)
    }

    /// The whole sign, for the hit test and the rows it sends.
    pub fn rect(&self) -> Rect {
        self.bounds
    }

    pub fn form(&self) -> SignForm {
        self.form
    }

    /// The coloured block and its corner radius: all of a banner or badge,
    /// the chip's square.
    fn fill_rect(&self) -> (Rect, usize) {
        match self.form {
            SignForm::Banner => (self.bounds, BANNER.radius),
            SignForm::Badge => (self.bounds, BADGE.radius),
            SignForm::Chip => (
                Rect {
                    right: self.bounds.left + CHIP_SQUARE,
                    ..self.bounds
                },
                CHIP.radius,
            ),
        }
    }
}

fn paint_block(row: &mut Row, rect: Rect, radius: usize, color: u16) {
    paint_rounded_over(row, rect, radius, 0, |x, y, under| {
        match rounded(rect, radius, x, y) {
            0 => None,
            coverage => Some(blend(color, under, coverage)),
        }
    });
}

#[cfg(test)]
mod tests;
