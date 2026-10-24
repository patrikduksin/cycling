//! The readings the rider chose for the main page's lower line.
//!
//! The line has two slots, left and right. Each holds one [`Reading`], a
//! fixed set with a stored id (see `app::preferences`). The defaults are the
//! cadence and the distance the page always showed.
use super::{kilometres, speed, whole};
use crate::screens::chrome::{CADENCE, GRADE, HEART, POWER, SPEED, WHITE};
use crate::screens::workout::{Text, View};
use core::fmt::Write;

/// Bytes the slots take in a record: one id per slot.
pub const LENGTH: usize = 2;

/// A count that must fit four digits: whole up to 9999, then to the nearest
/// thousand ("10K") so a long ride never shows a cut-off number.
fn thousands(value: &mut Text<12>, reading: Option<u32>) {
    match reading {
        Some(count) if count > 9_999 => {
            let _ = write!(value, "{}K", count.saturating_add(500) / 1000);
        }
        _ => whole(value, reading),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    Cadence,
    Distance,
    NormalizedPower,
    AvgPower,
    AvgHeart,
    AvgSpeed,
    Kcal,
    Work,
    Ascent,
}

impl Reading {
    /// Every reading, in the order Settings steps through them and by stored id.
    pub const ALL: [Self; 9] = [
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

    pub const fn id(self) -> u8 {
        match self {
            Self::Cadence => 0,
            Self::Distance => 1,
            Self::NormalizedPower => 2,
            Self::AvgPower => 3,
            Self::AvgHeart => 4,
            Self::AvgSpeed => 5,
            Self::Kcal => 6,
            Self::Work => 7,
            Self::Ascent => 8,
        }
    }

    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Cadence),
            1 => Some(Self::Distance),
            2 => Some(Self::NormalizedPower),
            3 => Some(Self::AvgPower),
            4 => Some(Self::AvgHeart),
            5 => Some(Self::AvgSpeed),
            6 => Some(Self::Kcal),
            7 => Some(Self::Work),
            8 => Some(Self::Ascent),
            9..=u8::MAX => None,
        }
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
        }
    }

    /// The unit written beside the value on the page. The three averages
    /// share one, and their colour says which reading it is: the half-width
    /// slot has no room for more.
    pub const fn unit(self) -> &'static [u8] {
        match self {
            Self::Cadence => b"RPM",
            Self::Distance => b"KM",
            Self::NormalizedPower => b"NP",
            Self::AvgPower | Self::AvgHeart | Self::AvgSpeed => b"AVG",
            Self::Kcal => b"KCAL",
            Self::Work => b"KJ",
            Self::Ascent => b"M",
        }
    }

    pub const fn color(self) -> u16 {
        match self {
            Self::Cadence => CADENCE,
            Self::Distance | Self::Kcal | Self::Work => WHITE,
            Self::NormalizedPower | Self::AvgPower => POWER,
            Self::AvgHeart => HEART,
            Self::AvgSpeed => SPEED,
            Self::Ascent => GRADE,
        }
    }

    /// The reading's value for this frame, or "--" without one.
    pub fn write(self, view: &View, value: &mut Text<12>) {
        let totals = &view.totals;
        match self {
            Self::Cadence => whole(
                value,
                view.sample.cadence_tenths.map(|tenths| (tenths + 5) / 10),
            ),
            Self::Distance => kilometres(value, totals.distance_mm()),
            Self::NormalizedPower => whole(value, totals.normalized_power()),
            Self::AvgPower => whole(value, totals.avg_power()),
            Self::AvgHeart => whole(value, totals.avg_heart_bpm()),
            Self::AvgSpeed => speed(value, totals.avg_speed_mm_s()),
            Self::Kcal => thousands(value, totals.kcal_for(view.rider)),
            Self::Work => whole(value, totals.work_kj()),
            Self::Ascent => whole(value, Some((totals.ascent_dm() + 5) / 10)),
        }
    }

    /// The reading after this one, wrapping.
    pub fn next(self) -> Self {
        let at = usize::from(self.id());
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// Which reading each slot of the lower line shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slots {
    pub left: Reading,
    pub right: Reading,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            left: Reading::Cadence,
            right: Reading::Distance,
        }
    }
}

impl Slots {
    pub const fn encode(self) -> [u8; LENGTH] {
        [self.left.id(), self.right.id()]
    }

    /// The slots a record's bytes stand for; none when an id means nothing.
    pub const fn decode([left, right]: [u8; LENGTH]) -> Option<Self> {
        match (Reading::from_id(left), Reading::from_id(right)) {
            (Some(left), Some(right)) => Some(Self { left, right }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_the_position_in_all_and_round_trip() {
        for (at, reading) in Reading::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(reading.id()), at);
            assert_eq!(Reading::from_id(reading.id()), Some(reading));
        }
        for id in Reading::ALL.len() as u8..=u8::MAX {
            assert_eq!(Reading::from_id(id), None);
        }
    }

    #[test]
    fn next_visits_every_reading_once_and_wraps() {
        let mut seen = std::vec::Vec::new();
        let mut reading = Reading::Cadence;
        for _ in 0..Reading::ALL.len() {
            seen.push(reading);
            reading = reading.next();
        }
        assert_eq!(reading, Reading::Cadence);
        assert_eq!(seen, Reading::ALL);
    }

    #[test]
    fn a_count_past_four_digits_reads_in_thousands() {
        let text = |count| {
            let mut value = Text::<12>::new();
            thousands(&mut value, count);
            value
        };
        assert_eq!(text(None).as_str(), "--");
        assert_eq!(text(Some(9_999)).as_str(), "9999");
        assert_eq!(text(Some(10_000)).as_str(), "10K");
        assert_eq!(text(Some(10_499)).as_str(), "10K");
        assert_eq!(text(Some(10_500)).as_str(), "11K");
        assert_eq!(text(Some(u32::MAX)).as_str(), "4294967K");
    }

    #[test]
    fn the_defaults_are_the_cadence_and_distance_the_page_always_had() {
        assert_eq!(Slots::default().encode(), [0, 1]);
        assert_eq!(Slots::decode([0, 1]), Some(Slots::default()));
        assert_eq!(Slots::decode([9, 1]), None);
        assert_eq!(Slots::decode([0, 255]), None);
    }
}
