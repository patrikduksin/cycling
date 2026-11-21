use crate::screens::chrome::{CADENCE, GRADE, HEART, POWER, SPEED, WHITE};
use crate::screens::ride;
use crate::screens::workout::Text;
use core::fmt::Write;

pub trait Data {
    /// The metric's value in its base unit (see [`Metric::write`]), or none
    /// without a reading.
    fn metric(&self, metric: Metric) -> Option<i32>;
    /// The zone, from 1, that the effort sits in.
    fn zone(&self, effort: Effort) -> Option<u8>;
    fn list(&self, list: ListKind) -> u16;
    /// Where the effort's gliding reading sits on its zone gauge.
    fn gauge(&self, _: Effort) -> Option<OnGauge> {
        None
    }
    /// The range a workout step asks of the effort, on its gauge.
    fn target(&self, _: Effort) -> Option<[OnGauge; 2]> {
        None
    }
    /// The metric's reading `ago` seconds before this one, in its base
    /// unit; none where the ride has none.
    fn sample(&self, _metric: Metric, _ago: u16) -> Option<i32> {
        None
    }
}

/// A point on an effort's zone gauge: the zone it falls in, from 1, and
/// `within` of the zone's `range` past the zone's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnGauge {
    pub zone: u8,
    pub within: u32,
    pub range: u32,
}

pub struct Nothing;

impl Data for Nothing {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }

    fn list(&self, _: ListKind) -> u16 {
        0
    }
}

/// Every number a page shows. The first nine are the readings the main
/// page's lower line offers, by the id the preference record stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    /// Revolutions a minute.
    Cadence,
    /// Metres.
    Distance,
    /// Watts.
    NormalizedPower,
    /// Watts.
    AvgPower,
    /// Beats a minute.
    AvgHeart,
    /// Millimetres a second.
    AvgSpeed,
    Kcal,
    /// Kilojoules.
    Work,
    /// Metres.
    Ascent,
    /// Millimetres a second.
    Speed,
    /// Watts.
    Power,
    /// Beats a minute.
    Heart,
    /// Seconds.
    Elapsed,
    /// Tenths of a percent.
    Grade,
}

impl Metric {
    pub const ALL: [Self; 14] = [
        Self::Cadence,
        Self::Distance,
        Self::NormalizedPower,
        Self::AvgPower,
        Self::AvgHeart,
        Self::AvgSpeed,
        Self::Kcal,
        Self::Work,
        Self::Ascent,
        Self::Speed,
        Self::Power,
        Self::Heart,
        Self::Elapsed,
        Self::Grade,
    ];

    /// The readings the lower line offers, in the order Settings steps
    /// through them.
    pub const LOWER: [Self; 9] = [
        Self::Cadence,
        Self::Distance,
        Self::NormalizedPower,
        Self::AvgPower,
        Self::AvgHeart,
        Self::AvgSpeed,
        Self::Kcal,
        Self::Work,
        Self::Ascent,
    ];

    pub const fn from_id(id: u8) -> Option<Self> {
        if (id as usize) < Self::ALL.len() {
            Some(Self::ALL[id as usize])
        } else {
            None
        }
    }

    pub const fn id(self) -> u8 {
        self as u8
    }

    /// The lower-line reading after this one, wrapping.
    pub fn next(self) -> Self {
        let at = Self::LOWER.iter().position(|&m| m == self).unwrap_or(0);
        Self::LOWER[(at + 1) % Self::LOWER.len()]
    }

    /// What Settings calls it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cadence => "CADENCE",
            Self::Distance => "DISTANCE",
            Self::NormalizedPower => "NP",
            Self::AvgPower => "AVG POWER",
            Self::AvgHeart => "AVG HR",
            Self::AvgSpeed => "AVG SPEED",
            Self::Kcal => "KCAL",
            Self::Work => "WORK",
            Self::Ascent => "ASCENT",
            Self::Speed => "SPEED",
            Self::Power => "POWER",
            Self::Heart => "HEART",
            Self::Elapsed => "TIME",
            Self::Grade => "GRADE",
        }
    }

    /// The caption over the value where a page names it, as the arrival
    /// totals do. Each names the unit or needs none beside the value, but
    /// the distance, whose unit follows the value.
    pub const fn caption(self) -> &'static str {
        match self {
            Self::Elapsed => "RIDE TIME",
            Self::AvgSpeed => "AVG KM/H",
            Self::AvgPower => "AVG W",
            Self::AvgHeart => "AVG BPM",
            _ => self.name(),
        }
    }

    /// The unit written beside the value. The three averages share one,
    /// and their colour says which reading it is: the half-width slot has
    /// no room for more.
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Cadence => "RPM",
            Self::Distance => "KM",
            Self::NormalizedPower => "NP",
            Self::AvgPower | Self::AvgHeart | Self::AvgSpeed => "AVG",
            Self::Kcal => "KCAL",
            Self::Work => "KJ",
            Self::Ascent => "M",
            Self::Speed => "KM/H",
            Self::Power => "W",
            Self::Heart => "BPM",
            Self::Elapsed | Self::Grade => "",
        }
    }

    /// The unit beside a captioned value: only where the caption does not
    /// name it.
    pub const fn captioned_unit(self) -> &'static str {
        match self {
            Self::Distance => self.unit(),
            _ => "",
        }
    }

    pub const fn color(self) -> u16 {
        match self {
            Self::Cadence => CADENCE,
            Self::Distance | Self::Kcal | Self::Work | Self::Elapsed => WHITE,
            Self::NormalizedPower | Self::AvgPower | Self::Power => POWER,
            Self::AvgHeart | Self::Heart => HEART,
            Self::AvgSpeed | Self::Speed => SPEED,
            Self::Ascent | Self::Grade => GRADE,
        }
    }

    /// The reading's name in a trace's shown line, if it has one there.
    pub const fn shown(self) -> Option<&'static str> {
        match self {
            Self::Cadence => Some("cad"),
            Self::Distance => Some("dist"),
            Self::Speed => Some("speed"),
            Self::Power => Some("power"),
            Self::Heart => Some("hr"),
            Self::Elapsed => Some("time"),
            Self::Grade => Some("grade"),
            _ => None,
        }
    }

    /// The lowest and highest reading `write` shows, in the reading's own
    /// unit. A reading past either end shows as that end. A reading sheds
    /// precision before it needs a wider look (see `turn`); one with none
    /// to shed, as whole watts past 999, has its looks measured on its end.
    pub(crate) const fn ends(self) -> (i32, i32) {
        match self {
            Self::Speed | Self::AvgSpeed => (0, crate::ride::gps_speed::MAX_SPEED_MM_S as i32),
            Self::Power | Self::AvgPower | Self::NormalizedPower => (0, 9999),
            Self::Heart | Self::AvgHeart | Self::Cadence => (0, 999),
            Self::Distance => (0, 9_999_900),
            Self::Elapsed => (0, 99 * 3600 + 59 * 60 + 59),
            Self::Grade => (-999, 999),
            Self::Kcal => (0, 999_499),
            Self::Work | Self::Ascent => (0, 99_999),
        }
    }

    /// The last reading `write` shows in its first format, for a reading
    /// that changes format before its high end: speed sheds its decimal
    /// from 100 km/h, the distance one of its two from 100 km, and the kcal
    /// count goes to thousands past 9999.
    pub(crate) const fn turn(self) -> Option<i32> {
        match self {
            Self::Speed | Self::AvgSpeed => Some(SPEED_TENTHS_MOST),
            Self::Distance => Some(99_994),
            Self::Kcal => Some(WHOLE),
            _ => None,
        }
    }

    /// The texts that bound what `write` shows: the dash of no reading,
    /// the two ends, and the last reading before a change of format. Every
    /// digit has one advance in every face, so no reading draws wider than
    /// the widest of them.
    pub fn extremes(self) -> [Text<12>; 4] {
        let (low, high) = self.ends();
        let mut texts = [Text::new(), Text::new(), Text::new(), Text::new()];
        self.write(None, &mut texts[0]);
        self.write(Some(low), &mut texts[1]);
        self.write(Some(high), &mut texts[2]);
        self.write(Some(self.turn().unwrap_or(high)), &mut texts[3]);
        texts
    }

    pub fn write(self, value: Option<i32>, out: &mut Text<12>) {
        let (low, high) = self.ends();
        let value = value.map(|value| value.clamp(low, high));
        let whole = |out: &mut Text<12>| ride::whole(out, value);
        let unsigned = value.map(|value| value.max(0) as u32);
        match self {
            Self::Speed | Self::AvgSpeed => match unsigned {
                Some(mm_s) if mm_s > SPEED_TENTHS_MOST as u32 => {
                    let _ = write!(out, "{}", u64::from(mm_s) * 36 / 10_000);
                }
                _ => ride::speed(out, unsigned),
            },
            Self::Distance => ride::kilometres(out, u64::from(unsigned.unwrap_or(0)) * 1000),
            Self::Elapsed => ride::elapsed(out, u64::from(unsigned.unwrap_or(0))),
            Self::Grade => ride::grade(out, value),
            Self::Kcal => thousands(out, unsigned),
            Self::Cadence
            | Self::NormalizedPower
            | Self::AvgPower
            | Self::AvgHeart
            | Self::Work
            | Self::Ascent
            | Self::Power
            | Self::Heart => whole(out),
        }
    }
}

const WHOLE: i32 = 9_999;

/// The fastest speed, in mm/s, shown with its tenth: 99.9 km/h. From
/// 100 km/h speed reads in whole km/h, so it never needs a wider look.
const SPEED_TENTHS_MOST: i32 = (1_000_000 - 1) / 36;

/// A count that must fit four digits: whole up to 9999, then to the nearest
/// thousand ("10K") so a long ride never shows a cut-off number.
fn thousands(value: &mut Text<12>, reading: Option<u32>) {
    match reading {
        Some(count) if count > WHOLE as u32 => {
            let _ = write!(value, "{}K", count.saturating_add(500) / 1000);
        }
        _ => ride::whole(value, reading),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effort {
    Power,
    Heart,
}

impl Effort {
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Power),
            1 => Some(Self::Heart),
            _ => None,
        }
    }

    pub const fn id(self) -> u8 {
        self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Power => "POWER",
            Self::Heart => "HEART",
        }
    }

    pub const fn zones(self) -> u8 {
        match self {
            Self::Power => 7,
            Self::Heart => 5,
        }
    }

    pub const fn color(self) -> u16 {
        match self {
            Self::Power => POWER,
            Self::Heart => HEART,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Rides,
    Workouts,
    Settings,
    Sensors,
}

impl ListKind {
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Rides),
            1 => Some(Self::Workouts),
            2 => Some(Self::Settings),
            3 => Some(Self::Sensors),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panels::ALL;
    use firmware_shell::rendering::font::{Face, FaceRole};
    use std::vec::Vec;

    /// Every reading across each metric's whole range draws no wider, in
    /// any face a reading takes on any panel, than the widest of the texts
    /// its looks are measured on.
    #[test]
    fn no_reading_draws_wider_than_its_extremes() {
        let roles = [
            FaceRole::Hero,
            FaceRole::Large,
            FaceRole::Value,
            FaceRole::Body,
            FaceRole::Caption,
        ];
        let mut faces: Vec<&'static Face> = Vec::new();
        for layout in ALL {
            for role in roles {
                let face = layout.scaled.face(role);
                if !faces.iter().any(|seen| core::ptr::eq(*seen, face)) {
                    faces.push(face);
                }
            }
        }
        for metric in Metric::ALL {
            let extremes = metric.extremes();
            let bounds: Vec<usize> = faces
                .iter()
                .map(|face| {
                    let widths = extremes.iter().map(|text| face.width(text.as_bytes()));
                    widths.max().unwrap()
                })
                .collect();
            let (low, high) = metric.ends();
            for value in low..=high {
                let mut text = Text::<12>::new();
                metric.write(Some(value), &mut text);
                for (face, &bound) in faces.iter().zip(&bounds) {
                    let width = face.width(text.as_bytes());
                    assert!(
                        width <= bound,
                        "{metric:?} {value} reads {:?}, {width} px past {bound}",
                        text.as_str()
                    );
                }
            }
        }
    }
}
