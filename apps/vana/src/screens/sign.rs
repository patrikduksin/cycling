//! The course sign in three forms: a banner across a data page, a badge on
//! the map and a chip on the main page.
use super::Device;
use super::chrome::{self, MUTED, RAISED, Rect, Row, WHITE, blend, paint_rounded_over, rounded};
use super::icons::{SignForm, SignGlyph, StaticIcon};
use super::ride::RidePage;
use super::scaled::{Scaled, Track};
use crate::ride::course::nav::{Distance, Glyph, Sign, Unit};
use core::fmt::Write;
use firmware_shell::rendering::font::{Align, FaceRole, Label, Line};

pub const TURN_FILL: u16 = chrome::rgb(0x0a, 0x6f, 0xe0);
pub const OFF_FILL: u16 = chrome::rgb(0xd6, 0x74, 0x00);
pub const ARRIVED_FILL: u16 = chrome::rgb(0x1b, 0x9a, 0x45);

const UNIT_GAP: f32 = 4.0;
const INK_TO_EDGE: f32 = 8.25;

pub(crate) const fn baseline_under_mockup(mockup: usize) -> usize {
    mockup + 1
}

/// Where the sign's banner and badge sit on one panel. The chip sits where
/// the main page's tree places it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Forms {
    pub banner: Spot,
    pub badge: Spot,
}

/// One form on a panel. The banner fills `fill`; the badge and the chip
/// fill its rows from its left edge to a margin past their ink, which stays
/// inside it. The glyph and text are measured from column `origin`: the
/// fill's left edge, but for a banner beside a radar, whose content moves
/// right by half the strip as the page's does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spot {
    pub fill: Rect,
    pub origin: usize,
    pub radius: usize,
}

/// Rows of content the banner covers under the status bar: the main page's
/// time, grade and chip, which its tree keeps above them.
pub const BANNER_UNITS: usize = 80;

/// The banner spans the content from the status bar down `BANNER_UNITS`,
/// over the main page's time, grade and chip; the badge floats on the map
/// 8 units in from its corner.
pub const fn forms(at: &Scaled, radar: bool) -> Forms {
    use Track::{Fr, Unit};
    let under_bar = Rect {
        top: chrome::status_bar(at).bottom,
        ..at.screen()
    };
    let banner = Rect {
        bottom: chrome::content(at).top + at.unit(BANNER_UNITS),
        ..under_bar
    };
    let [_, badge, _] = under_bar.rows([Unit(8), Unit(60), Fr(1)], Unit(0), at);
    if !radar {
        return Forms {
            banner: Spot {
                fill: banner,
                origin: banner.left,
                radius: 0,
            },
            badge: badge_on(badge, at),
        };
    }
    let strip = super::ride::radar_columns(at);
    let beside_strip = Rect {
        left: strip.right,
        ..banner
    };
    Forms {
        banner: Spot {
            fill: beside_strip,
            origin: banner.left + strip.width() / 2,
            radius: 0,
        },
        badge: badge_on(badge.with_columns(beside_strip), at),
    }
}

const fn badge_on(map: Rect, at: &Scaled) -> Spot {
    let [_, fill] = map.cols([Track::Unit(8), Track::Fr(1)], Track::Unit(0), at);
    Spot {
        fill,
        origin: fill.left,
        radius: at.unit(12),
    }
}

const fn chip_at(fill: Rect, at: &Scaled) -> Spot {
    Spot {
        fill,
        origin: fill.left,
        radius: at.unit(8),
    }
}

struct Mockup {
    text_left: f32,
    baseline: f32,
    digits: FaceRole,
    unit: (FaceRole, u16),
    notices: Option<Notices>,
}

struct TwoLines {
    face: FaceRole,
    upper: f32,
    lower: f32,
}

struct Notices {
    back: f32,
    turn_back_line: f32,
    turn_back: FaceRole,
    arrived: (FaceRole, f32),
    turn_around: TwoLines,
}

const fn from(edge: usize, drawn: f32) -> f32 {
    drawn - edge as f32
}

const BANNER_TOP: usize = 22;
const BANNER_HEIGHT: f32 = 82.0;
const BANNER_ARRIVED: f32 = from(BANNER_TOP, 72.5175);

const BANNER: Mockup = Mockup {
    text_left: 89.75,
    baseline: from(BANNER_TOP, 80.625),
    digits: FaceRole::Sign50,
    unit: (FaceRole::Sign20, WHITE),
    notices: Some(Notices {
        back: from(BANNER_TOP, 68.25),
        turn_back_line: from(BANNER_TOP, 93.0),
        turn_back: FaceRole::Sign18,
        arrived: (FaceRole::Sign27, BANNER_ARRIVED),
        turn_around: TwoLines {
            face: FaceRole::Sign27,
            upper: from(BANNER_TOP, 58.0),
            lower: from(BANNER_TOP, 86.0),
        },
    }),
};

const BADGE_LEFT: usize = 8;
const BADGE_TOP: usize = 30;
const BADGE_HEIGHT: f32 = 60.0;

const BADGE: Mockup = Mockup {
    text_left: from(BADGE_LEFT, 63.4167),
    baseline: from(BADGE_TOP, 71.28),
    digits: FaceRole::Sign32,
    unit: (FaceRole::Sign14, WHITE),
    notices: Some(Notices {
        back: from(BADGE_TOP, 60.56),
        turn_back_line: from(BADGE_TOP, 82.0),
        turn_back: FaceRole::Sign14,
        arrived: (
            FaceRole::Sign20,
            BANNER_ARRIVED * BADGE_HEIGHT / BANNER_HEIGHT,
        ),
        turn_around: TwoLines {
            face: FaceRole::Sign20,
            upper: from(BADGE_TOP, 56.0),
            lower: from(BADGE_TOP, 78.0),
        },
    }),
};

const CHIP_LEFT: usize = 24;
const CHIP_TOP: usize = 72;

const CHIP: Mockup = Mockup {
    text_left: from(CHIP_LEFT, 60.0),
    baseline: from(CHIP_TOP, 93.05),
    digits: FaceRole::Sign20,
    unit: (FaceRole::Sign13, MUTED),
    notices: None,
};

/// The widest a chip reads, its glyph's square included: a course's
/// distance of three digits and "KM".
pub fn chip_width(at: &Scaled) -> usize {
    let pen = Pen {
        at,
        left: 0.0,
        top: 0.0,
        scale: at.scale.factor(),
    };
    let digits = at.face(CHIP.digits).ink_width(b"999") as f32;
    let unit = at.face(CHIP.unit.0).ink_width(b"KM") as f32;
    round(pen.x(CHIP.text_left) + digits + pen.size(UNIT_GAP) + unit + pen.size(INK_TO_EDGE))
}

/// The form `page` shows `sign` in: a banner on the data pages, the chip
/// only on main, and on the map the badge, at rest too.
pub fn form(page: RidePage, sign: Sign) -> Option<SignForm> {
    match (page, sign) {
        (_, Sign::None) => None,
        (RidePage::Map, _) => Some(SignForm::Badge),
        (RidePage::Main, Sign::Chip { .. }) => Some(SignForm::Chip),
        (RidePage::Climb, Sign::Chip { .. }) => None,
        (RidePage::Main | RidePage::Climb, _) => Some(SignForm::Banner),
        (RidePage::Workout, _) => None,
    }
}

/// `sign` laid out as `page` shows it on `device`, if it does: a chip in
/// `chip`, the box the main page's tree gives it.
pub fn on_page(
    page: RidePage,
    sign: Sign,
    radar: bool,
    device: &'static Device,
    chip: Option<Rect>,
) -> Option<Placed> {
    match form(page, sign)? {
        SignForm::Chip => place_chip(sign, chip?, device),
        form => place(sign, form, radar, device),
    }
}

/// A sign laid out for one frame.
#[derive(Clone, Hash)]
pub struct Placed {
    form: SignForm,
    spot: Spot,
    right: usize,
    fill: u16,
    glyph: StaticIcon,
    glyph_at: (usize, usize),
    lines: heapless::Vec<Label<9>, 3>,
}

struct Pen<'a> {
    at: &'a Scaled,
    left: f32,
    top: f32,
    scale: f32,
}

impl Pen<'_> {
    fn x(&self, mockup: f32) -> f32 {
        self.left + self.size(mockup)
    }

    fn y(&self, mockup: f32) -> f32 {
        self.top + self.size(mockup)
    }

    fn size(&self, mockup: f32) -> f32 {
        mockup * self.scale
    }
}

/// Lay `sign` out as a banner or a badge on `device`; none for a sign that
/// form does not draw, or for the chip, which `place_chip` lays out.
pub fn place(sign: Sign, form: SignForm, radar: bool, device: &Device) -> Option<Placed> {
    let forms = forms(&device.scaled, radar);
    match form {
        SignForm::Banner => lay(sign, form, forms.banner, &device.scaled),
        SignForm::Badge => lay(sign, form, forms.badge, &device.scaled),
        SignForm::Chip => None,
    }
}

/// Lay `sign` out as a chip in `chip`.
pub fn place_chip(sign: Sign, chip: Rect, device: &Device) -> Option<Placed> {
    let at = &device.scaled;
    lay(sign, SignForm::Chip, chip_at(chip, at), at)
}

fn lay(sign: Sign, form: SignForm, spot: Spot, at: &Scaled) -> Option<Placed> {
    let mockup = match form {
        SignForm::Banner => &BANNER,
        SignForm::Badge => &BADGE,
        SignForm::Chip => &CHIP,
    };
    let pen = Pen {
        at,
        left: spot.origin as f32,
        top: spot.fill.top as f32,
        scale: at.scale.factor(),
    };
    let text_left = pen.x(mockup.text_left);
    let mut lines = heapless::Vec::new();
    let (fill, glyph, ink) = match (sign, &mockup.notices) {
        (Sign::Chip { glyph, distance }, _) | (Sign::Turn { glyph, distance }, Some(_)) => {
            let base = pen.y(mockup.baseline);
            let ink = distance_line(&mut lines, &pen, mockup, distance, text_left, base);
            (TURN_FILL, art(glyph), ink)
        }
        (Sign::OffCourse { back }, Some(notices)) => {
            let base = pen.y(notices.back);
            let ink = distance_line(&mut lines, &pen, mockup, back, text_left, base);
            let words = push(
                &mut lines,
                &pen,
                notices.turn_back,
                b"TURN BACK",
                text_left,
                pen.y(notices.turn_back_line),
                WHITE,
            );
            (OFF_FILL, SignGlyph::Back, ink.max(words))
        }
        (Sign::Arrived, Some(notices)) => {
            let (face, base) = notices.arrived;
            let ink = push(
                &mut lines,
                &pen,
                face,
                b"ARRIVED",
                text_left,
                pen.y(base),
                WHITE,
            );
            (ARRIVED_FILL, SignGlyph::Flag, ink)
        }
        (Sign::TurnAround, Some(notices)) => {
            let TwoLines { face, upper, lower } = notices.turn_around;
            let (upper, lower) = (pen.y(upper), pen.y(lower));
            let turn = push(&mut lines, &pen, face, b"TURN", text_left, upper, WHITE);
            let around = push(&mut lines, &pen, face, b"AROUND", text_left, lower, WHITE);
            (OFF_FILL, art(Glyph::UTurn), turn.max(around))
        }
        (Sign::None, _) | (_, None) => return None,
    };
    let right = match form {
        SignForm::Banner => spot.fill.right,
        SignForm::Badge | SignForm::Chip => round(ink + pen.size(INK_TO_EDGE)),
    };
    let art = at.art.sign(glyph, form);
    Some(Placed {
        form,
        spot,
        right,
        fill,
        glyph: StaticIcon(&art.icon),
        glyph_at: (
            spot.origin + usize::from(art.left),
            spot.fill.top + usize::from(art.top),
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
    pen: &Pen,
    mockup: &Mockup,
    distance: Distance,
    left: f32,
    base: f32,
) -> f32 {
    let mut digits = heapless::String::<9>::new();
    let _ = write!(digits, "{distance}");
    let at = push(
        lines,
        pen,
        mockup.digits,
        digits.as_bytes(),
        left,
        base,
        WHITE,
    ) + pen.size(UNIT_GAP);
    let unit: &[u8] = match distance.unit() {
        Unit::Metres => b"M",
        Unit::Kilometres => b"KM",
    };
    let (face, color) = mockup.unit;
    push(lines, pen, face, unit, at, base, color)
}

fn push(
    lines: &mut heapless::Vec<Label<9>, 3>,
    pen: &Pen,
    role: FaceRole,
    text: &[u8],
    left: f32,
    base: f32,
    color: u16,
) -> f32 {
    let face = pen.at.face(role);
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
        let bounds = self.rect();
        if y < bounds.top || y >= bounds.bottom {
            return;
        }
        let (fill, radius) = self.fill_rect();
        if self.form == SignForm::Chip {
            paint_block(row, bounds, radius, RAISED);
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
        let bounds = self.rect();
        if !bounds.contains(x, y) {
            return None;
        }
        let ink = |color| chrome::Look::PLAIN.ink(color);
        let (fill, radius) = self.fill_rect();
        let mut color = under;
        if self.form == SignForm::Chip {
            color = blend(ink(RAISED), color, rounded(bounds, radius, x, y));
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
        Rect {
            right: self.right,
            ..self.spot.fill
        }
    }

    pub fn form(&self) -> SignForm {
        self.form
    }

    fn fill_rect(&self) -> (Rect, usize) {
        let bounds = self.rect();
        let fill = match self.form {
            SignForm::Banner | SignForm::Badge => bounds,
            SignForm::Chip => Rect {
                right: bounds.left + bounds.height(),
                ..bounds
            },
        };
        (fill, self.spot.radius)
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
