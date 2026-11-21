use crate::ride::week::{DAYS, Weeks};
use crate::screens::chrome::{ACCENT, CADENCE, GRADE, HEART, POWER, SECONDARY, SPEED, WHITE};
use crate::screens::ride::{self, ClimbRoad as Road, LapBar, Training};
use crate::screens::workout::Text;
use core::fmt::Write;

pub trait Data {
    /// The metric's value in its base unit (see [`Metric::write`]), or none
    /// without a reading.
    fn metric(&self, metric: Metric) -> Option<i32>;
    /// The zone, from 1, that the effort sits in.
    fn zone(&self, effort: Effort) -> Option<u8>;
    fn list(&self, list: ListKind) -> u16;
    /// How the list shows now: its scroll, its focus and the item under a
    /// finger.
    fn listed(&self, _: ListKind) -> Listed {
        Listed::REST
    }
    /// Item `index` of the list, while it is in view.
    fn entry(&self, _: ListKind, _: u16) -> Option<Entry<'_>> {
        None
    }
    /// The item a list's template draws, as the list hands it to its tiles.
    fn row(&self) -> Option<Row> {
        None
    }
    /// This week and last at a glance.
    fn glance(&self) -> Glance {
        Glance::UNKNOWN
    }
    /// What the list's heading says: its name, or the section a list of
    /// sections shows.
    fn heading(&self, list: ListKind) -> &str {
        list.name()
    }
    /// Whether what the rider changed is saved, and whether that is fine;
    /// none while a toast covers the foot.
    fn saving(&self) -> Option<(&str, bool)> {
        None
    }
    /// The number being edited.
    fn edited(&self) -> Option<Edited<'_>> {
        None
    }
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
    /// The finished laps, oldest first, and the open lap.
    fn laps(&self) -> Option<(&[LapBar], LapBar)> {
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
    /// What the summary shown adds up, on a summary page.
    fn summary(&self) -> Option<ride::summary::Facts<'_>> {
        None
    }
    /// What a static page shows besides numbers.
    fn page(&self) -> PageData<'_> {
        PageData::None
    }
}

/// What a tile reads, for a trace's shown line: a metric, or whatever the
/// workout step targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    Metric(Metric),
    Target,
}

/// The numbers the static screens state: the questions, the sheets, BESTS
/// and FORM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Count {
    /// Rides the phone has not confirmed.
    Unsaved,
    /// What formatting takes, as its place in `format::Goes::ALL`.
    Goes,
    /// Why BESTS shows a note, as its place in `bests::NOTES`.
    BestsNote,
    /// A best by its place on BESTS: its value, the day it was set, and
    /// watts a kilo in tenths for a power best.
    Best(u8),
    BestDay(u8),
    BestPerKg(u8),
    /// Why FORM shows a note, as its place in `form::NOTES`.
    FormNote,
    Fitness,
    Fatigue,
    Form,
    /// Form at the end of a week of the trend, oldest first.
    FormWeek(u8),
    /// 1 when rides without power are left out of FORM.
    LeftOut,
    /// The FTP the sheet offers, and the one in force.
    FtpNew,
    FtpOld,
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

/// A list's state as a frame shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listed {
    /// How far the page is scrolled, in pixels; negative past its top.
    pub offset: i32,
    /// The item the buttons act on.
    pub focus: Option<u16>,
    /// The item under a resting finger.
    pub pressed: Option<u16>,
}

impl Listed {
    pub const REST: Self = Self {
        offset: 0,
        focus: None,
        pressed: None,
    };
}

/// What an item of a list says: its name, a line under it, a note at the
/// right of its name in the note's colour, and whether it is the one chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub note: &'a str,
    pub note_color: u16,
    /// The note keeps its room before the title takes the rest.
    pub note_first: bool,
    pub chosen: bool,
}

/// A number being edited: what it is, its value, a note on it and the
/// step a pad moves it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edited<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub note: &'a str,
    pub step: u16,
}

/// The rides' week, or none while the clock is unknown, and whether the
/// index is still reading rides, so the week may grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glance {
    pub week: Option<Week>,
    pub reading: bool,
}

impl Glance {
    pub const UNKNOWN: Self = Self {
        week: None,
        reading: false,
    };

    pub fn of(weeks: Option<&Weeks>, reading: bool) -> Self {
        Self {
            week: weeks.map(Week::of),
            reading,
        }
    }
}

/// This week as its glance shows it, with last week's distance: a frame
/// keeps this, not the weeks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Week {
    pub rides: u32,
    pub distance_mm: u64,
    pub active_ms: u64,
    /// The stress of the rides that had power; none without one.
    pub stress: Option<u32>,
    pub last_mm: u64,
    /// Monday 0.
    pub today: u8,
    /// Each day's distance in metres, Monday first.
    pub day_m: [u32; DAYS],
    /// The days ridden, Monday the lowest bit.
    pub ridden: u8,
}

impl Week {
    pub fn of(weeks: &Weeks) -> Self {
        let this = &weeks.this;
        let mut ridden = 0;
        for (day, &rides) in this.day_rides.iter().enumerate() {
            ridden |= u8::from(rides > 0) << day;
        }
        Self {
            rides: this.rides,
            distance_mm: this.distance_mm,
            active_ms: this.active_ms,
            stress: (this.powered > 0).then_some(this.stress),
            last_mm: weeks.last.distance_mm,
            today: weeks.today as u8,
            day_m: this
                .day_mm
                .map(|mm| u32::try_from(mm / 1000).unwrap_or(u32::MAX)),
            ridden,
        }
    }

    pub const fn rode(&self, day: usize) -> bool {
        self.ridden & (1 << day) != 0
    }
}

/// The item a template's tiles draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub list: ListKind,
    pub index: u16,
    pub focused: bool,
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
    /// Seconds moving.
    Moving,
    /// Millimetres a second.
    MaxSpeed,
    /// Hundredths of the intensity factor.
    Intensity,
    /// The training stress score.
    Stress,
    /// Seconds of the open lap.
    LapTime,
    /// Millimetres.
    LapDistance,
    /// Watts.
    LapPower,
    /// Millimetres a second.
    LapSpeed,
    /// Beats a minute.
    LapHeart,
    /// The open lap's number, from 1.
    LapNumber,
    /// The last finished lap's number.
    PrevLapNumber,
    /// Seconds.
    PrevLapTime,
    /// Watts.
    PrevLapPower,
    /// The open lap's average speed less the last finished lap's, in tenths
    /// of a km/h, each side truncated as a speed reads.
    SpeedDiff,
    /// The open lap's average power less the last finished lap's, in watts.
    PowerDiff,
    /// Seconds the ride has been paused.
    Paused,
}

impl Metric {
    pub const ALL: [Self; 30] = [
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
        Self::LapTime,
        Self::LapDistance,
        Self::LapPower,
        Self::LapSpeed,
        Self::LapHeart,
        Self::LapNumber,
        Self::PrevLapNumber,
        Self::PrevLapTime,
        Self::PrevLapPower,
        Self::SpeedDiff,
        Self::PowerDiff,
        Self::Paused,
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
            Self::Moving => "MOVING",
            Self::MaxSpeed => "MAX SPEED",
            Self::Intensity => "IF",
            Self::Stress => "TSS",
            Self::LapTime => "LAP TIME",
            Self::LapDistance => "LAP KM",
            Self::LapPower => "LAP W",
            Self::LapSpeed => "LAP KM/H",
            Self::LapHeart => "LAP BPM",
            Self::LapNumber => "LAP",
            Self::PrevLapNumber => "PREV LAP",
            Self::PrevLapTime => "PREV LAP",
            Self::PrevLapPower => "PREV W",
            Self::SpeedDiff => "KM/H DIFF",
            Self::PowerDiff => "W DIFF",
            Self::Paused => "PAUSED",
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
            Self::Power | Self::LapPower | Self::PrevLapPower | Self::PowerDiff => "W",
            Self::Heart | Self::LapHeart => "BPM",
            Self::MaxSpeed | Self::LapSpeed | Self::SpeedDiff => "KM/H",
            Self::LapDistance => "KM",
            Self::Stress => "TSS",
            Self::Elapsed
            | Self::Grade
            | Self::Moving
            | Self::Intensity
            | Self::LapTime
            | Self::LapNumber
            | Self::PrevLapNumber
            | Self::PrevLapTime
            | Self::Paused => "",
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
            | Self::LapTime
            | Self::LapDistance
            | Self::PrevLapTime => WHITE,
            Self::NormalizedPower
            | Self::AvgPower
            | Self::Power
            | Self::Intensity
            | Self::Stress
            | Self::LapPower
            | Self::PrevLapPower => POWER,
            Self::AvgHeart | Self::Heart | Self::LapHeart => HEART,
            Self::AvgSpeed | Self::Speed | Self::MaxSpeed | Self::LapSpeed => SPEED,
            Self::Ascent | Self::Grade => GRADE,
            Self::LapNumber | Self::PrevLapNumber => ACCENT,
            Self::SpeedDiff | Self::PowerDiff | Self::Paused => SECONDARY,
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed | Self::LapSpeed => {
                (0, crate::ride::gps_speed::MAX_SPEED_MM_S as i32)
            }
            Self::Power
            | Self::AvgPower
            | Self::NormalizedPower
            | Self::LapPower
            | Self::PrevLapPower
            | Self::Stress
            | Self::LapNumber
            | Self::PrevLapNumber => (0, 9999),
            Self::Heart | Self::AvgHeart | Self::Cadence | Self::LapHeart => (0, 999),
            Self::Distance | Self::LapDistance => (0, 9_999_900),
            Self::Elapsed | Self::Moving | Self::LapTime | Self::PrevLapTime | Self::Paused => {
                (0, 99 * 3600 + 59 * 60 + 59)
            }
            Self::Grade | Self::SpeedDiff => (-999, 999),
            Self::Intensity => (0, 999),
            Self::PowerDiff => (-9999, 9999),
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed | Self::LapSpeed => {
                Some(SPEED_TENTHS_MOST)
            }
            Self::Distance | Self::LapDistance => Some(99_994),
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
            Self::Speed | Self::AvgSpeed | Self::MaxSpeed | Self::LapSpeed => match unsigned {
                Some(mm_s) if mm_s > SPEED_TENTHS_MOST as u32 => {
                    let _ = write!(out, "{}", u64::from(mm_s) * 36 / 10_000);
                }
                _ => ride::speed(out, unsigned),
            },
            Self::Distance | Self::LapDistance => {
                ride::kilometres(out, u64::from(unsigned.unwrap_or(0)) * 1000)
            }
            Self::Elapsed => ride::elapsed(out, u64::from(unsigned.unwrap_or(0))),
            Self::Moving | Self::LapTime | Self::PrevLapTime | Self::Paused => match unsigned {
                Some(seconds) => ride::lap_time(out, u64::from(seconds)),
                None => whole(out),
            },
            Self::Grade => ride::grade(out, value),
            Self::Intensity => hundredths(out, unsigned),
            Self::SpeedDiff => signed(out, value, |out, tenths| {
                write!(out, "{}.{}", tenths / 10, tenths % 10)
            }),
            Self::PowerDiff => signed(out, value, |out, watts| write!(out, "{watts}")),
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
            | Self::LapPower
            | Self::LapHeart
            | Self::LapNumber
            | Self::PrevLapNumber
            | Self::PrevLapPower => whole(out),
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

fn signed(
    out: &mut Text<12>,
    value: Option<i32>,
    magnitude: impl FnOnce(&mut Text<12>, u32) -> core::fmt::Result,
) {
    let Some(delta) = value else {
        let _ = out.push_str("--");
        return;
    };
    let _ = out.push_str(match delta {
        1.. => "+",
        ..0 => "-",
        0 => "",
    });
    let _ = magnitude(out, delta.unsigned_abs());
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

const SECTION_TITLES: [&str; crate::screens::settings::SECTIONS.len()] = {
    let sections = &crate::screens::settings::SECTIONS;
    let mut titles = [""; crate::screens::settings::SECTIONS.len()];
    let mut at = 0;
    while at < titles.len() {
        titles[at] = sections[at].0;
        at += 1;
    }
    titles
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Rides,
    Workouts,
    Settings,
    /// The saved sensors, then ADD SENSOR.
    Sensors,
    /// The sensors a search found that are not saved.
    Nearby,
}

impl ListKind {
    /// What its heading calls it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rides => "RIDES",
            Self::Workouts => "WORKOUTS",
            Self::Settings => "SETTINGS",
            Self::Sensors => "SENSORS",
            Self::Nearby => "NEARBY",
        }
    }

    /// Every heading the list may show, for its widest.
    pub const fn headings(self) -> &'static [&'static str] {
        match self {
            Self::Rides => &["RIDES"],
            Self::Workouts => &["WORKOUTS"],
            Self::Settings => &SECTION_TITLES,
            Self::Sensors => &["SENSORS"],
            Self::Nearby => &["NEARBY"],
        }
    }

    /// Whether the heading says where the focus is among the items.
    pub const fn placed(self) -> bool {
        !matches!(self, Self::Settings)
    }

    /// Cards a frame keeps of this list: accept refuses a scroll that
    /// shows more at once.
    pub const fn most_in_view(self) -> usize {
        match self {
            Self::Rides => 6,
            Self::Workouts | Self::Settings | Self::Sensors | Self::Nearby => {
                super::list::MOST_IN_VIEW
            }
        }
    }

    /// What the viewport says while the list is empty: a line and a caption
    /// under it.
    pub const fn empty(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Rides => Some(("NO RIDES YET", "SAVED RIDES SHOW HERE")),
            Self::Workouts | Self::Settings | Self::Sensors | Self::Nearby => None,
        }
    }

    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Rides),
            1 => Some(Self::Workouts),
            2 => Some(Self::Settings),
            3 => Some(Self::Sensors),
            4 => Some(Self::Nearby),
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

/// A static page's words and state, as the runtime prepared them for the
/// frame.
#[derive(Clone, Copy)]
pub enum PageData<'a> {
    None,
    Home(&'a crate::screens::home::Home),
    Ready(&'a crate::screens::ready::Readiness),
    /// The step shown and the note beside each card.
    Setup {
        step: crate::screens::setup::Step,
        notes: &'a [Text<38>],
    },
    /// The lines, by `screens::phone`'s indices, and the code.
    Phone {
        lines: &'a [Text<38>],
        qr: Option<&'a crate::screens::qr::Placed>,
        /// The light look, whose palette the code's panel must undo.
        light: bool,
    },
    /// The reveal's progress, in thousandths.
    Brand(u16),
}
