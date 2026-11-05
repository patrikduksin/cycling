//! Anti-aliased icons from pre-rasterized 4-bit coverage.
//!
//! Each icon is rasterized at every size a screen draws it (`apps/vana/icons`)
//! and painted like a glyph run: one colour blended over what the row already
//! holds, so the same icon sits on black, a card or a focused card.
mod art;

use super::chrome::{Rect, Row, blend};
use crate::sensors::ant::device_type;
pub use art::{ARROW, PLUS, REFRESH, Sensor};

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

    /// The icon in `color` at (`x`, `y`) with its square's top-left corner at
    /// (`left`, `top`), over `background`; `None` outside its ink.
    pub fn pixel(
        &self,
        x: usize,
        y: usize,
        left: usize,
        top: usize,
        color: u16,
        background: u16,
    ) -> Option<u16> {
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
        let bits = &self.bits[y * self.stride()..][..self.stride()];
        let pixels = row.pixels_mut();
        for column in 0..usize::from(self.width) {
            let x = start + column;
            if x < clip.start {
                continue;
            }
            let Some(pixel) = pixels.get_mut(x) else {
                break;
            };
            let byte = bits[column / 2];
            let coverage = if column.is_multiple_of(2) {
                byte >> 4
            } else {
                byte & 0x0f
            };
            match coverage {
                0 => {}
                15 => *pixel = color,
                coverage => *pixel = blend(color, *pixel, coverage),
            }
        }
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
        let set = match size {
            Size::Small => &art::SENSORS_22,
            Size::Row => &art::SENSORS_26,
            Size::Large => &art::SENSORS_34,
        };
        &set[self as usize]
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
    use crate::screens::chrome::{BLACK, CARD, WHITE};

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

    fn icons() -> impl Iterator<Item = &'static Icon> {
        ALL.into_iter()
            .flat_map(|sensor| [Size::Small, Size::Row, Size::Large].map(|size| sensor.icon(size)))
            .chain([&PLUS, &REFRESH, &ARROW])
    }

    #[test]
    fn each_icon_has_solid_ink_inside_its_square() {
        for icon in icons() {
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
        for icon in icons() {
            for background in [BLACK, CARD] {
                for y in 0..usize::from(icon.size) + 2 {
                    let mut pixels = [background; 60];
                    icon.paint(&mut Row::new(y + 9, &mut pixels), 7, 9, WHITE);
                    for (x, &got) in pixels.iter().enumerate() {
                        let want = icon.pixel(x, y + 9, 7, 9, WHITE, background);
                        assert_eq!(got, want.unwrap_or(background), "{x},{y}");
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
