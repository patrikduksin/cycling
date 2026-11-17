//! The course sign in three forms: a banner across a data page, a badge on
//! the map and a chip on the main page.
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

const UNIT_GAP: f32 = 4.0;
const INK_TO_EDGE: f32 = 8.25;
const RADAR: usize = super::ride::RADAR_WIDTH;
const RADAR_SHIFT: usize = super::ride::CENTRE_SHIFT;

pub(crate) const fn baseline_under_mockup(mockup: usize) -> usize {
    mockup + 1
}

const BADGE_TOP: usize = 30;
const BADGE_HEIGHT: usize = 60;

const fn badge_baseline_from_banner(banner_baseline: f32) -> f32 {
    BADGE_TOP as f32
        + (banner_baseline - BANNER.top as f32) * BADGE_HEIGHT as f32 / BANNER.height as f32
}

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

struct TwoLines {
    face: &'static Face,
    upper: f32,
    lower: f32,
}

struct Notices {
    back: f32,
    turn_back_line: f32,
    turn_back: &'static Face,
    arrived: (&'static Face, f32),
    turn_around: TwoLines,
}

const BANNER_ARRIVED: f32 = 72.5175;

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
        back: 68.25,
        turn_back_line: 93.0,
        turn_back: &SIGN_18,
        arrived: (&SIGN_27, BANNER_ARRIVED),
        turn_around: TwoLines {
            face: &SIGN_27,
            upper: 58.0,
            lower: 86.0,
        },
    }),
};

const BADGE_RADIUS: usize = 12;

const BADGE: Geometry = Geometry {
    left: 8,
    top: BADGE_TOP,
    height: BADGE_HEIGHT,
    radius: BADGE_RADIUS,
    text_left: 63.4167,
    baseline: 71.28,
    digits: &SIGN_32,
    unit: (&SIGN_14, WHITE),
    notices: Some(Notices {
        back: 60.56,
        turn_back_line: 82.0,
        turn_back: &SIGN_14,
        arrived: (&SIGN_20, badge_baseline_from_banner(BANNER_ARRIVED)),
        turn_around: TwoLines {
            face: &SIGN_20,
            upper: 56.0,
            lower: 78.0,
        },
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

const CHIP_SQUARE: usize = 28;

/// The form `page` shows `sign` in: a banner on the data pages, the chip
/// only on main, and on the map the badge, at rest too.
pub fn form(page: RidePage, sign: Sign) -> Option<SignForm> {
    match (page, sign) {
        (_, Sign::None) => None,
        (RidePage::Map, _) => Some(SignForm::Badge),
        (RidePage::Main, Sign::Chip { .. }) => Some(SignForm::Chip),
        (RidePage::Stats | RidePage::Climb, Sign::Chip { .. }) => None,
        (RidePage::Main | RidePage::Stats | RidePage::Climb, _) => Some(SignForm::Banner),
        (RidePage::Lap | RidePage::Workout, _) => None,
    }
}

/// `sign` laid out as `page` shows it, if it does.
pub fn on_page(page: RidePage, sign: Sign, radar: bool) -> Option<Placed> {
    place(sign, form(page, sign)?, radar)
}

/// A sign laid out for one frame.
#[derive(Clone, Hash)]
pub struct Placed {
    form: SignForm,
    bounds: Rect,
    fill: u16,
    glyph: StaticIcon,
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
    let shift = match (radar, form) {
        (false, _) => 0,
        (true, SignForm::Badge) => RADAR,
        (true, SignForm::Banner | SignForm::Chip) => RADAR_SHIFT,
    };
    let text_left = geometry.text_left + shift as f32;
    let mut lines = heapless::Vec::new();
    let (fill, glyph, ink) = match (sign, &geometry.notices) {
        (Sign::Chip { glyph, distance }, _) | (Sign::Turn { glyph, distance }, Some(_)) => {
            let ink = distance_line(&mut lines, geometry, distance, text_left, geometry.baseline);
            (TURN_FILL, art(glyph), ink)
        }
        (Sign::OffCourse { back }, Some(notices)) => {
            let ink = distance_line(&mut lines, geometry, back, text_left, notices.back);
            let words = push(
                &mut lines,
                notices.turn_back,
                b"TURN BACK",
                text_left,
                notices.turn_back_line,
                WHITE,
            );
            (OFF_FILL, SignGlyph::Back, ink.max(words))
        }
        (Sign::Arrived, Some(notices)) => {
            let (face, base) = notices.arrived;
            let ink = push(&mut lines, face, b"ARRIVED", text_left, base, WHITE);
            (ARRIVED_FILL, SignGlyph::Flag, ink)
        }
        (Sign::TurnAround, Some(notices)) => {
            let TwoLines { face, upper, lower } = notices.turn_around;
            let turn = push(&mut lines, face, b"TURN", text_left, upper, WHITE);
            let around = push(&mut lines, face, b"AROUND", text_left, lower, WHITE);
            (OFF_FILL, art(Glyph::UTurn), turn.max(around))
        }
        (Sign::None, _) | (_, None) => return None,
    };
    let left = geometry.left + shift;
    let right = match form {
        SignForm::Banner => chrome::WIDTH,
        SignForm::Badge | SignForm::Chip => round(ink + INK_TO_EDGE),
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

fn push(
    lines: &mut heapless::Vec<Label<9>, 3>,
    face: &'static Face,
    text: &[u8],
    left: f32,
    base: f32,
    color: u16,
) -> f32 {
    let top = baseline_under_mockup(base as usize) - usize::from(face.top) - usize::from(face.cap);
    let label = Label::new(
        Line::new(face, text),
        left as usize,
        top,
        Align::Left,
        color,
    );
    let _ = lines.push(label);
    left + face.ink_width(text) as f32
}

fn round(value: f32) -> usize {
    (value + 0.5) as usize
}

impl Placed {
    /// Paint the sign over the row, writing every pixel of `bounds` it covers.
    /// Its colours read the same in every look, so `row` takes none.
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

    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize, under: chrome::Shown) -> Option<chrome::Shown> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        let ink = |color| chrome::Look::PLAIN.ink(color);
        let (fill, radius) = self.fill_rect();
        let mut color = under;
        if self.form == SignForm::Chip {
            color = blend(ink(RAISED), color, rounded(self.bounds, radius, x, y));
        }
        if fill.contains(x, y) {
            color = blend(ink(self.fill), color, rounded(fill, radius, x, y));
        }
        let (left, top) = self.glyph_at;
        color = self
            .glyph
            .pixel(x, y, left, top, ink(WHITE), color)
            .unwrap_or(color);
        for label in &self.lines {
            color = label
                .pixel(chrome::Look::PLAIN, x, y, color)
                .unwrap_or(color);
        }
        Some(color)
    }

    pub fn rect(&self) -> Rect {
        self.bounds
    }

    pub fn form(&self) -> SignForm {
        self.form
    }

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
    let color = row.ink(color);
    paint_rounded_over(row, rect, radius, 0, |x, y, under| {
        match rounded(rect, radius, x, y) {
            0 => None,
            coverage => Some(blend(color, under, coverage)),
        }
    });
}

#[cfg(test)]
mod tests;
