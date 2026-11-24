//! The two looks as data: one `Theme` shape with a night instance and a
//! day instance, and one `LAYOUT` of gaps, radii and faces that both share.
//! A kit tile reads every colour from the theme in force (`Data::theme`)
//! and every gap, radius and face from `LAYOUT`, and never names one
//! itself, so both looks run one code path and solve to the same boxes.
mod dark;
mod layout;
mod light;

pub use dark::DARK;
pub use layout::LAYOUT;
pub use light::LIGHT;

use crate::compose::data::Effort;
use firmware_shell::rendering::font::FaceRole;

/// RGB565 from 8-bit channels.
pub const fn rgb(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// A look's colours by role, RGB565.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Theme {
    pub bg: u16,
    /// Words and plain numbers.
    pub text: u16,
    /// Labels and secondary words.
    pub sub: u16,
    /// Lines between fields and list rows.
    pub rule: u16,
    /// A grouped list's card.
    pub card: u16,
    /// Buttons, and a focused card's inside.
    pub raised: u16,
    /// Unlit segments and dots.
    pub off: u16,
    /// The focus ring.
    pub focus: u16,
    /// Ink on a zone or steepness fill.
    pub on_fill: u16,
    /// Ink on a fill of a state tone, such as a solid button.
    pub on_tone: u16,
    pub speed: u16,
    pub power: u16,
    pub heart: u16,
    pub cadence: u16,
    pub grade: u16,
    /// Live, fixed, go.
    pub good: u16,
    /// Paused, low, finding.
    pub warn: u16,
    /// Recording, danger, failure.
    pub bad: u16,
    pub info: u16,
    /// Zone bars and charts, Z1 to Z7.
    pub zone: [u16; 7],
    /// A field filled with its zone or steepness, under `on_fill` ink.
    pub zone_fill: [u16; 7],
}

/// What a word, number or mark means, which picks its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    Text,
    Sub,
    Speed,
    Power,
    Heart,
    Cadence,
    Grade,
    Good,
    Warn,
    Bad,
    Info,
}

impl Theme {
    pub const fn tone(&self, tone: Tone) -> u16 {
        match tone {
            Tone::Text => self.text,
            Tone::Sub => self.sub,
            Tone::Speed => self.speed,
            Tone::Power => self.power,
            Tone::Heart => self.heart,
            Tone::Cadence => self.cadence,
            Tone::Grade => self.grade,
            Tone::Good => self.good,
            Tone::Warn => self.warn,
            Tone::Bad => self.bad,
            Tone::Info => self.info,
        }
    }

    /// The fill of zone `zone`, from 1, of `effort` and the ink on it; none
    /// outside the effort's zones.
    pub const fn fill(&self, effort: Effort, zone: u8) -> Option<(u16, u16)> {
        if zone == 0 || zone > effort.zones() {
            return None;
        }
        Some((self.zone_fill[zone as usize - 1], self.on_fill))
    }

    /// The fill of a grade in tenths of a percent, and the ink on it: the
    /// zone fills from Z2 up.
    pub const fn steep(&self, tenths: i32) -> (u16, u16) {
        let band = match tenths {
            ..-15 => 1,
            -15..30 => 2,
            30..60 => 3,
            60..90 => 4,
            90..120 => 5,
            _ => 6,
        };
        (self.zone_fill[band], self.on_fill)
    }

    /// The bar colour of zone `zone`, from 1, or the unlit colour.
    pub const fn zone(&self, zone: u8) -> u16 {
        if zone == 0 || zone as usize > self.zone.len() {
            self.off
        } else {
            self.zone[zone as usize - 1]
        }
    }
}

/// Gaps, radii and faces both looks share, so a layout solves once.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub space: Space,
    pub radius: Radius,
    pub face: Type,
}

/// Gaps in scaled units.
#[derive(Debug, PartialEq, Eq)]
pub struct Space {
    /// A field's or row's inset from its side.
    pub gutter: u8,
    /// A field's inset from its top and bottom.
    pub pad: u8,
    /// Between neighbours in a group.
    pub gap: u8,
    /// Between an icon and its words.
    pub tight: u8,
    /// A rule's thickness.
    pub rule: u8,
    /// The ride pages' top line.
    pub top_line: u8,
    /// The recording dot's diameter.
    pub dot: u8,
    /// A grouped list's row.
    pub row: u8,
    /// A button's height.
    pub button: u8,
    /// A focus ring's width.
    pub ring: u8,
    /// A zone bar's height.
    pub bar: u8,
    /// A chart's outline, and the ring around its marker.
    pub stroke: u8,
    /// The rider's marker on a chart, across.
    pub marker: u8,
    /// The fewest rows a chart keeps.
    pub chart: u8,
}

/// Corner radii in scaled units.
#[derive(Debug, PartialEq, Eq)]
pub struct Radius {
    pub button: u8,
    pub card: u8,
    pub bar: u8,
}

/// The face each kind of text takes.
#[derive(Debug, PartialEq, Eq)]
pub struct Type {
    /// Capital titles over a field, zone tags, the battery.
    pub tag: FaceRole,
    /// A ride field's number.
    pub digits: FaceRole,
    /// The % beside `digits`, and a field's look when `digits` does not fit.
    pub digits_small: FaceRole,
    /// A field's number where a chart shares the page.
    pub digits_mid: FaceRole,
    /// The % beside `digits_mid`.
    pub percent_mid: FaceRole,
    /// A chart's summary numbers.
    pub summary: FaceRole,
    /// The top line's time.
    pub line: FaceRole,
    /// A phone-style screen's title.
    pub heading: FaceRole,
    /// List values and button labels.
    pub label: FaceRole,
    /// Subtitles and list labels.
    pub note: FaceRole,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome;

    #[test]
    fn night_is_main_s_palette() {
        let t = &DARK;
        assert_eq!(
            [
                t.bg, t.text, t.sub, t.rule, t.card, t.raised, t.off, t.focus
            ],
            [
                chrome::BLACK,
                chrome::WHITE,
                chrome::SECONDARY,
                chrome::PRESSED,
                chrome::CARD,
                chrome::RAISED,
                chrome::TERTIARY,
                chrome::ACCENT
            ]
        );
        assert_eq!(
            [t.speed, t.power, t.heart, t.cadence, t.grade],
            [
                chrome::SPEED,
                chrome::POWER,
                chrome::HEART,
                chrome::CADENCE,
                chrome::GRADE
            ]
        );
        assert_eq!(
            [t.good, t.warn, t.bad, t.info],
            [chrome::GREEN, chrome::AMBER, chrome::RED, chrome::ACCENT]
        );
        assert_eq!(t.zone, chrome::ZONES);
        assert_eq!(t.zone_fill, chrome::ZONES);
    }

    #[test]
    fn day_writes_every_word_black_and_fills_as_bright_as_night() {
        let black = LIGHT.text;
        for tone in [
            Tone::Text,
            Tone::Speed,
            Tone::Power,
            Tone::Heart,
            Tone::Cadence,
            Tone::Grade,
        ] {
            assert_eq!(LIGHT.tone(tone), black, "{tone:?}");
        }
        assert_eq!(LIGHT.on_fill, black);
        assert_eq!(LIGHT.bg, rgb(0xff, 0xff, 0xff));
        assert_eq!(LIGHT.zone_fill, DARK.zone_fill);
    }

    #[test]
    fn steepness_fills_follow_main_s_climb_bands() {
        let band = |tenths| DARK.steep(tenths).0;
        assert_eq!(band(-16), DARK.zone_fill[1]);
        assert_eq!(band(-15), DARK.zone_fill[2]);
        assert_eq!(band(29), DARK.zone_fill[2]);
        assert_eq!(band(30), DARK.zone_fill[3]);
        assert_eq!(band(119), DARK.zone_fill[5]);
        assert_eq!(band(120), DARK.zone_fill[6]);
    }
}
