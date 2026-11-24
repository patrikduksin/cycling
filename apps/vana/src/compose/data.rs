use crate::screens::chrome::{Battery, CADENCE, GRADE, Gps, HEART, POWER, SECONDARY, SPEED, WHITE};
use crate::screens::ride::{self, ClimbRoad as Road, Training};
use crate::screens::theme::{self, Theme};
use crate::screens::workout::Text;
use core::fmt::Write;

pub trait Data {
    /// The metric's value in its base unit (see [`Metric::write`]), or none
    /// without a reading.
    fn metric(&self, metric: Metric) -> Option<i32>;
    /// The zone, from 1, that the effort sits in.
    fn zone(&self, effort: Effort) -> Option<u8>;
    /// The sensor pages, while one shows.
    fn sensors(&self) -> Option<&crate::screens::sensors::Menu> {
        None
    }
    /// Where the effort's gliding reading sits on its zone gauge.
    fn gauge(&self, _: Effort) -> Option<OnGauge> {
        None
    }
    /// The ride's time in each of the effort's zones, none before any.
    fn zone_time(&self, _: Effort) -> Option<Shares> {
        None
    }
    /// The ride paused itself and resumes when it moves.
    fn auto_paused(&self) -> bool {
        false
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
    /// The road the climb page shows, and its chart for this frame.
    fn road(&self) -> Option<Road<'_>> {
        None
    }
    /// The workout chosen or running.
    fn training(&self) -> Option<&Training> {
        None
    }
    /// The ride is recording, not ready or paused.
    fn recording(&self) -> bool {
        false
    }
    /// A number a static screen states, or none where it has none.
    fn count(&self, _: Count) -> Option<i32> {
        None
    }
    /// The map as last filled, on the map page.
    fn map(&self) -> Option<&ride::MapFrame> {
        None
    }
    /// What a static page shows besides numbers.
    fn page(&self) -> PageData<'_> {
        PageData::None
    }
    /// What Ride done shows, while it shows.
    fn done(&self) -> Option<&crate::screens::done::Done> {
        None
    }
    /// The swipe-down sheet's brightness, look and sensors, while it shows.
    fn sheet(&self) -> Option<&crate::screens::sheet::Facts> {
        None
    }
    /// The look in force, whose colours a kit tile draws in.
    fn theme(&self) -> &'static Theme {
        &theme::DARK
    }
    /// The fix and the battery, as a ride page's top line shows them.
    fn status(&self) -> Status {
        Status::default()
    }
    fn clock(&self) -> &str {
        ""
    }
}

/// The fix and the battery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub gps: Gps,
    pub battery: Option<Battery>,
}

/// What a tile reads, for a trace's shown line: a metric, a card's
/// metrics, or whatever the workout step targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    Metric(Metric),
    Card(crate::compose::kit::facts::Card),
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Count {
    /// Rides the phone has not confirmed.
    Unsaved,
    /// What formatting takes, as its place in `format::Goes::ALL`.
    Goes,
    /// Seconds left before the idle warning turns VANA off.
    Seconds,
    /// Thousandths of the way through the farewell.
    Farewell,
    /// The battery's charge in percent, none until it reports; how far the
    /// charging arc has climbed; 1 while it charges.
    Charge,
    ChargeFill,
    Charging,
}

/// A point on an effort's zone gauge: the zone it falls in, from 1, and
/// `within` of the zone's `range` past the zone's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnGauge {
    pub zone: u8,
    pub within: u32,
    pub range: u32,
}

/// Where each zone's share of a whole ends, in [`Shares::WHOLE`] parts
/// from its start: zone 1 first, the zones the effort lacks at the whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shares(pub [u16; 7]);

impl Shares {
    pub const WHOLE: u16 = 1000;
}

pub struct Nothing;

impl Data for Nothing {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
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
    /// Seconds moving.
    Moving,
    /// Millimetres a second.
    MaxSpeed,
    /// Hundredths of the intensity factor.
    Intensity,
    /// The training stress score.
    Stress,
    /// Seconds the ride has been paused.
    Paused,
    /// Metres above the sea.
    Altitude,
    /// Metres of the barometer's climb under way, so far.
    ClimbDistance,
    /// Metres gained on the barometer's climb under way.
    ClimbGain,
    /// Metres to the top of the map's climb.
    ToTop,
    /// Metres of height left to the top of the map's climb.
    UpToGo,
}

impl Metric {
    pub const ALL: [Self; 24] = [
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
        Self::Moving,
        Self::MaxSpeed,
        Self::Intensity,
        Self::Stress,
        Self::Paused,
        Self::Altitude,
        Self::ClimbDistance,
        Self::ClimbGain,
        Self::ToTop,
        Self::UpToGo,
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

    /// What a caption calls it.
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
            Self::Moving => "MOVING",
            Self::MaxSpeed => "MAX SPEED",
            Self::Intensity => "IF",
            Self::Stress => "TSS",
            Self::Paused => "PAUSED",
            Self::Altitude => "ALTITUDE",
            Self::ClimbDistance => "CLIMB DIST",
            Self::ClimbGain => "CLIMB ELEV",
            Self::ToTop => "DIST TO TOP",
            Self::UpToGo => "ELEV TO TOP",
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
            Self::Ascent | Self::Altitude | Self::ClimbGain | Self::UpToGo => "M",
            Self::ClimbDistance | Self::ToTop => "KM",
            Self::Speed => "KM/H",
            Self::Power => "W",
            Self::Heart => "BPM",
            Self::MaxSpeed => "KM/H",
            Self::Stress => "TSS",
            Self::Elapsed | Self::Grade | Self::Moving | Self::Intensity | Self::Paused => "",
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
            Self::Distance
            | Self::Kcal
            | Self::Work
            | Self::Elapsed
            | Self::Moving
            | Self::Altitude
            | Self::ClimbDistance
            | Self::ClimbGain
            | Self::ToTop
            | Self::UpToGo => WHITE,
            Self::NormalizedPower
            | Self::AvgPower
            | Self::Power
            | Self::Intensity
            | Self::Stress => POWER,
            Self::AvgHeart | Self::Heart => HEART,
            Self::AvgSpeed | Self::Speed | Self::MaxSpeed => SPEED,
            Self::Ascent | Self::Grade => GRADE,
            Self::Paused => SECONDARY,
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed => {
                (0, crate::ride::gps_speed::MAX_SPEED_MM_S as i32)
            }
            Self::Power | Self::AvgPower | Self::NormalizedPower | Self::Stress => (0, 9999),
            Self::Heart | Self::AvgHeart | Self::Cadence => (0, 999),
            Self::Distance => (0, 9_999_900),
            Self::ClimbDistance | Self::ToTop => (0, 999_900),
            Self::Altitude => (-999, 9999),
            Self::ClimbGain | Self::UpToGo => (0, 9999),
            Self::Elapsed | Self::Moving | Self::Paused => (0, 99 * 3600 + 59 * 60 + 59),
            Self::Grade => (-999, 999),
            Self::Intensity => (0, 999),
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed => Some(SPEED_TENTHS_MOST),
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed => match unsigned {
                Some(mm_s) if mm_s > SPEED_TENTHS_MOST as u32 => {
                    let _ = write!(out, "{}", u64::from(mm_s) * 36 / 10_000);
                }
                _ => ride::speed(out, unsigned),
            },
            Self::Distance => ride::kilometres(out, u64::from(unsigned.unwrap_or(0)) * 1000),
            Self::ClimbDistance | Self::ToTop => match unsigned {
                Some(metres) => ride::kilometres(out, u64::from(metres) * 1000),
                None => whole(out),
            },
            Self::Elapsed => ride::elapsed(out, u64::from(unsigned.unwrap_or(0))),
            Self::Moving | Self::Paused => match unsigned {
                Some(seconds) => ride::lap_time(out, u64::from(seconds)),
                None => whole(out),
            },
            Self::Grade => ride::grade(out, value),
            Self::Intensity => hundredths(out, unsigned),
            Self::Kcal => thousands(out, unsigned),
            Self::Cadence
            | Self::NormalizedPower
            | Self::AvgPower
            | Self::AvgHeart
            | Self::Work
            | Self::Ascent
            | Self::Power
            | Self::Heart
            | Self::Stress
            | Self::Altitude
            | Self::ClimbGain
            | Self::UpToGo => whole(out),
        }
    }
}

const WHOLE: i32 = 9_999;

/// The fastest speed, in mm/s, shown with its tenth: 99.9 km/h. From
/// 100 km/h speed reads in whole km/h, so it never needs a wider look.
const SPEED_TENTHS_MOST: i32 = (1_000_000 - 1) / 36;

fn hundredths(out: &mut Text<12>, value: Option<u32>) {
    let _ = match value {
        Some(v) => write!(out, "{}.{:02}", v / 100, v % 100),
        None => out.push_str("--"),
    };
}

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
        for device in ALL {
            for role in roles {
                let face = device.scaled.face(role);
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

/// A static page's words and state, as the runtime prepared them for the
/// frame.
#[derive(Clone, Copy)]
pub enum PageData<'a> {
    None,
    Home(&'a crate::screens::home::Home),
    /// The reveal's progress, in thousandths.
    Brand(u16),
}
