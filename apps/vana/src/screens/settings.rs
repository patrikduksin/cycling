//! Settings: grouped sections of rows, one section per page. The rows scroll
//! under the finger between the section's title and the save status
//! (`scroll::Layout`), with a scroll bar at the right when they do not fit.
//!
//! Buttons keep their roles. MOVE moves to the next row (the page follows
//! into the next section), ACT acts on it and BACK goes back Home. Acting
//! cycles a choice, opens a page or edits a number. While a number is
//! edited, MOVE lowers and ACT raises it by `STEP`, and BACK finishes.
//! Touch does the same directly: swipe between sections, drag a long
//! section, tap a row to act on it, and tap the large minus and plus pads
//! while editing. The runtime prepares each row's value and note; this draws
//! them and maps touches to `Target`s.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BODY, CAPTION, CARD, LARGE, Line, PRESSED, RAISED, Rect, SECONDARY,
    TITLE, VALUE, WHITE,
};
use super::ride::RidePage;
use super::scroll::{Column, Layout};
use super::workout::View;

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
    PageStats,
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
    (
        "RIDE PAGES",
        &[Row::PageStats, Row::PageLap, Row::PageClimb, Row::PageMap],
    ),
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

    /// The target under a touch at panel point (`x`, `y`) on the section
    /// that holds `cursor`, scrolled to `offset`: its rows, or the pads while
    /// a number is edited.
    pub fn target(
        self,
        x: usize,
        y: usize,
        cursor: usize,
        editing: bool,
        offset: i32,
    ) -> Option<Target> {
        if !(LEFT..RIGHT).contains(&x) {
            return None;
        }
        if editing {
            return (PAD_TOP..PAD_TOP + PAD_HEIGHT)
                .contains(&y)
                .then_some(if x < PAD_SPLIT {
                    Target::Lower
                } else {
                    Target::Raise
                });
        }
        let (rows, first) = self.rows(cursor);
        LAYOUT
            .card_at(rows.len(), offset, x, y)
            .map(|row| Target::Row(first + row))
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
            Self::PageStats => "STATS",
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
            Self::PageStats => Some(RidePage::Stats),
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
            | Self::PageStats
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

const RADIUS: usize = 12;
/// Text keeps this far inside a row or pad.
const INSET: usize = 16;
const LEFT: usize = 10;
const RIGHT: usize = 230;
const TITLE_BASE: usize = 62;
const ROW_TOP: usize = 76;
const ROW_HEIGHT: usize = 46;
const ROW_PITCH: usize = 52;
/// The rows scroll between the title and the save status.
pub const LAYOUT: Layout = Layout {
    top: ROW_TOP - 4,
    bottom: STATUS_BASE - 12,
    column: Column {
        left: LEFT,
        right: RIGHT,
        top: 4,
        height: ROW_HEIGHT,
        pitch: ROW_PITCH,
    },
    end: 4,
    margin: 4,
    pin: None,
};
const FOCUS: usize = 3;
/// A long section's scroll bar, in the margin right of the rows.
const BAR_LEFT: usize = RIGHT + 4;
const BAR_WIDE: usize = 3;
/// The number editor's field and its minus and plus pads.
const FIELD_TOP: usize = 76;
const FIELD_HEIGHT: usize = 92;
const PAD_TOP: usize = 208;
const PAD_HEIGHT: usize = 58;
const PAD_SPLIT: usize = 120;
const PAD_GAP: usize = 4;
const STATUS_BASE: usize = 300;

/// What a touch landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A row's cursor position.
    Row(usize),
    Lower,
    Raise,
}

/// The section's rows, or the number editor, with the save status.
pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let sections = view.sections;
    let title = sections.title(sections.section(view.cursor).0);
    let row = sections.row(view.cursor);
    if view.editing && row.kind() == Kind::Number {
        editor(&mut canvas, view, title, row);
    } else {
        canvas.fit(
            &[&TITLE, &BODY],
            title.as_bytes(),
            RIGHT - LEFT,
            LEFT,
            TITLE_BASE,
            Align::Left,
            WHITE,
        );
        let (rows, first) = sections.rows(view.cursor);
        for index in LAYOUT.visible(rows.len(), view.scroll) {
            row_card(
                &mut canvas,
                view,
                index,
                rows[index],
                first + index == view.cursor % sections.fields(),
            );
        }
        // A long section's track down the viewport, the part in view lit.
        let (top, bottom) = (LAYOUT.top, LAYOUT.bottom);
        if let Some((start, end)) = LAYOUT.thumb(rows.len(), view.scroll) {
            // The bar stays put while the rows scroll under it.
            let at = |y| LAYOUT.held(y, view.scroll);
            for (top, bottom, color) in [(top, bottom, CARD), (start, end, SECONDARY)] {
                canvas.shape(Shape::Bar {
                    rect: Rect {
                        left: BAR_LEFT,
                        top: at(top),
                        right: BAR_LEFT + BAR_WIDE,
                        bottom: at(bottom),
                    },
                    radius: 1,
                    color,
                });
            }
        }
    }
    // A toast takes the bottom band; the save status waits under it.
    if view.toast.is_some() {
        return canvas;
    }
    let status = view.lines()[STATUS].as_bytes();
    let fine = matches!(status, b"SAVED" | b"SAVING");
    canvas.fit(
        &[&CAPTION],
        status,
        RIGHT - LEFT,
        LEFT,
        STATUS_BASE,
        Align::Left,
        if fine { SECONDARY } else { AMBER },
    );
    canvas
}

/// A row: its label over its value, and a note at the right. Every row can
/// be tapped, so each sits on a small card; the focused one is raised and
/// ringed.
fn row_card(canvas: &mut Canvas, view: &View, index: usize, row: Row, focused: bool) {
    let rect = LAYOUT.rect(index);
    let slot = index;
    if focused {
        canvas.shape(Shape::Panel {
            rect,
            radius: RADIUS,
            color: ACCENT,
        });
    }
    canvas.shape(Shape::Panel {
        rect: if focused { rect.inset(FOCUS) } else { rect },
        radius: if focused { RADIUS - FOCUS } else { RADIUS },
        color: if view.pressed_setting() == Some(index as u16) {
            PRESSED
        } else if focused {
            RAISED
        } else {
            CARD
        },
    });
    let (left, right) = (LEFT + INSET, RIGHT - INSET);
    canvas.caption(
        &crate::screens::scaled::BASE,
        row.label().as_bytes(),
        right - left,
        left,
        rect.top + 19,
        Align::Left,
    );
    let base = rect.top + 38;
    let value = canvas.fit(
        &[&BODY, &CAPTION],
        view.lines()[VALUES + slot].as_bytes(),
        right - left,
        left,
        base,
        Align::Left,
        WHITE,
    );
    // A note too wide beside the value gives way to its part after a comma, then to none.
    let whole = view.lines()[NOTES + slot].as_bytes();
    let room = right.saturating_sub(value.right() + 12);
    let last = whole
        .windows(2)
        .position(|pair| pair == b", ")
        .map_or(whole, |at| &whole[at + 2..]);
    for note in [whole, last] {
        if !note.is_empty() && Line::<24>::fit(&[&CAPTION], note, room).text().len() == note.len() {
            canvas.caption(
                &crate::screens::scaled::BASE,
                note,
                room,
                right,
                base,
                Align::Right,
            );
            break;
        }
    }
}

/// The edited number on an accent field, with its note, then the pads.
fn editor(canvas: &mut Canvas, view: &View, title: &str, row: Row) {
    canvas.caption(
        &crate::screens::scaled::BASE,
        title.as_bytes(),
        RIGHT - LEFT,
        LEFT,
        44,
        Align::Left,
    );
    let rect = Rect {
        left: LEFT,
        top: FIELD_TOP,
        right: RIGHT,
        bottom: FIELD_TOP + FIELD_HEIGHT,
    };
    canvas.shape(Shape::Panel {
        rect,
        radius: RADIUS,
        color: ACCENT,
    });
    let (left, width) = (LEFT + INSET, RIGHT - LEFT - 2 * INSET);
    canvas.fit(
        &[&CAPTION],
        row.label().as_bytes(),
        width,
        left,
        rect.top + 24,
        Align::Left,
        WHITE,
    );
    canvas.fit(
        &[&VALUE, &BODY],
        view.lines()[VALUES].as_bytes(),
        width,
        left,
        rect.top + 60,
        Align::Left,
        WHITE,
    );
    canvas.fit(
        &[&CAPTION],
        view.lines()[NOTES].as_bytes(),
        width,
        left,
        rect.top + 80,
        Align::Left,
        WHITE,
    );
    let (lower, raise): (&[u8], &[u8]) = if row.step() == 1 {
        (b"-1", b"+1")
    } else {
        (b"-5", b"+5")
    };
    for (left, right, label) in [
        (LEFT, PAD_SPLIT - PAD_GAP, lower),
        (PAD_SPLIT + PAD_GAP, RIGHT, raise),
    ] {
        canvas.shape(Shape::Panel {
            rect: Rect {
                left,
                top: PAD_TOP,
                right,
                bottom: PAD_TOP + PAD_HEIGHT,
            },
            radius: RADIUS,
            color: RAISED,
        });
        let base = PAD_TOP + (PAD_HEIGHT + LARGE.cap as usize) / 2;
        canvas.fit(
            &[&LARGE],
            label,
            right - left,
            (left + right) / 2,
            base,
            Align::Center,
            WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    fn view(cursor: usize, editing: bool, values: &[(&str, &str)], status: &str) -> View<'static> {
        let mut view = View::new(Page::Settings, cursor, None);
        view.editing = editing;
        let lines = view.show_settings();
        for (slot, (value, note)) in values.iter().enumerate() {
            let _ = lines[VALUES + slot].push_str(value);
            let _ = lines[NOTES + slot].push_str(note);
        }
        let _ = lines[STATUS].push_str(status);
        view.prepare();
        view
    }

    fn profile(cursor: usize, editing: bool) -> View<'static> {
        let values: &[(&str, &str)] = if editing {
            &[("215", "Z4 195-226 W")]
        } else {
            &[
                ("215", "Z4 195-226 W"),
                ("185", "Z5 FROM 170 BPM"),
                ("NONE YET", ""),
            ]
        };
        view(cursor, editing, values, "SAVED")
    }

    fn panels(view: &View) -> std::vec::Vec<(Rect, u16)> {
        view.canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Panel { rect, color, .. } => Some((*rect, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rows_walk_every_section_in_order() {
        for (sections, fields) in [(Sections::All, 36), (Sections::NoPhone, 35)] {
            assert_eq!(sections.fields(), fields);
            for index in 0..fields {
                assert_eq!(sections.field(sections.row(index)), Some(index));
                let (section, first) = sections.section(index);
                assert_eq!(sections.first_of(section), first);
                let (rows, top) = sections.rows(index);
                assert!(rows.len() <= SECTION_ROWS && rows.contains(&sections.row(index)));
                assert_eq!(rows[0], sections.row(top));
            }
            assert_eq!(sections.row(fields), Row::Ftp, "wraps");
            assert_eq!(sections.first_of(sections.count()), 0);
        }
        let all = Sections::All;
        let at = |row| all.field(row).unwrap();
        assert_eq!(all.section(at(Row::Weight)), (1, at(Row::Weight)));
        assert_eq!(all.section(at(Row::Sex)), (1, at(Row::Weight)));
        assert_eq!(all.section(at(Row::TimeZone)), (3, at(Row::TimeZone)));
        assert_eq!(all.section(at(Row::Sounds)), (7, at(Row::Sounds)));
        assert_eq!(all.section(at(Row::PowerLow)), (9, at(Row::HeartHigh)));
    }

    #[test]
    fn without_wifi_device_drops_only_the_phone_row() {
        let (all, bare) = (Sections::All, Sections::NoPhone);
        assert!(all.phone() && !bare.phone());
        let rows = |sections: Sections| (0..sections.fields()).map(move |i| sections.row(i));
        assert!(rows(all).filter(|row| *row != Row::Phone).eq(rows(bare)));
        let device = bare.field(Row::Sensors).unwrap();
        assert_eq!(
            bare.rows(device).0,
            [Row::Sensors, Row::Storage, Row::Firmware]
        );
        assert_eq!(bare.title(bare.section(device).0), "DEVICE");
    }

    #[test]
    fn rings_the_focused_row_and_fills_the_edited_number() {
        let mut moving = profile(1, false);
        let colors: std::vec::Vec<_> = panels(&moving).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [CARD, ACCENT, RAISED, CARD, CARD], "no pads");
        assert_eq!(moving.pixel(LEFT, ROW_TOP), 0, "rounded corner");
        let second = LAYOUT.column.rect(1);
        let second = Rect {
            top: LAYOUT.top + second.top,
            bottom: LAYOUT.top + second.bottom,
            ..second
        };
        assert_eq!(moving.pixel(LEFT + 1, second.top + 20), ACCENT, "ring");
        assert_eq!(moving.pixel(LEFT + 6, second.top + 20), RAISED);
        assert!(moving.canvas().shows("PROFILE") && moving.canvas().shows("Z4 195-226 W"));
        assert_eq!(
            moving.canvas().find("215").map(|run| run.color),
            Some(WHITE)
        );
        // No permanent button labels.
        assert!((0..240).all(|x| (306..320).all(|y| moving.pixel(x, y) == 0)));

        let mut editing = profile(0, true);
        let colors: std::vec::Vec<_> = panels(&editing).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [ACCENT, RAISED, RAISED]);
        assert!(editing.canvas().find("215").unwrap().line.face().cap == VALUE.cap);
        assert!(editing.canvas().shows("FTP WATTS") && !editing.canvas().shows("LTHR BPM"));
        assert!(editing.canvas().shows("-5") && editing.canvas().shows("+5"));
        assert_eq!(editing.pixel(LEFT + 2, PAD_TOP + 20), RAISED);
    }

    #[test]
    fn body_numbers_move_a_unit_at_a_time_and_say_when_unset() {
        let mut editing = View::new(
            Page::Settings,
            Sections::All.field(Row::Weight).unwrap(),
            None,
        );
        editing.editing = true;
        let lines = editing.show_settings();
        let _ = lines[VALUES].push_str("NOT SET");
        let _ = lines[NOTES].push_str("ADD FOR W/KG");
        editing.prepare();
        assert!(editing.canvas().shows("WEIGHT KG") && editing.canvas().shows("NOT SET"));
        assert!(editing.canvas().shows("-1") && editing.canvas().shows("+1"));
        assert!(!editing.canvas().shows("+5"));
        assert_eq!(Row::Weight.step(), 1);
        assert_eq!(Row::Age.step(), 1);
        assert_eq!(Row::Ftp.step(), STEP);
        assert_eq!(Row::Sex.kind(), Kind::Choice);
    }

    /// Long values, notes and statuses still lay out whole on every section.
    #[test]
    fn extreme_settings_never_overlap_or_clip() {
        // The rows lie on the page, below the screen's rows.
        let screen = Rect {
            left: 0,
            top: 24,
            right: 240,
            bottom: 100_000,
        };
        let long = [
            ("1000", "Z4 905-1050 W"),
            ("220", "Z5 FROM 220 BPM"),
            ("SEND RIDES", ""),
            ("99H LEFT", "120 RIDES"),
            ("OFF", "NO CLIMB POP-UP"),
        ];
        for cursor in 0..Sections::All.fields() {
            for editing in [false, true] {
                for status in ["SAVED", "SAVE FAILED, RETRYING", "NOT SAVED: DATA KEPT"] {
                    let view = view(cursor, editing, &long, status);
                    let shown = if editing && Sections::All.row(cursor).kind() == Kind::Number {
                        1
                    } else {
                        Sections::All.rows(cursor).0.len()
                    };
                    let mut texts = std::vec![status];
                    for (value, note) in &long[..shown] {
                        texts.push(value);
                        if !note.is_empty() {
                            texts.push(note);
                        }
                    }
                    view.canvas().assert_laid_out(
                        screen,
                        &texts,
                        &std::format!("{cursor} {editing} {status}"),
                    );
                }
            }
        }
    }

    #[test]
    fn touches_find_rows_of_the_shown_section_and_pads_only_while_editing() {
        let row = |slot| ROW_TOP + slot * ROW_PITCH + 10;
        assert_eq!(
            Sections::All.target(100, row(0), 0, false, 0),
            Some(Target::Row(0))
        );
        assert_eq!(
            Sections::All.target(200, row(1), 0, false, 0),
            Some(Target::Row(1))
        );
        assert_eq!(
            Sections::All.target(100, row(2), 0, false, 0),
            Some(Target::Row(2))
        );
        assert_eq!(
            Sections::All.target(100, row(3), 0, false, 0),
            Some(Target::Row(3))
        );
        let device = Sections::All.field(Row::Sensors).unwrap();
        assert_eq!(
            Sections::All.target(100, row(3), device, false, 0),
            Some(Target::Row(device + 3))
        );
        assert_eq!(
            Sections::All.target(100, ROW_TOP + ROW_HEIGHT + 2, 0, false, 0),
            None
        );
        let ride = Sections::All.field(Row::TimeZone).unwrap();
        assert_eq!(Sections::All.target(30, PAD_TOP + 10, ride, false, 0), None);
        assert_eq!(
            Sections::All.target(30, PAD_TOP + 10, 0, true, 0),
            Some(Target::Lower)
        );
        assert_eq!(
            Sections::All.target(200, PAD_TOP + 10, 0, true, 0),
            Some(Target::Raise)
        );
        assert_eq!(Sections::All.target(100, row(0), 0, true, 0), None);
        assert_eq!(Sections::All.target(5, PAD_TOP + 10, 0, true, 0), None);
    }

    #[test]
    fn a_long_section_scrolls_and_shows_where_it_is() {
        let display = Sections::All.field(Row::Brightness).unwrap();
        let bar = |view: &mut View, y| view.pixel(BAR_LEFT + 1, y);
        let scrolled = |offset| {
            let mut view = View::new(Page::Settings, display, None);
            view.scroll = offset;
            let _ = view.show_settings()[STATUS].push_str("SAVED");
            view.prepare();
            view
        };
        let rows = Sections::All.rows(display).0.len();
        assert_eq!(rows, 5);
        // From the top: the thumb at the top of the track.
        let mut top = scrolled(0);
        assert!(top.canvas().shows("DISPLAY"));
        assert_eq!(bar(&mut top, LAYOUT.top + 2), SECONDARY);
        assert_eq!(bar(&mut top, LAYOUT.bottom - 2), CARD);
        // Scrolled to the end: the thumb at the bottom, the fifth row whole
        // and under a tap.
        let end = LAYOUT.content(rows) as i32 - LAYOUT.viewport() as i32;
        assert!(end > 0);
        let mut bottom = scrolled(end);
        assert_eq!(bar(&mut bottom, LAYOUT.top + 2), CARD);
        assert_eq!(bar(&mut bottom, LAYOUT.bottom - 2), SECONDARY);
        assert!(LAYOUT.whole(4, end));
        let fifth = (LAYOUT.top as i32 + LAYOUT.span(4).0 - end + 10) as usize;
        assert_eq!(
            Sections::All.target(100, fifth, display, false, end),
            Some(Target::Row(display + 4))
        );
        // A short section has no bar.
        assert_eq!(bar(&mut view(0, false, &[], "SAVED"), LAYOUT.top + 2), 0);
    }

    #[test]
    fn a_row_under_a_resting_finger_is_pressed() {
        let mut view = View::new(Page::Settings, 0, None);
        let _ = view.show_settings()[STATUS].push_str("SAVED");
        view.press_setting(Some(1));
        view.prepare();
        let colors: std::vec::Vec<_> = panels(&view).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors, [ACCENT, RAISED, PRESSED, CARD, CARD]);
    }
}
