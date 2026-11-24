//! Anti-aliased icons from pre-rasterized 4-bit coverage.
//!
//! Each icon is rasterized at every size a screen draws it (`apps/vana/icons`)
//! and painted like a glyph run: one colour blended over what the row already
//! holds, so the same icon sits on black, a card or a focused card.
mod art;
mod art_0_75;

use super::chrome::{Rect, Row, Shown, blend};
use crate::sensors::ant::device_type;
pub use art::{ARROW, PLUS, PUCK, REFRESH, Sensor, SignGlyph, Symbol};

/// One icon at one pixel size: coverage for the box its ink fills inside a
/// `size` square, two columns a byte with the left one in the high nibble.
#[derive(Debug)]
pub struct Icon {
    pub size: u8,
    left: u8,
    top: u8,
    width: u8,
    height: u8,
    bits: &'static [u8],
}

#[derive(Clone, Copy, Debug)]
pub struct StaticIcon(pub &'static Icon);

impl core::hash::Hash for StaticIcon {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        core::ptr::hash(self.0, state);
    }
}

/// An icon is one static: icons are the same only when they are one.
impl PartialEq for StaticIcon {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0, other.0)
    }
}

impl Eq for StaticIcon {}

impl core::ops::Deref for StaticIcon {
    type Target = Icon;

    fn deref(&self) -> &Icon {
        self.0
    }
}

impl Icon {
    /// Coverage 0..=15 at (`x`, `y`) of the icon's square.
    pub fn coverage(&self, x: usize, y: usize) -> u8 {
        let (Some(x), Some(y)) = (
            x.checked_sub(usize::from(self.left)),
            y.checked_sub(usize::from(self.top)),
        ) else {
            return 0;
        };
        if x >= usize::from(self.width) || y >= usize::from(self.height) {
            return 0;
        }
        let byte = self.bits[y * self.stride() + x / 2];
        if x.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0f
        }
    }

    /// The icon in shown colour `color` at (`x`, `y`) with its square's
    /// top-left corner at (`left`, `top`), over `background`; `None` outside
    /// its ink.
    pub fn pixel(
        &self,
        x: usize,
        y: usize,
        left: usize,
        top: usize,
        color: Shown,
        background: Shown,
    ) -> Option<Shown> {
        let coverage = self.coverage(x.checked_sub(left)?, y.checked_sub(top)?);
        (coverage > 0).then(|| blend(color, background, coverage))
    }

    /// Blend the icon's row at `row.y()` over the row, with its square's
    /// top-left corner at (`left`, `top`). Matches `pixel` over the same
    /// background.
    pub fn paint(&self, row: &mut Row, left: usize, top: usize, color: u16) {
        let Some(y) = row
            .y()
            .checked_sub(top + usize::from(self.top))
            .filter(|&y| y < usize::from(self.height))
        else {
            return;
        };
        let start = left + usize::from(self.left);
        let clip = row.clip();
        let end = (start + usize::from(self.width)).min(clip.end);
        if start >= end {
            return;
        }
        let bits = &self.bits[y * self.stride()..][..self.stride()];
        let coverage = |column: usize| {
            let byte = bits[column / 2];
            if column.is_multiple_of(2) {
                byte >> 4
            } else {
                byte & 0x0f
            }
        };
        let color = row.ink(color);
        row.pixels_mut(
            start..end,
            |x| coverage(x - start),
            |pixels| {
                let skipped = clip.start.saturating_sub(start);
                for (column, pixel) in (skipped..).zip(pixels) {
                    match coverage(column) {
                        0 => {}
                        15 => *pixel = color,
                        coverage => *pixel = blend(color, *pixel, coverage),
                    }
                }
            },
        );
    }

    /// The box the icon's ink fills with its square's top-left corner at
    /// (`left`, `top`).
    pub const fn ink(&self, left: usize, top: usize) -> Rect {
        let (left, top) = (left + self.left as usize, top + self.top as usize);
        Rect {
            left,
            top,
            right: left + self.width as usize,
            bottom: top + self.height as usize,
        }
    }

    const fn stride(&self) -> usize {
        (self.width as usize).div_ceil(2)
    }
}

/// The course sign's three forms, as the mockups in
/// `docs/design/course-navigation` draw them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SignForm {
    /// Full width on a data page.
    Banner,
    /// Floating on the map.
    Badge,
    /// The square at the left of the main page's chip.
    Chip,
}

/// A sign glyph in one form: its icon, whose square's top-left corner sits
/// (`left`, `top`) from the top-left corner of the form's fill.
#[derive(Debug)]
pub struct SignArt {
    pub left: u8,
    pub top: u8,
    pub icon: Icon,
}

/// Every icon at one scale step, as `Faces` holds every face.
pub struct Art {
    sensors: [&'static [Icon; 10]; 3],
    pub refresh: &'static Icon,
    pub plus: &'static Icon,
    pub arrow: &'static Icon,
    pub puck: &'static Icon,
    signs: &'static [[SignArt; 3]; 8],
    symbols: &'static [Icon; 5],
}

impl Art {
    pub fn sign(&self, glyph: SignGlyph, form: SignForm) -> &'static SignArt {
        &self.signs[glyph as usize][form as usize]
    }

    pub fn sensor(&self, sensor: Sensor, size: Size) -> &'static Icon {
        &self.sensors[size as usize][sensor as usize]
    }

    pub fn symbol(&self, symbol: Symbol) -> &'static Icon {
        &self.symbols[symbol as usize]
    }
}

/// Each step has one set, so sets compare by address, and a set adds
/// nothing to a hash that its step does not.
impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other)
    }
}

impl Eq for Art {}

impl core::hash::Hash for Art {
    fn hash<H: core::hash::Hasher>(&self, _: &mut H) {}
}

impl core::fmt::Debug for Art {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Art({} px puck)", self.puck.size)
    }
}

pub static ART_1: Art = Art {
    sensors: [&art::SENSORS_22, &art::SENSORS_26, &art::SENSORS_34],
    refresh: &art::REFRESH,
    plus: &art::PLUS,
    arrow: &art::ARROW,
    puck: &art::PUCK,
    signs: &art::SIGN_GLYPHS,
    symbols: &art::SYMBOLS,
};

pub static ART_0_75: Art = Art {
    sensors: [
        &art_0_75::SENSORS_17,
        &art_0_75::SENSORS_20,
        &art_0_75::SENSORS_26,
    ],
    refresh: &art_0_75::REFRESH,
    plus: &art_0_75::PLUS,
    arrow: &art_0_75::ARROW,
    puck: &art_0_75::PUCK,
    signs: &art_0_75::SIGN_GLYPHS,
    symbols: &art_0_75::SYMBOLS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Size {
    /// Home's sensor card and NEARBY's rows.
    Small,
    Row,
    /// A sensor's detail and replacement pages.
    Large,
}

impl Sensor {
    pub const fn of(device_type: u8) -> Self {
        match device_type {
            device_type::HEART_RATE => Self::Heart,
            device_type::POWER => Self::Power,
            device_type::SPEED => Self::Speed,
            device_type::CADENCE => Self::Cadence,
            device_type::SPEED_CADENCE => Self::SpeedCadence,
            device_type::RADAR => Self::Radar,
            device_type::SHIFTING | device_type::SHIFTING_DI2 => Self::Shifting,
            device_type::FITNESS_EQUIPMENT => Self::Trainer,
            device_type::BIKE_LIGHTS => Self::Lights,
            _ => Self::Generic,
        }
    }

    pub fn icon(self, size: Size) -> &'static Icon {
        ART_1.sensor(self, size)
    }

    /// The colour of the metric the sensor measures, or white for one that
    /// measures none of them.
    pub const fn tint(self) -> u16 {
        use super::chrome;
        match self {
            Self::Heart => chrome::HEART,
            Self::Power => chrome::POWER,
            Self::Speed | Self::SpeedCadence => chrome::SPEED,
            Self::Cadence => chrome::CADENCE,
            Self::Radar | Self::Shifting | Self::Trainer | Self::Lights | Self::Generic => {
                chrome::WHITE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{BLACK, CARD, Rect, WHITE};

    const ALL: [Sensor; 10] = [
        Sensor::Heart,
        Sensor::Power,
        Sensor::Speed,
        Sensor::Cadence,
        Sensor::SpeedCadence,
        Sensor::Radar,
        Sensor::Shifting,
        Sensor::Trainer,
        Sensor::Lights,
        Sensor::Generic,
    ];

    fn icons(art: &'static Art) -> impl Iterator<Item = &'static Icon> {
        ALL.into_iter()
            .flat_map(|sensor| {
                [Size::Small, Size::Row, Size::Large].map(|size| art.sensor(sensor, size))
            })
            .chain([art.plus, art.refresh, art.arrow, art.puck])
            .chain(SYMBOLS.map(|symbol| art.symbol(symbol)))
            .chain(
                SIGN_GLYPHS
                    .into_iter()
                    .flat_map(|glyph| FORMS.map(|form| &art.sign(glyph, form).icon)),
            )
    }

    const SIGN_GLYPHS: [SignGlyph; 8] = [
        SignGlyph::Right,
        SignGlyph::Left,
        SignGlyph::SharpRight,
        SignGlyph::SharpLeft,
        SignGlyph::UTurnLeft,
        SignGlyph::UTurnRight,
        SignGlyph::Back,
        SignGlyph::Flag,
    ];
    const FORMS: [SignForm; 3] = [SignForm::Banner, SignForm::Badge, SignForm::Chip];
    const SYMBOLS: [Symbol; 5] = [
        Symbol::Power,
        Symbol::Heart,
        Symbol::Ramp,
        Symbol::Peak,
        Symbol::Satellite,
    ];

    fn ink(glyph: SignGlyph, form: SignForm) -> Rect {
        let art = ART_1.sign(glyph, form);
        art.icon.ink(usize::from(art.left), usize::from(art.top))
    }

    #[test]
    fn every_sign_glyph_sits_inside_its_fill() {
        for glyph in SIGN_GLYPHS {
            for (form, width, height) in [
                (SignForm::Banner, 240, 82),
                (SignForm::Badge, 118, 60),
                (SignForm::Chip, 28, 28),
            ] {
                let ink = ink(glyph, form);
                assert!(ink.left > 0 && ink.top > 0, "{glyph:?} {form:?}");
                assert!(
                    ink.right < width && ink.bottom < height,
                    "{glyph:?} {form:?}"
                );
            }
        }
    }

    #[test]
    fn left_glyphs_mirror_their_right_twins_about_the_mockup_centre() {
        let centres = [
            (SignForm::Banner, 52.75),
            (SignForm::Badge, 26.33),
            (SignForm::Chip, 13.44),
        ];
        let pairs = [
            (SignGlyph::Right, SignGlyph::Left),
            (SignGlyph::SharpRight, SignGlyph::SharpLeft),
            (SignGlyph::UTurnRight, SignGlyph::UTurnLeft),
        ];
        for (right, left) in pairs {
            for (form, centre) in centres {
                let (r, l) = (ink(right, form), ink(left, form));
                let mirrored = (r.left + l.right) as f32 / 2.0;
                assert!(
                    (mirrored - centre).abs() <= 0.5,
                    "{right:?} {form:?}: {mirrored}"
                );
                assert_eq!((r.top, r.bottom), (l.top, l.bottom));
            }
        }
        assert_eq!(ink(SignGlyph::Right, SignForm::Banner).top, 63 - 22 - 30);
    }

    #[test]
    fn each_icon_has_solid_ink_inside_its_square_at_every_step() {
        for icon in icons(&ART_1).chain(icons(&ART_0_75)) {
            let size = usize::from(icon.size);
            assert!(usize::from(icon.left + icon.width) <= size);
            assert!(usize::from(icon.top + icon.height) <= size);
            assert_eq!(icon.bits.len(), icon.stride() * usize::from(icon.height));
            let solid = (0..size).any(|y| (0..size).any(|x| icon.coverage(x, y) == 15));
            assert!(solid, "an icon of {size} px has a solid pixel");
            assert_eq!(icon.coverage(size, 0), 0);
        }
        assert_eq!(Sensor::Generic.icon(Size::Row).size, 26);
        assert_eq!(Sensor::Heart.icon(Size::Large).size, 34);
        assert_eq!((ART_1.puck.size, ART_0_75.puck.size), (20, 15));
        let sizes = |art: &'static Art| SYMBOLS.map(|symbol| art.symbol(symbol).size);
        assert_eq!(sizes(&ART_1), [11, 11, 11, 12, 15]);
        assert_eq!(sizes(&ART_0_75), [8, 8, 8, 9, 11]);
    }

    #[test]
    fn sensors_of_every_type_have_their_own_icon() {
        use crate::sensors::ant::device_type::*;
        let kinds = [
            HEART_RATE,
            POWER,
            SPEED,
            CADENCE,
            SPEED_CADENCE,
            RADAR,
            SHIFTING,
            FITNESS_EQUIPMENT,
            BIKE_LIGHTS,
            0,
        ];
        assert_eq!(kinds.map(Sensor::of), ALL);
        assert_eq!(Sensor::of(SHIFTING_DI2), Sensor::Shifting);
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.icon(Size::Row).bits, b.icon(Size::Row).bits);
            }
        }
    }

    #[test]
    fn painted_rows_blend_like_the_per_pixel_icon() {
        for icon in icons(&ART_1) {
            for background in [BLACK, CARD] {
                for y in 0..usize::from(icon.size) + 2 {
                    let mut pixels = [background; 60];
                    icon.paint(&mut Row::new(y + 9, &mut pixels), 7, 9, WHITE);
                    for (x, &got) in pixels.iter().enumerate() {
                        let want = icon.pixel(x, y + 9, 7, 9, Shown(WHITE), Shown(background));
                        assert_eq!(Shown(got), want.unwrap_or(Shown(background)), "{x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_clipped_row_keeps_the_pixels_outside_its_clip() {
        let icon = Sensor::Heart.icon(Size::Large);
        let mut pixels = [BLACK; 40];
        let mut row = Row::new(17, &mut pixels);
        icon.paint(&mut row.within(17..40), 0, 0, WHITE);
        assert!(pixels[..17].iter().all(|&p| p == BLACK));
        assert!(pixels[17..].contains(&WHITE));
    }
}
