use crate::screens::chrome::{CADENCE, GRADE, HEART, POWER, SPEED, WHITE};
use crate::screens::workout::Text;
use core::fmt::Write;

pub trait Data {
    /// The metric's value in its base unit (see [`Metric::write`]), or none
    /// without a reading.
    fn metric(&self, metric: Metric) -> Option<i32>;
    /// The zone, from 1, that the effort sits in.
    fn zone(&self, effort: Effort) -> Option<u8>;
    fn list(&self, list: ListKind) -> u16;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    /// Millimetres a second.
    Speed,
    /// Watts.
    Power,
    /// Beats a minute.
    Heart,
    /// Revolutions a minute.
    Cadence,
    /// Metres.
    Distance,
    /// Seconds.
    Elapsed,
    /// Tenths of a percent.
    Grade,
}

impl Metric {
    pub const ALL: [Self; 7] = [
        Self::Speed,
        Self::Power,
        Self::Heart,
        Self::Cadence,
        Self::Distance,
        Self::Elapsed,
        Self::Grade,
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

    pub const fn name(self) -> &'static str {
        match self {
            Self::Speed => "SPEED",
            Self::Power => "POWER",
            Self::Heart => "HEART",
            Self::Cadence => "CADENCE",
            Self::Distance => "DISTANCE",
            Self::Elapsed => "TIME",
            Self::Grade => "GRADE",
        }
    }

    pub const fn unit(self) -> &'static str {
        match self {
            Self::Speed => "KM/H",
            Self::Power => "W",
            Self::Heart => "BPM",
            Self::Cadence => "RPM",
            Self::Distance => "KM",
            Self::Elapsed => "",
            Self::Grade => "%",
        }
    }

    pub const fn color(self) -> u16 {
        match self {
            Self::Speed => SPEED,
            Self::Power => POWER,
            Self::Heart => HEART,
            Self::Cadence => CADENCE,
            Self::Distance | Self::Elapsed => WHITE,
            Self::Grade => GRADE,
        }
    }

    /// The lowest and highest number `write` shows, in the unit it shows
    /// (tenths for speed, distance and grade). A reading past either end
    /// shows as that end.
    const fn ends(self) -> (i64, i64) {
        match self {
            Self::Speed => (
                0,
                (crate::ride::gps_speed::MAX_SPEED_MM_S as i64 * 36 + 500) / 1000,
            ),
            Self::Power => (0, 9999),
            Self::Heart | Self::Cadence => (0, 999),
            Self::Distance => (0, 99_999),
            Self::Elapsed => (0, 99 * 3600 + 59 * 60 + 59),
            Self::Grade => (-999, 999),
        }
    }

    /// The texts that bound what `write` shows: the dash of no reading and
    /// the two ends. Every digit has one advance in every face, so no
    /// reading draws wider than the widest of them.
    pub fn extremes(self) -> [Text<12>; 3] {
        let (low, high) = self.ends();
        let mut texts = [Text::new(), Text::new(), Text::new()];
        self.write(None, &mut texts[0]);
        self.show(low, &mut texts[1]);
        self.show(high, &mut texts[2]);
        texts
    }

    pub fn write(self, value: Option<i32>, out: &mut Text<12>) {
        let Some(value) = value else {
            let _ = out.write_str("--");
            return;
        };
        let value = i64::from(value);
        let shown = match self {
            Self::Speed => (value * 36 + 500).div_euclid(1000),
            Self::Distance => (value + 50).div_euclid(100),
            Self::Power | Self::Heart | Self::Cadence | Self::Elapsed | Self::Grade => value,
        };
        let (low, high) = self.ends();
        self.show(shown.clamp(low, high), out);
    }

    fn show(self, shown: i64, out: &mut Text<12>) {
        let _ = match self {
            Self::Speed | Self::Distance | Self::Grade => tenths(out, shown),
            Self::Elapsed => write!(
                out,
                "{}:{:02}:{:02}",
                shown / 3600,
                shown / 60 % 60,
                shown % 60
            ),
            Self::Power | Self::Heart | Self::Cadence => write!(out, "{shown}"),
        };
    }
}

fn tenths(out: &mut Text<12>, tenths: i64) -> core::fmt::Result {
    let sign = if tenths < 0 { "-" } else { "" };
    let tenths = tenths.unsigned_abs();
    write!(out, "{sign}{}.{}", tenths / 10, tenths % 10)
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
