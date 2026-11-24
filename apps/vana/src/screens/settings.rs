//! Settings: grouped sections of rows, one section per page, and what each
//! row is and does. The screen is a tree (`compose::builtin::settings`),
//! the rows and the number editor alike; the runtime prepares each row's
//! value and note in the lines this names.
//!
//! Acting on a row cycles a choice, opens a page or edits a number. While
//! a number is edited, MOVE lowers and ACT raises it by its step, and BACK
//! finishes; touch swipes between sections and taps rows and pads.
use super::ride::RidePage;

/// Every row, in BottomLeft order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Ftp,
    Threshold,
    Bests,
    Form,
    Weight,
    Age,
    Sex,
    Brightness,
    AutoDim,
    Daylight,
    Theme,
    Mode,
    TimeZone,
    AutoPause,
    AutoLap,
    Wheel,
    WheelMode,
    MainLeft,
    MainRight,
    PageLap,
    PageClimb,
    PageMap,
    Sounds,
    RadarAlerts,
    FuelAlert,
    CadenceLow,
    SunsetAlert,
    HeartHigh,
    HeartLow,
    PowerHigh,
    PowerLow,
    Sensors,
    Phone,
    Storage,
    Firmware,
}

/// What acting on a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Opens the number editor.
    Number,
    /// Steps to the next choice.
    Choice,
    /// Opens another page or flow.
    Open,
    /// Shows a fact; acting does nothing.
    Info,
}

/// What an `Open` row asks the runtime for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    /// The sensor menu.
    Sensors,
    /// Connect phone: send rides to a phone over Wi-Fi.
    ConnectPhone,
    /// The rider's personal bests.
    Bests,
    /// Fitness, fatigue and form.
    Form,
}

/// Sections in order, each with its title and the rows it holds.
pub const SECTIONS: [(&str, &[Row]); 11] = [
    (
        "PROFILE",
        &[Row::Ftp, Row::Threshold, Row::Bests, Row::Form],
    ),
    ("BODY", &[Row::Weight, Row::Age, Row::Sex]),
    (
        "DISPLAY",
        &[
            Row::Brightness,
            Row::AutoDim,
            Row::Daylight,
            Row::Theme,
            Row::Mode,
        ],
    ),
    ("CLOCK", &[Row::TimeZone]),
    (
        "RIDE",
        &[Row::AutoPause, Row::AutoLap, Row::Wheel, Row::WheelMode],
    ),
    ("MAIN PAGE", &[Row::MainLeft, Row::MainRight]),
    ("RIDE PAGES", &[Row::PageLap, Row::PageClimb, Row::PageMap]),
    ("SOUND", &[Row::Sounds]),
    (
        "ALERTS",
        &[
            Row::RadarAlerts,
            Row::FuelAlert,
            Row::CadenceLow,
            Row::SunsetAlert,
        ],
    ),
    (
        "LIMITS",
        &[Row::HeartHigh, Row::HeartLow, Row::PowerHigh, Row::PowerLow],
    ),
    (
        "DEVICE",
        &[Row::Sensors, Row::Phone, Row::Storage, Row::Firmware],
    ),
];

/// The most rows a section holds; more than four scroll.
pub const SECTION_ROWS: usize = 5;
const _: () = {
    let mut i = 0;
    while i < SECTIONS.len() {
        assert!(SECTIONS[i].1.len() <= SECTION_ROWS);
        i += 1;
    }
};
const NO_PHONE: [(&str, &[Row]); 11] = {
    let mut sections = SECTIONS;
    sections[SECTIONS.len() - 1] = ("DEVICE", &[Row::Sensors, Row::Storage, Row::Firmware]);
    sections
};

/// The sections this device shows; cursor positions count their rows in
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sections {
    All,
    NoPhone,
}

impl Sections {
    fn table(self) -> &'static [(&'static str, &'static [Row])] {
        match self {
            Self::All => &SECTIONS,
            Self::NoPhone => &NO_PHONE,
        }
    }

    pub fn phone(self) -> bool {
        self.field(Row::Phone).is_some()
    }

    pub fn count(self) -> usize {
        self.table().len()
    }

    /// All rows, the cursor's range.
    pub fn fields(self) -> usize {
        self.table().iter().map(|(_, rows)| rows.len()).sum()
    }

    pub fn title(self, section: usize) -> &'static str {
        self.table()[section % self.count()].0
    }

    /// The row at cursor position `index`, wrapping.
    pub fn row(self, index: usize) -> Row {
        let (rows, first) = self.rows(index);
        rows[index % self.fields() - first]
    }

    /// The row's cursor position, if this device shows it.
    pub fn field(self, row: Row) -> Option<usize> {
        let mut first = 0;
        for (_, rows) in self.table() {
            if let Some(i) = rows.iter().position(|r| *r == row) {
                return Some(first + i);
            }
            first += rows.len();
        }
        None
    }

    /// The section holding cursor position `index`, and its first position.
    pub fn section(self, index: usize) -> (usize, usize) {
        let index = index % self.fields();
        let mut first = 0;
        for (section, (_, rows)) in self.table().iter().enumerate() {
            if index < first + rows.len() {
                return (section, first);
            }
            first += rows.len();
        }
        unreachable!("index is below fields")
    }

    /// The rows of the section holding cursor position `index`, and the
    /// cursor position of its first row.
    pub fn rows(self, index: usize) -> (&'static [Row], usize) {
        let (section, first) = self.section(index);
        (self.table()[section].1, first)
    }

    /// The first cursor position of `section`, wrapping.
    pub fn first_of(self, section: usize) -> usize {
        self.table()[..section % self.count()]
            .iter()
            .map(|(_, rows)| rows.len())
            .sum()
    }
}

impl Row {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ftp => "FTP WATTS",
            Self::Threshold => "LTHR BPM",
            Self::Bests => "BESTS",
            Self::Form => "FORM",
            Self::Weight => "WEIGHT KG",
            Self::Age => "AGE YEARS",
            Self::Sex => "SEX",
            Self::Brightness => "BRIGHTNESS",
            Self::AutoDim => "DIM AFTER",
            Self::Daylight => "DAY / NIGHT",
            Self::Theme => "THEME",
            Self::Mode => "MODE",
            Self::TimeZone => "TIME ZONE",
            Self::AutoPause => "AUTO PAUSE",
            Self::AutoLap => "AUTO LAP",
            Self::Wheel => "WHEEL",
            Self::WheelMode => "WHEEL MODE",
            Self::MainLeft => "LOWER LEFT",
            Self::MainRight => "LOWER RIGHT",
            Self::PageLap => "LAP",
            Self::PageClimb => "CLIMB",
            Self::PageMap => "MAP",
            Self::Sounds => "SOUNDS",
            Self::RadarAlerts => "RADAR ALL CLEAR",
            Self::FuelAlert => "EAT AND DRINK",
            Self::CadenceLow => "LOW CADENCE",
            Self::SunsetAlert => "SUNSET",
            Self::HeartHigh => "HR ABOVE",
            Self::HeartLow => "HR BELOW",
            Self::PowerHigh => "POWER ABOVE",
            Self::PowerLow => "POWER BELOW",
            Self::Sensors => "SENSORS",
            Self::Phone => "PHONE",
            Self::Storage => "STORAGE",
            Self::Firmware => "FIRMWARE",
        }
    }

    /// The ride page a `RIDE PAGES` row switches.
    pub const fn ride_page(self) -> Option<RidePage> {
        match self {
            Self::PageLap => Some(RidePage::Lap),
            Self::PageClimb => Some(RidePage::Climb),
            Self::PageMap => Some(RidePage::Map),
            _ => None,
        }
    }

    pub const fn action(self) -> Option<SettingsAction> {
        match self {
            Self::Sensors => Some(SettingsAction::Sensors),
            Self::Phone => Some(SettingsAction::ConnectPhone),
            Self::Bests => Some(SettingsAction::Bests),
            Self::Form => Some(SettingsAction::Form),
            _ => None,
        }
    }

    pub const fn kind(self) -> Kind {
        match self {
            Self::Ftp | Self::Threshold | Self::Wheel | Self::Weight | Self::Age => Kind::Number,
            Self::Brightness
            | Self::AutoDim
            | Self::Daylight
            | Self::Theme
            | Self::Mode
            | Self::TimeZone
            | Self::AutoPause
            | Self::AutoLap
            | Self::WheelMode
            | Self::MainLeft
            | Self::MainRight
            | Self::PageLap
            | Self::PageClimb
            | Self::PageMap
            | Self::Sounds
            | Self::RadarAlerts
            | Self::FuelAlert
            | Self::CadenceLow
            | Self::SunsetAlert
            | Self::HeartHigh
            | Self::HeartLow
            | Self::PowerHigh
            | Self::PowerLow
            | Self::Sex => Kind::Choice,
            Self::Sensors | Self::Phone | Self::Bests | Self::Form => Kind::Open,
            Self::Storage | Self::Firmware => Kind::Info,
        }
    }
}

/// Each press or tap moves a number by this much: watts, bpm or a wheel's mm.
pub const STEP: u16 = 5;

impl Row {
    /// How far a press or tap moves this row's number: a kilogram or a year
    /// at a time, `STEP` for the rest.
    pub const fn step(self) -> u16 {
        match self {
            Self::Weight | Self::Age => 1,
            _ => STEP,
        }
    }
}

/// Lines the runtime prepares for this page.
pub const STATUS: usize = 0;
/// Each of the section's rows' values, in order; the edited number's value.
pub const VALUES: usize = 1;
/// A note beside each value, such as the zone a number anchors.
pub const NOTES: usize = VALUES + SECTION_ROWS;

#[cfg(test)]
mod tests;
